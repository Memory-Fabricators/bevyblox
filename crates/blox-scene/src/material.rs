//! Part materials: texture folder, tiling and shader constants.
//! Sources: App/include/util/PartMaterial.h (tokens), Rendering/GfxRender/MaterialGenerator.cpp
//! (`getTiling`, `getSpecular`) and shaders/source/<material>.hlsl (`CFG_*`).

/// Serialized `Material` token for Plastic, the default.
pub const PLASTIC: u32 = 256;
pub const SMOOTH_PLASTIC: u32 = 272;
pub const NEON: u32 = 288;

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
    /// `CFG_OPT_BLEND_COLOR`: the diffuse alpha selects how much of the part colour applies.
    pub blend_color: bool,
    /// `CFG_OPT_DIFFUSE_CONST`: the diffuse map is ignored; the part colour is used as is.
    pub diffuse_const: bool,
    /// `CFG_OPT_NORMAL_CONST`: the normal map is ignored.
    pub normal_const: bool,
}

const fn surface(
    folder: &'static str,
    tiling: f32,
    specular_scale: f32,
    gloss_scale: f32,
    normal_shadow: f32,
) -> SurfaceMaterial {
    SurfaceMaterial {
        folder,
        tiling,
        specular_scale,
        gloss_scale,
        normal_shadow,
        blend_color: false,
        diffuse_const: false,
        normal_const: false,
    }
}

/// The textured part material for a `Material` token, or `None` for Plastic, SmoothPlastic,
/// Neon and the terrain-only tokens (which render as Plastic).
pub fn surface_material(token: u32) -> Option<SurfaceMaterial> {
    let blend = |m: SurfaceMaterial| SurfaceMaterial {
        blend_color: true,
        ..m
    };
    let flat_diffuse = |m: SurfaceMaterial| SurfaceMaterial {
        diffuse_const: true,
        ..m
    };
    let flat_normal = |m: SurfaceMaterial| SurfaceMaterial {
        normal_const: true,
        ..m
    };
    Some(match token {
        512 => surface("wood", 0.2, 2.0, 256.0, 0.3),
        528 => blend(surface("woodplanks", 0.2, 2.0, 256.0, 0.3)),
        784 => flat_normal(surface("marble", 0.1, 1.0, 128.0, 0.1)),
        800 => surface("slate", 0.1, 0.9, 128.0, 0.5),
        816 => flat_normal(surface("concrete", 0.15, 1.3, 128.0, 0.0)),
        832 => flat_normal(surface("granite", 0.1, 0.5, 128.0, 0.1)),
        848 => blend(surface("brick", 0.125, 1.3, 64.0, 0.1)),
        864 => surface("pebble", 0.1, 2.5, 128.0, 0.0),
        // Cobblestone uses Wang tiling in the original; plain tiling at its tile scale here.
        880 => blend(surface("cobblestone", 0.2, 3.0, 256.0, 0.3)),
        1040 => blend(surface("rust", 0.1, 1.0, 256.0, 0.5)),
        1056 => surface("diamondplate", 0.2, 2.7, 256.0, 0.5),
        1072 => flat_diffuse(surface("aluminum", 0.1, 1.0, 256.0, 0.0)),
        1088 => surface("metal", 0.2, 2.0, 256.0, 0.3),
        1280 => blend(surface("grass", 0.15, 1.0, 256.0, 0.5)),
        1296 => surface("sand", 0.1, 0.4, 32.0, 0.0),
        1312 => blend(surface("fabric", 0.15, 0.2, 128.0, 0.2)),
        1536 => flat_diffuse(surface("ice", 0.1, 1.2, 256.0, 0.0)),
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
                for file in ["diffuse.dds", "normal.dds", "specular.dds"] {
                    let path = root.join(material.folder).join(file);
                    assert!(path.is_file(), "{}", path.display());
                }
            }
        }
        assert_eq!(seen, 17);
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
