//! The legacy renderer's look: a forward Lambert shader in gamma space (`legacy.wesl`), no
//! tonemapping, and a six-face skybox.
use std::path::Path;

use bevy::{
    asset::{RenderAssetUsages, embedded_asset},
    image::{Image, ImageSampler},
    prelude::*,
    render::render_resource::{
        AsBindGroup, Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor,
        TextureViewDimension,
    },
    shader::ShaderRef,
};
use blox_model::{ModelError, ModelResult};
use blox_scene::lighting::{Lighting, Shading};

pub struct LegacyRenderPlugin;

impl Plugin for LegacyRenderPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "legacy.wesl");
        app.add_plugins(MaterialPlugin::<LegacyMaterial>::default());
    }
}

/// Distance at which edge outlines fade out (`shadingDistance` of the top quality tiers).
const SHADING_DISTANCE: f32 = 300.0;

pub const FLAG_STUDS: u32 = 1;
pub const FLAG_OUTLINE: u32 = 2;
pub const FLAG_TEXTURE: u32 = 4;
pub const FLAG_UNLIT: u32 = 8;

#[derive(Clone, Debug, PartialEq, bevy::render::render_resource::ShaderType)]
pub struct LegacyUniform {
    color: Vec4,
    ambient: Vec4,
    key: Vec4,
    fill: Vec4,
    fog_color: Vec4,
    fog: Vec4,
    spec: Vec4,
    packed: UVec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct LegacyMaterial {
    #[uniform(0)]
    pub data: LegacyUniform,
    #[texture(1)]
    #[sampler(2)]
    pub texture: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    /// Decals use a depth bias so they win against the part they sit on.
    pub depth_bias: f32,
}

impl Material for LegacyMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://blox_game/legacy.wesl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    fn depth_bias(&self) -> f32 {
        self.depth_bias
    }
}

/// What differs between parts; everything else comes from the place's lighting.
#[derive(Clone, Debug)]
pub struct Appearance {
    /// Gamma-space part colour; white for textured surfaces.
    pub color: [f32; 3],
    pub alpha: f32,
    /// Legacy `getSpecular`: (intensity, power).
    pub specular: (f32, f32),
    pub flags: u32,
    /// SurfaceType per face in NormalId order.
    pub surfaces: [u8; 6],
    pub texture: Option<Handle<Image>>,
}

impl LegacyMaterial {
    pub fn new(lighting: &Lighting, shading: &Shading, appearance: &Appearance) -> Self {
        let rgb = |c: [f32; 3]| Vec4::new(c[0], c[1], c[2], 1.0);
        let surfaces = appearance
            .surfaces
            .iter()
            .enumerate()
            .fold(0u32, |bits, (face, ty)| {
                bits | (u32::from(*ty & 15) << (4 * face))
            });
        Self {
            data: LegacyUniform {
                color: Vec4::new(
                    appearance.color[0],
                    appearance.color[1],
                    appearance.color[2],
                    appearance.alpha,
                ),
                ambient: rgb(shading.ambient),
                key: rgb(shading.key),
                fill: rgb(shading.fill),
                fog_color: rgb(lighting.fog_color),
                fog: Vec4::new(lighting.fog_start, lighting.fog_end, SHADING_DISTANCE, 0.0),
                spec: Vec4::new(appearance.specular.0, appearance.specular.1, 0.0, 0.0),
                packed: UVec4::new(surfaces, appearance.flags, 0, 0),
            },
            texture: appearance.texture.clone(),
            alpha_mode: if appearance.alpha < 1.0 {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            },
            depth_bias: 0.0,
        }
    }
}

/// MaterialGenerator.cpp::getSpecular as (intensity/255, power). Materials with no part
/// material (terrain-only tokens) fall back to Plastic.
pub fn specular(material: u32) -> (f32, f32) {
    let (intensity, power) = match material {
        272 | 288 => (191.0, 81.0),
        512 => (64.0, 32.0),
        528 => (71.0, 53.0),
        784 => (179.0, 54.0),
        800 => (36.0, 20.0),
        816 => (38.0, 22.0),
        832 => (48.0, 24.0),
        848 => (33.0, 44.0),
        864 => (18.0, 22.0),
        880 => (54.0, 22.0),
        1040 => (89.0, 103.0),
        1056 => (230.0, 160.0),
        1072 => (238.0, 240.0),
        1088 => (204.0, 120.0),
        1280 => (43.0, 18.0),
        1296 => (18.0, 6.0),
        1312 => (8.0, 16.0),
        1536 => (255.0, 190.0),
        _ => (102.0, 9.0),
    };
    (intensity / 255.0, power)
}

/// Default sky shipped in `content/sky` (the place's own `Sky` faces are remote asset ids).
/// Faces map as Sky.cpp: Rt +X, Lf -X, Up +Y, Dn -Y, Bk +Z, Ft -Z.
pub fn load_sky(content: &Path) -> ModelResult<Image> {
    let mut faces = Vec::with_capacity(6);
    for name in ["rt", "lf", "up", "dn", "bk", "ft"] {
        let path = content.join(format!("sky/null_plainsky512_{name}.jpg"));
        let bytes = std::fs::read(&path)
            .map_err(|e| ModelError(format!("sky face {}: {e}", path.display())))?;
        let image = Image::from_buffer(
            &bytes,
            bevy::image::ImageType::Extension("jpg"),
            bevy::image::CompressedImageFormats::NONE,
            true,
            ImageSampler::linear(),
            RenderAssetUsages::RENDER_WORLD,
        )
        .map_err(|e| ModelError(format!("sky face {}: {e}", path.display())))?;
        faces.push(image);
    }
    let (width, height) = (faces[0].width(), faces[0].height());
    let mut data = Vec::new();
    for face in &faces {
        if (face.width(), face.height()) != (width, height) || width != height {
            return Err(ModelError("sky faces must be equal squares".into()));
        }
        data.extend_from_slice(
            face.data
                .as_ref()
                .ok_or_else(|| ModelError("sky face has no data".into()))?,
        );
    }
    let mut cube = Image::new(
        Extent3d {
            width,
            height: height * 6,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    cube.reinterpret_stacked_2d_as_array(6)
        .map_err(|e| ModelError(format!("sky cubemap: {e:?}")))?;
    cube.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    Ok(cube)
}
