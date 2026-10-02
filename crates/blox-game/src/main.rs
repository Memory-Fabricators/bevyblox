use std::{collections::HashMap, path::PathBuf};

use bevy::{app::AppExit, prelude::*};
use blox_model::Model;
use blox_scene::{
    Geometry, SceneSnapshot, decal, mesh,
    texture::{self, Wrap},
};

mod first_person;
mod walk;
use first_person::{FirstPersonCamera, FirstPersonPlugin, InitialView, Locomotion};

#[derive(Resource)]
struct PreparedScene {
    snapshot: SceneSnapshot,
    meshes: Vec<Mesh>,
    mesh_indices: Vec<usize>,
    transforms: Vec<Transform>,
    surfaces: Surfaces,
    center: Vec3,
    radius: f32,
    /// Static collision shape per part (`None` for `CanCollide = false` or unusable meshes).
    colliders: Vec<Option<bevy_rapier3d::prelude::Collider>>,
}

/// A decal/texture drawn over its part with the part's transform.
struct DecalDraw {
    part: usize,
    mesh: usize,
    image: usize,
    transparency: f32,
    /// Position among the part's decals; later decals win depth ties.
    order: usize,
}

struct Surfaces {
    images: Vec<Image>,
    /// Mesh `TextureId` image for each part, if it loaded.
    part_images: Vec<Option<usize>>,
    decals: Vec<DecalDraw>,
}

/// Decodes each distinct texture once and builds decal geometry. Anything that cannot be
/// drawn is reported and skipped; the part itself still renders.
fn prepare_surfaces(
    snapshot: &SceneSnapshot,
    meshes: &mut Vec<Mesh>,
    mesh_indices: &[usize],
) -> Surfaces {
    let mut images = Vec::new();
    let mut cache: HashMap<(PathBuf, Wrap), Option<usize>> = HashMap::new();
    let mut image = |path: &PathBuf, wrap| {
        *cache
            .entry((path.clone(), wrap))
            .or_insert_with(|| match texture::load(path, wrap) {
                Ok(loaded) => {
                    images.push(loaded);
                    Some(images.len() - 1)
                }
                Err(error) => {
                    eprintln!("Not rendered: {error}");
                    None
                }
            })
    };
    let mut part_images = Vec::with_capacity(snapshot.parts.len());
    let mut decals = Vec::new();
    for (part_index, part) in snapshot.parts.iter().enumerate() {
        part_images.push(
            part.texture
                .as_ref()
                .and_then(|path| image(path, Wrap::Repeat)),
        );
        for (order, decal) in part.decals.iter().enumerate() {
            let wrap = if decal.studs_per_tile.is_some() {
                Wrap::Repeat
            } else {
                Wrap::Clamp
            };
            let Some(image) = image(&decal.texture, wrap) else {
                continue;
            };
            let mesh = match &part.geometry {
                // The legacy generator redraws the whole mesh with the decal texture.
                Geometry::Head | Geometry::File(_) => mesh_indices[part_index],
                geometry => {
                    match decal::quad(geometry, part.size, decal, part.decals_ignore_size) {
                        Some(quad) => {
                            meshes.push(quad);
                            meshes.len() - 1
                        }
                        None => {
                            eprintln!(
                                "Not rendered: {} decal on {geometry:?} part {} (projection not ported)",
                                decal.texture.display(),
                                part.name
                            );
                            continue;
                        }
                    }
                }
            };
            decals.push(DecalDraw {
                part: part_index,
                mesh,
                image,
                transparency: decal.transparency,
                order,
            });
        }
    }
    Surfaces {
        images,
        part_images,
        decals,
    }
}

fn main() -> Result<AppExit, Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("content/other/character.rbxm"));
    if args.next().is_some() {
        return Err("usage: blox-game [model.rbxm|model.rbxmx|place.rbxl|place.rbxlx]".into());
    }
    let model = Model::load(&path)?;
    let snapshot = SceneSnapshot::from_model(&model, &root.join("content"))?;
    println!(
        "Loaded {}: {} instances, {} rendered parts, {} scripts retained but not executed",
        path.display(),
        snapshot.instance_count,
        snapshot.parts.len(),
        snapshot.script_count
    );
    for omission in &snapshot.omissions {
        eprintln!("Not rendered: {omission}");
    }
    let prepared = prepare(snapshot, &root.join("content"))?;
    let initial_view = InitialView::from_model(&model, prepared.center)?;
    println!(
        "Camera starts at {:?}, FOV {:.1}°",
        initial_view.pose.translation,
        initial_view.fov.to_degrees()
    );
    let mut app = App::new();
    app.insert_resource(prepared)
        .insert_resource(initial_view)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!(
                    "Blox Rust renderer — {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.12, 0.16, 0.22)))
        .add_plugins(FirstPersonPlugin)
        .add_plugins(walk::WalkPlugin)
        .add_systems(Startup, setup_renderer);
    Ok(app.run())
}

