//! Terrain decoding and meshing.
//!
//! Places store terrain as a binary string on the `Terrain` instance: the modern `SmoothGrid`
//! (Voxel2, dual-contour meshing) or the legacy `ClusterGridV3` 4-stud cell grid. Only the legacy
//! grid is decoded here; it is what real 2016-era and community places ship, and it is the input
//! `Voxel2::Conversion::convertToSmooth` starts from. References:
//! `App/v8datamodel/MegaCluster.cpp` (`deserializeGridV3`, `readCountValue`),
//! `App/include/voxel/Cell.h`, `App/include/voxel2/Conversion.h`, `App/voxel2/MaterialTable.cpp`.
//!
//! Conventions match the C++: cells are 4 studs (`Voxel::kCELL_SIZE`), cell `c` spans world
//! `c * 4 .. c * 4 + 4`, and a legacy chunk is 32x16x32 cells iterated x fastest, then z, then y.

use glam::{Mat3, Vec3};
use std::{collections::HashMap, fmt};

/// Studs per terrain cell (`Voxel::kCELL_SIZE`).
pub const CELL_SIZE: f32 = 4.0;
/// Legacy cluster chunk dimensions (`Voxel::kXZ_CHUNK_SIZE`, `kY_CHUNK_SIZE`).
const CHUNK_XZ: i32 = 32;
const CHUNK_Y: i32 = 16;
const CELLS_PER_CHUNK: usize = (CHUNK_XZ * CHUNK_Y * CHUNK_XZ) as usize;

/// `Voxel::CELL_MATERIAL_Water`.
const LEGACY_MATERIAL_WATER: u8 = 17;
/// `Voxel::CELL_BLOCK_Empty`.
const BLOCK_EMPTY: u8 = 5;
/// `SolidTerrainCell`: material 0..2, block 3..5, orientation 6..7. The empty representation is
/// `block = Empty(5)`, `orientation = NegZ(0)`, material 0 (`App/voxel/Cell.cpp`), i.e. 0x28.
const CELL_EMPTY: u8 = BLOCK_EMPTY << 3;
/// V1/V2 handed the file a fixed 16x4x16 chunk lattice, offset by legacy region (8, 0, 8).
const LEGACY_CHUNKS: [i32; 3] = [16, 4, 16];
const LEGACY_CHUNK_OFFSET: [i32; 3] = [8, 0, 8];
/// `Voxel2` material slot order (`Conversion.h::kMaterialTable`).
const SLOT_AIR: u8 = 0;
const SLOT_WATER: u8 = 1;
const SLOT_GRASS: u8 = 2;
/// Water occupies the lower half of its cell (`materials.json` Water `water: 0.5`).
const WATER_HEIGHT: f32 = 0.5;

pub type Result<T, E = TerrainError> = std::result::Result<T, E>;

#[derive(Debug)]
pub struct TerrainError(pub String);

impl fmt::Display for TerrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for TerrainError {}

fn error(message: impl Into<String>) -> TerrainError {
    TerrainError(message.into())
}

/// One terrain cell: solid block, water, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cell {
    Solid { block: u8, material: u8 },
    Water,
}

/// Which terrain payload a `Terrain` instance carries.
pub enum Source {
    /// A decoded legacy cluster grid.
    Grid(Terrain),
    /// A modern `SmoothGrid` payload of this many bytes, which is not decoded yet.
    SmoothGrid(usize),
    /// No terrain payload; the instance is empty.
    None,
}

/// Which legacy cluster-grid encoding a byte stream uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyVariant {
    /// `ClusterGridV3`: a signed 16-bit chunk header precedes each chunk.
    V3,
    /// `ClusterGridV2`: a fixed 16x4x16 lattice with no per-chunk header.
    V2,
}

/// Sparse terrain grid keyed by cell coordinate (world divided by [`CELL_SIZE`]).
#[derive(Debug, Default)]
pub struct Terrain {
    cells: HashMap<[i32; 3], Cell>,
    origin: Vec3,
    rotation: Mat3,
}

