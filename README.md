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
- The native model retains class/name/referent/parent information and decoded rendering/basic property values. The 2016 binary property types are decoded, including `UDim`/`UDim2` (GUI `Size`/`Position`), `Vector2`, `Vector2int16`, `Ray`, `NumberSequence`, `ColorSequence`, `NumberRange` and `PhysicalProperties`; types with no representation (`Rect2D`) and unknown future formats are retained as `Property::Other` with their payload skipped, so a place is not rejected for a property the viewer does not consume. This is not a lossless round-trip serializer.
- Malformed supported data and unsupported render-relevant encodings return errors. Newer encodings/compression absent from the 2016 reference, including binary Color3uint8 and Zstandard, are not silently guessed.

## Rendering coverage

`SceneSnapshot::from_model` extracts actual Part properties and mesh children. It supports blocks, balls, horizontal cylinders, seats, spawn locations, wedges, corner wedges, and the supported SpecialMesh/BlockMesh/CylinderMesh/FileMesh variants. It imports world CFrames, sizes, BrickColor/Color3, transparency, mesh scale, and local mesh offsets. The 208-entry BrickColor palette comes from `App/util/BrickColor.cpp`.

`crates/blox-scene/src/mesh.rs` reads local `rbxasset://` mesh assets in version 1.00, 1.01, and 2.00 formats using `Rendering/GfxBase/FileMeshData.cpp` as the reference. Heads use the bundled `other/head.mesh` and the legacy cap scaling/extrusion rule. SpecialMesh torsos use the size-dependent shoulder inset from `Rendering/GfxRender/GeometryGenerator.cpp`, rather than substituting a cube or unrelated mesh. Shared primitive/file meshes are cached; size-dependent head/torso meshes are cached separately.

Real place files frequently reference meshes, textures and sounds that are not present locally (author uploads, `rbxassetid://` remote ids). A part whose mesh cannot be located is still drawn as its plain part geometry and reported, rather than failing the load, matching the legacy proxy fallback. Out-of-range `Transparency`/`Reflectance` (2016 places contain values such as 2.0 and -0.1) are clamped as the legacy renderer does (`GeometryGenerator.cpp::getBaseColor`, `PartCookie.h::getPartReflectance`) instead of being rejected.

This is a static model viewer, not a replacement Roblox game engine. Embedded `Script`/`LocalScript` sources run in Luau (`crates/blox-script`) against a partial instance bridge: `game`/`workspace`, `Instance` parent/name/children/`FindFirstChild`, generic properties, `CFrame`/`Position` writes, `Vector3`, a basic `CFrame`, `wait`, and `ClickDetector` `MouseClick`/`MouseHoverEnter`/`MouseHoverLeave`. Clicks are the camera-center ray while the mouse is captured, picking the nearest part (oriented box test) within `MaxActivationDistance` (32). Unsupported APIs (`Touched`, `Humanoid`, `Players`, `Instance.new`, `Clone`, `Sound` playback, `BodyVelocity` dynamics) raise Lua errors or are stored without effect. Rapier provides viewer walking and static part collisions, not a Roblox physics simulation. There is no animation, replication, GUI, remote asset fetching, or full Roblox scripting API. Terrain, effects, local lights, CSG/MeshPart/TrussPart geometry, and unsupported mesh variants are omitted and reported where recognized. Cylinder bevel/bulge controls and exact legacy material/shader behavior are not reproduced.

## Legacy look

Parts are drawn by `blox-game/src/legacy.wesl`, a port of the legacy forward shader (`shaders/source/default.hlsl`, `plastic.hlsl`, `common.h`), not Bevy's PBR. Lighting is the original model: `(Ambient + (saturate(N.L)*Lamp0 + saturate(-N.L)*Lamp1) * shadow) * albedo` plus a Blinn specular term, in non-linear colour space with no tonemapping (`Tonemapping::None`; only the final value is converted for Bevy's sRGB framebuffer).

