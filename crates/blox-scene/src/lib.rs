use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use bevy::prelude::{Mat3, Quat, Resource, Vec3};

pub mod dds;
pub mod decal;
pub mod lighting;
pub mod material;
pub mod mesh;
pub mod texture;

use blox_model::{Model, ModelError, ModelInstance, ModelResult, Property, brick_color};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Geometry {
    Block,
    Ball,
    Cylinder,
    VerticalCylinder,
    Wedge,
    CornerWedge,
    Head,
    Torso,
    File(PathBuf),
}

/// NormalId order from App/util/NormalId.h; also the serialized `Decal.Face` token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Right,
    Top,
    Back,
    Left,
    Bottom,
    Front,
}

impl Face {
    pub fn from_normal_id(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Right,
            1 => Self::Top,
            2 => Self::Back,
            3 => Self::Left,
            4 => Self::Bottom,
            5 => Self::Front,
            _ => return None,
        })
    }
    pub fn index(self) -> usize {
        self as usize
    }
}

/// A `Decal` (clamped, one stretched copy) or `Texture` (tiled, `studs_per_tile` is `Some`).
#[derive(Clone, Debug, PartialEq)]
pub struct DecalSnapshot {
    pub face: Face,
    pub texture: PathBuf,
    pub transparency: f32,
    pub studs_per_tile: Option<[f32; 2]>,
}

#[derive(Debug)]
pub struct PartSnapshot {
    pub instance_id: u64,
    pub name: String,
    pub position: Vec3,
    pub rotation: Quat,
    pub size: Vec3,
    pub geometry: Geometry,
    /// Part color. Ignored by the renderer when `texture` is set, as in GeometryGenerator.cpp.
    pub color_srgb: [f32; 3],
    pub transparency: f32,
    pub reflectance: f32,
    /// `TextureId` of the part's SpecialMesh/FileMesh, resolved to a local file.
    pub texture: Option<PathBuf>,
    pub decals: Vec<DecalSnapshot>,
    /// BlockMesh/CylinderMesh parents ignore part size for Texture tiling (`ignoreMaterialsStuds`).
    pub decals_ignore_size: bool,
    /// `CanCollide` (default true); physics ignores parts where it is false.
    pub can_collide: bool,
    /// Serialized `Material` token (App/include/util/PartMaterial.h); 256 is Plastic.
    pub material: u32,
    /// Surface type per face in NormalId order (right, top, back, left, bottom, front); see
    /// App/include/util/SurfaceType.h. Defaults are studs on top and an inlet underneath.
    pub surfaces: [u8; 6],
    /// No SpecialMesh/BlockMesh/CylinderMesh child: studs and outlines apply (legacy
    /// `ignoreMaterialsStuds` is false).
    pub plain_shape: bool,
}

#[derive(Resource, Debug)]
pub struct SceneSnapshot {
    pub parts: Vec<PartSnapshot>,
    pub omissions: Vec<String>,
    pub instance_count: usize,
    pub script_count: usize,
}

