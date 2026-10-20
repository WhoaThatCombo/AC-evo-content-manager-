# Formats

What this converter knows about the files it reads and writes, and where each fact comes from. Everything here was read off the games the user owns (Assetto Corsa EVO's `content.kspkg`, its executable and DLLs; the Assetto Corsa install), off public specifications, or off the code in this repository. Numbers marked **measured** come from `aceconv stockstats` (see the end) and can be re-checked after every game update.

Protobuf field numbers are written `fN`; "repeated" means the field occurs several times in one message.

## Assetto Corsa EVO

### kspkg packages (`content.kspkg`, mod packages)

- Data region from offset 0, then a 64 MiB table at the very end of the file.
- The table is 262,144 records of 256 bytes, XOR'd with the 8-byte key `c1 35 11 7d a9 21 97 9f` (the key restarts every record; 256 is a multiple of 8, so one pass over the table works).
- Record: path (UTF-8, NUL-padded, at most 228 bytes), kind u16 @228, path length in characters u16 @230, id u64 @232, size u32 @240, offset u64 @248.
- Kinds: 0x100 = file XOR'd with the same key at phase 0; 0 = file stored plain; 1 = directory (size 0). Unused records are encrypted zeros.
- `.texturemips` files are stored plain (kind 0): the game streams them to the GPU unchanged. Every other file is XOR'd.
- id = FNV-1a-64 over the **lower-cased** path encoded as UTF-16LE (offset basis 0xcbf29ce484222325, prime 0x100000001b3). **Measured**: all 117,511 file records of a stock content.kspkg match the lower-cased form; none matches only the path as written.
- Records are sorted by id, as Kunos's own archives are. The game finds a file by hashing its path, so the table must stay sorted and every directory needs its own record.
- Code: `src/io/kspkg.rs`.

### Protobuf conventions

- Every structured EVO file (`.mesh`, `.material`, `.texture` header, `.scene`, `.track`, `.actor`, car data files) is protobuf without a schema in the file. The editor in `src/io/pb.rs` keeps every field in file order and every unknown field, so decode then encode is byte-identical.
- Proto3 rules apply: zero numbers and empty sub-messages are left out. A zero `f32` is simply absent; a vector of zeros is an empty sub-message.
- Vectors are sub-messages `{f1 x, f2 y, f3 z[, f4 w]}` of `f32`.

### `.texture` / `.texturemips`

Header (protobuf), root fields as **measured** on every stock texture with a page table:

| field | meaning |
|---|---|
| f1 / f2 | width / height |
| f3 | mip count |
| f4 | format enum (below) |
| f6 | 1 |
| f10 | source: {f1 path (stock: the authoring machine's absolute path; ours: the AC1 texture's relative name, e.g. `texture/EXT_Skin.dds`), f2 / f3 source size, f4 1, f5 1, f6 source pixel layout (most common per format: 2 on enums 12 / 27 / 33, 5 on 5 / 6 / 13 / 26 / 34)} |
| f11 | import settings (below) |
| f12 | page table: {f1 / f2 tile size, f3 1, f4 first page per mip (packed varints), f5 pages per mip, f7 one value per page: 1024, 512 on the uncompressed enums 5 / 6} (per-format histogram of every root / source / page-table field: `examples/texhdr.rs`, `tools/texhdr_stock.tsv`) |
| f13 | 1 |

f20 (1 or 2) appears on about 1 % of stock files and is not written.

Format enum, tile size and import settings (**measured**, most common signature per format):

| f4 | format | tile | import f1 | import f9 | import f11 | import f13 | typical stock use |
|---|---|---|---|---|---|---|---|
| 34 | BC7 sRGB | 256 x 256 | - | 1 | 0.5 | - | car `_ao`, `_c`, track albedo |
| 33 | BC7 linear | 256 x 256 | 1 | 1 | 0.5 | - | track `_mask`, car `_wet` |
| 26 | BC6H | 256 x 256 | 3 | 1 | 0.5 | - | every `_nm` / `_normal` map |
| 13 | BC1 sRGB | 512 x 256 | 8 | 1 | 0.5 | - | car `_d`, track `_alb` |
| 12 | BC1 linear | 512 x 256 | 5 | 1 | 0.5 | - | car `_map`, `_wet`, track `_nm` |
| 27 | BC4 | 512 x 256 | 2 | 1 | 0.5 | - | `_r`, `_m`, `_at`, track roughness / ao |
| 6 | RGBA8 sRGB | 128 x 128 | - | - | 0.5 | mip count | car `ext_lights_f_*` lamp masks |
| 5 | RGBA8 linear | 128 x 128 | 2 | - | 0.5 | mip count | car `int_*_f` function masks |
| 30 | R11G11B10F volume | unpaged | | | | | track irradiance volumes |

- Import f2 (on about a third of stock files) is log2(source size / texture size): 1 = half, 2 = quarter, 3 = eighth. The converter records the size it encodes and leaves f2 out.
- Source f6 is 5 on most stock files (2, 3, 25, 28 also occur); the converter always hands the encoder RGBA8 and writes 5.
- The full per-format signature table is in `stockstats\stockstats.json`, `facts.textures`.
- Irradiance volumes (format 30) carry only f1, f2, f3, f4, f6 (= 16, the depth) and f8 (= 1); their `.texturemips` is w x h x d x 4 bytes per mip, unpaged.

`.texturemips` layout: each mip is cut into tiles of the size above; every tile fills one whole 64 KiB page, padded on both axes; tiles row-major; inside a tile the row pitch is the tile's row pitch (BC7 / BC6H 16 bytes per 4x4 block, BC1 / BC4 8, RGBA8 4 bytes per pixel). Uncompressed masks carry a single level. The file size is pages x 65,536.

Code: `src/io/texture.rs`.

### `.material`

- f1: shader name (`UberVehicleMaterial`, ...).
- f4 (repeated): property {f1 name, f2 value}. A value is {f1: f32} for a scalar or {fN: {f1..fN f32}} for an N-component vector (N = 2..4). Zero components are absent, so a zero scalar is an empty value.
- f5 (repeated): texture slot {f1 name, f2 {f2 texture path}}; an unused slot has an empty f2.
- Example (stock): `Splatmap_Constant` = (1, 0, 0, 0) is the value bytes `22 05 0d 00 00 80 3f`.
- The root is the game's `MaterialData` (Renderer.proto, one of the FileDescriptorProtos embedded in the executable): f2 blendMode (0 opaque, 1 alpha to coverage, 2 alpha blend, 3 RGB blend, 4 alpha test, 5 OIT), f3 depthMode, f6 cullMode (0 front-default, 1 back, 2 none), f8 alphaToCoverage channel, f9 / f10 / f11 alpha-to-coverage scale / cutoff / mip clamp, f12 flipNormalOnBackside, f18 renderOrder (int32), f20 baseMaterialPath, f21 / f22 the variable / resource names a child overrides, f27 specularProbeDistance, f28 screenSpaceShadowContrast, f110..f112 alpha test scale / cutoff / mip clamp. The variables f4 are the shader's own cbuffer members (`cullMode` and `blendMode` among them: the shader flips back-face normals by the `cullMode` variable).
- A material with f20 takes its parent's variables and slots except those its f21 / f22 lists name; the child's copies of the rest are stale (**measured**: stock `spa\asph_spa` carries a grey `albedo2` base that its parent `asphalt_new` replaces).
- Code: `src/io/material.rs`.

### Track materials (stock usage, **measured** over the 3,700 materials of 17 stock circuits + common_assets)

| shader | count | what it is |
|---|---|---|
| UberBiplanarMaterial | 1,607 | the general track surface: Base + Red / Green / Blue layers |
| UberMaterial | 839 | the same family, no biplanar |
| DynamicTrackMultilayer / DynamicTrack | 199 / 115 | kerbs, concrete, grass edges / tarmac (rubber, wetness) |
| Fence, Grass, Subsurface, DefaultMaterial | 97, 104, 230, 77 | |
| Particle, Semaphore, Glass | 31, 19, 5 | flipbook emitters, start lights, glass |

- Two-sided: f6 = 2 together with the variable `cullMode` = 2 (Fence 62 / 97, foliage). Decals: blendMode 2 and a negative renderOrder (-2 .. -20). Stock never sets depthMode.
- Texture format per slot (headers of every filled slot): base colour BC7 sRGB (34; `Base_BaseColorMap` also BC1 sRGB 13), layer colour (`Red_BaseColorMap`, ...) BC1 sRGB, normals BC6H (26), roughness / AO / metal BC4 (27), `SpatMap` BC1 or BC7 linear (12 / 33). Base maps are 2048 typical, 4096 on terrain and road bases; normals 1024, roughness 512.
- **The layered track shader** (DXIL of pixel shader 0x034e5ce0 and its permutations, lifted with `L:\evoview_work\shaderre\lift.py`, branch conditions restored): weights r, g, b from the SpatMap (or `Splatmap_Constant`), the weakest dropped unless `Splatmap_DisableRestrictBlendToTwoLayers`; base weight saturate(1 - r - g - b); all four normalised by their sum. Sum mode: albedo = w_base x Base + Σ w x Layer; multiply mode (`Splatmap_MultiplyBaseColor`): Π lerp(1, layer, w). Then the Detail step (`Detail_Enable`, `Detail_BaseColor`): albedo = saturate(albedo x lerp(`Detail_BaseColorMultiplier` x Base, 1, w_base)). A layer with `X_Triplanar` samples (camera + position).xz x (`X_TriplanarScale`.xz + 0.01). DynamicTrack (0x036d7ad0) is the same with the dynamic track's rubber added to the Blue weight. `txDiffuse` is not read by these shaders.
- Blinking: Particle materials with `Flipbook_Enable`, `Flipbook_Columns` / `_Rows`, `Flipbook_Duration` (seconds per cycle) and `Emissive_UseBaseColorAsEmissive` (stock `navigation_lights`: 16 x 1 frames, 3 s).
- Start lights: SMesh entities whose kinds add `starting_lights`, on a Semaphore material (parent `common_assets\materials\starting_lights.material`); the lamp class is in the mesh's colour stream: 1/255 in R (red lamps), G (green), B (others), measured on monza / kyalami / laguna_seca.

### Track lights (**measured** on the stock scenes: 1,659 Light, 34 InstancedLight)

- `LightData` (LogicScene.proto): f1 type (0 point, 1 spot: 96 %), f2 colour, f3 intensity, f4 radius, f5 / f6 inner / outer angle in degrees (outer 50..70), f7 cast shadow (on most), f9 / f10 / f16 shadow (ESM) settings, f14 specular scale (1), f19 volumetric scattering, f21 falloff exponent (median 2).
- Intensity by radius (p50): under 40 m about 5, 40..80 m 75, 80 m and more 150. A spot's beam is the light's local -Y (zero-rotation spots hang from poles; tilted ones aim at or below the horizon).
- `InstancedLightData` (Scene.proto, actor f139): f1 max distance (250..850), f2 fade distance (50..100), f3 the LightData, f4 instance set {f2 positions, f3 rotations, f4 scales} packed f32 x 3; 10..70 instances each.
- Entities carry active distance 250 and an xz box (on 40 %).

### Mesh LODs (**measured** on 480 stock track meshes)

LOD body f3 lodIn (metres): 97 % of stock track meshes have one level; the rest 2..4, level 1 from 50..200 m (median 200) at 12..50 % of the triangles. MeshData f4 lodOut (cull distance) on 28 % (400 m typical, 800).

### `.mesh`

- Top: f2, f3 (flags), f5 (repeated) LOD bodies, f6 / f7 bounds min / max, f16.
- LOD body: f4 (repeated) sections {f1 name, f2 first index, f3 index count, f4 material path}; f5 positions (3 f32 per vertex); f6 normals (3 f32); f7 UV0 (2 f32); f8 tangents (4 f32); f9 / f10 skin weights / bone indices (4 values per vertex); f11 indices (packed varints); f12 (repeated) bones {f1 name, f2 parent name, f3 4x4 matrix as f32 fields 1..16}; f13..f16 further per-vertex streams; f17 / f18 LOD bounds.
- Bone indices are local to each mesh: a mesh names its own bones in f12.
- Code: `src/validate/mod.rs` (`check_mesh`, `check_mesh_lighting`), `src/stats/stockstats.rs`.

### `.scene`, entities, containers

- A scene is f2 (repeated) entities: f1 name, f3 (repeated) kind names (the first is the entity's own kind; a Container lists the kinds it holds after it), f4 transform {f1 position, f2 rotation, f3 scale}, f13 xz box {f1 min, f2 max, f3 cull distance}, f50 kind data.
- Kind data used here: Container f50.f121 {f1 container path}; SMesh f50.f100 {f1 mesh path}; ISMesh f50.f105 {f1 mesh, f7 {f2 positions, f3 rotations, f4 scales} as packed f32 x 3 per instance}; Irradiance Probe f50.f120 {f1 / f12 textures, f6 256 ground heights as f32}.
- A container only loads the entity kinds its Container entity declares.
- **Measured** entity kinds on one stock track (imola): Irradiance Probe 640, SMesh 441, Light 137, ISMesh 113, Start Pos 92, Camera 71, Decal 70, Zone Sound 56, Marshal 40, Spline 31, Container 25, Reflection Probe 15, Zone 7, Surface Def 1, Track Info 1, Macro Irradiance Volume 1, Track Layout 1.
- Scene f1 (repeated): every kind string the scene's entities carry (the main scene's include the tags its Container entities declare); f3: SceneGraphicsSettings (f1 haze minimum altitude, f7 / f8 custom haze distance, f50 / f51 night irradiance gamma / clamp, f60 / f61 the omni-haze curve `content\tracks\common_assets\curves\omni_haze_density_by_sun_elevation.curve` on most stock tracks).
- Entity: f2 enabled = 1; f12 guid (fixed64); f14 active-distance override (100000 on Track Info). Stock writes the transform's position and rotation even when zero (empty messages), scale (1, 1, 1), and bounds f13.f3 = 250 on nearly every entity.
- **Rotation**: Euler degrees (x, y, z), R = Rz * Ry * Rx on column vectors (X applied first), Y up; a car's forward is local +Z. Found on stock spawns: a pit box facing a heading of 102.8 degrees is stored (-176.8, 77.2, -177.75), the pit slot in front of it (0.65, 11.95, -1.4), and the pitch / roll that convention gives both match the pit lane's measured slope. Code: `track::scene::euler_of` / `matrix_of`.
- Container f50.f121: f1 path, f3 loaded_on_demand = 1 for containers the declaration rows list; containers loaded with the scene (fmod, big screens, decor) carry no f3. The Container entity declares zone instance types too (`timelines`, `pitlane`, `speed_limit`) although its Zone entities carry only `Zone`.
- Surface Def f50.f109: repeated SurfaceDefinitionData (see `src/track/surfaces.rs` for the fields and the measured per-kind ranges). Stock walls carry only f1 and f2 = 2.
- Track Info f50.f111 {f1 SceneTrackInfo {f1 track type 1 (circuit), f2 display name, f4 coordinates {f1 longitude, f2 latitude} doubles, f5 time zone}}.
- SMesh f50.f100: f1 mesh, f2 surface id (the collision surface; absent = no collision), f3 physics_only, f4 collider (empty boxBounds f5 / f6 on stock), f5 physics_movable.
- Start Pos f50.f104: f6 packed types (3 grid, 4 pit, 5 box; a slot may carry 4 and 5), f2 slot number (absent for 0), f4 empty tags. Hotlap starts are type 3 without a slot. **Measured**: stock spawns sit 0.00..0.10 m above the road (oulton_park, kyalami).
- Zone f50.f112 {f30 debug {f1 0.05, f3 3}, f1000 ZoneData {f1 geometrics {f1 four corner transforms, f21 colour, f22 render height 0.8}, f2 instance {f100 timelines | f101 pitlane | ...}}}. Timelines: one Zone, one quad per gate, in lap order with the start / finish line **last** (measured on oulton_park and monza against their ideal lines: gates at 1254 / 2858 / 4283 m of a 4283 m lap). Stock gates are 26..50 m wide and 0.7..2.4 m deep.
- Spline f50.f108 {f1 SplineData {f3 nodes {f1 transform, f10 / f11 anchors}, f10 closed}, f21 colour, f30 debug}; the layout container's centre spline carries the extra kind `center_spline` on the entity itself.

### `.track`

- f1 TrackData (we write f8 / f9 from the dynamic-track frame when a preset exists); f1.f2 dynamic track on, f1.f4 base grip (0.98 on stock), f1.f8 track centre {x, y, z} (empty on oulton_park and kyalami), f1.f9 dynamic-track extent (6000 on most stock tracks), f1.f10 the wet `.dynamictrackpreset` path (kyalami uses imola's); f2 AI spline hints (empty); f3 altitude in metres.

### Stock heights

**Measured**: stock EVO roads go below y = 0 (spa's road meshes reach -29.0 m, suzuka's walls -9.1 m), so the converter keeps AC1 heights (no lift).

### Track lighting bakes (`src/track/{bakes, bvh, ao, irradiance, pigment}.rs`)

Run after the meshes are written: they read the `.mesh` files back, cast rays against the drawn opaque geometry (one ray tree per 1 km tile on maps over 6M occluder triangles, else one for the track) and rewrite the meshes' colour streams. `ACECONV_BAKES=ao,probes,pigment` (or `none`) picks them; unset = all.

**Vertex AO** (game facts, **measured** with the AO component of 2,600 stock track meshes of laguna_seca, brands_hatch, monza, imola, spa):
- LOD body f13 holds 4 floats per vertex, each at most 1/255; component 3 is the AO as ao / 255 (no AO = 1/255). Materials read it with `Vertex_UseVertexAmbientOcclusion` 1, `Vertex_AmbientOcclusionChannel` 3, `Vertex_AmbientOcclusionIntensity` 250 (255 with `Vertex_AmbientOcclusionOffset` 0.001 on some): the layered track shader takes saturate(intensity x map AO x (offset + colour[channel])). Component 0 carries road variation on stock DynamicTrack meshes with `Vertex_UseVertexVariation` (`Vertex_VariationChannel` 0); the converter's road / kerb templates have no such variable.
- Stock AO x 255 by shader, p10 / p50: DynamicTrack 0.90 / 0.94, DynamicTrackMultilayer 0.66 / 0.93, Fence 0.48 / 0.67, Uber* 0.22 / 0.60. Every stock mesh carries AO data, also where its material leaves it off.
- Ours: cosine-weighted rays (8..32 per vertex from a 30M-ray budget) to 8 m, a hit at t occluding by 1 - t / 8; parallel surfaces less than 25 cm above the vertex (AC1 overlay layers: tyre marks, grass borders, painted kerbs) are passed; values stop at 0.15 (a vertex under a barrier is hidden, but black there would smear into the visible triangles); LOD levels take the LOD0 value at the same position. Layered materials (DynamicTrack, DynamicTrackMultilayer) switch the vertex AO on (channel 3, intensity 255, offset 0); DefaultMaterial has no vertex AO variable.

**Irradiance volumes** (game facts, **measured** on stock laguna_seca, brands_hatch, monza, imola, spa; the shader side from the executable's DXIL: compute 0x046ff1a0 updates the SH volumes, the track pixel shaders read them):
- Main scene: one `Macro Irradiance Volume` entity (f50.f135 empty; position = centre of the probe grid, scale = its half extent, roughly; active-distance override 100000 (absent on some; 10000 watkins_glen, 1e7 nurburgring; never the probes' 1000); brands_hatch also tags it `GlobalIllumination Bounds`), and one `Irradiance Probe` per volume on a full rectangular grid (laguna 14 x 18, brands 20 x 20, monza 24 x 34, imola 32 x 20, spa 20 x 32); entity position = the volume's corner, y = 0; active-distance override 1000.
- IrradianceVolumeActorData (f50.f120): f1 bounced-sky visibility texture, f2 groundInterval (4.5; spa 5.0), f3 / f4 / f5 the three level heights above the ground (1.5 / 5.5 / 13.5; spa 15.0 on top), f6 256 ground heights (16 x 16 cells of groundInterval, x fastest; neighbouring volumes continue each other's edges), f8 collisionTolerance 1.5, f9 / f10 grid x / z (proto3: 0 absent), f11 the macro entity's guid (fixed64), f12 the night texture. A volume spans 16 cells: 72 m (spa 80 m).
- Textures `irradiance_volumes\iv_bounced_sky<guid>.texture` and `iv_night_irradiance_<guid>.texture` (the probe's guid in decimal), header {f1 64, f2 3, f3 1, f4 30, f6 16, f8 1}: R11G11B10 unsigned float, 64 x 3 x 16, x fastest, x = 4 x cell + SH coefficient k, y = level, z = cell; cells along world +X and +Z (edge continuity with the neighbouring volumes: 0.12..0.15 per edge step against 0.29..0.44 for the other axis).
- The GI update: SH_k = ((V_k (1 + sun x Y_k(sun mirrored in the ground) x sky) x bouncedSkyIntensity + max(lum(V_k), 0.1) x skyIntensity) x sky + night; the track shader: E(N) = max(0, 0.886 sh0 - 1.023 N.y sh1 + 1.023 N.z sh2 - 1.023 N.x sh3) / pi. So V_k is the L1 SH projection of the light reaching the point under a unit sky, basis (0.282, -0.489 y, 0.489 z, -0.489 x), sun bounce added at run time. The format holds no negatives (the sky's own term in coefficient 1 is negative and stored as 0).
- **Measured** texels, coefficient 0 (G) p50 by level: laguna_seca 0.91 / 1.03 / 1.05, brands_hatch 0.36 / 0.43 / 0.58; coefficients 1..3 p50 0..0.02, p90 0.04..0.19; night textures zero on 80..98 % of texels.
- Ours: grid over the drivable area + 120 m, 4.5 m cells (grown in 0.5 m steps while more than 900 volumes would be needed); ground = the highest drawn ground surface (drawn physics mesh) at each cell centre, else the highest upward-facing opaque surface, holes from the nearest filled cell; texels from 48 rays to 250 m (sky radiance 1; a hit reflects albedo x (0.5 + 0.5 n.y) x 0.85, the back of a surface albedo x 0.15, rays leaving the map below the horizon see albedo 0.2; alpha-tested cards (trees, bushes, mesh fences) on the way pass a quarter of the ray); enclosed texels (90 % back faces) take the level above; scale `irradiance::SCALE`, calibrated against the twins (below); night textures zero.

- **Calibration** (`irradiance::SCALE` 0.6, foliage pass 0.25), coefficient 0 (G) p10 / p50 / p90 per level, converted vs stock twin: ks_laguna_seca L0 0.52 / 0.98 / 1.09, L1 0.73 / 1.06 / 1.13, L2 1.02 / 1.11 / 1.14 against laguna_seca 0.20 / 0.91 / 1.44, 0.34 / 1.03 / 1.39, 0.59 / 1.05 / 1.33 (medians within 3..7 %); ks_brands_hatch medians 0.70 / 0.75 / 0.89 against 0.36 / 0.43 / 0.58: the stock grid also covers the forest around the circuit, the AC1 map has no geometry there. Converted tracks are less spread than stock (fewer deep shadows: AC1 trees are cards, not canopies).
- **With vegetation** (P9): the replaced AC1 cards are no longer occluders; each plant adds a crown proxy (`vegetation::canopies`: an octagonal bipyramid of 0.45 x width x `CANOPY_SCALE` radius as a partial foliage occluder, the lower half of a half-size core as a solid one; a closed solid core made texels inside crowns "enclosed" and bright). `CANOPY_SCALE` 1.4 calibrated with `probetree.py` (texels within 6 m of a plant, same xz as the stock twin), coefficient 0 (G) median L0 / L1 / L2: ks_brands_hatch near our plants 0.17 / 0.16 / 0.26 against stock brands_hatch 0.14 / 0.20 / 0.38 (P8 with cards 0.30 / 0.33 / 0.37; scale 1.0 gave 0.32 / 0.35 / 0.56); near the stock plants 0.25 / 0.27 / 0.55 against stock 0.10 / 0.15 / 0.35 (P8 0.39 / 0.46 / 0.83); ks_laguna_seca near our plants L0 0.65 against stock 0.36 (P8 0.60). Open texels are unchanged (brands 0.85 against stock 0.47: the stock grid also sees the forests outside the AC1 map; `irradiance::SCALE` stays at its laguna calibration).
- **Track lighting audit** (ks_brands_bobr against stock brands_hatch at the same xz, stock y = ours + 151.8): coefficient 0 (G) median L0 / L1 / L2 ours 0.69 / 0.76 / 0.94 against stock 0.29 / 0.36 / 0.54 over the 65,280 shared texels; at the pits (60 m around (-220, -330)) the irradiance E(N) of a wall is 0.30 against 0.14..0.15. The GI-update compute shader (0x046ff1a0) multiplies every texel by 0.66 x the global SH colour (texel (0, 0) of `globalSHCoefficents`) itself, so the texels hold visibility under a unit sky, not radiance. The DefaultMaterial pixel shaders (0x034c6530, 0x03802ba0, 0x03ae14f0, 0x03e0db30) never read COLOR (no vertex AO) and take their ambient diffuse from the irradiance volumes when a volume covers the pixel, else from the global probe at `globalProbeDiffuseMipLevel`, times lerp(`envIblDiffuseShadowFloor`, 1, sun shadow). Stock reflection probes on brands_hatch (19) are 6 x 2.5 x 4.6 m boxes inside the pit garages, one shared capture; sebring has none.

**Pigment** (game facts, stock brands_hatch, monza, imola, spa, laguna_seca): SceneGraphicsSettings f4 Pigment {f1 transform (position = centre, scale = half extent; y unused), f2 texture, f3 world_align, f4 world_align_scale_xz}; textures BC1 sRGB (13) or BC7 sRGB (34), 2048..4096 a side, full mips; mapping u = (x - cx) / 2hx + 0.5 along +X, v along +Z, texel row 0 at the low-z edge (stock road meshes laid over the decoded brands_hatch pigment line up in that orientation only). Read by the Grass, CrossImpostor and some Subsurface materials (`Pigment_Enable`), not by track surfaces. Stock means (sRGB, mip 3): brands (74, 72, 62), monza (48, 54, 39), spa (63, 66, 54), imola (97, 96, 75). Ours: `pigment\pigment_map.texture`, BC1 sRGB, long side 2048, over the drivable area + 300 m: the estimated albedo of the top-most upward-facing drawn ground surface per texel, unpainted texels the nearest painted one, then a Gaussian blur up to the stock smoothness (the lag where the luma autocorrelation falls to 0.9: stock median 1.09 m, `aceconv measure pigment`).

### Track vegetation (`src/track/{vegcat, vegetation, csptrees, grass}.rs`)

Game facts (**measured** on the stock scenes with the clean P9 tools; road_atlanta and interns left out):
- Plants are ISMesh entities (f50.f105 {f1 mesh, f7 {f2 positions, f3 rotations (Euler degrees), f4 scales}, f10 {f5 "", f6 ""}}) with f13 {xz box of the instances, f3 cull distance} and f14 = 100000. Instance positions are relative to the entity transform (stock brands_hatch places its nodes at y = 151.808 with instances at -20..16; ours sit at the origin with world positions).
- Cull distance f13.f3 = the mesh's MeshData f4 lodOut on nearly every node (beech_7 1200, bush_05_b 450..550, grass_2_b3 120, grass_2_b4 150, billboards 2500..5500). Instance-weighted medians per track: trees 850..1200, impostors 1700..3500, bushes 350..800, grass 120..150.
- Factory meshes: `common_assets\meshes\vegetation\big\trees\<species>\...lod0.mesh` (4..5 LOD bodies, the last a camera-facing card from 150..450 m), `big\trees\billboards\bb_<n>` and the species cards `cf<n>.mesh` (flat, camera-facing: stock places them with zero rotation), `mid\` / `acc\mid\` (bushes), `low\grass\tracks_terrain_vegetation\grass_2\...` (grass_2_b4 / b3 hold 10.3M / 3.7M of the stock grass instances). Leaf, grass and impostor materials already read the track pigment (`Pigment_Enable` 1); grass albedo is grey (the pigment colours it).
- Trees stand upright: rotations (0, yaw, 0) or (180, y, 180). Grass tufts carry a yaw and a tilt to the ground; scales xz 0.5..1.8, y 0.55..0.75 of it.
- Per track (stockstats `veg.*`): 3D trees 28..32k (median 481), 6..20 % of all trees, within 150 m of the road (brands_hatch: 438 of 485 within 50 m); impostors 1.5k..36k out to 1000 m+; bushes 0.7k..58k; grass 0.13M..4.1M within 150..200 m. Species per track: trees median 3 (p90 8), bushes 1 (4), impostors 3 (5). Drawn heights: trees p50 15..20 m (p90 20..24), impostors 14..21 m.
- Grass density against the distance to the road edge (instances per m² of covered ground, 4 m cells), brands_hatch / imola / spa / monza: 0..2 m 5.6 / 10.3 / 9.0 / 1.4, 10..20 m 3.5 / 6.0 / 5.8 / 0.9, 35..50 m 1.3 / 1.1 / 2.6 / 0.4, 100..150 m 0.4 / 0.19 / 0.38 / 0.23, none beyond 200 m.
- Stock grass containers (`ground_grass*.scene`, `ground_vegetation*.scene`) carry no f3 (loaded with the scene) and no row lists them.
- AC1 brands_hatch and EVO brands_hatch share x / z (ideal lines within 0.9 m, p50); EVO is 151.8 m higher. Plant positions can be compared directly: 93 % of the stock 3D trees have one of ours within 10 m.

AC1 side: plant cards use ksTree / ksGrass, or alpha-tested materials named after plants (`trees`, `treeline`, `treewall`, `bushwall`, `Leaves`, `branch`); forest walls are single cards 20..40 m tall and 30..500 m long (Barber `treewall`). CSP `[TREES]` lists: see `src/track/csptrees.rs`.

Ours (module headers have the details): catalogue built from the package at run time; card fragments clustered into trees, forest walls cut into rows; plants on the soil, off the road; species by height / shape / colour / climate with a per-track palette (4 trees, 2 bushes, 3 impostors); 3D within 60 m of an AI line, impostors beyond; caps 60k plants, nodes per species and 1024 m tile; grass on a 2 m raster with the brands_hatch density profile to 150 m, kept 3.5 m off the AI line, under 1.3M tufts, in `containers\decor_grass_<n>.scene` (at most 300k tufts each, loaded with the scene); canopies as bake occluders. `ACECONV_VEGETATION=trees,grass|none` picks the parts; `aceconv veg-catalogue` prints the catalogue.

### `system\tracks.table`, `system\track_containers.table`

TableData f2 PresetTableData {f3 (repeated) Line}. A tracks.table Line has f2 TrackContentData {f1 display name, f3 folder `content\tracks\<id>`, f4 the `.scene`, f5 nation (alpha-3; RSA for South Africa), f6 {f1 longitude, f2 latitude} doubles, f7 time zone, f8 the `.track`, f10 is_release 1, f13 continent ("Australia" for Oceania)}. A track_containers.table Line has f8 TrackContainersContentData; the stock circuits have four rows per layout:

| row (f1) | flag | f9 max drivers | f11 containers |
|---|---|---|---|
| `<Layout> Race` | f5 = 1 | grid size | layout, timelines, grid and pit spawns (+ marshals, pit zones) |
| `<Layout> Time Attack` | f6 = 1 | grid size | layout, timelines, pit spawns |
| `<Layout> Hotstint` | f7 = 1 | 1 | layout, timelines, hotlap spawns |
| `No Game Mode` | f15 = 1 | - | TV cameras, `common_assets\...\common_car_cam_sequence_left.scene` |

All rows: f8 lap length (m), f10 the tracks.table display name, f14 layout name (matched case-sensitively), f16 = 1, f17 = 10, f18 layout image {f1 10, f2 / f3 minimum x / z of the track, f4 its larger extent, f5 or f6 the whole slack on the shorter axis (f4 - extent; the map texture centres the track, half each side; x right, z down - measured on brands_hatch-gp)}; f20 ui_priority 1 on the default layout's No Game Mode row; f21 a Kunos backend id (left out). Only rows of the 18 Kunos circuits are read as evidence: the local tables also hold earlier custom installs. (Kunos keeps the layout container, and often the pit spawns, in the No Game Mode row; ours lists the layout container in the mode rows - equivalent, see below.)

**How a session finds its rows** (`aceconv check-track-decl` simulates it; all 17 stock circuits pass, road_atlanta skipped as locally modified):

- a session names the track (row f10 = tracks.table f1), the layout (f14; the UI's `TrackLayouts.items[].layout`, saved in `GameModeType_*.gamemodesave`) and the event (f1). The game's own id for it is "track name + __ + event name (ie Brands Hatch__Indy Hotstint)" (gflag `ai_tester_track__eventname`), and its log prints "Game Started! ... | Fuji Speedway GP Short Time Attack Practice";
- practice and time attack load the Time Attack row (f6), races the Race row (f5), hot laps the Hotstint row (f7), each together with the layout's No Game Mode row (f15 is_base_layout): Nordschleife GP Strecke's Time Attack row lists no spawn file and starts in the 30 pit slots of its base row;
- spawn types (Start Pos f50.f104 f6): 3 grid, 4 pit, 5 box. Race needs 3, practice / time attack 4 or 5, hotstint 3 (the Nordschleife's hotlap start is 4 + 5);
- a session whose layout / event no row has loads no container: "0 spawn points" of every type, "No starting position!", crash. The ks_black_cat_county crash (2026-10-19) was such a session: its saved selection named layout `layout_short` and event `Black Cat County` (with Fuji GP Short's 4526 m and 34 drivers left in it), while the rows say `Short` / `Short Time Attack`. A launcher must take `layout_name` and `event_names` from track_decl.json;
- stock event names are not always `<layout> <mode>` (Watkins Glen's GP Inner Loop has a `GP Hotstint` like GP); ours always are, and layout names are unique by UI id.

### Track UI images (`src/track/uiimages.rs`)

The UI (`uiresources\js\components.js`) asks per (track, layout) for `images\tracks\<getTrackId>.texture` (track select / loading page, stock 1920x1080 BC7 sRGB), `images\trackmaps\<getTrackId>.svg` (track select map, stock viewBox 380x240) and `images\trackmaps\<slug>-<slug>.texture` (HUD map, stock 1024x1024 BC7, a white outline on transparent in the f18 frame). getTrackId = name trimmed with its first `-` made `_`, `-`, layout, spaces `_`, lower case; slug = every run of non `[a-z0-9]` as `_` (stock `paul_ricard-layout_1a-v2.svg` but `paul_ricard-layout_1a_v2.texture`). coui serves them from the root path `uiresources\...` only (content.kspkg, mod kspkg, then loose `Saved Games\ACE\mods\`). Ours: AC1 `preview.png` scaled to cover 1920x1080; both maps drawn from the layout's centre line.

### `ui\track_decl.json` (aceconv track output)

The converter never writes content.kspkg; ACECM applies these rows:

- `format`: `aceconv-track-decl-1`; `track_id`, `display_name`, `aceconv_source` (the AC1 folder);
- `tracks_table_row`: hex of one tracks.table Line;
- `container_rows`: hex of each track_containers.table Line, every layout's Race row first;
- `layouts`: per layout its name (`name` = `layout_name`, the f14 a session must name), `event_names` {race, time_attack, practice, hotstint} (the f1 a session must name), `ui_track_id` / `ui_map_id`, AC1 layout folder (`ac1_layout`: input only, never a session key), length, grid / pit slots, container stems and row names;
- `ui_files`: [{from, to}] - the UI images, `from` relative to the track folder (`ui\uiresources\images\...`), `to` relative to the mods root: the installer copies each to `Saved Games\ACE\mods\<to>` (or packs it at the root of a mod kspkg). Inside the track folder they are not served;
- `geo` (nation, continent, time zone, coordinates, whether geotags were found) and `notes`.

The converted folder (`<out>\<id>\`): `<id>.scene`, `<id>.track`, `containers\layout_<L>.scene` (collision meshes, layout-only decor, centre spline), `containers\spawnpoints_{grid,pitlane,hotlap}_<L>.scene`, `containers\timelines_<L>.scene`, `containers\decor_lights.scene` (CSP lights, loaded with the scene), `containers\pitlane_zones_<L>.scene`, `containers\drs_zones_<L>.scene`, `containers\tv1_cameras_<L>.scene` / `tv2_cameras_<L>.scene` (+ `cameras\*.curve`), `containers\fmod_<id>_track.scene` (audio zones, loaded with the scene), `layouts\layout_<L>.{ideal_line,pitlane}.aisplinedata` and `.trackcontrolpoints`, `dynamic_track\<layout name>.dynamictrackpresetcompressed`, `static_mesh\*.mesh` (`static_mesh\physics\` for collision-only meshes), `materials\`, `textures\`, `irradiance_volumes\` (probe textures), `pigment\pigment_map.texture`, `ui\track_decl.json`, `ui\convert_report.txt`, `ui\look_albedo.tsv` (every material's shader, class and estimated albedo).

### Dynamic track preset (`dynamic_track\<layout>.dynamictrackpresetcompressed`)

1. f1 frame min {x, y, z}; f2 frame max (a square of side E centred on the track centre, min.y == max.y);
2. f3 (repeated) tile {f1 zlib of 64 x 64 cells x [rubber, grip], f2 tile id};
3. f4 (repeated, same order) tile {f1 (repeated) {f1 height f32} x 4096, f2 tile id};
4. f5 packed varint tile ids, ascending.

Grid: 256 x 256 tiles over the square, tile id = row * 256 + col (row along +z, col along +x from min), cell index = (z mod 64) * 64 + (x mod 64). Empty cell: rubber 0, grip 57, height -9,999,999. **Measured** over the stock presets: 318..1955 tiles; 79..99 % of road cells carry no rubber; grip median on road cells 118..189 (p10..p90 of the track medians 164..187). Code: `src/track/dyntrack.rs`.

The file name is the layout's display name in lower case (the rows' f14: `gp`, `gp short`, `layout 1a-v2`; checked on all 31 stock presets), not the container stem. Nothing references it by path: the `.track` carries the frame (f1.f8 centre, f1.f9 extent). **Measured**: the stock ideal and pit lines lie on the preset's road heights (brands_hatch, monza: p1..p99 within 0.02 m), and stock spawns 0.00..0.09 m above them, so the heights are the road surface. Rubber on stock brands_hatch falls off across the line: mean 37 within 0.5 m, 14 at 2..3 m, 0.9 at 4..6 m; monza 107 / 17 / 0.7. Grip on stock brands_hatch: road 184..193, 255 on 1.5 % of the cells, 86 on 24 % (the verge, 15..30 m from the line).

Built by `track::dyntrack::build` (ours; see its header): road / kerb / pit / concrete triangles rasterised at the cell centres, grass-like ground within 12 m of them as verge, rubber from the ideal line and the lap model, grip from smooth noise.

### AI splines (`layouts\layout_<L>.{ideal_line,pitlane}.aisplinedata`, `.trackcontrolpoints`)

Descriptors: ACECM `protos\AIData.proto.txt` (AISplineData, AISplinePayloadData, TrackControlPoints), `Math.proto.txt` (InterpolatingSplineData). Read off the 34 stock ideal / pit lines of 17 tracks; the module header of `src/track/ai.rs` lists every field. In short:

- f1 spline: points {f1 position, f2 cumulative length, f3 index}, f2 length (double), f3 closed, f5 mode (5 on stock ideal lines, 2 = Catmull-Rom on all pit lanes and cota club's ideal line; we write 2), f6 a nearest-point grid (10 m cells over the points' box widened by 350 m, rows along x, each cell the 10 nearest points to its centre in xz, nearest first; verified cell by cell on brands_hatch; optional: cota club has none).
- f2 one payload per point. The game does not take speeds from here (13.8 everywhere, gas 0.78); it reads the geometry: radius and turn direction, sides (left = up x forward, verified against monza's road edges), sides with kerbs (at least the 1.055 m half car width), walls (capped 80), camber, grade, normal, forward, distance from / to the nearest corner (absent inside corners), is_pitlane on the pit lane's limiter stretch.
- f3 -1, f5 the layout container path without `.scene`, f6 recorded half car width 1.055 (-1 on pit lanes), f7 the control points (also written as `.trackcontrolpoints`), f8 SceneTrackInfo.
- **Measured** (stockstats): ideal lines 1.67..1.80 m per point (nordschleife 5.8), closed on every circuit; pit lanes 700..1630 m at 2.16..2.26 m per point, starting and ending 0..6 m from the ideal line; 9..16 control points per km.
- `.carcategory_<c>.reference` (reference laps) exist on 4 of 17 stock tracks only; the game simulates a reference from the ideal line otherwise (its log says "can't create simulated reference, there's no ideal line" when the line is missing). We write none.
- Code: `src/track/ai.rs` (`build`, `trace_centre`, `control_points`, `encode`, `parse`), `src/track/gameplay.rs`.

### Pit lane zones (`containers\pitlane_zones_<L>.scene`)

Zone entities (ZoneData instances, `protos\Scene.proto.txt`): `pitlane_entry` f101 Pitlane {type Entry = 0}, `pitlane_main` f101 {f1 3 (Main)} + f111 SpeedLimit {f1 "pitlane"} (the limiter), `pitlane_exit` f101 {f1 1}, `safety_car_line_2` f114 {f1 2}; 4..31 corners each; the Container entity declares `Zone`, `pitlane`, `safety_car_line`, `speed_limit`; the rows list the container in Race, Time Attack and Hotstint. No stock track uses Pitbox zones; stock pit stops use the Start Pos slots (monza: type-4 slots 8 m off the pit spline, type-5 boxes 18 m off it, turned to the garage). Code: `src/track/pits.rs`.

### TV cameras (`containers\tv{1,2}_cameras_<L>.scene`, `cameras\*.curve`)

Camera entities (kinds `Camera`, `CameraTV`, `TV_set1` / `TV_set2`), f50.f102 CameraData {f12 {}, f14 {f21 graphics override}, f15 CamTVData {f1 / f2 start / end segment {position on the track, width, height, angle = track heading folded into -90..90}, f3 1, f6 FOV curve, f9 1, f10 settings}, f16 {f3 1, f4 {}}}; 1,155 of 1,178 stock cameras use 30 x 10 start and 10 x 5 end segments, and a camera's end is the next one's start. A `.curve` is keys {f1 x, f3 / f4 tangents, f10 y}; for TV cameras x = distance to the car (m), y = FOV (degrees). Listed only in the No Game Mode row. Code: `src/track/tvcams.rs`.

### DRS zones (`containers\drs_zones_<L>.scene`)

One Zone entity, one polygon (4..11 corners) per DRS zone, instance f118 DRSZone {} (no detection point in the descriptor); listed in Race, Time Attack and Hotstint. Stock adds `common_assets\meshes\props\track_dist\dist_drs.mesh` signs and a decal (we place none). Code: `src/track/drs.rs`.

### Audio zones (`containers\fmod_<id>_track.scene`)

"Zone Sound" entities, f50.f125 ZoneSoundData (f1 type 0 ambient / 1 reverb, f6 bank, f7 event or snapshot, f10 area 0 sphere / 1 polygon, f11 radius, f12 scale, f14 max volume, f15 polygon, f22 only focused car, f24 fading); one `event:/environment/track_ambience` sphere (1250..10000 m) per track in `content\sfx\environment.bank`, `snapshot:/snap_reverb` / `snapshot:/snap_underpass` polygons in `content\sfx\master_bank.bank`. Stock tracks also place crowd, announcer, swoosh and pit-lane alarm zones (18..79 zones in all); we write the ambience and the AC1 reverbs only. The container is loaded with the scene and listed in no row. Code: `src/track/audio.rs`.

### ISMesh limits (validator)

**Measured**: the largest stock ISMesh node holds 1,130,335 instances (cota, `grass_hi`); the largest stock track holds 4,432,375 in all (nurburgring). `src/validate/limits.rs` uses these: more per node is an error, more per track a warning. Stock content is known to load; past it is untested.

### Car space and the frame API (`car::frame`)

**Measured** on stock cars (WHEEL_ bone worlds of the exterior mesh): EVO car space is right-handed, Y up, metres; +X is the car's left (WHEEL_LF at +x), +Z forward; the origin is on the ground (wheel centre y = tyre radius, a few mm less on stock) and halfway between the axles (front / rear WHEEL_ z = +-wheelbase/2 within 3.5 cm on mx5_na, ae86_sprinter_trueno, 992_gt3_rs, 935). Kunos's AC1 kn5s are nearly in that space already: AC1 ks_mazda_miata's WHEEL_LF node is at the stock mx5_na WHEEL_LF bone, (0.705, 0.285, 1.168).

The datum (ours): EVO = kn5 world + datum, one translation per car. x and z centre the four kn5 `WHEEL_xx` nodes (axle midpoint at z = 0), y puts them at the mean tyres.ini radius (default compound). Missing wheel nodes are taken from the AC1 physics positions. AC1's own model placement is CoG frame = kn5 + car.ini `GRAPHICS_OFFSET`; in that frame the wheels are at (+-TRACK/2, BASEY, WHEELBASE x (1 - CG_LOCATION)) front and z = -WHEELBASE x CG_LOCATION rear (checked on ks_mazda_miata).

`Frame::build(&ac1::car::Car, &Kn5) -> Frame`, used by every car stage (geometry, wheels, animated parts; physics and car functions):

| member | meaning |
|---|---|
| `datum: [f64; 3]` | added to an AC1 kn5 world position to land in EVO car space |
| `wheel_centres["LF"/"RF"/"LR"/"RR"]` | wheel centres in EVO space (kn5 WHEEL_ nodes + datum) |
| `wheel_frames[c]: M4` | the WHEEL_ node world matrix, datum applied |
| `node_world[name]: M4` | every kn5 node's world matrix in AC1 space (no datum; first node of a name wins) |
| `physics_wheels[c]` | suspensions.ini wheel centres in EVO space (through GRAPHICS_OFFSET) |
| `cog` | the AC1 physics CoG in EVO space |
| `tyres: [Tyre; 2]` | front / rear tyres.ini radius, width, rim radius (default compound) |
| `graphics_offset`, `notes` | car.ini GRAPHICS_OFFSET; disagreements found (kn5 vs suspensions.ini) |
| `evo(p)`, `evo_m(&m)`, `node_evo(name)` | AC1 -> EVO for points / matrices / named nodes |
| `to_wheel(c, p)`, `wheel_unrotate(c)` | EVO point -> wheel frame (relative to the centre, car axes, small WHEEL node camber / toe removed) |
| `wheelbase()`, `track_width(front)`, `tyre(c)` | from the wheel centres / tyres.ini |

`M4` is row-major with row vectors (translation in 12..14), like AC1 and the `.mesh` bone matrices; `frame::{mul, inv, xform, xdir, translation, corner_of}` work on it. `aceconv car-frame <ac1 car dir>` prints the frame as json.

### Car mod layout written by `aceconv car` (P1)

Read off the donor (the SDK sample layout) and stock cars, with the game's descriptors (ACECM `protos`: `ActorData.car` = `CarActorData`, `Customization.proto`, `Mesh.proto`):

- **Meshes.** Stock exterior / interior meshes have 5 LODs (lodIn 0/10/20/50/100 m outside, 0/5/10/20/50 inside); rims, rim blur and tyres have 2 (0 / 20 m) and no bones (no f9 / f10 streams). Vertex streams: f5 positions, f6 normals, f7 UV0, f8 tangents (xyz + bitangent sign), f9 / f10 bone weights / indices (4 f32 each), f13 colours and f14..f16 UV1..3 (zero on stock cars). Positions are in car space (bind pose = the bones' rest worlds). Front faces: cross(b - a, c - a) points along the vertex normals (checked on every stock mesh looked at). Stock materials never disable culling (`cullMode` 0 everywhere): AC1 double-sided pairs are kept and each copy moved 0.4 mm along its own normal (ours).
- **Skeletons.** Exterior root CAR_BODY; per corner HUB_xx with DISC / WHEEL / SUSP / TYRE_xx children, all at the wheel centre with car axes (stock 992 GT3 Cup); DOOR_L / R, MIRROR_1 / 2, FRONT_BUMPER, REAR_BUMPER, HOOD (actor damage parts), the wiper chain. Interior root COCKPIT_HR, STEER_BASE, MIRROR_0, SEATBELT_ON / OFF. Animated AC1 nodes add `AC1_<node>` / `AC1_<node>_I` bones.
- **Actor base meshes** (`CarActorData.base_meshes`, f103.f12): f1 body mesh, f2..f5 tyres at TYRE_xx, f6..f9 rims at WHEEL_xx, f10 interior, f18..f21 rim blur, f23 / f24 belts, f27 rear plate.
- **Tyres.** `.tyremesh` (TyreMeshData): f2 width mm, f3 sidewall ratio, f4 rim inch, f5 / f10 shadow decal x / z size, f6 shadow y (-radius), f7 mesh. Stock tyre UVs: u = angle / 360 deg about the axle, v = arc length along the profile normalised 0..1 from the +x bead to the -x bead (80 segments, 19 profile points on the stock 215/35 R18).
- **Liveries.** One `.design` (customisable slots: slot = section name, material replacement, multilayer channel mode OemOnly = 1 / OemAndPickers = 2 / Inherit = 3), `.cardesign`, `.visualcarpreset` (f8 material states: f1 material, f2..f4 channel states {f1 colour, f10 OEM colour, f20 emissive}) and `.carfinalstate` (f10 body, f20 rims, f52 livery linker) per livery; the mechanical preset's f8 lists the visual presets. Menu thumbnails: `generated\thumbnails\<id>-<mech preset>-<visual preset>.texture`, 1024 x 1024 BC7.
- **Final-state ids.** Each stock `.carfinalstate` carries one number N unique across the game, written as the GUID `00000000-0000-0000-0000-<N hex>`, PGUID {b: N} per part and the livery linker {b: N}; ours are 0x7000_0000_0000 | hash (src/car/identity.rs).
- **Localisation keys** (stock en.cars.loc against stock files): presets `<car id>_<slug of the preset description>`, design slots `<car id>_<slot>`, designs and OEM colours `<slug of the description>`.
- **Car UI resources** (game facts: the executable's coui handler and file-system setup, static analysis of AssettoCorsaEVO.exe 0.9.1; in-game log of 2026-10-19). The game mounts `content.kspkg`, then every `Saved Games\ACE\mods\*.kspkg`, then `Saved Games\ACE\mods\` as a loose root, all into ONE file system addressed by archive-root paths (an earlier mount wins; loose files only fill absences). The Gameface `coui://` handler serves `coui://uiresources/<path>` by looking up `uiresources\<path>` in that file system (`.texture` through the texture manager, tried as `uiresources\<path>` and `<path>`; other files read directly), so a mod package serves UI files at their ROOT path. It does NOT map `content\cars\<id>\uiresources\...`: with the brand there, the log shows `Failed loading resource: coui://uiresources/branding/oem/pagani/badge.texture`. Stock ships every car's UI files at the root of content.kspkg (no stock car has a car-local `uiresources`). What a converted car carries:
  - `uiresources\branding\oem\<key>\{badge,logo}.texture` + `.texturemips` (non-stock brands only; a stock key is always served from content.kspkg);
  - `uiresources\localization\cars\<id>\en.loc` (car description) and `uiresources\localization\cars\<id>\<mech preset>.en.loc` (preset description), both requested when the car is selected. Stock format: UTF-8 text, CRLF, opening "\r\n\r\n", paragraphs split by a blank line, the UI's markdown (`**_Name_**`, `- **Label:** value`). Ours: the AC1 ui_car.json description (html `<br>` -> line breaks, tags dropped) and, for the preset, the bold name with the AC1 spec lines;
  - `content\cars\<id>\uiresources\localization\<lang>.cars.loc` and `setup.<lang>.loc` stay car-local: the exe lists `content\cars\*` (stock and mods) and merges each car's `uiresources\localization\<lang>.cars.loc` and `setup.<lang>.loc` into the UI strings (brand label `brand_<key>`, preset / livery names).
  - Installer (ACECM): nothing beyond copying `<id>.kspkg` + `<id>.json` into `Saved Games\ACE\mods\`; no loose UI files. Two mods of one non-stock brand both carry the brand's root files; whichever package mounts first serves the brand.
- **AC1 detail maps.** AC1 multimap shaders show `diffuse x lerp(detail(uv x detailUVMultiplier), 1, diffuse alpha)`: Kunos street cars keep the paint colour in each skin's `metal_detail.dds` (ks_mazda_miata: Skin_00 is grey with alpha 0); carbon, leather and alcantara are TILED details (ks_pagani_huayra_bc: multipliers 50..400). EVO's layer blend (UberVehicle / UberVehicleInterior, stock: 288 materials use `RedOverrideMultiply_BaseColor`, 110 Blue) is `lerp(base, layer x base, w)` per layer, the layer's colour map sampled at `<Layer>_UVscale`; a layer attribute with neither `OverrideReplace_` nor `OverrideMultiply_` set is left as the base has it. Ours (`materials::tiled_detail`, `detail_layer`, `apply_detail`): a multimap detail with multiplier >= 1 goes to the **Blue** layer (the converter's designs customise only the primary channel, which drives Red; the stock Caterham decals keep a coloured Blue layer under an empty tertiary state), colour multiply, `Blue_UVscale` = the multiplier, the detail normal (`txNormalDetail`, `detailNormalBlend`) combined on the same layer, weight `Splatmap_Constant` (0, 0, 1) when the diffuse alpha is 0 everywhere, else a `SpatMap` of 1 - alpha; the base map is the diffuse alone, one per diffuse. A textured detail (colour variance > 0.0002) becomes the layer's map (`<detail>_detail.texture`, BC1 sRGB; on paint d^0.75, its share of the paint tone curve); a flat one the layer's constant colour; a flat detail on paint (the livery colour) and a multiplier under 1 stay baked into the base map. Before this every detail was baked, and a tiled one only as its MEAN (the weave / grain vanished), with one full copy of the base map per detail variant (the Pagani: six 2048 EXT_Skin copies per skin), which then hit the texture budget. Texture budget (ours): the longest side drops to 1024 past 560 MB of mips and 512 past 700 MB (stock cars 132..553 MB); it was 200 / 320 MB.

### Car physics (`car::physics`, P2)

Read off the game's descriptors (ACECM `protos`: `CarData`, `TyresData`, `CarSetup`, `CarSetupLimits`, `AiCarData`, `Customization`) and the 71 stock cars; tools in the clean work folder decode them with the descriptors.

- **Files.** CarData (`data\cardata.car`; the game's `system\cars.table` names each car's CarData) holds inline copies of the engine (f4, file f20 `.carengine`), drivetrain (f3 / f22), gearbox (f23 / f24), clutch (f25 / f26), brakes (f5 / f27), coilovers and suspension geometries (f2.f8 / f9 / f40 / f41, files f2.f20 .. f23) and wings (f12.f3, files f12.f10). On stock, 40 of 71 engines and 65 of 71 front geometries equal their file (the rest are stale copies). The converter edits the inline copy and writes every file from it; `check-car` errors when they differ. Electronics are inline only on 67 stock cars.
- **Curves** (`.curve`): CurveData {1 point {1 x, 3 {}, 4 {}, 10 y}}; y sits in field 10.
- **Same meaning as AC1** (stock twins): springs, dampers, ARBs, track, wheelbase, BASEY (`base_y_*`), CG_LOCATION, gears, final, diff power / coast / preload, fuel, tank position, pickup heights, colliders, controls: ks_toyota_ae86 -> ks_toyota_ae86_sprinter_trueno equal; double-wishbone hard points (relative to the wheel centre, x inboard) equal on ks_mazda_miata -> ks_mazda_mx5_na; `powerCurve` is power.lut (N m); `limiterCycles` = LIMITER_HZ; `nominal_rpm` = [DAMAGE] RPM_THRESHOLD; brakes `frontBias` = FRONT_SHARE x 100, `totalTorque` = MAX_TORQUE; aero wings: CHORD, SPAN, POSITION, LUT_AOA_CL / CD, CL_GAIN / CD_GAIN, the four ZONE_* damage values (AE86, supra, 190E give identical CL.A / CD.A on both sides); turbos keep MAX_BOOST, WASTEGATE, LAG_*, REFERENCE_RPM, GAMMA; bump stops: Force at Reference 0.01 m = BUMP_STOP_RATE x 0.01 (mx5 35000 -> 350). BumpStopData {1 Range, 2 Reference, 3 Force, 4 Gamma, 5 length, 6 damping}: over the 142 stock coilovers (`examples/bumpstops.rs`, `tools/bumpstops_stock.tsv`) damping is on 135 bump / 132 droop stops, median 400 / 500 (the mx5_na has none); the converter writes it only where the template's stop has it, at that median.
- **Drivetrain** (DrivetrainData, CarData f3). TractionType 0 RWD, 1 FWD, 2 FourWD, 3 AWD, 4 AWDF. Every stock car's enum matches the real car: FWD Abarth 695, Alfa Junior, Alpine A290, Mini, R5 GT Turbo, Golf GTI mk1 / mk8, i30 N; FourWD Sport quattro, Delta Integrale, Escort Cosworth, 205 T16, RS 6 (all with an EpicyclicTorsen / EpicyclicLSD centre and `front_share` 0.2 .. 0.5); AWD R8 V10 Performance and M8 (rear-driven, `awd_clutches` at positions 0 / 1); AWDF RS 3 and Golf R (front-driven, clutches 2 / 3); every other car RWD (incl. 964 Turbo, Huracan STO / ST EVO2). The game's drivetrain set-up (exe 0x1422e23d0) builds per type: RWD rear / left / right lock controllers (f5 / f50 / f51); FWD front (f8) + left / right; FourWD rear (f5), front-share (f6), centre (f7) and front (f8); AWD rear only; AWDF front + left / right. Its clutch set-up (0x1422e3e00) puts the four clutch slots on the wheels LF, RF, LR, RR and frees (lets slip) 0 / 1 under AWD, 2 / 3 under AWDF; each `AWDVectoringClutch` is {1 position, 2 preload, 3 controllers}. DifferentialData: 1 power, 2 coast (0..1), 3 preload (N m), 4 type (LSD 0, Spool 1, Torsen 2, EpicyclicTorsen 3, EpicyclicLSD 4, TorqueVectoring 5), 13 front_share (0..1). The setup's differential equals `differential_data` on 2WD stock cars and the rear diff on the R8 V10 (AWD). DrivetrainData also carries stale gearbox-like fields 4, 9 .. 38 that are not in the descriptor (their gear lists differ from f23 on 42 stock cars; the game reads f23). AC1 -> EVO (`physics::drivetrain`): RWD / FWD as is; AWD -> FourWD, EpicyclicLSD centre with CENTRE_DIFF_* and `front_share` = FRONT_SHARE / 100 (always percent in AC1: Gallardo 1, A1 quattro 98); AWD2 (AC1's rear-driven electronic centre clutch, `ctrl_awd2.ini` output = lock torque N m) -> AWD with front clutches 0 / 1 each carrying the controller at half torque (OURS: two wheel clutches share AC1's one centre clutch), or a preload of min(CENTRE_RAMP_TORQUE, CENTRE_MAX_TORQUE) / 2 without a controller file (OURS); `ctrl_single_lock.ini` -> the driven axle's lock controller (rear f5, FWD front f8); an AWD car's `ctrl_awd_center_lock.ini` (Kunos Gallardo, A1 quattro, RUF RT12 R AWD) -> the FourWD centre lock controller (f7). AC1 locks above 1 (mods writing percent) are clamped to 1 (a full lock, as AC1 drives them). For the four-wheel types `differential_data` and the setup diff repeat the rear diff. `check-car` checks the enum against the clutches / centre and against `aceconv_ac1_traction` in `<id>.json`.
- **Enums.** TractionType RWD 0 / FWD 1 / FourWD 2 / AWD 3 / AWDF 4 (above); DifferentialType LSD 0 / Spool 1; controller inputs (DynamicControllerStageData.Input) 18 RPMS, 2 Gas, 6 Speed, 7 Gear, 17 Const; an EVO turbo controller is the boost target (stock turbos with one may omit max_boost). Tyre compounds of mods: `TyreCompound_Mod_1..10` (5001..5010).
- **Setup.** `.carsetup` values share CarData's units (`wheel_rate` = springRate); `aero.front/rear_target_height` (mm) is the ride height the game solves the collars from (stock setups' collars differ from CarData's); limits are SetupSettingLimits {1 step, 2 min, 3 max, 4 is_modifiable}. Game-wide envelope of the 81 stock `.carsetuplimits` (`examples/setupenv.rs`, `tools/setupenv_stock.tsv`; min / max over the limits in use = modifiable ranges and fixed values): front bias 48 .. 90 (modifiable ranges 48 .. 90, fixed 72 .. 85), steer ratio -18 .. 20 (modifiable 8 .. 18.3, step 0.5), wheel rate 0 .. 400000, dampers 0 .. 26500, pressure 13 .. 45 psi, camber -6 .. 1.5, toe -0.5 .. 0.5, brake power 75 .. 110 %, target heights 30 .. 190 mm, fuel 0 .. 150 l, diff 0 .. 1, preload 0 .. 300 N m, turbo 0 .. 100 (step 10). Unmodifiable ranges (min < max with is_modifiable unset) are placeholders (front bias 0 .. 100, diff 0 .. 90) and are left out.
- **AI hints** (`.aicardata` AiCarData: 1 gears {1 upshift, 2 downshift, 5 redline}, 5 physics_hints {1 aero_hint, 4 / 5 / 6 / 7 low / high speed lateral plateau start / end {1 g, 2 tyre slip}, 8 top speed km/h, 9 its drivetrain hp, 10 forward g, 11 braking g, 12 acceleration table}): `aero_hint` = high- over low-speed plateau end (stock median 7.1 / 5.1 / aero_hint 0.999, p10 .. p90 0.992 .. 1.003); plateau start / end median 0.9936 (low) / 0.9954 (high); the acceleration table is a full-throttle run sampled every 10 km/h and at shifts, gear index R = 0, N = 1, 1st = 2; `acceleration_G_forward` matches the run's acceleration at 120 km/h. Auto shift (`tools/aifit.py`, the 69 stock cars with a redline): GearBoxData.autoshifter.up / engine limiter median 0.9538 (p10 .. p90 0.898 .. 0.976), AiGears upshift / redline 0.9835, downshift / redline 0.5716; no stock gearbox has autoshifter 5 (down threshold).
- **Performance indicator.** Not in the content (the game gets stock PIs elsewhere); mods give it in `<id>.json`.

OURS (with the stock evidence, measured by the clean tools):

| decision | rule | evidence |
|---|---|---|
| tyre grip | template = the EVO class's stock tyre closest in size; Dy0 / Dx0 solved so mu(F_static) = 0.98 x AC1's (mu = D (Fz / Fz0)^(ls - 1)) | 26 twin axles: EVO / AC1 lateral mu at 3 kN median 0.978, mostly +-5 % |
| tyre class | by target mu: < 1.12 eco, < 1.32 road, < 1.40 supercar, else hypercar; AC1 names with "slick" (not semi) or >= 1.45 racing slicks; >= 1.70 F1 | stock class mu bands at 3 kN: eco 1.02 .. 1.14, road 1.17 .. 1.34, supercar 1.26 .. 1.33, hypercar 1.37 .. 1.48, slicks 1.46 .. 1.53 |
| ride height | collar = sag - x, x = (drawn wheel y - CoG y) - BASEY - tyre static deflection; setup target height = (CoG height + pickup height) of the drawn model | stock: body offset from design follows sag - collar within +-3 cm |
| travel | bump / droop at rest from AC1 (BUMPSTOP_UP - x_ac1, BUMPSTOP_DN + x_ac1, x_ac1 = sag - ROD_LENGTH), capped 0.09 / 0.05 m, at least abs(x) + 1.5 cm | stock Range p90 0.09 (bump), p50 0.045 (droop); evoview finds AC1's far droop stops hang the wheel into the arch |
| coast curve | -TORQUE x ((1 - n) r / RPM + n (r / RPM)^2) from AC1 [COAST_REF], 0 .. 1.2 x limiter | shape of stock coast curves (negative, rising with rpm) |
| AI hints | fitted by `tools/aifit.py` on `tools/aifit_stock.tsv` (`examples/aifit.rs`: the `stats::derive` model of each stock CarData + the shipped hints), forms picked from the scatter (simplest first; a more complex form only when it cuts the median abs residual or the RMS by 5 % without raising the other), clamps = stock min / max -+ 10 % of the span. aero_hint = 0.96618 + 953.25 s, s = CL.A rho / (2 m g) (s^2/m^2: "x at 30.9 m/s"; a load-sensitive a (1 + s V^2)^n_y fits no better), fitted on the 50 cars without CZ aero maps; low-speed plateau = -0.53245 + 1.25233 mu_y (beats k mu_y and c mu_y^p); braking = min(0.89453 mu_x, 1.53239 T / (r m g)) (median regression; beats k mu_x, k T / (r m g), c mu_x^p (T / (r m g))^q); slips: 1.768 (mu_y <= 1.2308) / 1.133 at the low-speed end, start / high-start / high-end ratios 0.871 / 0.764 / 0.917 (stock medians) | residuals (stock - fit), median / median abs / p10 .. p90: aero_hint -0.004 / 0.026 / -0.054 .. +0.056 (50 cars; the 21 aero-map cars sit +0.10 above); plateau +0.000 / 0.039 / -0.091 .. +0.084 g (71); braking +0.000 / 0.122 / -0.181 .. +0.259 g (71); `check.rs` top speed vs the shipped one -1.9 % median, p10 .. p90 -8.4 .. +16.0 % (50 cars) |
| PI | average km/h over a fixed 4 km synthetic circuit / 10 | scale ours; stock PIs to be read in game |

### Car functions (P3)

Read off the stock cars (`.actor` = `ActorData`, `car` f103 = `CarActorData`; ACECM `protos`: `Scene`, `CarInstrumentationData`, `LogicScene`, `Customization`) and the AC1 install. Code: `src/car/{lights, leds, display, cameras, exhaust, needles, belts, driver, fx}.rs`.

- **Lamps.** VehicleLight (reflectors) / VehicleGlass (lenses) carry `FunctionMask1` (front R high, G low, B daylight; rear R rain-fog, G brake, B tail) and `FunctionMask2` (B indicator, G reverse, R special), RGBA8 sRGB (format 6). Front / rear and left / right are the lamp vertices' colour stream f13 in 1/255 steps: R left, G right, B rear, A 1 (**measured** on mx5_na, ae86_sprinter_trueno, 992_gt3_rs). `.carlightingsystem`: f1 one stage per switch position {f1 sets {f1 category, f3 entries {f1 function, f3 reflector, f20 glass, f14 emitter intensity, f27 blinker, f28 pop-up}}}, f20 the shared stage (brake 4, reverse 9, left 7, right 8, hazard 10, rain 5), f23 blinkers, f7 intensity filters. Actor f2 physical lights {f1 name, f2 functions, f3 transform, f6 location 0..5}, f25 lamp batches (section names).
- **AC1 lamps.** `lights.ini` LIGHT_n = with the headlights (front low beam, rear tail), BRAKE_n = brake (+ tail with OFF_COLOR); without a declared high beam the low beams carry it (game evidence); CSP `[EMISSIVE_<TYPE>_n]` NAME / MESHES and `[CustomEmissive]` `@ = TurningLights* / ReverseLights / BrakeLights` add indicators etc. (car `extension\ext_config.ini`, else the install's `extension\config\cars\loaded\<car>.ini`). Cockpit / dash entries are backlights: their own interior section, emissive map = colour map. Light cones (FRONT_LIGHT_BEAM) are left out. A LIGHT mesh's name changes its function only through whole-word keywords measured on the AC1 install (`examples/lampcorpus.rs` -> `data/lamp_tokens.tsv`: 4,448 lamp meshes of 393 cars labelled by their CSP / lights.ini declaration, else colour and kn5 position; a token is a keyword when >= 90 % of its labelled lamps share one function on >= 5 cars; undeclared headlights count against every function but high / low beam): 38 tokens, e.g. turn / blinker / hazard (indicator), reverse / reversinglight, highbeam / hb, lowbeam; a low-beam word beats a high-beam word (`anabbaglianti` is never a high beam). No CSP kind declares daylight lamps and only 4 lamps are declared fog, so no name makes a DRL or a fog lamp. Ours: one section per function set, a uniform 128 x 128 mask per section; mx5_na's intensities and colours.
- **LEDs.** VehicleLed + `LedMask` (RGBA8 linear, format 5): red 10 x (k - 1) of 255 picks Led_k, 255 none (common_assets `led_1`, mx5_na `int_decals_f`). `.carledsystem`: groups of instances {f1 led, f2 physics value, f6 / f8 on / off time, f7 value, f20 / f21 colours}; rpm f1, engine limiter f3, fuel f4, light stages f19; f33 varint = limiter; every stock file ends with f35 intensity filters and a fixed32 f33 (2.0), kept. AC1 `digital_instruments.ini` LED_n / RPM_SERIE_n / FUEL_WARNING_n; no AC1 LED = every group empty.
- **Display.** Actor f11 {f1 html, f2 material, f4 / f5 size, f9 display enum, f10 `.cardisplaysystem`}; the page is Gameface HTML on the display material over the section UVs (u right, v down: the sample's INT_DISPLAY). Bindings `data-bind-value="{{ModelCurrentCar.display_speed_kmh / gear / rpm / ...}}"`, the shared `common_assets\displays` scripts, CSS and fonts. AC1 `[ITEM_n]` PARENT dummy / POSITION / SIZE: the dummy's XY plane faces the car's front, so the page's u runs along the dummy's -x. Ours: one page per dummy, a quad per cluster of items 1.5 mm in front of the AC1 screen.
- **Cameras.** Actor f10 drivable cameras {f1 transform, f2 {f2 name, f14 {f2 fov, f13 mode 1 cockpit / 2 external / 3 fixed / 4 dash / 5 bonnet}}}, f1 onboard cameras, f17 cockpit probe, f32 showroom, f24 rear camera, f23 mirrors. A camera looks along its -Z; (pitch, 180, 0) is the car's forward. AC1 car.ini DRIVEREYES / BONNET_CAMERA_POS / BUMPER_CAMERA_POS, `dash_cam.ini` POS and `cameras.ini` are in the kn5 frame (+ datum).
- **Mirrors.** Each actor mirror (f23 {f1 type 0 centre / 1 left / 2 right, f2 batch, f3 bone, f8 fov, ...}) renders into a target of its own (`mirror_texture` + index, AssettoCorsaEVO.exe RVA 0x1de1c54) shown on the batch of that name through **texcoords1**: on all 202 stock MIRROR_0 / 1 / 2 batches texcoords1 is an affine stretch of texcoords0 into a per-mirror box (medians: side u 0.11..0.89, centre 0.03..0.97, v 0.01..0.99), u rising with +X, v with -Y. AC1 mirrors share one target, each on part of it (ks_mazda_miata: left mirror u 0.50..0.86), so the side mirrors' u is stretched; an axis that already spans 0.6 of 0..1 is kept. MIRROR_n bones: at the glass centre, identity rotation (stock: children of DOOR_L / DOOR_R).
- **Brake discs.** EXT_DISC rides the DISC_xx bones (children of HUB_xx, at the wheel centre; 70 of 71 stock cars entirely), UVs inside 0..1 (71 of 71). AC1 rotors are root `DISC_xx` nodes beside `WHEEL_xx`. VehicleDisc blends each map to its `*Blur` twin by saturate(Velocity x BlurBias) where `Base_Has*Blur` is set (48 of 71 stock discs set the colour one; AC1 `txBlur`); `Base_EmissiveMap` is a white glow mask (common `Disc_Glow_Mask`) the shader tints with the brake temperature colour (ColorFront / ColorRear); AC1's `txGlow` is coloured, so ours is its brightest channel, normalised.
- **Saved per-car settings.** `Saved Games\ACE\CarCameras.carcamerausersettings` (CarCameraUserSettings) keeps per car id the drivable cameras' positions / rotations; the loader (RVA 0x8a4ba0) creates the entry from the actor the first time an id loads and from then on copies it over the actor's cameras (0x8a4e27..0x8a4f08). A mod reinstalled under the same id keeps its first cameras until the entry goes (`aceconv reset-car-settings <id>`, game closed). `CarMirrors.carmirrorsusersettings` keeps per-id mirror adjustments the same way.
- **Exhaust.** Actor f40 emitters {f1 name, f2 bone, f3 transform, f7 kind}: exhaust kinds 2..4 = backfire type + 1 (`common_assets\emitters\backfire_type_<n>`), road 2, race 3, a few exotics 4 (**measured**, 72 actors). `.carexhaustbackfire` f1..f3 rpm at ~0.83 / 0.62 / 0.45 x limiter (mx5_na, f2004), f10..f14 audio, f20..f24 visual probabilities. AC1 `flames.ini` FLAME_n POSITION is in the kn5 frame (f2004: (0.218, 0.46, -1.23) vs stock (0.22, 0.46, -1.242)); Kunos cars without flames mark outlets with `EXHAUST*` dummies.
- **Needles.** `.caranalogicsystem`: per channel field (rpm 1, speed_kmh 3, fuel 4, oil pressure 7, turbo 8, water 9, oil temp 10, place holder 12) instances {f1 bone, f2 axis (Z = 2), f3 LUT {f1 value, f2 degrees}, f7 / f8 place-holder values, f30 / f31 filters}. The AC1 needle node frame is the stock bone frame and the AC1 degrees are the stock LUT (ks_mazda_miata ARROW_RPM vs mx5_na; `analog_speed_curve.lut`).
- **Ground shadow.** Actor f28 {f1 x, f2 z size, f5 z position, f4 texture}: x 2.12 on every stock car, z 1.02..1.07 x body length (median 1.04, 10 cars), centred on the body.
- **Belts.** Actor base meshes f12 `seatbelt_on` / f22 `seatbelt_off` (mx5_na `belt_auto` / `belt_off`, one LOD); AC1 `CINTURE_ON` / `CINTURE_OFF` subtrees go there, out of the interior mesh.
- **Driver.** Actor f31 layers (109 position, 101 steer, 102 / 103 / 104 pedals, 105 / 106 shift, 107 approach, 110 handbrake, 112 / 113 paddles, 115 idle) of `.animation` clips whose tracks are rig bone LOCALS in car space (rig COG > A_Hips > ...; parents from the stock character meshes); each slot also has a legacy mirror at slot - 100 ({f1 path}). Stock steer clips: 201 frames, one full turn each way (frame = middle + middle x angle / 360). Stock brake and clutch layers both drive the left leg, and stock leaves the clutch slot empty. AC1 `driver_base_pos.knh`: `i32 name length, name, 16 f32 local, i32 children`, depth first; `DRIVER:RIG_*` joints in the kn5 frame. Ours: the stock driver retargeted onto the AC1 joints (pelvis, torso, two-bone IK for arms and legs), steer from `steer.ksanim` over -LOCK..+LOCK as a full turn.

### Ids

**Measured**: 71 stock cars, every id `ks_` + `[a-z0-9_]`, 13..37 characters; 19 stock tracks, `[a-z0-9_]`, 3..14 characters, no prefix.

Converted ids (ours, `src/ids.rs`), a pure function of the AC1 folder name:
- car: `ks_` + slug (at most 20 characters) + `_` + at least 5 base36 characters, 13..29 characters;
- track: slug (at most 9 characters, starting with a letter) + `_` + 4 base36 characters, at most 14 characters;
- slug = the lower-cased folder name, every run of other characters turned into one `_` (a leading `ks_` is dropped for cars and added back); the suffix is FNV-1a-64 of the lower-cased folder name in base 36; on a clash with a stock id the suffix grows by one character.
- No fixed length is needed: the donor builder already renames the 21-character stock id to the 13-character donor id through protobuf (length prefixes re-encoded), and the game loads the result.
- A converted car's `<id>.json` sidecar carries `aceconv_source` (the AC1 folder); `aceconv legacy-id` finds earlier conversions by it, or by the display name matching the AC1 `ui_car.json` / `ui_track.json` name.

## FMOD

### FSB5 (sample banks inside FMOD banks)

- Header 0x3C bytes: `FSB5`, version 1, sample count, sample-header bytes, name-table bytes, data bytes, codec mode (2 PCM16, 0x0F Vorbis, 0x10 FADPCM), then 0x1C..0x3C kept as is.
- Per sample a u64: bit 0 = chunks follow, bits 1..4 rate index (1 8000, 2 11000, 3 11025, 4 16000, 5 22050, 6 24000, 7 32000, 8 44100, 9 48000, 10 96000), bit 5 = stereo, bits 6..33 data offset / 16, bits 34..63 frame count. Chunks: u32 {bit 0 more, bits 1..24 size, bits 25..31 type}; type 11 = the Vorbis setup header's CRC32.
- Vorbis packets: `u16 length + bytes`; a zero length is padding to the end.
- FADPCM: 0x8C-byte frames per channel of 256 samples (see `src/sound/fadpcm.rs`).
- Code: `src/sound/fsb5.rs`, writer `src/sound/bankwrite/fsb.rs`.

### FMOD Vorbis setup table (inside fmod.dll)

FSB5 Vorbis samples keep neither the identification nor the setup header. The identification header is rebuilt per the Vorbis I spec (section 4.2.2): channels, rate, blocksizes 256 / 2048. The setup header comes from a table inside the game's own `fmod.dll` (FMOD 2.03), read at run time; nothing of FMOD's is shipped:

- an array of 32-byte entries: {u64 pointer, u32 length, u32 crc32} then {u64 base pointer, u32 offset, u32 count};
- with a null base, the header is the `length` bytes at `pointer`; otherwise it is the base header's first `length` bytes with `count` bytes at `offset` replaced by the bytes at `pointer` (most setups differ from another one in a few bytes);
- an entry is accepted when the rebuilt header starts with `\x05vorbis` and its CRC32 (zlib's) equals the stored value.

**Measured** on the game's fmod.dll: 161 entries, all CRC-verified, 32 stored whole and 129 as patches of another. Code: `src/sound/vorbis.rs`.

### FMOD Studio projects and banks

- Project metadata (`metadata\**\*.xml`): `<object class=".." id="{guid}">` with `<property name=".."><value>..</value></property>` and `<relationship name=".."><destination>{guid}</destination></relationship>`; parser `src/sound/bankwrite/studioxml.rs`.
- The compiled bank (RIFF `FEV `, FMT 0x92) is described in [FMOD_BANK.md](FMOD_BANK.md); writer `src/sound/bankwrite`, reader `src/sound/bankread.rs`.

## Assetto Corsa (AC1)

### `data.acd`

Found black-box from the user's AC1 install (no implementation or description of the scheme was used):

- Optional 8-byte header whose first i32 is negative (-1111 on 92 of the 375 packed cars), then entries until the end: `i32 name length, name, i32 n, n x i32`; the low byte of each i32 is one encrypted byte.
- Byte i of a file = plain[i] + key[i mod key length] (mod 256). The key is ASCII: eight numbers 0..255 joined by `-`.
- The numbers, over the lower-cased car folder name b[0..n] (i32 arithmetic, wrapping, C division / remainder):
  0. sum of all bytes;
  1. from 0, for i = 0, 2, 4, ... while i + 1 < n: a = a * b[i] - b[i + 1];
  2. from 0, for i = 1, 4, 7, ... while i + 3 < n: a = a * b[i] / (b[i + 1] + 27) - 27 - b[i - 1];
  3. 0x1683 minus every byte after the first;
  4. from 66, for i = 1, 5, 9, ... while i + 4 < n: a = (b[i] + 15) * a * (b[i - 1] + 15) + 22;
  5. 101 minus b[i] for i = 0, 2, 4, ... while i + 2 < n;
  6. from 171, for i = 0, 2, 4, ... while i + 2 < n: a = a mod b[i];
  7. from 0, for i = 0 .. n - 2: a = a / b[i] + b[i + 1];

  each taken mod 256.
- How: 12 cars ship both `data.acd` and a loose `data\` folder. Encrypted minus plain gives each car's repeating key text. Numbers 0, 1, 3, 5 and 7 fell out of a brute-force search over small loop formulas. For 2 and 4 the 12 pairs were too few: every other packed car's missing numbers were recovered by trying all values until `car.ini` decrypted to text with `[HEADER]` / `[BASIC]`; cars whose names share a prefix then showed which characters each number reads (collisions), which fixed the loop bounds, and a search inside those bounds found the constants. Number 6's bound was corrected the same way.
- Check: all 375 packed cars of the install decrypt to a `car.ini` with `[BASIC]`; for the 12 paired cars every file whose loose copy has the same length is byte-identical (the 6 that differ have different lengths: loose files edited after packing). Test: `tests/ac1_library.rs`.

### INI / LUT

- `[SECTION]`, `KEY=VALUE`; comments from `;` or `//`; names upper-cased; values trimmed text; a later duplicate key replaces the earlier value; UTF-8 BOM tolerated; file order kept.
- LUT: `x|y` lines; inline `(|x=y|x=y|)`.
- Code: `src/ac1/ini.rs`.

### `.kn5`

See the module header of `src/ac1/kn5.rs` (public AC1 format: textures, materials, one node tree; mesh vertices 11 f32, skinned 19 f32).

### AC1 track folders

Conventions read off the AC1 install's tracks (code: `src/track/scan.rs`):

- **Layouts**: every subfolder holding `data\surfaces.ini` is a layout, named after the folder; without one, the track root is the only layout (name ""). A layout's models come from `models_<layout>.ini`, else `models.ini`, else `<folder>.kn5`. UI text: `ui\<layout>\ui_track.json`, else `ui\ui_track.json`. AI: `<layout>\ai\`, else `ai\`.
- **models*.ini**: `[MODEL_n] FILE / POSITION / ROTATION`. ROTATION is heading, pitch, roll in degrees; 1,126 of the library's 1,134 entries are 0,0,0, the rest headings only (180, -17, 213, 225).
- **Placement**: kn5 matrices are row-vector, translation in elements 12..14, world = local * parent; the model placement multiplies last. A heading turns local +Z towards +X (right-handed about +Y).
- **Spawn dummies** point along their local +Z: on Barber `AC_START_0`'s +Z is (0.704, 0, -0.710), the AI line's first step (0.70, -0.70). Dummies may float: Barber's `AC_HOTLAP_START_0` is 6.7 m above the road.
- **Dummies** never drawn (nor their subtree): `AC_START_`, `AC_PIT_`, `AC_HOTLAP_START_`, `AC_TIME_`, `AC_AUDIO_`, `AC_CREW_`, `AC_SPAWN`. `AC_POBJECT_*` and other `AC_*` nodes with geometry are drawn. Inactive nodes hide their subtree.
- **Physics meshes**: a name starting with digits is physical; the rest starts with the surface KEY (longest key wins; else the longest key of 3+ characters it contains: Brands Hatch's `15CNC_PITS` is PITS). Kunos's own collision meshes are `01WALL`, `03ROAD`, ... (invisible, non-renderable): only a bare `0` prefix is decor without collision (`0FENCE`, `0BUSHES` on Barber). AC1 has no wall surface: a wall is a steep physics mesh, and meshes whose key a track does not define still collide (Glen Sheil drives on `1MIDROAD`, Kunos tracks define no WALL key for their `01WALL` meshes). Ours: such a mesh becomes an EVO wall when it is steep (area-weighted |normal.y| < 0.5) or named like one, else the AC1 default ROAD.
- **Surfaces**: per layout `data\surfaces.ini` `[SURFACE_n]` KEY, FRICTION, DAMPING, WAV, WAV_PITCH, DIRT_ADDITIVE, IS_VALID_TRACK, IS_PITLANE, BLACK_FLAG_TIME, SIN_HEIGHT, SIN_LENGTH, VIBRATION_GAIN, VIBRATION_LENGTH; keys a track uses without defining come from the AC1 install's `system\data\surfaces.ini` (ROAD 1.0, GRASS 0.6, KERB 0.92, SAND 0.8 with damping 0.1).
- **Protected kn5** (CSP encryption marker `__AC_SHADERS_PATCH_KN5ENC` at the head or the tail) are refused.
- **CM skins**: a 1x1 / 2x2 placeholder texture is replaced by the same-named file of the first `skins\cm_skins\*` folder.
- **CSP config**: the track's `extension\ext_config.ini`, else the AC1 install's `extension\config\tracks\loaded\<folder>.ini` (or `...\tracks\<folder>.ini`).

### AC1 AI lines (`fast_lane.ai`, `pit_lane.ai`)

Decoded from the Kunos tracks of the install (version 7): header 4 x i32 (version, point count, lap time, sample count); per point 3 x f32 position, f32 distance along the line, i32 id (20 bytes); then i32 count (= points) and 18 x f32 per point: speed, gas, brake, an unused slot, radius, distance to the left / right edge, camber, direction, normal (3), length, forward (3), tag, grade. Later data (game caches) is ignored. Code: `src/ac1/ai.rs`.

### AC1 `ui_track.json`

Often not strict JSON: BOM, raw newlines / tabs inside strings, `//` and `/* */` comments, trailing commas; `src/ac1/ui.rs` repairs those. Fields used: name, country, city, length ("3.7 km", "2.3 miles", "3700"), pitboxes, geotags (decimal or degrees / minutes / seconds, N/S/E/W; latitude first unless the letters say otherwise; an optional third tag is the altitude).

### AC1 track shading (the install's `system\shaders\win\*_ps.fxo`, disassembled with the Windows SDK's fxc)

- Every ks* shader: colour = albedo x (ksAmbient x ambient x hemisphere + ksEmissive + ksDiffuse x sun x N.L x shadow) + specular. Lighting works on the raw texture samples and the tonemapper (`ksPostToneMap`) writes its result with no encoding curve: AC1 shades in gamma space, so a scalar factor k there is k^2.2 in linear light (a product of two textures is unchanged).
- `ksMultilayer`, `ksMultilayer_fresnel_nm`: albedo = txDiffuse(uv0) x Σ_c txMask(uv0).c x txDetailC(world.xz x multC) x magicMult over R, G, B, A (the vertex shader passes the world position: detail layers are planar from above); `_objsp` uses object-space coordinates. A mult of 0 samples the detail's first texel.
- `ksPerPixelMultiMap*` with useDetail: albedo = lerp(D, D x txDetail(uv0 x detailUVMultiplier), 1 - D.alpha).
- The alpha-tested shaders do not read ksAlphaRef.
- `data\semaphore.ini`: `[OBJECT_n]` NAME (a node), TYPE (0 disqualified, 1 ready / red, 2 start / green), ORDER.

### Track look (`src/track/{look, materials, textures, csplights}.rs`)

The decisions (ours unless marked) are documented in the module headers; in short:
- shader families: plain / alpha-tested / blended / decal AC1 materials -> DefaultMaterial (template `common_assets\...\road_signs\common\materials\276.material`); AC1 multilayers and MultiMap details on ground -> DynamicTrackMultilayer (`brands_hatch\materials\curb-grass`), tarmac -> DynamicTrackMultilayer too, with an unused Blue layer dark (that shader adds the dynamic track's rubber to the Blue weight; the game's DynamicTrack pixel shaders read no albedo map of a material - only Base_Basecolor x the stock procedural Base_BaseMap.b x Detail_Map.r x Variation / Imperfections - so an AC1 road texture and the lines painted into it cannot be shown there), both in sum mode with the AC1 diffuse as Base, the mask's three heaviest channels as layers, layer maps normalised to a mean of 0.4 and Detail_BaseColorMultiplier 2.5, the remaining brightness baked into the Base map; CSP flashing -> Particle flipbook; semaphore lamps -> Semaphore;
- brightness: each material shaded with the AC1 shader formula under one AC1 reference light (colorCurves.ini SUN / AMBIENT HIGH, sun at 35 degrees, the default exposure; `tools/ac1_lighting.py`) and taken to linear light; then per class (road, grass, kerb, building, tree, other) one scale puts the track's median inside the stock p25..p75 of the per-track class medians, and each material stays inside the stock class p05..p95 (`aceconv albedo`, `stockstats/measure_albedo.txt`);
- `aceconv albedo <track dir | content.kspkg>`: effective albedo per material class with the layered maths above (`src/stats/albedo.rs`). **Measured** on 17 stock circuits (median [p25..p75] of the per-track medians): building 0.273 [0.254..0.283], grass 0.185 [0.169..0.223], kerb 0.307 [0.213..0.347], road 0.060 [0.052..0.099] (DynamicTrack by its own shader maths; a child material takes its parent's value for every f21 / f22 entry with f2 = 0).

### The track scan and ground API (`src/track`)

Every track stage reads the AC1 track through one scan, parsed once:

```rust
let src = track::Source::open(dir)?;          // layouts found, every kn5 memory-mapped once
let scan = track::Scan::new(&src)?;           // kn5s parsed once, meshes flattened to world space
```

`Scan` (`src/track/scan.rs`):
- `layouts: Vec<Layout>`: per layout `name`, `ui: TrackUi`, `surfaces_ini`, `fast_lane` / `pit_lane: Option<AiLine>`, `dummies: Vec<Dummy>` (every `AC_*` transform node, world space; `position()`, `forward()` = local +Z, `up()`), `data` (the other `data\*.ini` files by lower-case name); `dummies_with(prefix)`.
- `meshes: Vec<MeshInst>`: one per placed, shown kn5 mesh: `world` matrix (row vectors), `layouts` (bit i = in layout i), `material` (global index), `role` (`Visual` or `Physics { surface, drawn }`), AC1 `name`. `scan.mesh(m)` is the kn5 mesh (vertices stay in the map), `scan.positions(m)` its world positions, `scan.triangles(m)` its valid triangles (degenerate / out-of-range dropped; this order is the triangle index every stage uses).
- `materials: Vec<MatRef>` / `textures: Vec<TexRef>`: deduplicated across kn5s by content; a name that collides with different content gets the kn5 stem added. `scan.material(g)` (the kn5 material), `scan.material_texture(g, "txDiffuse")`, `scan.texture_bytes(t)` (a CM skin file for placeholders).
- `surfaces: Vec<Surface>` (union over layouts, `layouts` mask; global AC1 keys marked `global`), `csp: Option<(path, Ini)>`, `notes`, `report()`.
- `scan::LIFT` = 0: no vertical offset. Stock EVO tracks drive below y = 0 (**measured**: spa's road reaches -29.0 m), so positions are kept; geometry, dummies and AI lines all go through the scan, so changing the constant moves everything together.

`Ground` (`src/track/ground.rs`), built from every physics mesh of a layout, invisible ones included:

```rust
let g = track::Ground::build(&scan, layout, drivable_flags);   // flags per scan surface
g.below(x, y_from, z)        // highest surface at or below y_from (Hit)
g.nearest(x, y, z)           // the surface closest in height
g.nearest_drivable(x, y, z, reach)
g.hits(x, z)                 // every surface at (x, z), highest first
g.surface_at(x, y, z)        // the scan surface index there
g.triangles_of(&[surfaces])  // all triangles of a set of surfaces
g.bounds(), g.drivable_bounds()
```

`Hit { y, surface, drivable, normal }`. Index (ours): a uniform xz grid, cell size twice the mean triangle size clamped to 2..16 m, triangles listed in every cell their xz box touches (CSR arrays); a query reads one cell. `Ground::from_triangles` builds one from any triangle soup.

Surface classes (`src/track/classes.rs`): `surface_kind(&Surface) -> Kind` (Road, Kerb, FlatKerb, Grass, Sand, Concrete, Pit, Verge, Turf, Wall; rule table in the module header) and `material_classes(&scan)` (per material: drivable, tarmac, grass family, from the physics surfaces under it, else its name and textures).

## Geography

`data/countries.tsv`: ISO 3166-1 alpha-2 and numeric codes, English short names, UN M49 sub-region and continent (Americas split into North America = sub-regions 021, 013, 029, and South America = 005), 249 rows, taken from the ISO 3166-1 / UN M49 lists as Windows' geographical-location API reports them (cross-checked against the IANA tz database's iso3166.tab: identical code set except XK, which IANA lacks, and EH, which Windows lacks), plus the IANA zones from zone1970.tab (public domain) covering each country. The game's own time-zone and nation enums are left to the track packages.

## stockstats

`aceconv stockstats [content.kspkg] [out dir]` reads the package (never writes it) and writes `stockstats.json`:

- `cars.<id>` / `tracks.<id>`: per-item metrics;
- `car_ranges` / `track_ranges`: per metric n, min, p10, p50, p90, max over the stock items;
- `facts`: ids, kspkg record ids, ISMesh limits, the texture header table.

Car metrics: `phys.*` (car::physics::stats: mass, CoG, power, torque, limiter, gearing, tyre radius and mu, springs, travel, brakes, steering, CD.A / CL.A, our top speed, lateral g and PI; `aceconv car-phys <pkg | stock id>` prints the whole model); files and MB; counts by kind (`ext.mesh`, `ext.material`, `ext.texture`, `ext.animation`, ...); `mesh.*` (vertices, tris, sections of LOD0, LODs, bones) over all meshes and for `exterior.*` / `interior.*`; `tex.*` (count per format, largest size, mips MB); materials (shaders, used slots); `sys.<file>.fN`, the number of top-level fN entries of the lighting, LED, analogue-gauge, actor and backfire files.

Track metrics: the same file / mesh / texture / material metrics; `entity.<kind>` over all scenes; `ismesh.instances` and `ismesh.node_max`; irradiance probes, volume textures and probes per km² (over the probes' xz box); dynamic-track tiles, extent, rubber-free share, peak rubber, grip p10 / p50 / p90 on road cells; AI splines (`ai.ideal_len_m`, `ai.ideal_m_per_point`, `ai.ideal_closed`, `ai.ideal_cp_per_km`, `ai.pit_len_m`, `ai.pit_m_per_point`, `ai.control_points_max`: the largest line of the track); the `.track` extent.

`aceconv stockstats compare <car.kspkg | track dir> stockstats.json` measures one converted item and prints every metric outside min..max (OUTSIDE) or outside p10..p90 (unusual), and metrics most stock items have but the item lacks. A stock-content run takes about a minute at two threads.

Vegetation (`veg.<class>.*` for tree, impostor, bush, grass, classed by the factory mesh folder): `instances`, `per_km2` (over the probe box), `cull_p50` (instance-weighted), `species`, `top_share` (the most used mesh's share).

Track metrics also cover the Surface Def entries (`surface.entries`, `surface.grip_min` / `grip_max` over category-1 entries, `surface.sin_height_max`, `surface.damping_max`) and the spawn types (`spawn.grid_or_hotlap`, `spawn.pit`, `spawn.box`).

Caveat: the measurement is of the content.kspkg on the measuring machine. If tracks were installed into it (track-slot installs replace a stock track's files), that track's metrics describe the installed track. `ACECONV_STOCK_EXCLUDE=<id>,<id>` leaves items out (`facts.excluded` records them). The committed `stockstats.json` was measured with `ACECONV_STOCK_EXCLUDE=road_atlanta,interns`: road_atlanta's files were replaced by a local install on the measuring machine, and `interns` holds the garage / showroom scenes, not a circuit; 17 tracks remain.
