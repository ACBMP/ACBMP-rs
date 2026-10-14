# RE notes

Findings from the Mac build (`acbmp_sf.exe`, full symbols; Ghidra addresses
are Mac VAs) that the code relies on or will need. Verified facts are marked
as such; the rest is read from decompiled code and not yet exercised.

## Shaders

- `ShaderPermutation` and `LightingShaderPermutation` FastLoad append an
  8-byte custom tail (`CustomSerializeFastLoad`, 0x0097b560 / 0x0097b3e0:
  just advances 8). Verified: with it all MaterialTemplates decode.
- `CompiledCode` is plain D3D9 `vs_3_0` / `ps_3_0` bytecode with a CTAB
  comment block (constant names such as `g_WorldViewProj`, `g_EyePosition`,
  `g_BoneMatrixArray`, `DiffuseMap_2`, `NormalMap_0`). Verified on every
  permutation of DataPC.forge and the 11 MP maps (`acbmp shaderlab`).
- Permutations are looked up by `ShaderKey` through
  `GfxDeviceResourcePool<ShaderPermutation, VertexShader>::Get(key)`.
- `GfxVertexShaderKey` (32 bits, two u16 words), from
  `GfxGeometryVertexShaderKey::SetFlag` (0x00965b50) and
  `GfxVertexFormatDescOptions::Initialize(GfxVertexShaderKey)` (0x009680d0):
  - word0 bits 0-4: geometry type (2, 5, 6 enable flag 4 -> word bit 0x10)
  - word0 bits 5-6: primitive class (0 = mesh vertex formats below, 1 = a
    second family whose options come from word1 bits 0-3)
  - word1 bits 0-3: vertex format 0..10, each enabling a set of attribute
    slots in `GfxVertexFormatDescOptions` (slot meaning still to map)
  - word1 bit 5: option slot 9
  - flags 0-3, 5 (0x20), 6 (0x40) set directly by `SetFlag`
- `GfxGeometryPixelShaderKey::SetFlag` (0x00965a90): flag 0 -> bit 1 (only
  valid if MaterialTemplate byte 0x3c), flag 1 -> bit 2 for geometry types
  2/5/6, flag 2 -> bit 4 (MaterialTemplate byte 0x3f), flag 3 -> bit 8 for
  geometry type 1.
- Main lit forward pass of a static mesh: vertex permutation
  `(VertexFormat << 16) | 1`, pixel = the `LightingPermutation` with
  `ShaderKey 1, LightingKey 1` (ambient cube + one direct light + fog +
  world lightmap). Lighting permutations have empty `Parameters`; their
  material registers are those of the `PixelShaderPermutation` with the same
  `ShaderKey` (CTAB names agree). Drawn by `acb-render/src/retail.rs`; on
  San Donato all 202 material/format pairs translate (29 programs).

### Material parameters

- A Material's values are DynamicProperties keyed by
  `PropertyID = CRC32(PropertyName)` (zlib crc32): `Layer0_Diffuse`
  0x5D09D951, `Layer1_Diffuse` 0x4A72CD12, `Layer0_Normal` 0x30FB00F6,
  `Layer1_Normal` 0xFC510068, `Layer0_Color` 0x19350632, `Layer1_Color`
  0xBF420D86, `PixelBlenderScale` 0x37B93E61, `PixelBlenderBias` 0x06555971,
  `Layer0_UVScale` 0xE1E639A5, `Layer1_UVScale` 0xF69D2DE6. Floats are raw
  f32 bits, vectors f32x4, textures `TextureSelector` objects.
- Missing values: template `EditableProperties` (PropertyName,
  LinkedOperator) -> `MaterialDefinition.Operators` defaults
  (`ConstantVectorValue.Value`, `ConstantFloatValue.Value`).
- `TextureSelector` (`GetTextureMap` 0x009db730, `GetUsedTextureSet`
  0x009db5b0): `TextureSpecificationMethod` 0 = material's TextureSet,
  1 = explicit TextureSet, 2 = explicit Texture, 3 = global/gradient; sets
  index `Maps[MapType]`. Method 0/1 selectors also carry the Texture ref.
- Permutation `Parameters`: PropertyID, ConstantIndex, NbVectors, Type
  (2 = sampler, 0/1 = float). Tex2 PS key 1: Layer0_Normal s0,
  Layer1_Normal s1, Layer0_Diffuse s2, Layer1_Diffuse s3, Layer1_UVScale
  c128, PixelBlenderScale c129, PixelBlenderBias c130, Layer0_Color c131,
  Layer1_Color c132; VS: Layer0_UVScale c100.

