//! Decal/Texture face quads, ported from the `decal` branches of addBlock, addWedge and
//! addTorso in Rendering/GfxRender/GeometryGenerator.cpp and getDecalUV (same file).
//!
//! Blocks and wedges are generated in unit space (+-0.5) and scaled by the part transform;
//! torsos are baked at part size. File-mesh decals reuse the part's mesh, so they need no quad.
use bevy::{
    mesh::Mesh,
    prelude::{Vec2, Vec3},
};

use crate::{DecalSnapshot, Face, Geometry, mesh::torso_corners};

/// Cube corner `i` has +X, +Y, +Z when bits 2, 1, 0 are set.
fn unit_corners() -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 4 != 0 { 0.5 } else { -0.5 },
            if i & 2 != 0 { 0.5 } else { -0.5 },
            if i & 1 != 0 { 0.5 } else { -0.5 },
        )
    })
}

// The `decal` face tables in GeometryGenerator.cpp (they differ from the regular tables
// only for the bottom face), indexed by NormalId.
const BLOCK: [[usize; 4]; 6] = [
    [6, 7, 4, 5],
    [3, 7, 2, 6],
    [7, 3, 5, 1],
    [3, 2, 1, 0],
    [5, 1, 4, 0],
    [2, 6, 0, 4],
];
const WEDGE: [[usize; 4]; 6] = [
    [7, 7, 4, 5],
    [3, 7, 0, 4],
    [7, 3, 5, 1],
    [3, 3, 1, 0],
    [5, 1, 4, 0],
    [3, 7, 0, 4],
];
const CORNER_WEDGE: [[usize; 4]; 6] = [
    [6, 6, 4, 5],
    [0, 1, 6, 5],
    [6, 6, 5, 1],
    [6, 6, 1, 0],
    [5, 1, 4, 0],
    [6, 6, 0, 4],
];

/// getDecalUVVertical<false>: UV extent of the face. Plain decals and block/cylinder
/// meshes (`ignore_size`) stretch one copy over the face; Textures repeat per studs.
pub fn uv_extent(decal: &DecalSnapshot, size: Vec3, ignore_size: bool) -> Vec2 {
    let Some(tile) = decal.studs_per_tile.filter(|_| !ignore_size) else {
        return Vec2::ONE;
    };
    let extent = match decal.face {
        Face::Right => Vec2::new(size.z, size.y),
        Face::Top => Vec2::new(-size.x, size.z),
        Face::Back => Vec2::new(size.x, size.y),
        Face::Left => Vec2::new(-size.z, size.y),
        Face::Bottom => Vec2::new(size.x, -size.z),
        Face::Front => Vec2::new(-size.x, size.y),
    };
    extent / Vec2::from(tile)
}