fn prepare(
    snapshot: SceneSnapshot,
    content: &std::path::Path,
) -> Result<PreparedScene, Box<dyn std::error::Error>> {
    let mut meshes = Vec::new();
    let mut mesh_indices = Vec::with_capacity(snapshot.parts.len());
    let mut transforms = Vec::with_capacity(snapshot.parts.len());
    let mut shared = HashMap::new();
    let mut sized_meshes = HashMap::new();
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for part in &snapshot.parts {
        let index = if matches!(part.geometry, Geometry::Head | Geometry::Torso) {
            let key = (
                part.geometry.clone(),
                part.size.to_array().map(f32::to_bits),
            );
            if let Some(index) = sized_meshes.get(&key) {
                *index
            } else {
                let index = meshes.len();
                meshes.push(if part.geometry == Geometry::Head {
                    mesh::head(mesh::load(&content.join("other/head.mesh"))?, part.size)?
                } else {
                    mesh::torso(part.size)
                });
                sized_meshes.insert(key, index);
                index
            }
        } else if let Some(index) = shared.get(&part.geometry) {
            *index
        } else {
            let index = meshes.len();
            let geometry = match &part.geometry {
                Geometry::Block => Mesh::from(Cuboid::default()),
                Geometry::Ball => Mesh::from(Sphere::new(0.5)),
                Geometry::Cylinder => Mesh::from(Cylinder::new(0.5, 1.0))
                    .rotated_by(Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)),
                Geometry::VerticalCylinder => Mesh::from(Cylinder::new(0.5, 1.0)),
                Geometry::Wedge => mesh::wedge(false),
                Geometry::CornerWedge => mesh::wedge(true),
                Geometry::File(path) => mesh::load(path)?,
                Geometry::Head | Geometry::Torso => {
                    unreachable!("size-dependent meshes are baked separately")
                }
            };
            meshes.push(geometry);
            shared.insert(part.geometry.clone(), index);
            index
        };
        let size = match part.geometry {
            Geometry::Head | Geometry::Torso => Vec3::ONE,
            Geometry::Cylinder => Vec3::new(
                part.size.x,
                part.size.y.min(part.size.z),
                part.size.y.min(part.size.z),
            ),
            Geometry::VerticalCylinder => Vec3::new(
                part.size.x.min(part.size.z),
                part.size.y,
                part.size.x.min(part.size.z),
            ),
            _ => part.size,
        };
        let transform = Transform {
            translation: part.position,
            rotation: part.rotation,
            scale: size,
        };
        let positions = meshes[index]
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|values| values.as_float3())
            .ok_or("mesh has no positions")?;
        if part.transparency < 1.0 {
            for position in positions {
                let point = transform.transform_point(Vec3::from_array(*position));
                low = low.min(point);
                high = high.max(point);
            }
        }
        mesh_indices.push(index);
        transforms.push(transform);
    }
    if !low.is_finite() || !high.is_finite() {
        return Err("model contains no visible geometry".into());
    }
    let center = (low + high) * 0.5;
    let radius = ((high - low).length() * 0.5).max(0.1);
    let surfaces = prepare_surfaces(&snapshot, &mut meshes, &mesh_indices);
    let mut shapes = HashMap::new();
    let colliders = snapshot
        .parts
        .iter()
        .zip(&mesh_indices)
        .zip(&transforms)
        .map(|((part, &mesh), transform)| {
            if !part.can_collide {
                return None;
            }
            shapes
                .entry(mesh)
                .or_insert_with(|| walk::part_collider(&part.geometry, &meshes[mesh]))
                .clone()
        })
        .collect();
    Ok(PreparedScene {
        snapshot,
        meshes,
        mesh_indices,
        transforms,
        surfaces,
        center,
        radius,
        colliders,
    })
}

