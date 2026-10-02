# Native Rust Roblox model/place renderer

Cargo workspace (`rust/Cargo.toml`):

- `blox-model`: std-only XML/binary RBXM/RBXMX/RBXL/RBXLX readers, LZ4, BrickColor palette.
- `blox-scene`: model → `SceneSnapshot` and Bevy meshes (file meshes, head, torso, wedges).
- `blox-game`: the Bevy viewer binary.

The standalone Bevy viewer loads actual Roblox model/place files without building or linking any legacy C++. There is no custom Luau scene format or C ABI. The file readers, LZ4 decoder, mesh readers, and BrickColor palette are Rust ports of the source in this repository; no model/XML/compression crates were added. Bevy comes from its Git `main` branch, with the resolved 0.20.0-dev revision pinned by `Cargo.lock`.

## Run

From the repository root:

```sh
# Defaults to the real bundled content/other/character.rbxm.
cargo run --manifest-path rust/Cargo.toml -p blox-game

# Load another model or place.
cargo run --manifest-path rust/Cargo.toml -- path/to/model.rbxm
cargo run --manifest-path rust/Cargo.toml -- "$HOME/Downloads/12.d21efbcf188bddecc60b877666b01edd.rbxlx"
cargo run --manifest-path rust/Cargo.toml -- RobloxTest/Tests/Serialization/TerrainLights.bin.rbxl

cargo test --manifest-path rust/Cargo.toml
```

A graphical desktop and GPU are required for the viewer. Arrow keys orbit the model; `+`/`-` zoom. The camera frames the loaded geometry and adjusts for the window's aspect ratio. Positions and sizes remain in Roblox studs, with Y up. Part CFrames are world-space transforms, not transforms relative to their Model parents.

## Native model readers

`blox-model` (`crates/blox-model/src/lib.rs`) detects the format from the file contents, not its extension. The bundled `character.rbxm`, for example, is XML. XML models (`.rbxmx`) and places (`.rbxlx`) use the same reader and viewer pipeline; no conversion is required.

- `crates/blox-model/src/xml.rs`: legacy Roblox XML, including nested Items, referents, typed rendering properties, Content, entities, and CDATA. Numeric entities for nonzero control characters in legacy strings/scripts are accepted for compatibility with Roblox's writer; raw XML control characters and NUL entities are rejected. References: `App/v8xml/SerializerV2.cpp`, `App/v8xml/XmlSerializer.cpp`, and the corresponding instance property readers.
- `crates/blox-model/src/binary.rs`: the version-zero RBXM/RBXL header, INST/PROP/PRNT/END chunks, raw LZ4 compression, interleaved numeric arrays, delta referents, and compact/matrix/quaternion CFrames. References: `App/v8xml/SerializerBinary.cpp`, `App/lz4/lz4_decoder.h`, and `App/util/Math.cpp`.
- The native model retains class/name/referent/parent information and decoded rendering/basic property values. Unsupported non-render property types are recorded as `Property::Other`; this is not a lossless round-trip serializer.
- Malformed supported data and unsupported rendering-property encodings return errors. Newer encodings/compression absent from the 2016 reference, including binary Color3uint8 and Zstandard, are not silently guessed.

## Rendering coverage

`SceneSnapshot::from_model` extracts actual Part properties and mesh children. It supports blocks, balls, horizontal cylinders, seats, spawn locations, wedges, corner wedges, and the supported SpecialMesh/BlockMesh/CylinderMesh/FileMesh variants. It imports world CFrames, sizes, BrickColor/Color3, transparency, mesh scale, and local mesh offsets. The 208-entry BrickColor palette comes from `App/util/BrickColor.cpp`.

`crates/blox-scene/src/mesh.rs` reads local `rbxasset://` mesh assets in version 1.00, 1.01, and 2.00 formats using `Rendering/GfxBase/FileMeshData.cpp` as the reference. Heads use the bundled `other/head.mesh` and the legacy cap scaling/extrusion rule. SpecialMesh torsos use the size-dependent shoulder inset from `Rendering/GfxRender/GeometryGenerator.cpp`, rather than substituting a cube or unrelated mesh. Shared primitive/file meshes are cached; size-dependent head/torso meshes are cached separately.

This is a static model viewer, not a replacement Roblox game engine. Embedded scripts are decoded/countable but **not executed**; no Luau runtime is linked yet (`mlua` was removed until script porting begins). There is no physics, animation, replication, GUI, remote asset fetching, or Roblox scripting API. Terrain, effects, local lights, CSG/MeshPart/TrussPart geometry, and unsupported mesh variants are omitted and reported where recognized. Legacy surface textures, cylinder bevel/bulge controls, and exact legacy material/shader behavior are not reproduced; Bevy supplies the viewer camera, directional light, and PBR shading.

## Legacy look

Parts are drawn by `blox-game/src/legacy.wesl`, a port of the legacy forward shader (`shaders/source/default.hlsl`, `plastic.hlsl`, `common.h`), not Bevy's PBR. Lighting is the original model: `(Ambient + (saturate(N.L)*Lamp0 + saturate(-N.L)*Lamp1) * shadow) * albedo` plus a Blinn specular term, in non-linear colour space with no tonemapping (`Tonemapping::None`; only the final value is converted for Bevy's sRGB framebuffer).