impl SceneSnapshot {
    pub fn from_model(model: &Model, content: &Path) -> ModelResult<Self> {
        let mut meshes = HashMap::new();
        let mut decal_children: HashMap<&str, Vec<&ModelInstance>> = HashMap::new();
        let mut omissions = Vec::new();
        let mut script_count = 0;
        for instance in &model.instances {
            match instance.class.as_str() {
                "SpecialMesh" | "BlockMesh" | "CylinderMesh" | "FileMesh" => {
                    if let Some(parent) = &instance.parent {
                        meshes.entry(parent.as_str()).or_insert(instance);
                    }
                }
                "Decal" | "Texture" => {
                    if let Some(parent) = &instance.parent {
                        decal_children
                            .entry(parent.as_str())
                            .or_default()
                            .push(instance);
                    }
                }
                "Script" | "LocalScript" | "ModuleScript" => script_count += 1,
                "Terrain" | "ParticleEmitter" | "Smoke" | "Fire" | "Sparkles" | "PointLight"
                | "SpotLight" | "SurfaceLight" | "BillboardGui" | "SurfaceGui" | "ScreenGui" => {
                    omissions.push(format!(
                        "{} {}: not rendered",
                        instance.class, instance.name
                    ));
                }
                _ => {}
            }
        }
        let mut parts = Vec::new();
        for (index, instance) in model.instances.iter().enumerate() {
            let mut geometry = match instance.class.as_str() {
                "Part" => match number(instance, &["shape", "Shape", "shap"], 1.0)? as u32 {
                    0 => Geometry::Ball,
                    1 => Geometry::Block,
                    2 => Geometry::Cylinder,
                    other => return Err(error(instance, &format!("unknown Part shape {other}"))),
                },
                "Seat" | "VehicleSeat" | "SpawnLocation" => Geometry::Block,
                "WedgePart" => Geometry::Wedge,
                "CornerWedgePart" => Geometry::CornerWedge,
                "MeshPart" | "UnionOperation" | "NegateOperation" | "TrussPart" => {
                    omissions.push(format!(
                        "{} {}: geometry not implemented",
                        instance.class, instance.name
                    ));
                    continue;
                }
                _ => continue,
            };
            let (position, matrix) = match property(instance, &["CFrame", "CoordinateFrame"]) {
                Some(Property::CFrame { position, rotation }) => {
                    (Vec3::from_array(*position), matrix(*rotation))
                }
                None => (Vec3::ZERO, Mat3::IDENTITY),
                _ => return Err(error(instance, "CFrame has an incompatible property type")),
            };
            if !position.is_finite()
                || !matrix.is_finite()
                || (matrix.determinant() - 1.0).abs() > 0.02
            {
                return Err(error(instance, "invalid CFrame"));
            }
            let rotation = Quat::from_mat3(&matrix).normalize();
            let mut position = position;
            // PartInstance.h defaults to Vector3(4, 1.2, 2); XML size is already in studs.
            let mut size = vector(instance, &["size", "Size", "siz"], Vec3::new(4.0, 1.2, 2.0))?;
            if let Some(mesh) = meshes.get(instance.referent.as_str()) {
                let scale = vector(mesh, &["Scale"], Vec3::ONE)?.abs();
                let offset = vector(mesh, &["Offset"], Vec3::ZERO)?;
                position += rotation * offset;
                geometry = match mesh.class.as_str() {
                    "BlockMesh" => Geometry::Block,
                    "CylinderMesh" => Geometry::VerticalCylinder,
                    "FileMesh" => match local_mesh(mesh, content)? {
                        Some(path) => Geometry::File(path),
                        None => {
                            omissions.push(format!("{}: remote or missing MeshId", instance.name));
                            continue;
                        }
                    },
                    _ => match number(mesh, &["MeshType"], 0.0)? as u32 {
                        0 => Geometry::Head,
                        1 => Geometry::Torso,
                        2 => Geometry::Wedge,
                        3 => Geometry::Ball,
                        4 => Geometry::Cylinder,
                        5 => match local_mesh(mesh, content)? {
                            Some(path) => Geometry::File(path),
                            None => {
                                omissions
                                    .push(format!("{}: remote or missing MeshId", instance.name));
                                continue;
                            }
                        },
                        6 => Geometry::Block,
                        other => {
                            omissions.push(format!(
                                "{}: unsupported SpecialMesh type {other}",
                                instance.name
                            ));
                            continue;
                        }
                    },
                };
                if matches!(geometry, Geometry::File(_)) {
                    size = scale; // File meshes do not inherit Part size: getMeshScale in GeometryGenerator.cpp.
                } else if geometry == Geometry::Head {
                    let scaled = size * scale;
                    size =
                        Vec3::new(scaled.x.min(scaled.z), scaled.y, scaled.x.min(scaled.z)) / 1.25;
                } else {
                    size *= scale;
                }
            }
            if !size.is_finite() || size.min_element() <= 0.0 {
                return Err(error(instance, "invalid part or mesh size"));
            }
            let color_srgb = match property(instance, &["Color3uint8", "Color3", "Color"]) {
                Some(Property::Color3(value)) => *value,
                Some(_) => return Err(error(instance, "unsupported color encoding")),
                None => {
                    let color = number(instance, &["BrickColor"], 194.0)? as u32;
                    brick_color::rgb(color)
                        .ok_or_else(|| error(instance, &format!("unknown BrickColor {color}")))?
                }
            };
            let transparency = number(instance, &["Transparency"], 0.0)? as f32;
            let reflectance = number(instance, &["Reflectance"], 0.0)? as f32;
            if color_srgb
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                || !transparency.is_finite()
                || !(0.0..=1.0).contains(&transparency)
                || !reflectance.is_finite()
                || !(0.0..=1.0).contains(&reflectance)
            {
                return Err(error(instance, "invalid material values"));
            }
            let part_mesh = meshes.get(instance.referent.as_str());
            let mut texture = None;
            if let Some(mesh) =
                part_mesh.filter(|m| matches!(m.class.as_str(), "SpecialMesh" | "FileMesh"))
            {
                texture = resolve_texture(mesh, "TextureId", content, &mut omissions)?;
            }
            let mut decals = Vec::new();
            for decal in decal_children
                .get(instance.referent.as_str())
                .into_iter()
                .flatten()
            {
                let Some(path) = resolve_texture(decal, "Texture", content, &mut omissions)? else {
                    continue;
                };
                let face = number(decal, &["Face"], 5.0)?;
                let face = Face::from_normal_id(face as u32)
                    .filter(|_| face.fract() == 0.0)
                    .ok_or_else(|| error(decal, "invalid Face"))?;
                let decal_transparency = number(decal, &["Transparency"], 0.0)? as f32;
                if !(0.0..=1.0).contains(&decal_transparency) {
                    return Err(error(decal, "invalid decal Transparency"));
                }
                let studs_per_tile = if decal.class == "Texture" {
                    let u = number(decal, &["StudsPerTileU"], 2.0)? as f32;
                    let v = number(decal, &["StudsPerTileV"], 2.0)? as f32;
                    if !(u.is_finite() && v.is_finite() && u > 0.0 && v > 0.0) {
                        return Err(error(decal, "StudsPerTile must be positive"));
                    }
                    Some([u, v])
                } else {
                    None
                };
                decals.push(DecalSnapshot {
                    face,
                    texture: path,
                    transparency: decal_transparency,
                    studs_per_tile,
                });
            }
            parts.push(PartSnapshot {
                instance_id: index as u64 + 1,
                name: instance.name.clone(),
                position,
                rotation,
                size,
                geometry,
                color_srgb,
                transparency,
                reflectance,
                can_collide: match property(instance, &["CanCollide"]) {
                    Some(Property::Bool(value)) => *value,
                    None => true,
                    _ => return Err(error(instance, "CanCollide must be a bool")),
                },
                material: material_token(instance)?,
                surfaces: surface_types(instance)?,
                plain_shape: part_mesh.is_none(),
                texture,
                decals,
                decals_ignore_size: part_mesh
                    .is_some_and(|m| matches!(m.class.as_str(), "BlockMesh" | "CylinderMesh")),
            });
        }
        if parts.is_empty() {
            return Err(ModelError(format!(
                "model has no supported renderable parts; {}",
                omissions.join("; ")
            )));
        }
        Ok(Self {
            parts,
            omissions,
            instance_count: model.instances.len(),
            script_count,
        })
    }
}