- **Lighting service** (`blox-scene/src/lighting.rs`, from `Lighting.cpp`, `LightingParameters.cpp`, `RenderView::presetLighting`): `Ambient`, `Brightness`, `ColorShift_Top/Bottom`, `TimeOfDay` (unwrapped, as `getGameTime`), `GeographicLatitude`, `FogColor/Start/End`, `Outlines` and `GlobalShadows` are read from the place. The sun and moon directions reproduce the C++ `setTime` (seasonal tilt, latitude, moon phase) and match the compiled `LightingParameters.cpp` on 150 sampled times and latitudes to within 5e-7. The key light is `lightColor*0.9*Brightness` clamped to 1, the fill light is 0.4 of that from the opposite side. The directional shadow map is drawn only when `GlobalShadows` is true (default false), and attenuates only the key light, by at most 75% at high sun, as `common.h` does. `FogStart` is clamped below `FogEnd - 0.1`, as `RenderView::updateFog` does.
- **Edge outlines:** block faces darken along their edges by the `default.hlsl` formula, fading with distance (shading distance 300 studs). The darkening applies to the whole lit colour (diffuse and specular), as the C++ does. An edge shared with a `NoOutlines` face is pushed out 10 studs, as `computeOutlineOffsetPart` does.
- **Studs, inlets and plastic bump:** the real `PlatformContent/pc/textures/plastic/diffuse.dds` stud atlas and `normal.dds`, indexed per face by `TopSurface`/`BottomSurface`/etc. (studs, weld/glue, inlet, universal rows) exactly as `getStudsAtlasInfo` and `plastic.hlsl` do. Studs show only on Plastic blocks without a mesh child.
- **Part materials:** Wood, WoodPlanks, Marble, Slate, Concrete, Granite, Brick, Pebble, Cobblestone, Rust, DiamondPlate, Aluminum, Metal, Grass, Sand, Fabric and Ice use the real diffuse, normal, specular and `normaldetail` maps, tiled per stud with the original tiling and every `CFG_*` constant (`blox-scene/src/material.rs`). The specular far-distance values are `CFG_SPECULAR_LOD`/`CFG_GLOSS_LOD`, the normal detail is applied even where the normal map is constant, and far-distance tiling cross-fades in over `CFG_FAR_*` cutoffs. Brick and WoodPlanks offset by whole studs. Neon is unlit. Reflectance only thins Plastic transparency.
- **Non-block material mapping:** wedges, corner wedges, balls, cylinders and other untextured meshes use dominant-face projection in local stud coordinates, with the normal-map tangent projected onto the actual surface. Explicit mesh textures retain their mesh UVs and take precedence. This is an approximation, not the legacy shape-specific tessellated UV unwrap; curved parts can have visible seams where the projection changes. Studs and edge outlines remain block-only.
- **Sky:** a skybox from `content/sky/null_plainsky512_*.jpg`, tinted by the time-of-day sky colour. The place's own `Sky` faces are remote asset ids and are not fetched. Sun, moon and star sprites are not drawn.
- **Not ported:** the exact sphere/cylinder/wedge material UV unwrap, Cobblestone's Wang tiling, environment reflections (`CFG_REFLECTION_SCALE` is ignored), the neon blend state (the C++ writes `1 - fog*Color.a` as alpha and blends; neon is drawn opaque here), the glow scale on neon (taken as 1), terrain, the light grid and point lights, per-part colour/normal jitter, the random per-part texture offset's exact table (a position hash is used), and live `TimeOfDay` changes (lighting is fixed at load). DDS files are decoded on the CPU (DXT1/DXT5 only, which is all `PlatformContent` uses).

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

The regression suite covers native XML/binary model decoding, LZ4 overlap and truncation, numeric/CFrame encodings, the 2016 UDim/UDim2/Vector2/Ray/sequence/physical-property layouts, malformed binary graphs, mesh scaling/UVs, head extrusion, torso shoulder boundaries, transparency/reflectance clamping, and portrait/landscape camera framing. It loads the real XML character and compressed binary TerrainLights fixtures.

GPU smoke runs rendered the XML character (11 instances, 6 parts) and binary TerrainLights place (39 instances, 7 parts), including camera orbit. Native importer smokes also load `ComplexTerrain.bin.rbxl` (599 instances, 524 supported parts) and the 2016 Starter Place (2,471 instances, 1,774 parts), and a full community place, `slender.rbxl` (30,989 instances, 19,309 parts, 749 scripts retained). Terrain data itself is not rendered.