/// `None` for geometry whose decal projection is not ported (ball, cylinders) or that
/// draws decals by re-drawing the part mesh (head, file meshes).
pub fn quad(
    geometry: &Geometry,
    size: Vec3,
    decal: &DecalSnapshot,
    ignore_size: bool,
) -> Option<Mesh> {
    let (corners, faces, wedge, corner) = match geometry {
        Geometry::Block => (unit_corners(), &BLOCK, false, false),
        Geometry::Wedge => (unit_corners(), &WEDGE, true, false),
        Geometry::CornerWedge => (unit_corners(), &CORNER_WEDGE, false, true),
        Geometry::Torso => (torso_corners(size), &BLOCK, false, false),
        _ => return None,
    };
    let face = decal.face;
    let uv = uv_extent(decal, size, ignore_size);
    let mut uvs = if corner && face == Face::Top {
        // Stand the decal on the base of the corner wedge.
        [[0.0, 0.0], [0.0, uv.y], [uv.x, 0.0], [uv.x, uv.y]]
    } else {
        [[uv.x, 0.0], [0.0, 0.0], [uv.x, uv.y], [0.0, uv.y]]
    };
    // Collapsed triangle sides of wedges (the "fix" lines in addWedge).
    if (wedge && face == Face::Left) || (corner && matches!(face, Face::Right | Face::Back)) {
        uvs[1] = uvs[0];
    }
    let [a, b, c, d] = faces[face.index()].map(|i| corners[i]);
    // Two triangles (0,1,2) and (2,1,3), as fillQuadIndices; the first can be degenerate.
    let normal = [(a, b, c), (c, b, d)]
        .into_iter()
        .map(|(p, q, r)| (q - p).cross(r - p))
        .find(|n| n.length_squared() > 0.0)?
        .normalize();
    Some(crate::mesh::build(
        vec![a.to_array(), b.to_array(), c.to_array(), d.to_array()],
        vec![normal.to_array(); 4],
        uvs.to_vec(),
        vec![0, 1, 2, 2, 1, 3],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::{Indices, VertexAttributeValues};
    use std::path::PathBuf;

    fn decal(face: Face, studs_per_tile: Option<[f32; 2]>) -> DecalSnapshot {
        DecalSnapshot {
            face,
            texture: PathBuf::new(),
            transparency: 0.0,
            studs_per_tile,
        }
    }
    fn positions(mesh: &Mesh) -> Vec<[f32; 3]> {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap()
            .to_vec()
    }
    fn uvs(mesh: &Mesh) -> Vec<[f32; 2]> {
        match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(VertexAttributeValues::Float32x2(uvs)) => uvs.clone(),
            _ => panic!("missing UVs"),
        }
    }

    #[test]
    fn front_decal_reads_left_to_right_from_the_front() {
        // Front is -Z; seen from there, +X is on the viewer's left, so u=1 is at -X.
        let mesh = quad(
            &Geometry::Block,
            Vec3::ONE,
            &decal(Face::Front, None),
            false,
        )
        .unwrap();
        let points = positions(&mesh);
        let uvs = uvs(&mesh);
        for (point, uv) in points.iter().zip(&uvs) {
            assert_eq!(point[2], -0.5);
            assert_eq!(uv[0], if point[0] < 0.0 { 1.0 } else { 0.0 });
            assert_eq!(uv[1], if point[1] > 0.0 { 0.0 } else { 1.0 });
        }
    }

    #[test]
    fn decal_winding_faces_outward_for_every_block_face() {
        for index in 0..6 {
            let face = Face::from_normal_id(index).unwrap();
            let mesh = quad(&Geometry::Block, Vec3::ONE, &decal(face, None), false).unwrap();
            let p = positions(&mesh);
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("indices")
            };
            for tri in indices.chunks(3) {
                let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| Vec3::from_array(p[i as usize]));
                let geometric = (b - a).cross(c - a);
                let outward = (a + b + c) / 3.0;
                assert!(geometric.dot(outward) > 0.0, "face {face:?} winds inward");
            }
        }
    }

    #[test]
    fn textures_tile_by_studs_but_block_meshes_and_decals_do_not() {
        let size = Vec3::new(8.0, 6.0, 4.0);
        let texture = decal(Face::Top, Some([2.0, 4.0]));
        assert_eq!(uv_extent(&texture, size, false), Vec2::new(-4.0, 1.0));
        assert_eq!(uv_extent(&texture, size, true), Vec2::ONE);
        assert_eq!(uv_extent(&decal(Face::Top, None), size, false), Vec2::ONE);
        assert_eq!(
            uv_extent(&decal(Face::Bottom, Some([2.0, 2.0])), size, false),
            Vec2::new(4.0, -2.0)
        );
    }

    #[test]
    fn wedge_slope_has_a_valid_normal_and_collapsed_edges_are_skipped() {
        let slope = quad(
            &Geometry::Wedge,
            Vec3::ONE,
            &decal(Face::Front, None),
            false,
        )
        .unwrap();
        let normal = slope
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .unwrap()
            .as_float3()
            .unwrap()[0];
        assert!(
            normal[1] > 0.0 && normal[2] < 0.0,
            "slope faces up and forward"
        );
        // Face 0 starts with a doubled corner; the normal must still be +X.
        let side = quad(
            &Geometry::Wedge,
            Vec3::ONE,
            &decal(Face::Right, None),
            false,
        )
        .unwrap();
        assert_eq!(
            side.attribute(Mesh::ATTRIBUTE_NORMAL)
                .unwrap()
                .as_float3()
                .unwrap()[0],
            [1.0, 0.0, 0.0]
        );
    }

    #[test]
    fn unsupported_projections_are_not_invented() {
        for geometry in [
            Geometry::Ball,
            Geometry::Cylinder,
            Geometry::VerticalCylinder,
            Geometry::Head,
        ] {
            assert!(quad(&geometry, Vec3::ONE, &decal(Face::Front, None), false).is_none());
        }
    }
}
