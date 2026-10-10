//! Part materials: texture folder, tiling and shader constants.
//! Sources: App/include/util/PartMaterial.h (tokens), Rendering/GfxRender/MaterialGenerator.cpp
//! (`getTiling`, `getSpecular`) and shaders/source/<material>.hlsl (`CFG_*`).

/// Serialized `Material` token for Plastic, the default.
pub const PLASTIC: u32 = 256;
pub const SMOOTH_PLASTIC: u32 = 272;
pub const NEON: u32 = 288;

/// MaterialGenerator.cpp::getTiling(PLASTIC): the Uv tiling plastic's normal detail samples at.
pub const PLASTIC_TILING: f32 = 1.3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceMaterial {
    /// Folder under `PlatformContent/pc/textures`.
    pub folder: &'static str,
    /// Texture repeats per stud (`getTiling`).
    pub tiling: f32,
    /// `CFG_SPECULAR_SCALE` and `CFG_GLOSS_SCALE`: multiply the specular map's r and g.
    pub specular_scale: f32,
    pub gloss_scale: f32,
    /// `CFG_NORMAL_SHADOW_SCALE`: brightens or darkens by the normal's x tilt.
    pub normal_shadow: f32,
    /// `CFG_SPECULAR_LOD` and `CFG_GLOSS_LOD`: the specular values at the shading distance.
    pub specular_lod: f32,
    pub gloss_lod: f32,
    /// `CFG_NORMAL_DETAIL_TILING` and `CFG_NORMAL_DETAIL_SCALE`: the `normaldetail` map.
    pub normal_detail_tiling: f32,
    pub normal_detail_scale: f32,
    /// `CFG_FAR_TILING`: far-distance tiling of the diffuse, normal and specular maps.
    pub far_tiling: f32,
    /// `CFG_FAR_DIFFUSE_CUTOFF`, `CFG_FAR_NORMAL_CUTOFF`, `CFG_FAR_SPECULAR_CUTOFF`. Zero never
    /// tiles.
    pub far_diffuse_cutoff: f32,
    pub far_normal_cutoff: f32,
    pub far_specular_cutoff: f32,
    /// `CFG_OPT_BLEND_COLOR`: the diffuse alpha selects how much of the part colour applies.
    pub blend_color: bool,
    /// `CFG_OPT_DIFFUSE_CONST`: the diffuse map is ignored; the part colour is used as is.
    pub diffuse_const: bool,
    /// `CFG_OPT_NORMAL_CONST`: the normal map is ignored.
    pub normal_const: bool,
}

const BASE: SurfaceMaterial = SurfaceMaterial {
    folder: "",
    tiling: 1.0,
    specular_scale: 1.0,
    gloss_scale: 1.0,
    normal_shadow: 0.0,
    specular_lod: 0.0,
    gloss_lod: 0.0,
    normal_detail_tiling: 0.0,
    normal_detail_scale: 0.0,
    far_tiling: 0.0,
    far_diffuse_cutoff: 0.0,
    far_normal_cutoff: 0.0,
    far_specular_cutoff: 0.0,
    blend_color: false,
    diffuse_const: false,
    normal_const: false,
};

