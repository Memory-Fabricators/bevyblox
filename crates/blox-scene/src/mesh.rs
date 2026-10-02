use std::path::Path;

use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use blox_model::{ModelError, ModelResult};

/// Port of the v1/v2 layouts in Rendering/GfxBase/FileMeshData.cpp and MeshFileStructs.h.
pub fn load(path: &Path) -> ModelResult<Mesh> {
    decode(&std::fs::read(path)?)
}

pub fn decode(bytes: &[u8]) -> ModelResult<Mesh> {
    let line = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(|| invalid("missing mesh version"))?;
    let version = std::str::from_utf8(&bytes[..line])
        .map_err(|_| invalid("invalid mesh version"))?
        .trim_end();
    let data = &bytes[line + 1..];
    let (positions, normals, uvs, indices) = match version {
        "version 1.00" | "version 1.01" => {
            let text = std::str::from_utf8(data).map_err(|_| invalid("invalid text mesh"))?;
            let first = text
                .find('[')
                .ok_or_else(|| invalid("missing mesh vertices"))?;
            let faces: usize = text[..first]
                .trim()
                .parse()
                .map_err(|_| invalid("invalid face count"))?;
            let vertex_count = faces
                .checked_mul(3)
                .ok_or_else(|| invalid("mesh count overflow"))?;
            if vertex_count > data.len() / 9 {
                return Err(invalid("mesh count exceeds payload"));
            }
            let scale = if version == "version 1.00" { 0.5 } else { 1.0 };
            let mut values = text[first..].trim();
            let mut positions = Vec::with_capacity(vertex_count);
            let mut normals = Vec::with_capacity(vertex_count);
            let mut uvs = Vec::with_capacity(vertex_count);
            for _ in 0..vertex_count {
                let position = triple(&mut values)?;
                let normal = Vec3::from_array(triple(&mut values)?).normalize_or_zero();
                let uv = triple(&mut values)?;
                positions.push(position.map(|component| component * scale));
                normals.push(normal.to_array());
                uvs.push([uv[0], 1.0 - uv[1]]);
            }
            if !values.trim().is_empty() {
                return Err(invalid("unexpected text mesh tail"));
            }
            (positions, normals, uvs, (0..vertex_count as u32).collect())
        }
        "version 2.00" => {
            if data.len() < 12 {
                return Err(invalid("truncated mesh header"));
            }
            let header_size = u16::from_le_bytes([data[0], data[1]]) as usize;
            let vertex_stride = data[2] as usize;
            let face_stride = data[3] as usize;
            let vertex_count = uint(data, 4)? as usize;
            let face_count = uint(data, 8)? as usize;
            if header_size < 12 || ![36, 40].contains(&vertex_stride) || face_stride != 12 {
                return Err(invalid("unsupported v2 mesh layout"));
            }
            let vertices_end = header_size
                .checked_add(
                    vertex_count
                        .checked_mul(vertex_stride)
                        .ok_or_else(|| invalid("mesh count overflow"))?,
                )
                .ok_or_else(|| invalid("mesh count overflow"))?;
            let end = vertices_end
                .checked_add(
                    face_count
                        .checked_mul(12)
                        .ok_or_else(|| invalid("mesh count overflow"))?,
                )
                .ok_or_else(|| invalid("mesh count overflow"))?;
            if end > data.len() || vertex_count == 0 || face_count == 0 {
                return Err(invalid("truncated or empty mesh"));
            }
            let mut positions = Vec::with_capacity(vertex_count);
            let mut normals = Vec::with_capacity(vertex_count);
            let mut uvs = Vec::with_capacity(vertex_count);
            for vertex in data[header_size..vertices_end].chunks_exact(vertex_stride) {
                positions.push([float(vertex, 0)?, float(vertex, 4)?, float(vertex, 8)?]);
                normals.push(
                    Vec3::new(float(vertex, 12)?, float(vertex, 16)?, float(vertex, 20)?)
                        .normalize_or_zero()
                        .to_array(),
                );
                uvs.push([float(vertex, 24)?, float(vertex, 28)?]);
            }
            let mut indices = Vec::with_capacity(face_count * 3);
            for index in data[vertices_end..end].chunks_exact(4) {
                let index = uint(index, 0)?;
                if index as usize >= vertex_count {
                    return Err(invalid("mesh index out of range"));
                }
                indices.push(index);
            }
            (positions, normals, uvs, indices)
        }
        _ => return Err(invalid(&format!("unsupported mesh format {version}"))),
    };
    Ok(build(positions, normals, uvs, indices))
}