### Static vertex shaders (formats 3/4)

- Inputs: v0 SHORT4 (pos = xyz * |w| * 3.8148e-6, sign(w) = bitangent sign),
  TexCoord0 SHORT2N * 16 (= short / 2048), Normal/Tangent raw UBYTE4
  ((b - 127) * 0.007874), format 3 adds a D3DCOLOR at +16 (uv at +20).
- Engine constants: c0-3 WVP rows, c8-11 World rows, c12 eye, c18 clip plane
  (PS kills on its distance; (0,0,0,-1) keeps all), c84-86
  WorldToLightProj, c116 + b15 deconstruction.
- `TexCoord1 = (normal.w, tangent.w, 0, 255) / 255`; `.x` is the per-vertex
  layer blend weight of two-layer templates:
  `w = sat((1+s)(v - 0.5) + s((1 - a0 + a1)/2 + bias - 0.5) + 0.5)`, s =
  PixelBlenderScale, a0/a1 = diffuse alphas (height masks).

### Lighting pixel shader (key 1 / LightingKey 1)

- albedo = Layer0Diffuse * Layer0_Color (lerp Layer1) * 2 * Color1 * Color0.x;
  alpha = diffuse.a * Layer0_Color.w * Color1.w (decals fade by vertex alpha).
- Ambient cube c24-29 (w = intensity), direct light c40 (towards light) /
  c41 (colour), lightmap s12 + c87-89 (neutral: c88.y = 0, c89.y = 1),
  fog `f = sat((dist - c17.x) * c17.y) * c17.z`, `lerp(colour, c16, f)`.
- Writes oC1 (depth, MRT) too; dropped. Normal maps sample `.xyw`
  (DXT5nm-style), so textures are uploaded non-sRGB and the shader's gamma
  output is converted to linear before the sRGB target.
- Unhandled globals still left at 0 on San Donato: g_EyeDirection,
  g_LODBlendFactor (dithered LOD cross-fade), g_TimeOfDay,
  g_LayeredSkyUserColor, g_EntityVisualScaleFactor, g_WorldToLightProjVS,
  g_NumLights/g_OmniLights/g_SpotLights, g_ShadowedDirect.
- Saturation check (2026-10-14): five San Donato views have 0-52 pixels
  at >= 250 in every channel, nearly all `GEN_FabricW_01A` (white cloth,
  Layer0_Color 0.6 -> albedo ~1.2) and `GEN_Veg_Fougere_03A`; no blown-out
  walls reproduced. `acbmp pick <map> --eye --target --px x,y` names the
  entity/mesh/material under a `shot` pixel.
- Next: `StaticPrimitive::StaticPrimitive(..., GfxVertexShaderKey, ...)`
  (0x009682b0) for the other key bits; sky/time-of-day constants; shadows.

## Maps

- Out-of-bounds walls: entity with `OutOfBoundsComponent`, material
  `AC2MP_Oob` (no diffuse). Hidden in the renderer.
- `Material.MaterialDisabled` marks gameplay helpers (e.g.
  `AC2MP_Hay_Lock_disable_01`). Hidden.
- Maps reference TextureSets / MaterialTemplates in the core `DataPC.forge`.

## Collision

- Scenery collision hangs off entity components with a `RigidBody`
  (`InertComponent` for static scenery, `RigidBodyComponent` for props,
  `SuspendedObjectComponent`): `RigidBody.Shape` -> shape root,
  `FilterInfo.Layer`, `IsFixed`; `InertComponent` also has `IsGround`,
  `IsRoof`, `IsFreeRunStarter`, `GPSurfaceNavType`.
- `MeshShape`: `Vertices` f32x3 in the entity frame (same matrix as the
  Visual), `IndicesTriangles` u16x3, `IndicesMaterial` u8 per triangle into
  `Materials` (PhysicMaterial roots such as `Stone_Dirt`). `MoppCode` is the
  Havok BV tree over them, not needed. `BoxShape`: `HalfExtents` around
  `Matrix`; `SphereShape`: `Center`, `Radius`.
- San Donato: 1032 colliders, 203 MeshShapes, 56k triangles. Layers seen:
  1 scenery, 2 props (benches, fixed; small loose spheres not fixed),
  10 hay cart / haystack bodies, 18 camera-only proxies (`*_CamCol_*`,
  `GEN_HayStack_01X`), 24 the OOB wall. `acbmp collision <map> [--obj]`.