impl Terrain {
    /// Decodes the legacy `ClusterGridV3` payload: an explicit signed chunk header before each
    /// chunk's cell and material runs (`MegaCluster.cpp::deserializeGridV3`).
    pub fn from_cluster_grid_v3(bytes: &[u8]) -> Result<Self> {
        Self::from_legacy(bytes, LegacyVariant::V3)
    }

    /// Decodes the legacy `ClusterGridV2` payload: a fixed 16x4x16 chunk lattice with no headers
    /// (`MegaCluster.cpp::setPackagedClusterGridV2`).
    pub fn from_cluster_grid_v2(bytes: &[u8]) -> Result<Self> {
        Self::from_legacy(bytes, LegacyVariant::V2)
    }

    fn from_legacy(bytes: &[u8], variant: LegacyVariant) -> Result<Self> {
        let mut terrain = Self {
            cells: HashMap::new(),
            origin: Vec3::ZERO,
            rotation: Mat3::IDENTITY,
        };
        let mut reader = Cursor { bytes, offset: 0 };
        let mut cells = [0u8; CELLS_PER_CHUNK];
        let mut materials = [0u8; CELLS_PER_CHUNK];
        let mut headers: Vec<[i32; 3]> = Vec::new();
        if variant == LegacyVariant::V2 {
            // The C++ iterates i, j, k over the fixed lattice; (i,j,k) - offset is the chunk.
            for i in 0..LEGACY_CHUNKS[0] {
                for j in 0..LEGACY_CHUNKS[1] {
                    for k in 0..LEGACY_CHUNKS[2] {
                        headers.push([i - LEGACY_CHUNK_OFFSET[0], j, k - LEGACY_CHUNK_OFFSET[2]]);
                    }
                }
            }
        }
        let mut next = 0;
        while reader.remaining() != 0 {
            let chunk = if variant == LegacyVariant::V3 {
                [
                    i32::from(reader.i16()?),
                    i32::from(reader.i16()?),
                    i32::from(reader.i16()?),
                ]
            } else {
                // V2's lattice can run out before the stream does; extra bytes are unexpected.
                *headers
                    .get(next)
                    .ok_or_else(|| error("ClusterGridV2 has more chunks than its lattice"))?
            };
            next += 1;
            reader.fill_run_length(&mut cells)?;
            reader.fill_run_length(&mut materials)?;
            let base = [chunk[0] * CHUNK_XZ, chunk[1] * CHUNK_Y, chunk[2] * CHUNK_XZ];
            // x fastest, then z, then y (`MegaCluster.cpp::deserializeGridV3`).
            let mut offset = 0;
            for y in 0..CHUNK_Y {
                for z in 0..CHUNK_XZ {
                    for x in 0..CHUNK_XZ {
                        let raw = cells[offset];
                        let block = (raw >> 3) & 0x7;
                        let material = materials[offset];
                        // Only the one empty representation is empty; other `block == Empty`
                        // cells carry explicit water bits (`Cell::isEmpty`, `isExplicitWaterCell`).
                        let decoded = if raw == CELL_EMPTY {
                            None
                        } else if block == BLOCK_EMPTY {
                            Some(Cell::Water)
                        } else if material == LEGACY_MATERIAL_WATER {
                            Some(Cell::Water)
                        } else {
                            Some(Cell::Solid {
                                block,
                                material: material_slot(material),
                            })
                        };
                        if let Some(cell) = decoded {
                            terrain
                                .cells
                                .insert([base[0] + x, base[1] + y, base[2] + z], cell);
                        }
                        offset += 1;
                    }
                }
            }
        }
        Ok(terrain)
    }