pub fn head(mut mesh: Mesh, scale: Vec3) -> ModelResult<Mesh> {
    let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    else {
        return Err(invalid("head mesh lacks positions"));
    };
    // Exact head-cap scaling/extrusion rule in GeometryGenerator.cpp:616-638.
    let extrusion = 0.625 * (scale.y - scale.x);
    for position in positions {
        position[0] *= scale.x;
        position[1] = position[1] * scale.x
            + if position[1] > 0.0 {
                extrusion
            } else {
                -extrusion
            };
        position[2] *= scale.z;
    }
    Ok(mesh)
}

pub fn wedge(corner: bool) -> Mesh {
    let corners = [
        Vec3::new(-0.5, -0.5, -0.5),
        Vec3::new(-0.5, -0.5, 0.5),
        Vec3::new(-0.5, 0.5, -0.5),
        Vec3::new(-0.5, 0.5, 0.5),
        Vec3::new(0.5, -0.5, -0.5),
        Vec3::new(0.5, -0.5, 0.5),
        Vec3::new(0.5, 0.5, -0.5),
        Vec3::new(0.5, 0.5, 0.5),
    ];
    // Face tables ported from GeometryGenerator.cpp:1462-1480; skip absent +Y face.
    let faces = if corner {
        [
            [6, 6, 4, 5],
            [6, 6, 5, 1],
            [6, 6, 1, 0],
            [0, 4, 1, 5],
            [6, 6, 0, 4],
        ]
    } else {
        [
            [7, 7, 4, 5],
            [7, 3, 5, 1],
            [3, 3, 1, 0],
            [0, 4, 1, 5],
            [3, 7, 0, 4],
        ]
    };
    polyhedron(&corners, &faces)
}

pub fn torso(size: Vec3) -> Mesh {
    polyhedron(
        &torso_corners(size),
        &[
            [6, 7, 4, 5],
            [3, 7, 2, 6],
            [7, 3, 5, 1],
            [3, 2, 1, 0],
            [0, 4, 1, 5],
            [2, 6, 0, 4],
        ],
    )
}

/// Corner order shared with the cube tables: index = 4*(x>0) + 2*(y>0) + (z>0).
pub fn torso_corners(size: Vec3) -> [Vec3; 8] {
    // GeometryGenerator::addTorso: the shoulder inset depends on both X and Z.
    let extent = size * 0.5;
    let inset = extent.x.min(size.z * 0.3);
    [
        Vec3::new(-extent.x, -extent.y, -extent.z),
        Vec3::new(-extent.x, -extent.y, extent.z),
        Vec3::new(-extent.x + inset, extent.y, -extent.z),
        Vec3::new(-extent.x + inset, extent.y, extent.z),
        Vec3::new(extent.x, -extent.y, -extent.z),
        Vec3::new(extent.x, -extent.y, extent.z),
        Vec3::new(extent.x - inset, extent.y, -extent.z),
        Vec3::new(extent.x - inset, extent.y, extent.z),
    ]
}