fn property<'a>(instance: &'a ModelInstance, names: &[&str]) -> Option<&'a Property> {
    names.iter().find_map(|name| instance.properties.get(*name))
}
fn number(instance: &ModelInstance, names: &[&str], default: f64) -> ModelResult<f64> {
    match property(instance, names) {
        Some(Property::Int(value)) => Ok(*value as f64),
        Some(Property::Uint(value)) => Ok(f64::from(*value)),
        Some(Property::Float(value)) => Ok(*value),
        None => Ok(default),
        _ => Err(error(
            instance,
            &format!("{} has an incompatible numeric type", names[0]),
        )),
    }
}
fn vector(instance: &ModelInstance, names: &[&str], default: Vec3) -> ModelResult<Vec3> {
    match property(instance, names) {
        Some(Property::Vector3(value)) => Ok(Vec3::from_array(*value)),
        None => Ok(default),
        _ => Err(error(
            instance,
            &format!("{} has an incompatible vector type", names[0]),
        )),
    }
}
enum ContentRef {
    Null,
    Remote(String),
    Local(PathBuf),
}

/// Resolves a Content property. Only `rbxasset://` ids map to files, and only inside `content`.
fn content_ref(instance: &ModelInstance, name: &str, content: &Path) -> ModelResult<ContentRef> {
    let Some(Property::Text(id)) = property(instance, &[name]) else {
        return Ok(ContentRef::Null);
    };
    if id.trim().is_empty() {
        return Ok(ContentRef::Null);
    }
    let Some(relative) = id.strip_prefix("rbxasset://") else {
        return Ok(ContentRef::Remote(id.clone()));
    };
    if Path::new(relative)
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(error(
            instance,
            &format!("{name} must name a file inside content"),
        ));
    }
    Ok(ContentRef::Local(content.join(relative)))
}

