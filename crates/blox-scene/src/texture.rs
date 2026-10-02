//! Local texture decoding. Sampler modes follow createTexturedMaterial in
//! Rendering/GfxRender/MaterialGenerator.cpp.
use std::path::Path;

use bevy::{
    asset::RenderAssetUsages,
    image::{
        CompressedImageFormats, Image, ImageAddressMode, ImageSampler, ImageSamplerDescriptor,
        ImageType,
    },
};
use blox_model::{ModelError, ModelResult};

/// How a texture's UVs outside [0, 1] sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wrap {
    /// `Decal`: Address_Clamp, anisotropic.
    Clamp,
    /// `Texture` and mesh `TextureId`: Address_Wrap. Texture also uses anisotropic filtering.
    Repeat,
}

pub fn load(path: &Path, wrap: Wrap) -> ModelResult<Image> {
    let bytes =
        std::fs::read(path).map_err(|e| ModelError(format!("texture {}: {e}", path.display())))?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| ModelError(format!("texture {} has no extension", path.display())))?;
    decode(&bytes, extension, wrap)
        .map_err(|e| ModelError(format!("texture {}: {e}", path.display())))
}

pub fn decode(bytes: &[u8], extension: &str, wrap: Wrap) -> Result<Image, String> {
    let address_mode = match wrap {
        Wrap::Clamp => ImageAddressMode::ClampToEdge,
        Wrap::Repeat => ImageAddressMode::Repeat,
    };
    Image::from_buffer(
        bytes,
        ImageType::Extension(&extension.to_ascii_lowercase()),
        CompressedImageFormats::NONE,
        // The legacy shader samples and blends raw (gamma-space) values; see blox-game's legacy.wesl.
        false,
        ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: address_mode,
            address_mode_v: address_mode,
            address_mode_w: address_mode,
            anisotropy_clamp: 16,
            ..ImageSamplerDescriptor::linear()
        }),
        RenderAssetUsages::RENDER_WORLD,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::image::ImageSampler;

    fn content(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/textures")
            .join(name)
    }

    #[test]
    fn real_png_and_jpeg_decode_with_the_requested_wrap_mode() {
        let face = load(&content("face.png"), Wrap::Clamp).unwrap();
        assert_eq!((face.width(), face.height()), (128, 128));
        let ImageSampler::Descriptor(sampler) = &face.sampler else {
            panic!("descriptor")
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::ClampToEdge);
        let grass = load(&content("Grass_Texture.jpg"), Wrap::Repeat).unwrap();
        let ImageSampler::Descriptor(sampler) = &grass.sampler else {
            panic!("descriptor")
        };
        assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat);
    }

    #[test]
    fn corrupt_and_unsupported_files_are_errors() {
        assert!(decode(b"not a png", "png", Wrap::Clamp).is_err());
        assert!(decode(&[], "png", Wrap::Clamp).is_err());
        assert!(decode(b"x", "unknownext", Wrap::Clamp).is_err());
        assert!(load(&content("missing.png"), Wrap::Clamp).is_err());
    }
}
