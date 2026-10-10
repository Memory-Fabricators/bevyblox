# Rust/Bevy port: what is left

Status of the C++ -> Rust port of the 2016 client, what a real place still needs,
and an ordered plan. Legacy file references are paths in this repository; Rust
references are under `rust/`.

## Where the port is today

| Layer | State |
| --- | --- |
| Model/place readers | Done. XML (`.rbxmx`/`.rbxlx`) and binary v0 (`.rbxm`/`.rbxl`), LZ4, referents, parents, CFrames, the 2016 property layouts incl. `UDim`/`UDim2` (`crates/blox-model`). |
| Static rendering | Blocks, balls, cylinders, wedges, corner wedges, seats, spawns, heads, torsos, file/special meshes, decals/textures, the 208-entry BrickColor palette, legacy-look forward shader and global `Lighting` (`crates/blox-scene`, `crates/blox-game/src/legacy.wesl`). |
| Player | Kinematic capsule walker + fly camera over Rapier static colliders, saved `Workspace.CurrentCamera` pose (`crates/blox-game/src/walk.rs`, `first_person.rs`). |
| Not present | Terrain, CSG/MeshPart/Truss geometry, local lights, particles, sky bodies, water, GUI, most of the scripting API, sound playback, animation, dynamic physics, joints, terrain collision, replication. |

Verified against real data: `slender.rbxl` (30,989 instances / 19,309 parts / 749
scripts), `2016 Starter Place.rbxl` (2,471 / 1,774), `ComplexTerrain.bin.rbxl`
(599 / 524) all load and render.

## Guiding decisions

1. **Prefer pre-baked data over reimplementation.** Places ship pre-baked CSG
   `MeshData`, pre-baked terrain grids and pre-baked mesh assets. Decoding those
   is far cheaper and more faithful than porting CSG booleans, voxel meshing
   heuristics or the light-grid bake.
2. **Missing assets degrade, never abort.** A place referencing an absent mesh,
   texture or sound must still load. Remote `rbxassetid://` fetching stays out of
   scope; local `rbxasset://` plus a user-supplied cache directory is the path.
3. **Keep the legacy look.** Constants, clamps and defaults come from the C++
   (e.g. reflectance `clamp(x,0,1)*0.8`, `walkSpeed 16`, `gravity 196.2`), not
   from modern Roblox.
4. **One system at a time, smoke-tested on a real place.** Every phase below ends
   with loading `slender.rbxl`/`2016 Starter Place.rbxl` and observing the change.

## Phase 1 - Terrain (biggest visual blocker)

Legacy: `App/v8datamodel/MegaCluster.cpp`, `App/include/v8datamodel/MegaCluster.h`,
`Rendering/GfxRender/{SmoothCluster,FastCluster,MegaCluster,SuperCluster}.cpp`,
`App/v8world/SmoothClusterGeometry.cpp`, `shaders/source/{megacluster,smoothcluster,smoothwater}.hlsl`.

- Decode `Terrain` `SmoothGrid`/`PackagedGrid` (V1/V2/V3) and the legacy 4x4x4
  cell grid; both currently land in `Property::Other("BinaryString")`.
- Mesh occupied cells; per-material diffuse from the existing material table.
- Add Rapier colliders for solid cells (4-stud cells, `App/include/voxel/Cell.h`).
- Water cells: colour/transparency/wave properties, animated normals from
  `PlatformContent/pc/textures/water/normal_01..25.dds`.

Effort L. Without it, terrain-only places have no ground.

## Phase 2 - Union/CSG and MeshPart geometry

Legacy: `App/v8datamodel/CSGMesh.cpp`, `App/include/v8datamodel/CSGMesh.h`,
`App/v8datamodel/PartOperation.cpp`, `Rendering/GfxRender/GeometryGenerator.cpp`
(`addCSGPrimitive`, `addFileMesh`).

- Decode the CSG binary mesh format (headerTag, version 2, vertex stride,
  indices) from `MeshData`; keep `PhysicsData` for colliders. No boolean
  evaluation needed for shipped assets.
- Render `UnionOperation`/`NegateOperation`; per-face box UV projection from
  `CSGVertex` `UVGenerationType`.
- `MeshPart`: reuse `crates/blox-scene/src/mesh.rs`, plumb `MeshId`/`TextureId`.

Effort M (MeshPart) / L (CSG). Both appear in community places today.

## Phase 3 - Dynamics, joints and terrain collision (playability)

Legacy: `App/v8world/{Primitive,Assembly,JointBuilder,ContactManager,Motor6DJoint}.cpp`,
`App/v8world/MaterialProperties.cpp`, `App/include/v8world/Primitive.h`.

- Read `Anchored` (already decoded) and give unanchored parts a dynamic Rapier
  body with mass from density; anchored parts stay static.