## Animation

All from the Mac build; layouts verified by every core Animation decoding to
its exact byte length and by key continuity (`acbmp animstats`).

- `Animation::FastLoad` = schema fields, then `CustomSerializeFastLoad`
  (0x002e4820): u32 track count; per track a u8 descriptor index into
  `AnimTrackTable::ms_AnimTrackDescriptorTable` or 0xFE + u32 built-in id.
- `AnimTrackEvent` appends `AnimTrack::KeyTimesOnlyCustomSerializeFastLoad`
  (0x0031a790): u16 count + count x u16.
- Descriptor table (`AnimTrackTable::Init` 0x002cdcb0, slots resolved via the
  Mach-O indirect-symbol table): index = compression * 3 + format, formats
  8and8 / Var16 / Bitmap, compressions QuatNone, Quat16, Quat24, Quat32,
  Quat48, Quat64, QuatZFloat8, QuatZFloat16, Quat96, Vector3None,
  Vector332, Vector348, FloatNone, Float8, Float16, Byte x3.
- Track body: u32 alloc size, u32 key count, key times (8and8: (n-1) x u8;
  Var16: (n-1) x u16; Bitmap: u8 (len | shift << 6) + len bytes, key k at
  the k-th set bit << shift), then n x value.
- Decompressors: smallest-three quaternions (missing slot + sqrt rebuild)
  at 4/7/10/15/20 bits, Quat96 with the slot in float mantissa LSBs,
  single-axis Z rotations, mm-quantized vectors, x0.008 floats. Details in
  `crates/acb-anim/src/lib.rs`; constants confirmed from the constant pool
  at 0x022c7730..0x022c77d0.
- Keys are at 60 fps (`max key frame / Length` = 60.00 for every animation).
- `AnimTrackDataMapping.TrackID` (one per track, same order) equals a
  skeleton `Bone.BoneID` for 91% of tracks; others are 0 and small ints.
- Builtin tracks (0xFE + u32 id, 35% of all tracks) point into
  `AnimTrackBuiltinTracks::ms_trackDataTable` (Mac 0x02205760, 0xc1cb4
  bytes; `GetPermanentTrack(id)` = table + id - 1). It is byte-identical in
  ACBMP.exe (file offset 0x1daad80), found there by signature. Entries are
  in-memory `AnimTrackData2`: u8 descriptor, then 8and8: u8 count + (n-1)
  x u8 times; Var16: u8 count (0 = u16 count at +2) + (n-1) x u16;
  Bitmap: u8 (len | shift << 6) + bitmap. Values start at the compression's
  alignment (`GetKeyDataPtr`): QuatNone 16; Quat32/64/96, FloatNone,
  Vector3None/332 4; Quat16/48, QuatZFloat16, Float16, Vector348 2; the rest
  1. All 209,458 references in DataPC decode, fit their clips, and give
  unit quaternions (`acbmp animstats --exe`).
- Root motion: TrackID 0 carries the entity's displacement (a Vector3 and a
  Quat track, builtin): walk 1.012 m / 32 frames, run 1.707 m / 20 frames,
  along +Y. The Reference bone (0x2c52cbb0) is the pelvis in entity space
  (z ~0.93, yaw ~90 degrees). TrackID 2 is a Byte track (unknown).

## Skinned meshes and characters

- Vertex format 0, 32 bytes: short4n position (xyz x 10 m) | ubyte4 normal,
  tangent, binormal ((b - 127) / 127) | short2n uv (x 16) | ubyte4 palette
  slots | ubyte4n weights (sum 255). From the skinned VS (CHR_Detail_DoNotUse
  key 1): `a0 = BlendIndices * 3`, float4x3 bones at `c[a0 + 120]`, 42 slots.
- Primitive i draws with `CompiledMesh.InstancingData[i]`
  (`HwBoneToMeshBoneTable`: palette slot -> `Mesh.Bones` index) and
  `CompiledMeshMaterials[i]`. `SubMeshIndex` is not the primitive index.
- `MeshBone.PackedInitialMatrixInverse` (u16 J0..K2, u32 TransA, TransB):
  J, K = i16 / 32767; inverse-bind rotation columns (J x K, J, K);
  translation = three signed 21-bit fields of TransA | TransB << 32, / 65536.
  Mesh space is skeleton model space turned +90 degrees about Z. Checked by
  bone bind origins landing on the centroids of the vertices they carry
  (`ACBMP_DEBUG_PALETTE=1 acbmp character`); a fit against skeleton bind
  alone cannot see that rotation.