fn polyhedron(corners: &[Vec3; 8], faces: &[[usize; 4]]) -> Mesh {
    let mut positions = Vec::with_capacity(faces.len() * 6);
    let mut normals = Vec::with_capacity(faces.len() * 6);
    let mut uvs = Vec::with_capacity(faces.len() * 6);
    let mut indices = Vec::with_capacity(faces.len() * 6);
    for &[a, b, c, d] in faces {
        for [a, b, c] in [[a, b, c], [c, b, d]] {
            let normal = (corners[b] - corners[a]).cross(corners[c] - corners[a]);
            if normal.length_squared() == 0.0 {
                continue;
            }
            let base = positions.len() as u32;
            positions.extend([
                corners[a].to_array(),
                corners[b].to_array(),
                corners[c].to_array(),
            ]);
            normals.extend([normal.normalize().to_array(); 3]);
            uvs.extend([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
            indices.extend([base, base + 1, base + 2]);
        }
    }
    build(positions, normals, uvs, indices)
}

pub(crate) fn build(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}
fn triple(text: &mut &str) -> ModelResult<[f32; 3]> {
    let trimmed = text
        .trim_start()
        .strip_prefix('[')
        .ok_or_else(|| invalid("expected mesh triple"))?;
    let end = trimmed
        .find(']')
        .ok_or_else(|| invalid("truncated mesh triple"))?;
    let mut components = trimmed[..end].split(',');
    let mut triple = [0.0_f32; 3];
    for value in &mut triple {
        *value = components
            .next()
            .ok_or_else(|| invalid("missing mesh component"))?
            .trim()
            .parse()
            .map_err(|_| invalid("invalid mesh float"))?;
        if !value.is_finite() {
            return Err(invalid("nonfinite mesh float"));
        }
    }
    if components.next().is_some() {
        return Err(invalid("extra mesh component"));
    }
    *text = &trimmed[end + 1..];
    Ok(triple)
}
fn uint(bytes: &[u8], offset: usize) -> ModelResult<u32> {
    let bytes = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| invalid("truncated mesh value"))?;
    Ok(u32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| invalid("invalid mesh value"))?,
    ))
}
fn float(bytes: &[u8], offset: usize) -> ModelResult<f32> {
    let value = f32::from_bits(uint(bytes, offset)?);
    if !value.is_finite() {
        return Err(invalid("nonfinite mesh float"));
    }
    Ok(value)
}
fn invalid(message: &str) -> ModelError {
    ModelError(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torso_shoulders_use_depth_and_clamp_at_center() {
        for (size, top_width) in [
            (Vec3::new(4.0, 2.0, 1.0), 1.7),
            (Vec3::new(1.0, 2.0, 10.0), 0.0),
        ] {
            let mesh = torso(size);
            let points = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .unwrap()
                .as_float3()
                .unwrap();
            for point in points {
                let expected = if point[1] > 0.0 {
                    top_width
                } else {
                    size.x * 0.5
                };
                assert!((point[0].abs() - expected).abs() < 0.00001);
                assert_eq!(point[1].abs(), size.y * 0.5);
                assert_eq!(point[2].abs(), size.z * 0.5);
            }
        }
    }
    #[test]
    fn v1_scale_and_uv_match_reference() {
        let data =
            b"version 1.00\n1\n[2,0,0][1,0,0][0,0.25,0][0,2,0][0,1,0][0,0,0][0,0,2][0,0,1][0,0,0]";
        let mesh = decode(data).unwrap();
        assert_eq!(
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                .unwrap()
                .as_float3()
                .unwrap(),
            &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        );
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("UV missing")
        };
        assert_eq!(uvs[0], [0.0, 0.75]);
        assert!(decode(&data[..data.len() - 1]).is_err());
    }

    #[test]
    fn head_caps_scale_radially_and_extrude_vertically() {
        let mesh = build(
            vec![
                [0.625, 0.625, 0.625],
                [-0.625, -0.625, -0.625],
                [0.25, 0.3, 0.25],
            ],
            vec![[0.0, 1.0, 0.0]; 3],
            vec![[0.0, 0.0]; 3],
            vec![0, 1, 2],
        );
        let mesh = head(mesh, Vec3::new(2.0, 3.0, 2.0)).unwrap();
        assert_eq!(
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                .unwrap()
                .as_float3()
                .unwrap(),
            &[
                [1.25, 1.875, 1.25],
                [-1.25, -1.875, -1.25],
                [0.5, 1.225, 0.5]
            ],
        );
    }
}