fn setup_renderer(world: &mut World) {
    let prepared = world
        .remove_resource::<PreparedScene>()
        .expect("model must be prepared before startup");
    let handles: Vec<_> = {
        let mut assets = world.resource_mut::<Assets<Mesh>>();
        prepared
            .meshes
            .into_iter()
            .map(|mesh| assets.add(mesh))
            .collect()
    };
    let images: Vec<_> = {
        let mut assets = world.resource_mut::<Assets<Image>>();
        prepared
            .surfaces
            .images
            .into_iter()
            .map(|image| assets.add(image))
            .collect()
    };
    let Surfaces {
        part_images,
        decals,
        ..
    } = prepared.surfaces;
    // One material per distinct appearance so Bevy can batch the thousands of parts in a place.
    let mut materials: HashMap<[u32; 6], Handle<StandardMaterial>> = HashMap::new();
    for (((part, mesh_index), transform), part_image) in prepared
        .snapshot
        .parts
        .iter()
        .zip(prepared.mesh_indices)
        .zip(&prepared.transforms)
        .zip(part_images)
    {
        // A textured mesh takes its color from the texture, not the part color.
        let tint = match part_image {
            Some(_) => [1.0; 3],
            None => part.color_srgb,
        };
        let key = [
            tint[0].to_bits(),
            tint[1].to_bits(),
            tint[2].to_bits(),
            part.transparency.to_bits(),
            part.reflectance.to_bits(),
            part_image.map_or(u32::MAX, |i| i as u32),
        ];
        let material = materials
            .entry(key)
            .or_insert_with(|| {
                world
                    .resource_mut::<Assets<StandardMaterial>>()
                    .add(StandardMaterial {
                        base_color: Color::srgba(
                            tint[0],
                            tint[1],
                            tint[2],
                            1.0 - part.transparency,
                        ),
                        base_color_texture: part_image.map(|i| images[i].clone()),
                        alpha_mode: if part.transparency == 0.0 {
                            AlphaMode::Opaque
                        } else {
                            AlphaMode::Blend
                        },
                        reflectance: part.reflectance,
                        ..default()
                    })
            })
            .clone();
        world.spawn((
            Name::new(part.name.clone()),
            Mesh3d(handles[mesh_index].clone()),
            MeshMaterial3d(material),
            *transform,
        ));
    }
    for ((part, collider), transform) in prepared
        .snapshot
        .parts
        .iter()
        .zip(prepared.colliders)
        .zip(&prepared.transforms)
    {
        if let Some(collider) = collider {
            world.spawn((
                Name::new(format!("{} collider", part.name)),
                *transform,
                collider,
            ));
        }
    }
    for draw in decals {
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::srgba(1.0, 1.0, 1.0, 1.0 - draw.transparency),
                base_color_texture: Some(images[draw.image].clone()),
                alpha_mode: AlphaMode::Blend,
                reflectance: 0.0,
                // Legacy decals use a rasterizer depth bias of -16 to win against the part.
                depth_bias: 16.0 + draw.order as f32,
                ..default()
            });
        world.spawn((
            Name::new(format!(
                "{} decal {}",
                prepared.snapshot.parts[draw.part].name, draw.order
            )),
            Mesh3d(handles[draw.mesh].clone()),
            MeshMaterial3d(material),
            prepared.transforms[draw.part],
        ));
    }
    let initial = world
        .remove_resource::<InitialView>()
        .expect("initial view prepared before startup");
    let mut pose = initial.pose;
    if let (Locomotion::Walk, Some(eye)) = (*world.resource::<Locomotion>(), initial.spawn_eye) {
        pose.translation = eye;
    }
    let camera = FirstPersonCamera::new(&pose, prepared.center, prepared.radius);
    let far = camera.far(pose.translation);
    world.spawn((
        Camera3d::default(),
        camera,
        Projection::Perspective(PerspectiveProjection {
            fov: initial.fov,
            near: 0.1,
            far,
            ..default()
        }),
        pose,
    ));
    walk::spawn_walker(world);
    world.spawn((
        DirectionalLight {
            shadow_maps_enabled: true,
            illuminance: 12_000.0,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.9, -0.5, 0.0)),
    ));
    println!(
        "Click to capture the mouse. Walking: WASD/arrows, Space jumps. F toggles legacy fly mode (Q/E down/up, Shift slow). Esc releases."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_character_meshes_keep_world_offsets_and_frame_the_actual_model() {
        let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content");
        let model = Model::load(content.join("other/character.rbxm")).unwrap();
        let scene = SceneSnapshot::from_model(&model, &content).unwrap();
        let prepared = prepare(scene, &content).unwrap();
        assert!((prepared.center.z - 25.5).abs() < 0.001);
        let head = prepared
            .snapshot
            .parts
            .iter()
            .position(|part| part.name == "Head")
            .unwrap();
        assert_eq!(prepared.transforms[head].scale, Vec3::ONE);
        let right_arm = prepared
            .snapshot
            .parts
            .iter()
            .position(|part| part.name == "Right Arm")
            .unwrap();
        let left_arm = prepared
            .snapshot
            .parts
            .iter()
            .position(|part| part.name == "Left Arm")
            .unwrap();
        assert_eq!(
            prepared.mesh_indices[right_arm],
            prepared.mesh_indices[left_arm]
        );
        assert_ne!(
            prepared.transforms[right_arm].translation,
            prepared.transforms[left_arm].translation
        );
    }
}