- Entity (e.g. `UA_Altair`): `Visual.Object` = Mesh or LODSelector
  (`LODDescs[0].Object` = nearest LOD; it may be null, `UseNullObjects`), `Visual.InstanceData.MaterialInfos` and the
  `MaterialOverrider` swap `*_Empty` materials. `SkeletonComponent` =
  `MainSkeleton` + `AddonSkeletons`; addon bones with an existing BoneID are
  that bone. Two MA_Ezio_LOD0 bones (eyes) are in no listed skeleton; they
  follow the nearest mapped bone.
- `acbmp character <forge> UA_Altair --obj out.obj --anim <clip> --t <s>`
  writes the CPU-skinned pose; verified on a walk and an air assassination.
- GPU skinning: each skinned draw owns its VS frame block (globals plus
  `m.row(0..3)` of each palette matrix at c120 + 3 * slot); attribute
  locations: pos 0, normal 3, tangent 6, binormal 7, uv 8, indices 2,
  weights 1. Unbound emissive samplers default to black (white doubled the
  light).
- `CHR_Detail_DoNotUse` (UCMA/ARCM_Altair_Cloth): only a normal map is
  bound and its PS masks direct/omni/spot light by c0.yzz = (1, 0, 0), so
  it renders red under a sun. Asset quirk, not a translation bug.
- EntityBuilder (`CU_*`, `AC2MP_IDxx_MainTemplate_Player/NPC`): `Template`
  Entity + `BuildColumn{Pass, TargetProperty.PathNodes, ColumnID}`;
  `Rows[i]` hold one dynamic property per column, keyed name_hash =
  ColumnID. Pass 5 with an empty path adds the value: BuildTable /
  BuildTableExt (apply its row), GraphicObject (Mesh/LODSelector),
  Skeleton (add-on), SoftBody (`Cloth`: `SoftBodyStates[0].GraphicObject`
  is the visual, usually also added as a GraphicObject; sim data =
  ClothLOD particles/constraints, not simulated yet). Pass 4 with
  [MaterialPropertyPathNodeSolver(TargetMaterial), TargetPropertyID
  0xd70e6670 = CRC32("TextureSet")] sets a shared material's TextureSet
  (`UA_Ezio_Head` -> `UA_Ezio_Head_Set`); `_Custom_Colors` columns (same
  pass-4 path) set `Custom_Color_1` 0xdf1f9c86 (some also
  `Custom_Color_2` 0x4616cd3c) to row f32x4 colours; the
  `AC2MP_Characters_Body` PS reads them at c128/c129 (materials default to
  white). Applied as `MaterialOverride` vectors.
- Cloth (`acb-anim/src/cloth.rs`, `acbmp cloth <forge> [name]`): the
  first `ClothLOD` with particles. `VertexPos`/`VertexNormals` are in the
  visual mesh's space (every constraint's sqrt(`SquareRestLength`) equals
  the particle distance exactly); particles skin to the LOD's own
  `MeshBones` by `BoneIndices` (u8 x4) / `BoneWeights` (f32 x4).
  `VertexFlags` 1 = pinned (skin factor 1, radius 0.01-0.02), 2 =
  simulated; `VertexSkinFactors` == `VertexRigidities` (painted 0..0.98);
  `VertexSphereRadius` = collision radius. `VisualVertexMappings[i]`:
  three particle indices (0xffff = none), barycentric `Weights`, `Offset`
  along the particle triangle's normal; rebuilds the visual mesh to < 1 mm
  where `UseVisualSkinning` = 0. The visual mesh's vertex order differs
  (seam splits, counts off by 1-2), so vertices are matched by bind
  position + UV. `ClothSettings`: gravity (0,0,-10), 3 constraint
  iterations, air friction, skinning velocity factors (unused yet),
  `Ragdoll` (body collision shapes, unused yet).
- Viewer cloth: world-space Verlet at 60 Hz, pinned particles follow the
  skin, others are pulled by skin_factor^2 / 2 per step and clamped to
  0.05 + 0.6 (1 - skin) m from the skin (stand-in for body collision);
  the part is posed on the CPU (model space, identity skin matrices).
