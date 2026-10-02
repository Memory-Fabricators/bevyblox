//! Minimal DDS reader for the legacy `PlatformContent` textures: 2D, DXT1 or DXT5, with the
//! stored mip chain. Everything is decoded to RGBA8 on the CPU so no GPU compression support
//! is needed. Values are returned raw (non-linear), as the legacy shaders sample them.
use bevy::{
    asset::RenderAssetUsages,
    image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use blox_model::{ModelError, ModelResult};

const HEADER: usize = 128;
const MAX_DIMENSION: u32 = 8192;

pub fn load(path: &std::path::Path) -> ModelResult<Image> {
    let bytes =
        std::fs::read(path).map_err(|e| ModelError(format!("texture {}: {e}", path.display())))?;
    decode(&bytes).map_err(|e| ModelError(format!("texture {}: {e}", path.display())))
}

fn le(bytes: &[u8], offset: usize) -> Result<u32, String> {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| "truncated DDS header".to_string())
}

pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    if bytes.get(..4) != Some(b"DDS ") || le(bytes, 4)? != 124 {
        return Err("not a DDS file".into());
    }
    let (height, width) = (le(bytes, 12)?, le(bytes, 16)?);
    let mips = le(bytes, 28)?.max(1);
    let four_cc = bytes.get(84..88).ok_or("truncated DDS header")?;
    let block_bytes = match four_cc {
        b"DXT1" => 8,
        b"DXT5" => 16,
        other => return Err(format!("unsupported DDS format {other:?}")),
    };
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(format!("invalid DDS size {width}x{height}"));
    }
    if mips > 32 - width.max(height).leading_zeros() {
        return Err(format!("{mips} mip levels exceed the texture size"));
    }
    let mut data = Vec::new();
    let mut offset = HEADER;
    let (mut w, mut h) = (width, height);
    for _ in 0..mips {
        let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
        let end = offset
            .checked_add(bw * bh * block_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or("truncated DDS data")?;
        let level_start = data.len();
        data.resize(level_start + (w * h * 4) as usize, 0);
        for (index, block) in bytes[offset..end].chunks_exact(block_bytes).enumerate() {
            let pixels = if block_bytes == 8 {
                bc1(block, true)
            } else {
                bc3(block)
            };
            let (bx, by) = ((index % bw) * 4, (index / bw) * 4);
            for (i, pixel) in pixels.iter().enumerate() {
                let (x, y) = (bx + i % 4, by + i / 4);
                if x < w as usize && y < h as usize {
                    let at = level_start + (y * w as usize + x) * 4;
                    data[at..at + 4].copy_from_slice(pixel);
                }
            }
        }
        offset = end;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    // `Image::new` validates the data length against level 0 only; the mip chain follows it.
    let level_zero = (width * height * 4) as usize;
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data[..level_zero].to_vec(),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mips;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..ImageSamplerDescriptor::linear()
    });
    Ok(image)
}

fn rgb565(value: u16) -> [u8; 3] {
    let expand = |v: u16, bits: u32| ((v << (8 - bits)) | (v >> (2 * bits - 8))) as u8;
    [
        expand(value >> 11, 5),
        expand((value >> 5) & 0x3f, 6),
        expand(value & 0x1f, 5),
    ]
}

/// One 4x4 BC1 block. DXT1 with `c0 <= c1` has a transparent fourth colour.
fn bc1(block: &[u8], allow_alpha: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u32, wb: u32, div: u32| -> [u8; 4] {
        let ch = |i: usize| ((u32::from(a[i]) * wa + u32::from(b[i]) * wb) / div) as u8;
        [ch(0), ch(1), ch(2), 255]
    };
    let palette = if c0 > c1 || !allow_alpha {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            mix(2, 1, 3),
            mix(1, 2, 3),
        ]
    } else {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            mix(1, 1, 2),
            [0, 0, 0, 0],
        ]
    };
    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    std::array::from_fn(|i| palette[((indices >> (2 * i)) & 3) as usize])
}