- Joints: `Weld`/`ManualWeld`/`Snap`/`Rotate` as fixed/composite bodies, `Motor6D`
  as a driven joint, in the legacy `Rotate -> Weld -> Snap -> Glue` order.
- Per-material density/friction/elasticity from `MaterialProperties.cpp`; honour
  `CustomPhysicalProperties` (now typed in `blox-model`).
- Terrain collision from Phase 1.

Effort M. Required for any mechanism, door, vehicle or falling prop.

## Phase 4 - Script execution (what makes a place a game)

Status: scripts run in Luau with a partial bridge and `ClickDetector` clicks/hovers work (see README). Not yet: `Touched`, `Humanoid`/`Players`, `Instance.new`, `Clone`, GUI, audio playback, dynamic physics.

Legacy: `App/Lua-5.1.4/src`, `App/script/ScriptContext.cpp`,
`App/script/LuaInstanceBridge.cpp`, `App/util/RunStateOwner.cpp`,
`App/v8kernel/Constants.cpp` (`uiStepsPerSec 60`), `App/v8datamodel/DataModel.cpp`.

- A Lua/Luau VM (the workspace already declares `mlua`) plus `ScriptContext`-style
  scheduling with one state per identity, `require`, and the
  `Stepped`/`Heartbeat`/`RenderStepped` loop.
- A live instance tree and mutable properties over the decoded model, services,
  signals and the datatype library (`CFrame`, `Vector3`, `UDim2`, `Instance.new`,
  `WaitForChild`, `GetService`).
- Startup order: local `Player`, `Character`, `PlayerGui` from `StarterGui`,
  `StarterPlayerScripts`, `StarterPack`, `ReplicatedFirst`.

Effort L. Without it, `Script`/`LocalScript` sources stay inert (`Property::Text`).

## Phase 5 - GUI

Legacy: `App/v8datamodel/{GuiObject,ScreenGui,Frame,GuiLayerCollector,BillboardGui,SurfaceGui}.cpp`,
`App/gui/{GUI,GuiDraw}.cpp`, `Rendering/GfxBase/Adorn.cpp`, `Typesetter.h`.

- `ScreenGui` -> Bevy UI: `Frame`, `TextLabel`, `TextButton`, `TextBox`,
  `ImageLabel`, `ImageButton`, with `UDim2` `Size`/`Position`, `AnchorPoint`,
  `ZIndex`, `SizeConstraint`, `Visible`, `ClipsDescendants`.
- `BillboardGui`/`SurfaceGui` as world-space quads.
- Input: hit-testing, `MouseButton1Click`/`Activated`, `MouseEnter`/`Leave`,
  `UserInputService` and `ContextActionService`.

Effort L. Slender-style places are mostly LocalScript + ScreenGui, so this pairs
with Phase 4.

## Phase 6 - Fidelity polish

- **Local lights** (`PointLight`/`SpotLight`/`SurfaceLight`) as Bevy lights first,
  with the legacy light grid (`Rendering/GfxRender/LightGrid.cpp`) later. Very
  visible in dark interiors. M, then L.
- **Sky bodies**: sun/moon billboards and stars (`Rendering/GfxRender/Sky.cpp`),
  plus the place's own skybox faces. S.
- **Particles/beams/trails**: `ParticleEmitter`/`Fire`/`Smoke`/`Sparkles`/
  `TextureTrail` (`Rendering/GfxRender/{ParticleEmitter,Emitter}.cpp`). M/L.
- **Sound**: `App/util/{Sound,SoundChannel,SoundService,SoundWorld}.cpp`; decode
  the shipped `content/sounds`/`content/music` assets. M.
- **Animation**: `Animator`/`KeyframeSequence`/`Motor6D` and the character rig
  (`App/v8datamodel/{Animator,KeyframeSequence}.cpp`, `App/humanoid`). L.
- **Adorn**: `SelectionBox`/handles (`Rendering/AppDraw/Draw{Adorn,}.cpp`). S.
- **Clothing**: Shirt/Pants/T-shirt composite textures
  (`Rendering/GfxRender/TextureCompositor.cpp`). M.
- **Exact curved-part UVs**: replace dominant-face projection with the legacy
  tessellated unwrap (`GeometryGenerator.cpp:699-1630`) and restore decals on
  balls/cylinders/trusses. M.
- **Humanoid fidelity**: state machine (swim/climb/sit/ragdoll) and leg/torso
  raycasts instead of one capsule (`App/humanoid/*`). L.

## Near-term wins (sub-day)

1. `MeshPart` geometry - reuses `mesh.rs`; unblocks decorated maps.
2. Sun/moon/stars - cheap, always visible.
3. `SelectionBox` adorn - trivial, useful for debugging.
4. Report unsupported GUI/effect children in the load summary so a place's
   remaining gaps are visible at a glance.

## Explicitly out of scope

CSG boolean evaluation, remote asset downloading, replication/networking,
physics-kernel parity (legacy spring kernel vs PGS), and the light-grid bake.