fn local_mesh(instance: &ModelInstance, content: &Path) -> ModelResult<Option<PathBuf>> {
    match content_ref(instance, "MeshId", content)? {
        ContentRef::Null | ContentRef::Remote(_) => Ok(None),
        ContentRef::Local(path) if !path.is_file() => Err(error(
            instance,
            &format!("mesh asset missing: {}", path.display()),
        )),
        ContentRef::Local(path) => Ok(Some(path)),
    }
}

/// Null textures draw nothing; remote or missing ones are reported and skipped.
fn resolve_texture(
    instance: &ModelInstance,
    name: &str,
    content: &Path,
    omissions: &mut Vec<String>,
) -> ModelResult<Option<PathBuf>> {
    Ok(match content_ref(instance, name, content)? {
        ContentRef::Null => None,
        ContentRef::Remote(id) => {
            omissions.push(format!(
                "{} {}: remote {name} {id} not fetched",
                instance.class, instance.name
            ));
            None
        }
        ContentRef::Local(path) if !path.is_file() => {
            omissions.push(format!(
                "{} {}: {name} missing: {}",
                instance.class,
                instance.name,
                path.display()
            ));
            None
        }
        ContentRef::Local(path) => Some(path),
    })
}
fn material_token(instance: &ModelInstance) -> ModelResult<u32> {
    let value = number(instance, &["Material"], 256.0)?;
    if value.fract() != 0.0 || !(0.0..=65535.0).contains(&value) {
        return Err(error(instance, "invalid Material token"));
    }
    Ok(value as u32)
}

