# RE notes

Tests: `cargo test` (unit); with `ACBMP_SCHEMAS` and `ACBMP_MULTI` (the
game's `multi` directory) set, `crates/acb-world/tests/san_donato.rs`
checks this file's San Donato numbers against the retail data.

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
- Scene depth: the lighting PS writes `oC1 = v1.z / v1.w` (NDC depth);
  shaders such as the water (`AC2MP_VEN_WaterSea_01a`) read it back as
  `g_DepthSampler` (s8) at `(vPos + 0.5) * g_VPosToUV`, linearize it as
  `g_ReverseProjectionParams.w / (d + .z)` and compare with
  `dot(pos, g_WorldToView[2])` for depth-faded opacity. The renderer
  draws opaque, blits the depth buffer to an R16Float target holding
  1 / view distance (so .z = 0, .w = 1), then draws blended materials,
  binding the copy to materials with a `g_DepthSampler` sampler.
  `g_WorldToView` row 2 is negated (distance ahead). Venice water now
  shows; its reflection sampler (s1, `Operator109`) is still unbound
  (white) and the sky dome (`3D_TimeOfDay_SkyDome_Layer0`) not drawn.
- Ambiance (`acbmp ambiance <map>`): `World.WorldGraphicData` ->
  `Ambiance` (WorldAmbiance: `Fog` {Color, Intensity, Density, Near, Far},
  `LayeredSky`, post effects) + `WorldLightMapSettings.WorldLightMaps[]`
  {texture, `TexelWorldSize` = uv scale, ZMin, ZMax} (day, night) +
  `AmbianceCurrentTime`. Every MP map's LayeredSky has the same dim bluish
  sun (30/30 deg): night defaults overridden at runtime, so the sun stays
  hand-set. `AmbientCube::GetEffectiveColors` (0x9cf880): face * Tint, HSV
  S *= Saturation, then * Scale + (Bias, Bias, Bias, 0); faces Right,
  Left, Front, Back, Top, Down.
- World lightmap (lighting PS): uv = (pos.xy + n.xy * c87.x) * c88.zw +
  0.5; mip = sat((dist - c89.x) / c89.y); texel.a * c87.w + c87.y = baked
  ground height, fade sat(1 - |h - z| * c87.z); lm = rgb * c88.x;
  colour = lerp(ambient-lit, albedo * lm, sat(dot(albedo * lm, c88.y))).
  Used: c87 = (0.5, ZMin, 0.5, ZMax - ZMin), c88 = (1, 3, TexelWorldSize
  x2), c89 = (0, 60): offset/fade/intensity/weight are guesses; alignment
  checked top-down (lit areas follow San Donato's streets).
  `ACBMP_NO_LIGHTMAP=1` disables, `ACBMP_LIGHTMAP_INTENSITY` overrides c88.x
  (2 looked over-orange indoors, Palazzio). `acbmp texture <forge> <name> out.png`
  decodes BC1/2/3 (rgb | alpha).
- Sky: `LayeredSky.Layers[i]` = {`Mesh` (3D_TimeOfDay_SkyDome_Layer0),
  `MaterialOverride`} drawn as one instance centred on the eye, scaled to
  0.9 * far. The dome PS samples `SkyDomeColorRampTex` (e.g.
  `AC2MP_SanDonato_NightDay_SkyColor_Map`, 32x16: u = time of day, dark
  at both ends) at u = 0.125 + g_TimeOfDay.x, so g_TimeOfDay.x = hours /
  24 - 0.125 (`AmbianceCurrentTime` 12 on every MP map). `ACBMP_NO_SKY=1`.
- LightingKey bits: 0-1 direct lights (1, 2), 2-3 omni lights, 0x10+
  spot/projector lights (`g_ProjectorShadow` s11, cookies s9), 0x80 =
  shadowed sun (`g_ShadowedDirect` instead of `g_DirectLights`, cascaded
  shadow map `g_ShadowSampler` s15, `g_ShadowNoiseSampler` s6, cloud
  shadows `g_CloudShadowSampler` s14 at world xy * `g_CloudUVScaleOffset`).
  The VS writes TexCoord6 = dot4(worldpos, `g_WorldToLightProjVS` rows).
- Shadow PS (key 1 / 0x80): cascades X, Y, Z, W = v8 * `g_ShadowScales*` +
  `g_ShadowOffsets*` (u, v, depth); the last of Y, Z, W with min(u,v) >=
  RangesMin and max(u,v) <= RangesMax (.y/.z/.w) wins, X otherwise, so W
  is the finest; atlas u = u * 0.25 + 0.25 k (X..W); 4 Poisson taps
  (constants in the shader) rotated by the 64x64 noise at vPos / 64,
  radius `g_ShadowNoiseScale` texels of `g_ShadowMapSize`; lit = texel >=
  depth (b15 `g_UseDepthTextureShadows` off); shadow = sat(sum / 4 +
  sat(dist^2 * `g_ShadowFadeParams`.x + .y)) * cloud.
- Renderer (`acb-render/src/shadow.rs`): 4 x 1024^2 R32Float atlas
  (FLOAT32_FILTERABLE requested; R16Float otherwise), cascades to 8 / 25 /
  70 / 200 m fitted as texel-snapped spheres in light space, casters 150 m
  towards the sun, depth range 400 m beyond (the shader selects by u, v
  only, so receivers behind a small cascade must still fit its depth).
  Opaque static meshes and skinned characters cast (no alpha-tested or
  blended surfaces): the skinned caster reads each draw's own VS constant
  block (bone rows at c120 + 3 slot) and its instance World rows.
  Cost (`acbmp shot --bench 30`, San Donato street, 1600x900 incl.
  readback): 3.7 ms with shadows, 3.0 ms without.
  `ACBMP_NO_SHADOWS=1`, `ACBMP_SHADOW_DUMP=file.pgm`,
  `ACBMP_SHADOW_XONLY=1` (debug). Sun direction is still hand-set.
- Shader parameters of `Type` 4 are operator-driven:
  `ShaderParameter.AssociatedOperator` -> an operator of the template's
  `MaterialDefinition.Operators`. For `UVTransformOperator`
  (`Parameters`: UVTransform {Scale, Translation, TranslationSpeed,
  Rotation, RotationSpeed, Animate*}) the shader reads 3 rows and does
  uv' = (dot((u, v, 1), row0), dot((u, v, 1), row1)). Rows built as
  scale * rotation with translation + speed * t (order unconfirmed) and
  rewritten every frame; the Venice water's normal maps scroll and the
  water looks right. Other operator kinds of type 4 (e.g. the water VS
  c100) are not fed.
- Saturation check (2026-10-14): five San Donato views have 0-52 pixels
  at >= 250 in every channel, nearly all `GEN_FabricW_01A` (white cloth,
  Layer0_Color 0.6 -> albedo ~1.2) and `GEN_Veg_Fougere_03A`; no blown-out
  walls reproduced. `acbmp pick <map> --eye --target --px x,y` names the
  entity/mesh/material under a `shot` pixel.
- Next: `StaticPrimitive::StaticPrimitive(..., GfxVertexShaderKey, ...)`
  (0x009682b0) for the other key bits; sky/time-of-day constants; shadows.

## Maps

- No sound data in this install (no Wwise `.pck`/`.bnk` next to the forges,
  no SoundBank roots): audio cannot be done from this copy.

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
- `MultiInertComponent.Shape` is an inline `MultiMeshShape`
  (`ContainedEntities[i]` + `ContainedShapes[i]`, own MOPP): a merged
  copy of entities whose own `InertComponent` has `IsMerged`; colliders
  are deduplicated by (entity, shape). EntityGroups are walked
  recursively. All 11 AC2MP maps + ACR Rome decode (9k triangles Siena,
  a small arena, to 296k Rome); Palazzio/SanMarco/Siena also use
  `CapsuleShape`/`CylinderShape` props (not decoded).
- San Donato: 998 colliders, 203 MeshShapes, 56k triangles. Layers seen:
  1 scenery, 2 props (benches, fixed; small loose spheres not fixed),
  10 hay cart / haystack bodies, 18 camera-only proxies (`*_CamCol_*`,
  `GEN_HayStack_01X`), 24 the OOB wall. `acbmp collision <map> [--obj]`.

## Navmesh

- `NavMeshManager` roots (one per tile, 18 on San Donato) hold
  `NavigationMeshes`: `VertexList` f32x4 in world space, `TriangleVertex`
  u16 x3, `Triangles[i].NeighborTriangleIndex` u16 x3 (same mesh; 0xffe2,
  0xffe3, 0xffe4 = -30..-28 mark border edges), `SourceEntityHandle` (the
  ground entity it was built from), MOPP. Cross-mesh links (`MetaLinks`
  with `SafePositions`/`TriangleList`, `DirectConnectionSets`,
  `WayPointNetwork`) are not decoded.
- `Archive::nav_graph` welds vertices (2 cm) and joins collinear
  overlapping border edges (T-junctions between meshes): San Donato 8321
  triangles, main component 7809. Other maps split into many components
  (rooftop islands, reached by climbing). `acbmp navmesh <map> [--obj]`.
- Cost: 150 crowd NPCs (with cloth, CPU-posed cloth parts) take about
  3.4 ms of simulation per 60 Hz frame on the dev PC (20 simulated seconds
  in 5.1 s vs 1.1 s with none).
- `CrowdFlow` components (59 on San Donato, 610 m of lanes):
  `NavFlowPointsLocal[i].FlowPosition` (entity-local polyline),
  `FlowWidth` (2-7 m), `BiDirectionalFlag` (else first -> last),
  `Weight`, `Head/TailConnectingNavFlows` (entities of the lanes joining at
  the first / last point), `Wander`, `Filter`, plus navmesh triangle and
  waypoint refs. `Archive::crowd_flows`.
- Viewer `--crowd N`: NPCs walk the lanes (lateral offset, weighted next
  lane, one-way lanes entered at their head, 15% pause at lane ends) or,
  without lanes, A* over navmesh triangles; idle/walk blend at the walk
  clip's root speed; 0.7 m separation from each other and the player.

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
- Locomotion cycles are stored as half strides, one clip per leading foot:
  `*_hipm_footl` ends in exactly the first frame of `*_hipm_footr` and
  `_footr` ends in `_footl`'s first frame (summed bone-angle seam 0.000 for
  walk and run, ~1.8 rad = one frame of motion for sprint), while
  `_footl`'s own last frame is far from its first (9.4 rad run, 3.8 walk,
  18.6 sprint). The Reference bone shows it too: run `_footl` turns the
  pelvis 101 -> 78 degrees, `_footl` + `_footr` 101 -> 101. Full strides:
  walk 0.533 + 0.467 s = 1.0 s / 1.897 m, run 0.333 + 0.333 s / 3.414 m,
  sprint 0.267 + 0.267 s / 3.348 m. Looping `_footl` alone was the "plays
  half the animation, then resets" bug (a 9.2 rad snap every 0.333 s at a
  run in the viewer); key times, `Length` and the 60 fps rate were fine
  (one-shots such as `ID07_Air2G_Front_Kill_ass`, 140 frames, play to
  their last key). `Clip::then` joins two halves into one loopable clip
  (keys shifted by the first half's length, root motion carried on);
  `Clip::seam` measures how well one clip continues into another and
  `Clip::looped_with` joins only true halves. `acbmp rootmotion --stride`
  / `animplay --stride` show the joined cycle.

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
  `Ragdoll` -> `LiteRagdoll.Capsules` (`BoneID`, `Height`, `Radius`,
  bone-local `LocalMatrix`). Segment convention not read from the engine
  (`LiteRagdollInstance::Update` 0x5816a0 calls it through a stub): used
  as origin -> +X * Height, since 60 of 89 child bones sit along +X;
  bind-pose penetration tests (`acbmp cloth --character`) cannot tell the
  conventions apart.
- Viewer cloth: world-space Verlet at 60 Hz, pinned particles follow the
  skin, others are pulled by skin_factor^2 / 2 per step and clamped to
  0.05 + 0.6 (1 - skin) m from the skin, and pushed out of the ragdoll
  capsules by capsule + particle radius;
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

## Freerun guidance

- `GuidanceSystem` components (also Capsule/Cylinder/Barrel variants with
  their own shapes): `CompressPoints` u16 x3 = (v - 32768) * 5 mm in
  entity space; `GuidanceObjects[i]`: `Index0` -> `Index1` edge, `Valid`,
  `SubType` (1 edge, 4 = 26 on San Donato), `DecN4Normal0/1` = 4 x i16
  / 511 each, the two faces meeting at the edge (roof ledge: up, then the
  wall's outward normal; `EdgeFilter` says which pairs were extracted:
  WallGround, GroundCeiling). `Partitioner` = BV tree over the edges.
- `acbmp guidance <map> [--obj]`: San Donato 4075 edges / 6.4 km, Firenze
  11981 / 13.5 km; edge midpoints sit on the collision mesh (median 2-3
  mm; 99.5% within 5 cm on Firenze and Siena; San Donato's other 800 lie
  where there is no collision at all).

## Kills

- Per persona `IDxx_G2G_{Front,Back,FarFront,FarBack}_{Walk,Run}2{All,Run}_
  Kill_ass` / `_Killed_Npc` (+ `_Kill_Missed_ass`, `_Kill_NpcTarget_ass`,
  `_Killed_NpcTarget_Npc`, `_Tr_out`, `_Cam1..8` camera clips); also
  Air2G, Drop2G, Narrow2Narrow, Hayground grabs, FallBack, Humil (stun)
  sets under `IDxx_` (generic).
- Assassin and victim clips share one frame, the victim's: at t = 0 the
  victim's Reference is at the origin (yaw 90 = facing +Y like idle), the
  assassin's ~1 m away (ID07 Front: (0.16, -1.03), yaw -90). Root tracks
  barely move; the Reference bones carry the motion. Verified by playing
  both in one frame (`acb-viewer --pair ass,victim`).
- Viewer: E kills the nearest person within 2.5 m ahead: Front/Back by
  the side of the victim the player is on, frame at the victim turned so
  the assassin's start pelvis points at the player, 0.25 s blend in;
  afterwards the player is re-rooted under the clip's final pelvis
  (Reference relative to the idle's neutral (0.03,-0.02,0.96)/66 deg) and
  locomotion fades back in. The victim holds the last frame. The game's
  approach (`positionf` clips) and target rules are not modelled.

- Stuns (`IDXX_G2G_Humil_{Front,Back}_{Win,Failed,DoubleHumil,Missed}_
  {ass,npc}`, `_tr_out`, `_positionf_*`) do NOT share a frame like kills:
  both Reference bones start within 0.2 m of their own origins (ass yaw
  -90, npc front +90). The relative placement comes from the positionf
  step. Viewer Q stuns with `IDXX_G2G_Humil_{Front,Back}_Win_{ass,npc}`
  reading ref yaw -90 as "faces +Y" and +90 as "faces -Y" (as the kills
  show): the assassin plays in the player's own entity frame, the victim
  in the same frame moved 0.9 m ahead (`kill::STUN_SPACING`, a guess that
  looks right: shove, the victim falls and stays down). Victims hold
  their last frame; get-up is not modelled.

## Player control (viewer `--play`)

- Code: `crates/acb-game` (`World`, `player.rs`, `locomotion.rs`,
  `jump.rs`, `climb.rs`, `kill.rs`, `crowd.rs`, `ground.rs`); the viewer
  only maps keys to a `PlayerInput` per tick and draws.

- `--play` drives the first `--character`: WASD relative to the orbit
  camera, Shift runs, right mouse orbits, wheel zooms. `--hold w,shift`
  presses keys from the start for `--capture` runs.
- Locomotion (`acb-game/src/locomotion.rs`): cycles
  `xx_l_wait_hipm_footl` / `xx_l_walk_hipm_footl` / `xx_h_run_hipm_footl` /
  `xx_h_sprint_hipm_footl`, each joined with its `_footr` half into a full
  stride (see Animation), blended by a 0..3 speed (tent weights), sharing
  one normalized phase (all start on the left foot; the walk's foot switch
  is at 0.53, the others' at 0.5). Space sprints. `ACBMP_TRACE=pose`
  prints the player's per-frame pose change (snaps and resets stand out).
  Measured root speeds: walk 1.90, run 5.12, sprint 6.28 m/s.
- One-shots (`acbmp rootmotion <forge> <clips>` prints length, displacement,
  yaw curve and Reference bone start/end):
  - releasing input at run speed plays `xx_h_runstop_foot{l,r}` (0.2 s,
    0.72 m) + `xx_h_runstop_foot{l}_tr_h_wait_hipm_foot{r}` (0.47 s, in
    place); from sprint `xx_h_sprintstop_foot{l,r}` (0.4 s, 1.33 m). The foot
    is the variant whose first frame is nearest the current pose.
  - standing with input > 60 degrees away:
    `xx_l_wait_hipm_footl_to_l_waitturn_{left,right}_{090,180}_footl`
    (0.4 s) + `xx_l_waitturn_..._footl_tr_l_wait_hipm_foot{l|r}` (0.8 s); the
    `h_` set is the same shape at half the length.
    The `_to_` clip's root track turns 90/180 degrees between 0.05 and
    0.1 s but its pose stays in the starting frame (Reference bone yaw
    70 -> 137 for left 90; the follow-up starts at 47 = 137 - 90), so the
    heading is applied when it ends. Stop clips' poses do follow their root.
  - standing with input in front: `xx_l_wait_footl_tr_l_walk_footr`
    (0.8 s, to 1.82 m/s) or `xx_l_wait_footl_tr_h_jog_footr` (0.73 s, to
    3.23 m/s), steered by the controller, cancelled on release; the cycle
    then continues at speed 1 / 1.6 from the phase whose pose is nearest
    the clip's last frame (32-sample search; a fixed phase fits badly).
  - `xx_h_runturn180_foot{l,r}` (0.2 s, in place) has no root yaw: its
    Reference ends at yaw 31 / (-0.25, -0.03) and
    `..._tr_jog_hipm_foot{l,r}` (0.47 s, to 3.5 m/s) starts at -149 /
    (0.25, 0.03), i.e. the heading flips 180 degrees between them. Used
    when running with input > 135 degrees behind; foot by pose match.
  - Cross-fades of 0.15 s from the last displayed pose.
- Collision = the map's fixed rigid-body shapes except camera layer 18, in
  a 2 m grid; normals with |nz| >= 0.6 are floor, the rest walls. Walls are swept at 0.55 m and 1.3 m (radius 0.3,
  one slide), floor snaps from 0.6 m above (step height); moves onto no
  floor are refused. No falling, jumping or climbing.
- Falling: a move whose floor is more than a step (0.6 m) below starts a
  fall with the locomotion velocity; gravity 10 m/s^2, walls still slide.
  Pose `xx_l_walklowfall_footl_tr_fall` / `xx_h_runlowfall_footl_tr_fall`
  held on its last frame (its root track, 3-3.3 m down, is ignored). On
  touchdown `xx_h_landing_forward_{soft,hard}_footl_tr_h_{wait_footl,
  jog_footr}_a` (< 3 m, < 6 m) or `xx_h_landing_damage_footl_tr_h_{wait_footr,
  jog_footl}`, by whether input is held. Thresholds are guesses.
  `--at x,y,z` starts the player elsewhere; `ACBMP_TRACE=1` prints its
  state four times a simulated second. Roof at (-31.3, 42.0, 7.5) next to
  the San Donato spawn plaza drops 4.5 m.
- Freerun jumps (`acb-game/src/jump.rs`): `xx_h_run_{up,front,down}_{050,300,550}cm_foot{l,r}_to_air`
  (takeoff, 0.13-0.2 s) and `xx_h_air_..._to_surface` (landing). Their
  root tracks are not trajectories (run_up_050cm rises 0.87 m in place):
  the engine flies the character. Viewer: while running, a guidance ledge
  0.4-2.2 m up within 2 m ahead (with floor behind it) or a drop with
  floor 1.5-5.5 m ahead triggers a jump; velocity = parabola landing on
  the target at t = dist / 6 m/s (0.4-0.8 s); pose = takeoff clip held on
  its last frame, landing clip at touchdown, then the run at the matching
  phase. Kind by height/distance (up > 0.3 m, down < -1 m; 050/300/550 by
  < 2.5 / < 4 m). Chains naturally (post -> stairs on San Donato).
- Climbing (`acb-game/src/climb.rs`): guidance edges with a horizontal
  face normal are holds (wall = that normal; `top` if the other face is
  up). On-wall pose `xx_climb_wait_1m`: hands 0.5 m ahead and 1.6 m above
  the entity (`acbmp posebounds`). Walking into a wall with a hold 1.3-2.6
  m up within 0.9 m mounts (`xx_h_wait_hipm_footl_tr_climbing_1m`, root
  +0.6 m). W/S/A/D look for the next hold on the same wall plane 0.5-2 m
  away and play `xx_h_climb_1m_tr_climb_{u,d,l,r}_{l,r}hand_{2,3}` +
  `xx_h_climb_..._{2,3}` + `..._tr_climb_1m` (class 2 = 1.2 m up / 1.5 m
  side, 3 = 1.8 m), carrying the entity in proportion to the clip's root
  progress. Up with no hold above a `top` edge hops onto the roof (jump
  up_050 landing); down with nothing below lets go (fall). Hanging
  (`hangwall`, `hangfree`), corner turns and `xx_h_hangwall_tr_passover_*`
  are not used yet; `xx_l_climb_1m_to_groundentry_*` steps off at the
  bottom.
- Falling (vz < -1, 0.3 m below the fall start) facing a hold 1.3-2.2 m
  above the feet within 0.7 m grabs it: 0.15 s glide into the wall pose
  (`xx_fall_tr_climb_*` are 4-frame blips, not used). `--at x,y,z,yaw`
  more than 1.5 m above the floor starts falling (test: --at=-31.67,49.0,
  7,0 catches the stone post by the San Donato spawn).
- Open: hanging, the real HumanGround state machine.
- The sim runs a fixed 60 Hz tick (`acb_game::DT`, never the frame
  time). The viewer runs the ticks due by the frame's time (sim up to one
  tick ahead; a frame on a tick shows it exactly), samples keys per tick
  (a press between ticks counts for one) and draws bodies interpolated
  between the last two ticks; cloth (visual only) steps per frame.
- `--capture` runs feed frames of 1/60 s (`--fps N`: 1/N s) from
  `about_to_wait` and never present, so they work with the window hidden
  and are repeatable. `--input "0=w,shift 1.5= 2=d"` scripts held keys
  over simulated time. `ACBMP_TRACE=1` output is identical at --fps 60 and
  144; the capture is the first frame at or past `--capture-after`, so
  with fewer frames than ticks it can land a tick later.

## Netcode (survey)

- Mac symbols: standard Net-Z DOs only (`_DO_Session`, `_DO_Station`,
  `_DO_SessionClock`, `_DO_RootDO`, `_DO_IDGenerator`, `_DO_DefaultCell`,
  `_DO_PromotionReferee`) and game datasets `_DS_RoundDataSet`,
  `_DS_TeamSpawnPointInfo`, `_DS_BufferAbilityManager`, ... The game layer
  replicates through its own classes: `NetEntity`/`NetEntityManager`
  (master/duplica, promotion, migration), `NetPlayer`, `NetObject`,
  `NetDataSet`, `NetRequest`, statechart transitions
  (`NetOnTransit*`, `NetReplicateAdditionalInfoForTransitions`), pickups,
  abilities, chatter, voice.
- Serializers are DDL-generated `Quazal::_Type_<X>::Add/Extract(DNZ::
  BitStream*, NetPlayerData::...)` thunks into `Serialize<X>` functions;
  the bit stream calls are dyld stubs. `tools/dis.py <bin> <symbol>`
  disassembles with stub calls named (Mach-O indirect symbol table) and
  `tools/rd.py <bin> f <vmaddr>...` reads constant-pool values.
- `SerializePlayerNetPos` (Mac 0x10b7c46), `HumanPlayerNetPos` fields by
  offset: x, y (+0, +4) as int16 = trunc(m * 100) clamped; z (+8) as
  trunc(z * 100 + 1) clamped to +-16383 in the low 15 bits, bit 15 = the
  bool at +0x1c ("moving"); float = u64 at +0x14 (ms) * 0.001, negated if
  bool +0x10; if moving: int16 x, y of +0x20/+0x24 (* 100), u8 yaw =
  floor((frac_signed(+0x28 / pi) + 1) * 128), i8 +0x2c; then 1 bit "has
  extra" (type +0x2d in 1..7 and length +0x2e <= 1023), and if set 3-bit
  type, 10-bit byte length, float +0x30, length bytes of the type's add
  data (+0x34). `operator<<` writes whole values; `WriteType<T>(v, bits)`
  packs bits.

## Climbing

`acb-game/src/climb.rs`, driven from `player.rs` inside the 60 Hz tick (all
moves advance by `dt` of simulated time; same end states at `--fps 60` and
`--fps 144`).

- Entry (retail order): running (Shift) at a wall starts a wall run
  (`HumanJumpDetector::DoWallingRaycastTest`: 1.5 m ray at 1.3 m, speed
  dependent facing tolerance, knee clearance) that grabs the lowest guidance
  edge 1.6-4.4 m above the feet (251/430 cm grab clips) or slides back down;
  otherwise pushing into a wall with a foot hold ~0.95 m ahead and a hand
  hold 1.2 m above it climbs straight on (vertical-stance entry).
- Bug fixed: the old mount only ran when the move was `blocked` this tick,
  and `Locomotion::blocked` eases the gait to a stop, so after the first
  contact tick the test never ran again (and its 1.3-2.6 m hold window
  missed most walls). Entry now runs every grounded tick with the stick
  pushing forward, independent of blocking.
- Stances: Climb (feet on a hold), HangWall, HangFree. Moves: climb
  u/d/l/r between holds, hang <-> climb (`hangwall_tr_climb1m_up`,
  `climb1m_tr_hangwall_down` when the lower hold has nothing under it),
  shimmy (strafe open/close pairs), inner/outer 90 degree corners,
  passover onto the top (`hangwall_tr_passover_handr`), step down / step off
  sideways to the ground, drop (`tr_fall`), catching a hold while falling.
- Gate (San Donato, ACBMP_TRACE): `--at -18,44.6,2.22,-90 --input
  "0=w,shift 1.5= 2.5=d 8= 8.5=s 8.8="` wall run -> hang -> 4 shimmies ->
  outer corner -> drop, ends at -10.60 39.67 2.15; `--at 5.24,16.5,0.04,180
  --input "0=d,shift 1.2= 3.5=w 3.7= 6=s 6.2= 9=a 12= 13=s 13.2="` wall run ->
  hang to climb -> climb down to hang -> shimmy -> drop; `--at
  -11.31,46.8,6.5,0 --input "0=a 2.5= 4=s 4.2="` walk-in mount -> climb ->
  down to hang. In-climb keys are W/S/A/D (not camera relative).

## MP rules (`acb-game/src/rules.rs`)
Pure, deterministic functions the server and client prediction share (no clock, no RNG; ties by index).
- Tuning in data: `DataPC.forge` uid 0xbc9 `GameParameters.GlobalParameters` (`acbmp dump DataPC.forge 0x00000bc9`;
  floats print as raw ints): Dist_visibility 9, Dist_gap 3.5, Dist_height_gap 1.8, PlayerNearTarget 10 /
  FarFromTarget 15, NearAssassin 15 / FarFromAssassin 70, NbAss_Max 2. `Wanted2 GamemodeParameters` (0x3723493d):
  Dist_kill_run 2.5, Dist_kill_droptech 8, threat meter speeds/angles. Game-mode XMLs are not shipped; brick defaults
  come from their `ReadFromXML` (LessFrustratingTarget 0x011c778e, PlayerDetector 0x012d140e, ScoreKill 0x0103832c).
- Kill/stun validity (`check`): kill needs Target (or Npc), stun needs Pursuer (`AssassinTask::IsTargetValidForAssassination`
  0x00b4a69e -> `GameInfo::IsAssassinFor`); free play treats NPCs as both. Cone cos >= 0.5 (0x00b4f648); |dz| <= 1.8.
  Walking ground kill: reach = clip pelvis spacing (~1 m) + Dist_gap, then a walk-in (`approach`) to the kill's start
  point (`KillClips::start_point`), arrive at 0.3 m, give up after 2.5 s. Running kill / stun: 2.5 m, immediate.
  Air -> ground within 8 m from above; hidden victim -> Ground2Haystack; climb/air victims refused. Distance bands
  47/48/49 split at 0.7 / 2.0 m, front/back at a right angle (`Ground2Ground::GetAnimationGeneric` 0x00e7a5d4).
  The use of Dist_gap in code was not found: "reach = spacing + gap" is our reading.
- Contracts (`Contracts`): pursuers per target = min(players-2, level cap) with level cap
  trunc(2(level-0.4)/(levels-1)+1) = 1/2/3 (`GetMaxTargeted` 0x011c099e). Candidates (`IsValidCandidateFor`
  0x012138d0): hard = active, not self, no mutual hunt unless 2 players, < players-2 pursuers; soft (dropped when
  nothing passes) = level cap, >= 15 m away and not the previous target while waited < 20 s. Hunters served longest
  wait first, then lower level; draw = most free slots, then furthest. Death: killer fulfilled, other pursuers lost,
  5 s cooldown (MinWaitTime, our reading); stunned hunter loses its contract.
- Compass (`locate`): fill 0 beyond 30 m (DetectionRange) to 1 at 10 m; near-target hysteresis 10/15; in view within
  9 m, |dz| <= 2 and line of sight. `threatened`: 15/70 m hysteresis.
- Gate: `--count 2 --wander 0 --at 38.6,23.96,0.5,171.4 --input "0= 0.5=e 0.6="` on San Donato (body 1 at 38,20):
  granted Far/Back at 4.0 m, walk-in 1.23 s (run, then walk under 2 m) to 1.06 m, then the back kill; identical
  trace at --fps 144.