/// The textured part material for a `Material` token, or `None` for Plastic, SmoothPlastic,
/// Neon and the terrain-only tokens (which render as Plastic). Values are the `CFG_*` constants
/// of each shader; Cobblestone's Wang tiling is not ported and uses plain tiling.
pub fn surface_material(token: u32) -> Option<SurfaceMaterial> {
    Some(match token {
        512 => SurfaceMaterial {
            folder: "wood",
            tiling: 0.2,
            specular_scale: 2.0,
            gloss_scale: 256.0,
            normal_shadow: 0.3,
            specular_lod: 0.25,
            gloss_lod: 32.0,
            normal_detail_tiling: 7.0,
            normal_detail_scale: 0.6,
            ..BASE
        },
        528 => SurfaceMaterial {
            folder: "woodplanks",
            tiling: 0.2,
            specular_scale: 2.0,
            gloss_scale: 256.0,
            normal_shadow: 0.3,
            specular_lod: 0.28,
            gloss_lod: 53.0,
            blend_color: true,
            ..BASE
        },
        784 => SurfaceMaterial {
            folder: "marble",
            tiling: 0.1,
            specular_scale: 1.0,
            gloss_scale: 128.0,
            normal_shadow: 0.1,
            specular_lod: 0.7,
            gloss_lod: 54.0,
            normal_const: true,
            ..BASE
        },
        800 => SurfaceMaterial {
            folder: "slate",
            tiling: 0.1,
            specular_scale: 0.9,
            gloss_scale: 128.0,
            normal_shadow: 0.5,
            specular_lod: 0.14,
            gloss_lod: 20.0,
            normal_detail_tiling: 5.0,
            normal_detail_scale: 1.0,
            far_tiling: 0.25,
            far_diffuse_cutoff: 0.75,
            ..BASE
        },
        816 => SurfaceMaterial {
            folder: "concrete",
            tiling: 0.15,
            specular_scale: 1.3,
            gloss_scale: 128.0,
            normal_shadow: 0.0,
            specular_lod: 0.07,
            gloss_lod: 22.0,
            normal_detail_tiling: 10.0,
            normal_detail_scale: 1.0,
            far_tiling: 0.25,
            far_diffuse_cutoff: 0.75,
            normal_const: true,
            ..BASE
        },
        832 => SurfaceMaterial {
            folder: "granite",
            tiling: 0.1,
            specular_scale: 0.5,
            gloss_scale: 128.0,
            normal_shadow: 0.1,
            specular_lod: 0.19,
            gloss_lod: 24.0,
            far_tiling: 0.25,
            far_diffuse_cutoff: 0.6,
            normal_const: true,
            ..BASE
        },
        848 => SurfaceMaterial {
            folder: "brick",
            tiling: 0.125,
            specular_scale: 1.3,
            gloss_scale: 64.0,
            normal_shadow: 0.1,
            specular_lod: 0.13,
            gloss_lod: 44.0,
            blend_color: true,
            ..BASE
        },
        864 => SurfaceMaterial {
            folder: "pebble",
            tiling: 0.1,
            specular_scale: 2.5,
            gloss_scale: 128.0,
            normal_shadow: 0.0,
            specular_lod: 0.15,
            gloss_lod: 22.0,
            normal_detail_tiling: 6.0,
            normal_detail_scale: 1.5,
            ..BASE
        },
        880 => SurfaceMaterial {
            folder: "cobblestone",
            tiling: 0.2,
            specular_scale: 3.0,
            gloss_scale: 256.0,
            normal_shadow: 0.3,
            specular_lod: 0.21,
            gloss_lod: 22.0,
            blend_color: true,
            ..BASE
        },
        1040 => SurfaceMaterial {
            folder: "rust",
            tiling: 0.1,
            specular_scale: 1.0,
            gloss_scale: 256.0,
            normal_shadow: 0.5,
            specular_lod: 0.35,
            gloss_lod: 103.0,
            far_tiling: 0.5,
            far_diffuse_cutoff: 0.6,
            blend_color: true,
            ..BASE
        },
        1056 => SurfaceMaterial {
            folder: "diamondplate",
            tiling: 0.2,
            specular_scale: 2.7,
            gloss_scale: 256.0,
            normal_shadow: 0.5,
            specular_lod: 0.9,
            gloss_lod: 160.0,
            ..BASE
        },
        1072 => SurfaceMaterial {
            folder: "aluminum",
            tiling: 0.1,
            specular_scale: 1.0,
            gloss_scale: 256.0,
            normal_shadow: 0.0,
            specular_lod: 0.94,
            gloss_lod: 240.0,
            far_tiling: 0.25,
            far_normal_cutoff: 0.75,
            diffuse_const: true,
            ..BASE
        },
        1088 => SurfaceMaterial {
            folder: "metal",
            tiling: 0.2,
            specular_scale: 2.0,
            gloss_scale: 256.0,
            normal_shadow: 0.3,
            specular_lod: 0.8,
            gloss_lod: 120.0,
            far_tiling: 0.25,
            far_specular_cutoff: 0.75,
            ..BASE
        },
        1280 => SurfaceMaterial {
            folder: "grass",
            tiling: 0.15,
            specular_scale: 1.0,
            gloss_scale: 256.0,
            normal_shadow: 0.5,
            specular_lod: 0.17,
            gloss_lod: 18.0,
            far_tiling: 0.25,
            far_diffuse_cutoff: 0.6,
            blend_color: true,
            ..BASE
        },
        1296 => SurfaceMaterial {
            folder: "sand",
            tiling: 0.1,
            specular_scale: 0.4,
            gloss_scale: 32.0,
            normal_shadow: 0.0,
            specular_lod: 0.07,
            gloss_lod: 6.0,
            ..BASE
        },
        1312 => SurfaceMaterial {
            folder: "fabric",
            tiling: 0.15,
            specular_scale: 0.2,
            gloss_scale: 128.0,
            normal_shadow: 0.2,
            specular_lod: 0.03,
            gloss_lod: 16.0,
            blend_color: true,
            ..BASE
        },
        1536 => SurfaceMaterial {
            folder: "ice",
            tiling: 0.1,
            specular_scale: 1.2,
            gloss_scale: 256.0,
            normal_shadow: 0.0,
            specular_lod: 1.0,
            gloss_lod: 190.0,
            far_tiling: 0.25,
            far_specular_cutoff: 0.75,
            diffuse_const: true,
            ..BASE
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_textured_token_has_a_texture_folder_in_the_repository() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../PlatformContent/pc/textures");
        let mut seen = 0;
        for token in 0..2100 {
            if let Some(material) = surface_material(token) {
                seen += 1;
                for file in [
                    "diffuse.dds",
                    "normal.dds",
                    "specular.dds",
                    "normaldetail.dds",
                ] {
                    let path = root.join(material.folder).join(file);
                    assert!(path.is_file(), "{}", path.display());
                }
            }
        }
        assert_eq!(seen, 17);
        assert!(root.join("plastic").join("normaldetail.dds").is_file());
    }

    #[test]
    fn plastic_family_and_terrain_only_tokens_have_no_surface_material() {
        for token in [
            PLASTIC,
            SMOOTH_PLASTIC,
            NEON,
            788,
            804,
            896,
            912,
            1328,
            1344,
            1360,
            1552,
            1792,
            2048,
        ] {
            assert_eq!(surface_material(token), None, "{token}");
        }
    }
}