- MP personas: `AC2MP_IDxx_Main` (BuildTable from the builder) adds head,
  eyes, legs; the body/arms/robe/accessories live in `AC2MP_IDxx_Custom1..3`
  (+`_ext`, spelling varies: `ID17_Cutom3_ext`) and `_Custom_Colors`
  BuildTableExts, which no data object references: the game picks them by
  the player's customization; row 0 is the default look. Player builders:
  ID07 Priest 0x1832900f, ID08 Noble, ID10 Hunter 0xaa74dca8, ID14, 15, 17
  Executioner, 18, 20, 22 Courtesan, 23-27. Base cycles:
  `xx_l_walk_hipm_footl`, `xx_h_run_hipm_footl`.
- `acbmp charshot <forge> AC2MP_ID07_MainTemplate_Player out.png --anim
  xx_h_run_hipm_footl --frames 4 --yaw 60` renders through the game shaders
  (`ACBMP_DEBUG_BONES=1` prints bind-pose skin residuals per bone).
- Placement: skeleton "Human Body" has `Reference` (bone 0) at the origin
  and bone 1 (ground) 0.964 m below it, turned 90 degrees; walk/run clips
  drive Reference only through builtin tracks. Meshes are authored feet on
  z = 0 facing -Y; entities face +Y (`FightUtils::IsInFrontOfPlayer` dots
  the matrix's Y row). `Character::model_to_entity` = Rz(180) * inverse
  bind-pose skin residual; only needed for poses without a Reference
  track (with builtins loaded, model space is entity space).
- Spawns: Entities with `MultiSpawnPlayerComponent{SpawnType, TeamIndex}`
  (`Archive::player_spawns`); SpawnType 0 = standard.
- `acb-viewer <map.forge> --character <builder> [...] --count 12 --anim
  xx_l_walk_hipm_footl` places personas round-robin on standard spawns;
  `--capture out.png --capture-after 3` writes one frame offscreen and exits
  (use this instead of desktop screenshots). With ACBMP.exe found (`--exe`,
  default next to `multi/`) characters follow root motion and return to
  their spawn every `--wander` seconds.

## Player control (viewer `--play`)

- `--play` drives the first `--character`: WASD relative to the orbit
  camera, Shift runs, right mouse orbits, wheel zooms. `--hold w,shift`
  presses keys from the start for `--capture` runs.
- Locomotion (`acb-viewer/src/locomotion.rs`): cycles
  `xx_l_wait_hipm_footl` / `xx_l_walk_hipm_footl` / `xx_h_run_hipm_footl` /
  `xx_h_sprint_hipm_footl` blended by a 0..3 speed (tent weights), sharing
  one normalized phase (all start on the left foot). Space sprints.
  Measured root speeds: walk 1.90, run 5.12, sprint 6.28 m/s.
- One-shots (`acbmp rootmotion <forge> <clips>` prints length, displacement,
  yaw curve and Reference bone start/end):
  - releasing input at run speed plays `xx_h_runstop_foot{l,r}` (0.2 s,
    0.72 m) + `xx_h_runstop_foot{l}_tr_h_wait_hipm_foot{r}` (0.47 s, in
    place); from sprint `xx_h_sprintstop_foot{l,r}` (0.4 s, 1.33 m). The foot
    is the variant whose first frame is nearest the current pose.
  - standing with input > 60 degrees away:
    `xx_h_wait_hipm_footl_to_h_waitturn_{left,right}_{090,180}_footl`
    (0.2 s) + `xx_h_waitturn_..._footl_tr_h_wait_hipm_foot{l|r}` (0.4 s).
    The `_to_` clip's root track turns 90/180 degrees between 0.05 and
    0.1 s but its pose stays in the starting frame (Reference bone yaw
    70 -> 137 for left 90; the follow-up starts at 47 = 137 - 90), so the
    heading is applied when it ends. Stop clips' poses do follow their root.
  - `xx_h_runturn180_*` have no root yaw at all: the engine turns the
    entity itself (not used yet).
  - Cross-fades of 0.15 s from the last displayed pose.
- Collision = the map's fixed rigid-body shapes except camera layer 18, in
  a 2 m grid; normals with |nz| >= 0.6 are floor, the rest walls. Walls are swept at 0.55 m and 1.3 m (radius 0.3,
  one slide), floor snaps from 0.6 m above (step height); moves onto no
  floor are refused. No falling, jumping or climbing.
- Open: start transitions (`xx_h_wait_foot*_tr_*`), run turns, the real
  HumanGround state machine.
- `--capture` runs step a fixed 60 Hz simulation from `about_to_wait` and
  never present, so they work with the window hidden and are repeatable.
  `--input "0=w,shift 1.5= 2=d"` scripts held keys over simulated time.