fn surface_types(instance: &ModelInstance) -> ModelResult<[u8; 6]> {
    // Property names in NormalId order; PartInstance.cpp:439 defaults.
    const NAMES: [&str; 6] = [
        "RightSurface",
        "TopSurface",
        "BackSurface",
        "LeftSurface",
        "BottomSurface",
        "FrontSurface",
    ];
    const DEFAULTS: [u8; 6] = [0, 3, 0, 0, 4, 0];
    let mut out = DEFAULTS;
    for (slot, name) in out.iter_mut().zip(NAMES) {
        let value = number(instance, &[name], f64::from(*slot))?;
        if value.fract() != 0.0 || !(0.0..=10.0).contains(&value) {
            return Err(error(instance, &format!("invalid {name}")));
        }
        *slot = value as u8;
    }
    Ok(out)
}
fn matrix(rows: [[f32; 3]; 3]) -> Mat3 {
    Mat3::from_cols(
        Vec3::new(rows[0][0], rows[1][0], rows[2][0]),
        Vec3::new(rows[0][1], rows[1][1], rows[2][1]),
        Vec3::new(rows[0][2], rows[1][2], rows[2][2]),
    )
}
fn error(instance: &ModelInstance, message: &str) -> ModelError {
    ModelError(format!(
        "{} {} ({}): {message}",
        instance.class, instance.name, instance.referent
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(xml: &str) -> SceneSnapshot {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let model = Model::decode(xml.as_bytes()).unwrap();
        SceneSnapshot::from_model(&model, &root.join("content")).unwrap()
    }

    #[test]
    fn actual_character_head_face_decal_resolves_to_the_content_texture() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let model = Model::load(root.join("content/other/character.rbxm")).unwrap();
        let scene = SceneSnapshot::from_model(&model, &root.join("content")).unwrap();
        let head = scene.parts.iter().find(|part| part.name == "Head").unwrap();
        assert_eq!(head.decals.len(), 1);
        assert_eq!(head.decals[0].face, Face::Front);
        assert_eq!(
            head.decals[0].texture,
            root.join("content/textures/face.png")
        );
        assert_eq!(head.decals[0].studs_per_tile, None);
        // The torso's `roblox` decal has a null texture and draws nothing.
        assert!(
            scene
                .parts
                .iter()
                .all(|p| p.name == "Head" || p.decals.is_empty())
        );
    }

    #[test]
    fn texture_class_reads_tiling_and_skips_unavailable_content() {
        let scene = snapshot(
            r#"<roblox version="4"><Item class="Part" referent="p"><Properties/>
              <Item class="BlockMesh" referent="m"><Properties/></Item>
              <Item class="Texture" referent="t"><Properties>
                <token name="Face">1</token><float name="StudsPerTileU">4</float>
                <float name="Transparency">0.25</float>
                <Content name="Texture"><url>rbxasset://textures/face.png</url></Content>
              </Properties></Item>
              <Item class="Decal" referent="remote"><Properties>
                <Content name="Texture"><url>http://example.invalid/a.png</url></Content>
              </Properties></Item>
              <Item class="Decal" referent="gone"><Properties>
                <Content name="Texture"><url>rbxasset://textures/does-not-exist.png</url></Content>
              </Properties></Item>
            </Item></roblox>"#,
        );
        let part = &scene.parts[0];
        assert!(part.decals_ignore_size);
        assert_eq!(part.decals.len(), 1);
        let texture = &part.decals[0];
        assert_eq!(texture.face, Face::Top);
        assert_eq!(texture.studs_per_tile, Some([4.0, 2.0]));
        assert_eq!(texture.transparency, 0.25);
        assert!(scene.omissions.iter().any(|o| o.contains("remote Texture")));
        assert!(scene.omissions.iter().any(|o| o.contains("does-not-exist")));
    }

    #[test]
    fn mesh_texture_id_applies_to_file_meshes_only() {
        let scene = snapshot(
            r#"<roblox version="4">
              <Item class="Part" referent="a"><Properties><string name="Name">A</string></Properties>
                <Item class="SpecialMesh" referent="ma"><Properties>
                  <token name="MeshType">5</token>
                  <Content name="MeshId"><url>rbxasset://other/head.mesh</url></Content>
                  <Content name="TextureId"><url>rbxasset://textures/face.png</url></Content>
                </Properties></Item></Item>
              <Item class="Part" referent="b"><Properties><string name="Name">B</string></Properties>
                <Item class="BlockMesh" referent="mb"><Properties>
                  <Content name="TextureId"><url>rbxasset://textures/face.png</url></Content>
                </Properties></Item></Item>
            </roblox>"#,
        );
        assert!(
            scene.parts[0]
                .texture
                .as_ref()
                .unwrap()
                .ends_with("textures/face.png")
        );
        assert_eq!(scene.parts[1].texture, None);
    }

    #[test]
    fn invalid_decal_values_are_errors_not_clamped() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        for property in [
            r#"<token name="Face">6</token>"#,
            r#"<float name="Transparency">1.5</float>"#,
            r#"<float name="StudsPerTileU">0</float>"#,
        ] {
            let xml = format!(
                r#"<roblox version="4"><Item class="Part" referent="p"><Properties/>
                  <Item class="Texture" referent="t"><Properties>{property}
                  <Content name="Texture"><url>rbxasset://textures/face.png</url></Content>
                  </Properties></Item></Item></roblox>"#
            );
            let model = Model::decode(xml.as_bytes()).unwrap();
            assert!(
                SceneSnapshot::from_model(&model, &root.join("content")).is_err(),
                "{property}"
            );
        }
    }
    #[test]
    fn actual_character_uses_serialized_world_transforms_colors_and_head_mesh() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let model = Model::load(root.join("content/other/character.rbxm")).unwrap();
        let scene = SceneSnapshot::from_model(&model, &root.join("content")).unwrap();
        let head = scene.parts.iter().find(|part| part.name == "Head").unwrap();
        assert_eq!(head.position, Vec3::new(0.0, 4.5, 25.5));
        assert_eq!(head.geometry, Geometry::Head);
        assert_eq!(
            head.color_srgb,
            [245.0 / 255.0, 205.0 / 255.0, 48.0 / 255.0]
        );
        assert!(
            head.rotation
                .abs_diff_eq(Quat::from_rotation_y(std::f32::consts::PI), 1e-5)
                || head
                    .rotation
                    .abs_diff_eq(-Quat::from_rotation_y(std::f32::consts::PI), 1e-5)
        );
        let torso = scene
            .parts
            .iter()
            .find(|part| part.name == "Torso")
            .unwrap();
        assert_eq!(torso.size, Vec3::new(2.0, 2.0, 1.0));
        assert_eq!(
            torso.color_srgb,
            [13.0 / 255.0, 105.0 / 255.0, 172.0 / 255.0]
        );
    }
}