- **Lighting service** (`blox-scene/src/lighting.rs`, from `Lighting.cpp`, `LightingParameters.cpp`, `RenderView::presetLighting`): `Ambient`, `Brightness`, `ColorShift_Top/Bottom`, `TimeOfDay`, `GeographicLatitude`, `FogColor/Start/End` and `Outlines` are read from the place. The sun direction and colour come from the original time-of-day splines; the key light is `lightColor*0.9*Brightness`, the fill light is 0.4 of that from the opposite side. Shadows come from Bevy's directional shadow map but attenuate only the key light, by at most 75%, as `common.h` does.
- **Edge outlines:** every block face darkens along its edges by the `default.hlsl` formula, fading with distance (shading distance 300 studs).
- **Studs and inlets** on Plastic block parts, per face `TopSurface`/`BottomSurface`/etc. The original atlas (`studs.dds`, `plastic/*.dds`) is not in this repository, so the relief is drawn procedurally; weld, glue, universal and motor surfaces are drawn flat.
- **Part materials:** specular intensity/power per `Material` (table from `MaterialGenerator.cpp`); Neon is unlit; reflectance only thins Plastic transparency.
- **Sky:** a skybox from `content/sky/null_plainsky512_*.jpg`, tinted by the time-of-day sky colour. The place's own `Sky` faces are remote asset ids and are not fetched. Sun, moon and star sprites are not drawn.
- **Not ported:** the material texture sets (wood, brick, grass, ...; not in the repository, so those materials render flat), normal maps, the light grid and point lights, environment reflections, per-part colour/normal jitter, and live `TimeOfDay` changes (lighting is fixed at load).

## Camera and movement

The viewer starts at the place's saved `Workspace.CurrentCamera` pose and FOV (70 degrees by default, as `Camera.cpp`); there is no map-fitting orbit camera. Click to capture the mouse, Esc releases it.

- **Look** (`Camera.cpp::onMousePan`): -0.4 degrees/pixel yaw, -0.3 degrees/pixel pitch, pitch limited to +-81 degrees.
- **Walk** (default, `blox-game/src/walk.rs`): WASD/arrows, Space to jump. Rapier (`bevy_rapier3d`, the same Bevy revision) provides static colliders for every part with `CanCollide` true (cuboid/ball analytically, wedges/cylinders/heads/torsos as convex hulls, file meshes as triangle meshes) and a kinematic character controller. Constants from the C++: `Humanoid::walkSpeed` 16 studs/s, jump velocity 50 studs/s (about 6.4 studs high), gravity 196.2 studs/s^2 (9.81 m/s^2, 20 studs/m). The character is a 2x5-stud capsule with the eye at head height (4.5 studs). It starts above the first `SpawnLocation`. The 1-stud auto-step and the capsule shape are approximations; the legacy humanoid raycasts its legs and torso.
- **Fly** (`F` toggles; `Camera::doFly`): W/S/A/D forward/back/strafe, E up, Q down, relative to the camera, collision-free. The base step is 1.5 studs per call and the legacy `Workspace::onHeartbeat` runs at 60 Hz (`uiStepsPerSec`), so 90 studs/s; Shift is 0.2x; after one second held the multiplier is `steps/60` (capped at 30). Space/Backspace are not fly keys.

Not implemented: humanoid state machine (swimming, climbing, ragdoll), moving or dynamic parts, and Terrain collision.

## Decals and textures

`Decal` and `Texture` children and mesh `TextureId` are rendered from local `rbxasset://` files (PNG and JPEG; `blox-scene/src/texture.rs`). Remote ids, missing files, and undecodable images are reported as `Not rendered` and skipped; a null texture draws nothing, as in `MaterialGenerator::createMaterialForDecal`.

- **Geometry** (`blox-scene/src/decal.rs`, from `GeometryGenerator.cpp`): the `decal` face tables and UV rules for blocks, wedges, corner wedges and torsos, including the bottom-face and wedge/corner-wedge UV fixes. `Decal.Face` defaults to Front. A plain `Decal` stretches one copy over the face; a `Texture` repeats every `StudsPerTileU/V` studs using the part size, except under a `BlockMesh`/`CylinderMesh`, which ignore size.
- **Head and file meshes** redraw the whole mesh with the decal texture, using the mesh's own UVs, as the legacy generator does; the face is not used.
- **Sampling and blending:** decals clamp, textures and mesh textures wrap; decals are alpha-blended with a depth bias so they win against the part, in child order. Decal `Transparency` is honored.
- **Mesh `TextureId`** (SpecialMesh/FileMesh) replaces the part color, as legacy `Flag_VertexColorMesh` does.
- **Not ported:** decals on balls, cylinders and trusses (reported, not drawn), DDS/TGA textures, remote assets, humanoid composite textures (shirts/pants/T-shirts), and `SurfaceGui`/`BillboardGui`.

## Verification

The regression suite covers native XML/binary model decoding, LZ4 overlap and truncation, numeric/CFrame encodings, malformed binary graphs, mesh scaling/UVs, head extrusion, torso shoulder boundaries, and portrait/landscape camera framing. It loads the real XML character and compressed binary TerrainLights fixtures.

GPU smoke runs rendered the XML character (11 instances, 6 parts) and binary TerrainLights place (39 instances, 7 parts), including camera orbit. A native import smoke also loaded `ComplexTerrain.bin.rbxl` (599 instances, 523 supported parts). Terrain data itself is not rendered.