/// One 4x4 BC3 block: an interpolated alpha block followed by a BC1 colour block.
fn bc3(block: &[u8]) -> [[u8; 4]; 16] {
    let (a0, a1) = (u32::from(block[0]), u32::from(block[1]));
    let alphas: [u8; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            n => (((8 - n as u32) * a0 + (n as u32 - 1) * a1) / 7) as u8,
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            6 => 0,
            7 => 255,
            n => (((6 - n as u32) * a0 + (n as u32 - 1) * a1) / 5) as u8,
        })
    };
    let bits = block[2..8]
        .iter()
        .enumerate()
        .fold(0u64, |acc, (i, b)| acc | (u64::from(*b) << (8 * i)));
    let mut pixels = bc1(&block[8..16], false);
    for (i, pixel) in pixels.iter_mut().enumerate() {
        pixel[3] = alphas[((bits >> (3 * i)) & 7) as usize];
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn textures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../PlatformContent/pc/textures")
    }

    fn pixel(image: &Image, mip: usize, x: usize, y: usize) -> [u8; 4] {
        let data = image.data.as_ref().unwrap();
        let (mut w, mut h, mut offset) = (image.width() as usize, image.height() as usize, 0);
        for _ in 0..mip {
            offset += w * h * 4;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        let at = offset + (y * w + x) * 4;
        data[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn bc1_palette_matches_the_specification() {
        // c0 = pure red (565 0xF800), c1 = pure blue (0x001F); c0 > c1 => 4-colour mode.
        let block = [0x00, 0xF8, 0x1F, 0x00, 0b11_10_01_00, 0, 0, 0];
        let pixels = bc1(&block, true);
        assert_eq!(pixels[0], [255, 0, 0, 255]);
        assert_eq!(pixels[1], [0, 0, 255, 255]);
        assert_eq!(pixels[2], [170, 0, 85, 255]);
        assert_eq!(pixels[3], [85, 0, 170, 255]);
        // c0 <= c1 => 3-colour mode with a transparent fourth entry.
        let block = [0x1F, 0x00, 0x00, 0xF8, 0b11_10_01_00, 0, 0, 0];
        let pixels = bc1(&block, true);
        assert_eq!(pixels[2], [127, 0, 127, 255]);
        assert_eq!(pixels[3], [0, 0, 0, 0]);
    }

    #[test]
    fn bc3_alpha_interpolates_in_both_modes() {
        let mut block = [0u8; 16];
        block[0] = 200;
        block[1] = 100;
        block[2] = 0b0000_0010; // pixel 0 -> index 2, pixel 1 -> index 0
        assert_eq!(bc3(&block)[0][3], ((6 * 200 + 100) / 7) as u8);
        assert_eq!(bc3(&block)[1][3], 200);
        block[0] = 10;
        block[1] = 100; // a0 <= a1: six interpolants then 0 and 255
        block[2] = 0b0011_0000; // pixel 0 -> index 0, pixel 1 -> index 6, pixel 2 -> index 0
        assert_eq!(bc3(&block)[1][3], 0);
    }

    #[test]
    fn real_stud_atlas_decodes_with_its_full_mip_chain() {
        let image = load(&textures().join("studs.dds")).unwrap();
        assert_eq!((image.width(), image.height()), (128, 2048));
        assert_eq!(image.texture_descriptor.mip_level_count, 12);
        let expected: usize = (0..12)
            .map(|m| ((128 >> m).max(1) * (2048 >> m).max(1)) * 4)
            .sum();
        assert_eq!(image.data.as_ref().unwrap().len(), expected);
        // The stud row (top quarter) holds relief; the smooth texel used for flat faces is near mid-gray.
        let relief: Vec<u8> = (0..64).map(|x| pixel(&image, 0, x, 20)[0]).collect();
        assert!(relief.iter().max().unwrap() - relief.iter().min().unwrap() > 20);
        let flat = pixel(&image, 0, 0, 0)[0];
        assert!((90..=170).contains(&flat), "flat texel {flat}");
    }

    #[test]
    fn dxt5_normal_maps_and_tiny_mips_decode() {
        let normal = load(&textures().join("brick/normal.dds")).unwrap();
        assert_eq!(normal.texture_descriptor.mip_level_count, 11);
        // The last mip is 1x1; sizes below a block must still be addressable.
        assert_eq!(pixel(&normal, 10, 0, 0)[3] > 0, true);
        let tiny = load(&textures().join("aluminum/diffuse.dds")).unwrap();
        assert_eq!((tiny.width(), tiny.height()), (4, 4));
    }

    #[test]
    fn corrupt_files_are_errors() {
        let good = std::fs::read(textures().join("aluminum/diffuse.dds")).unwrap();
        assert!(decode(&good[..good.len() - 1]).is_err());
        assert!(decode(&good[..100]).is_err());
        assert!(decode(b"PNG....").is_err());
        let mut bad = good.clone();
        bad[84..88].copy_from_slice(b"DXT3");
        assert!(decode(&bad).is_err());
        let mut huge = good.clone();
        huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&huge).is_err());
        let mut mips = good;
        mips[28..32].copy_from_slice(&40u32.to_le_bytes());
        assert!(decode(&mips).is_err());
    }
}