    /// Applies the `Terrain` instance's world transform to cell space.
    pub fn set_frame(&mut self, origin: Vec3, rotation: Mat3) {
        self.origin = origin;
        self.rotation = rotation;
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Solid cells as `(position, cell)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = ([i32; 3], Cell)> + '_ {
        self.cells.iter().map(|(&position, &cell)| (position, cell))
    }

    fn cell_world(&self, [x, y, z]: [i32; 3], corner: [f32; 3], height: f32) -> Vec3 {
        let local = Vec3::new(
            x as f32 * CELL_SIZE + corner[0] * CELL_SIZE,
            y as f32 * CELL_SIZE + corner[1] * CELL_SIZE * height,
            z as f32 * CELL_SIZE + corner[2] * CELL_SIZE,
        );
        self.origin + self.rotation * local
    }

    /// Builds solid and water render meshes. One quad per solid/empty and water/air boundary,
    /// with atlas UVs baked in (`Rendering/GfxRender/MegaCluster.cpp` face predicates).
    pub fn build_mesh(&self, atlas: &Atlas) -> TerrainMesh {
        let mut out = TerrainMesh::default();
        for (&position, &cell) in &self.cells {
            match cell {
                Cell::Solid { material, .. } => {
                    for (face, normal) in FACES.iter().enumerate() {
                        let neighbour = [
                            position[0] + normal[0],
                            position[1] + normal[1],
                            position[2] + normal[2],
                        ];
                        if matches!(self.cells.get(&neighbour), Some(Cell::Solid { .. })) {
                            continue;
                        }
                        let layer = atlas.layer(material, face);
                        push_quad(
                            &mut out.solid,
                            self,
                            position,
                            face,
                            layer,
                            seed(position, material),
                            1.0,
                        );
                    }
                }
                Cell::Water => {
                    // Water only shows faces against air; it never closes against terrain or
                    // other water, matching `WaterRenderPredicate`.
                    for (face, normal) in FACES.iter().enumerate() {
                        let neighbour = [
                            position[0] + normal[0],
                            position[1] + normal[1],
                            position[2] + normal[2],
                        ];
                        if self.cells.contains_key(&neighbour) {
                            continue;
                        }
                        push_quad(
                            &mut out.water,
                            self,
                            position,
                            face,
                            atlas.water,
                            seed(position, SLOT_WATER),
                            WATER_HEIGHT,
                        );
                    }
                }
            }
        }
        out
    }

    /// Collision boxes for the solid cells, greedily merged along X so a flat floor is one box
    /// instead of thousands. Each returns `(center, half_extents)` in world space.
    pub fn colliders(&self) -> Vec<(Vec3, Vec3)> {
        let mut remaining: Vec<[i32; 3]> = self
            .cells
            .iter()
            .filter(|(_, cell)| matches!(cell, Cell::Solid { .. }))
            .map(|(&position, _)| position)
            .collect();
        // Deterministic order so the merge is reproducible.
        remaining.sort_unstable();
        let solid: std::collections::HashSet<[i32; 3]> = remaining.iter().copied().collect();
        let mut used: std::collections::HashSet<[i32; 3]> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for start in remaining {
            if used.contains(&start) {
                continue;
            }
            let mut end = start;
            while solid.contains(&[end[0] + 1, end[1], end[2]])
                && !used.contains(&[end[0] + 1, end[1], end[2]])
            {
                end[0] += 1;
            }
            let length = (end[0] - start[0] + 1) as f32;
            for x in start[0]..=end[0] {
                used.insert([x, start[1], start[2]]);
            }
            let center = self.cell_world([start[0], start[1], start[2]], [0.5, 0.5, 0.5], 1.0)
                + self.rotation * Vec3::new((length - 1.0) * CELL_SIZE * 0.5, 0.0, 0.0);
            out.push((center, Vec3::new(length, 1.0, 1.0) * (CELL_SIZE * 0.5)));
        }
        out
    }
}

/// Solid and water meshes, which use different materials and render queues.
#[derive(Default, Debug)]
pub struct TerrainMesh {
    pub solid: Mesh,
    pub water: Mesh,
}

/// Renderable mesh: positions, normals, UVs, together with triangle indices.
#[derive(Default, Debug)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Face order: -X, +X, -Y, +Y, -Z, +Z.
const FACES: [[i32; 3]; 6] = [
    [-1, 0, 0],
    [1, 0, 0],
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
];

/// Corner offsets (in cell units, 0..1) per face, counter-clockwise seen from outside.
const CORNERS: [[[f32; 3]; 4]; 6] = [
    [
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
        [0.0, 1.0, 0.0],
    ],
    [
        [1.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 1.0, 1.0],
    ],
    [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
    ],
    [
        [0.0, 1.0, 1.0],
        [1.0, 1.0, 1.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ],
    [
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
    ],
    [
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ],
];

/// `Mesher::computeSeed`: a position hash that jitters each cell's texture offset.
fn seed([x, y, z]: [i32; 3], material: u8) -> f32 {
    let mut hash = (x as u32).wrapping_mul(0x9e37_79b9)
        ^ (y as u32).wrapping_mul(0x85eb_ca6b)
        ^ (z as u32).wrapping_mul(0xc2b2_ae35)
        ^ u32::from(material);
    hash ^= hash >> 15;
    (hash & 0xffff) as f32 / 65535.0
}

fn push_quad(
    mesh: &mut Mesh,
    terrain: &Terrain,
    cell: [i32; 3],
    face: usize,
    layer: AtlasLayer,
    seed: f32,
    height: f32,
) {
    let start = mesh.vertices.len() as u32;
    let normal = terrain.rotation * Vec3::from_array(FACES[face].map(|v| v as f32));
    for (corner_index, corner) in CORNERS[face].into_iter().enumerate() {
        let world = terrain.cell_world(cell, corner, height);
        // Same UV basis as the legacy generator: project onto the two axes the face spans.
        let (u, v) = match face {
            0 | 1 => (corner[2], corner[1]),
            2 | 3 => (corner[0], corner[2]),
            _ => (corner[0], corner[1]),
        };
        let tile = Vec3::new(
            (u * CELL_SIZE * layer.tiling + layer.detiling * seed).fract(),
            (v * CELL_SIZE * layer.tiling + layer.detiling * seed).fract(),
            0.0,
        );
        mesh.vertices.push(world.to_array());
        mesh.normals.push(normal.to_array());
        mesh.uvs.push([
            layer.u0 + tile.x * layer.tile_u,
            layer.v0 + tile.y * layer.tile_v,
        ]);
        mesh.indices.push(start + corner_index as u32);
    }
    mesh.indices.extend([start, start + 2, start + 3]);
}

/// One atlas layer: tiling plus the tile's origin and size in texture space.
#[derive(Clone, Copy, Debug, Default)]
pub struct AtlasLayer {
    tiling: f32,
    detiling: f32,
    u0: f32,
    v0: f32,
    tile_u: f32,
    tile_v: f32,
}

/// Terrain atlas from `PlatformContent/pc/terrain/materials.json`.
#[derive(Debug)]
pub struct Atlas {
    width: f32,
    height: f32,
    tile_size: f32,
    tile_count: u32,
    border: f32,
    layers: Vec<(f32, f32)>,
    /// `(top, side, bottom)` layer index per `Voxel2` material slot.
    materials: Vec<[usize; 3]>,
    water: AtlasLayer,
}

impl Atlas {
    /// Loads and parses the atlas description next to the terrain diffuse texture.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| error(format!("terrain materials {}: {e}", path.display())))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let root: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| error(format!("terrain materials JSON: {e}")))?;
        let platform = root["platform"].as_str().unwrap_or("");
        let atlas = &root["atlas"][platform];
        let width = number(&atlas["width"], 1.0);
        let height = number(&atlas["height"], 1.0);
        let tile_size = number(&atlas["tileSize"], 0.0);
        let tile_count = atlas["tileCount"].as_u64().unwrap_or(1) as u32;
        let border = number(&atlas["borderSize"], 0.0);
        if width <= 0.0 || height <= 0.0 || tile_count == 0 {
            return Err(error("terrain atlas has no usable tiles"));
        }
        let materials_json = root["materials"]
            .as_array()
            .ok_or_else(|| error("terrain materials has no material list"))?;
        let mut layers = Vec::new();
        let mut materials = Vec::new();
        for entry in materials_json {
            // `texture_top`/`texture_side` split the faces; a single `texture` covers all three.
            if let (Some(top), Some(side)) = (entry.get("texture_top"), entry.get("texture_side")) {
                let top = push_layer(&mut layers, top);
                let side = push_layer(&mut layers, side);
                let bottom = entry
                    .get("texture_bottom")
                    .map_or(Some(top), |b| Some(push_layer(&mut layers, b)));
                materials.push([top, side, bottom.unwrap_or(top)]);
            } else if let Some(texture) = entry.get("texture") {
                let layer = push_layer(&mut layers, texture);
                materials.push([layer, layer, layer]);
            } else {
                let layer = push_layer(&mut layers, &serde_json::Value::Null);
                materials.push([layer, layer, layer]);
            }
        }
        if layers.is_empty() {
            return Err(error("terrain atlas has no layers"));
        }
        let tile_full = tile_size + border * 2.0;
        let atlas = Self {
            width,
            height,
            tile_size,
            tile_count,
            border,
            layers,
            materials,
            water: AtlasLayer::default(),
        };
        let mut atlas = atlas;
        atlas.water = atlas.layer(SLOT_WATER, 3);
        Ok(atlas)
    }

    fn raw_layer(&self, index: usize) -> AtlasLayer {
        let (tiling, detiling) = self.layers[index];
        let tile_full = self.tile_size + self.border * 2.0;
        let column = (index as u32 % self.tile_count) as f32;
        let row = (index as u32 / self.tile_count) as f32;
        AtlasLayer {
            tiling,
            detiling,
            u0: (column * tile_full + self.border) / self.width,
            v0: (row * tile_full + self.border) / self.height,
            tile_u: self.tile_size / self.width,
            tile_v: self.tile_size / self.height,
        }
    }

    /// The layer for a material slot and face index: face 3 is the top, face 2 the bottom.
    fn layer(&self, material: u8, face: usize) -> AtlasLayer {
        let slot = usize::from(material);
        let Some([top, side, bottom]) = self.materials.get(slot).copied() else {
            return self.raw_layer(0);
        };
        let index = match face {
            3 => top,
            2 => bottom,
            _ => side,
        };
        self.raw_layer(index.min(self.layers.len().saturating_sub(1)))
    }
}

fn number(value: &serde_json::Value, default: f32) -> f32 {
    value.as_f64().map_or(default, |v| v as f32)
}

fn push_layer(layers: &mut Vec<(f32, f32)>, node: &serde_json::Value) -> usize {
    let tiling = number(&node["tiling"], 1.0);
    let detiling = number(&node["detiling"], 0.0);
    // Identical layers share a tile, as the C++ `parseMaterialLayer` does.
    if let Some(index) = layers.iter().position(|&layer| layer == (tiling, detiling)) {
        return index;
    }
    layers.push((tiling, detiling));
    layers.len() - 1
}

/// Maps a legacy `Voxel::CellMaterial` to a `Voxel2` material slot
/// (`Conversion.h::getMaterialFromCellMaterial` plus the `kMaterialTable` order).
fn material_slot(legacy: u8) -> u8 {
    match legacy {
        0 => SLOT_AIR,
        1 => SLOT_GRASS,
        2 => 6,           // Sand
        3 => 5,           // Brick
        4 | 11 | 13 => 3, // Granite/Gravel/Stone_Block -> Slate
        5 | 12 | 14 => 4, // Asphalt/Cinder_Block/Cement -> Concrete
        9 | 10 => 7,      // Wood_Plank/Wood_Log -> WoodPlanks
        17 => SLOT_WATER,
        _ => SLOT_GRASS, // kMaterialDefault
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Cursor<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    fn byte(&mut self) -> Result<u8> {
        let value = *self
            .bytes
            .get(self.offset)
            .ok_or_else(|| error("Truncated terrain data"))?;
        self.offset += 1;
        Ok(value)
    }

    /// `readInt16`: little-endian.
    fn i16(&mut self) -> Result<i16> {
        let lo = u16::from(self.byte()?);
        let hi = u16::from(self.byte()?);
        Ok((hi << 8 | lo) as i16)
    }

    /// `readCountValue`: a byte, or 255 followed by a big-endian 16-bit count.
    fn count(&mut self) -> Result<usize> {
        let first = self.byte()?;
        if first < 255 {
            return Ok(usize::from(first));
        }
        let hi = usize::from(self.byte()?);
        let lo = usize::from(self.byte()?);
        Ok(hi << 8 | lo)
    }

    fn fill_run_length(&mut self, out: &mut [u8]) -> Result<()> {
        let mut written = 0;
        while written < out.len() {
            let value = self.byte()?;
            let count = self.count()?;
            if count == 0 || count > out.len() - written {
                return Err(error("Terrain run length exceeds the chunk"));
            }
            out[written..written + count].fill(value);
            written += count;
        }
        Ok(())
    }
}
