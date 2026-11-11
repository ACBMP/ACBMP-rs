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
    `h_` set is the same shape at half the length. With the stick still
    held when the turn ends the settle is replaced by the turn's moving
    exit: `xx_l_waitturn_..._footl_tr_l_walk_hipm_footl` (0.67 s, 1.1 m/s)
    in low profile, and in high profile (Shift or Space) the `h_` turn
    `xx_h_wait_hipm_footl_to_h_waitturn_..._footl` (0.2 s) +
    `xx_h_waitturn_..._footl_tr_h_jog_hipm_footl` (0.57 s, to 3.6 m/s).
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
  touchdown by retail landing type (`jump::landing`, see Freerun moves):
  safe `xx_h_landing_forward_{soft,hard}_footl_tr_h_{wait_footl,jog_footr}_a`
  (hard from 5.25 m, our reading), damage `xx_h_landing_damage_footl_tr_h_
  wait_footr` or with input held `xx_h_landing_damage_footl_roll` (2.74 m
  forward), fatal `xx_h_landing_death_back`. Fall height counts from the
  highest point of the fall (retail ComputeFallHeight).
  `--at x,y,z` starts the player elsewhere; `ACBMP_TRACE=1` prints its
  state four times a simulated second. Roof at (-31.3, 42.0, 7.5) next to
  the San Donato spawn plaza drops 4.5 m.
- Freerun jumps (`acb-game/src/jump.rs`), after retail's targeted jumps:
  - Reach = `HumanSharedData::SetupJumpDetectionCurves` landing polygon
    (fwd, up): (0.5,-5) (0.5,1.3) (3.5,1.3) (4.7,0.8) (6.0,-0.5) (8.0,-3)
    (8.0,-5); inner points scaled by `Human::ForwardJumpDetectionScale`
    (1.0). 10 curve slots: default source 1-3 (1 = feet, 2/3 = hands, up
    2.5 / 3.0 m), detection type 3 slots 4-6, sources 1/2 or type 4 slots
    7-9 (short). Search box 9 m ahead, 5 down, 3 up.
  - Timing/arc: retail plays takeoff `xx_h_run_{dir}_{dist}_*_to_air` then
    `xx_h_air_{dir}_{dist}_*_to_freestep[_down]` (not `_to_surface`, that
    is onto a wall), progress = t / (len1 + len2) (InAirData +0x1e4/+0x1e8),
    clip displacement warped onto the target (IgnoreGravity). Clip pairs
    end (foot l, asserted by tests/jump_map.rs `clip_timings`): up +1.31 m
    (0 / 2.5 m ahead, 0.2 / 0.4 s), front -0.51 m (0.5 / 2.5 / 5.22 m,
    0.267 / 0.4 / 0.733 s), down 050 -3.40 m 0.5 m 0.667 s, down 300 -3.0 m
    2.5 m 0.533 s, down 550 -3.01 m 5.22 m 0.867 s. `Flight` blends the two
    nearest direction classes by height and the distance classes by
    distance, warps the end error linearly in time on all axes over the
    clips' length (retail, see "Flight warp" below; until 2026-11-03 the
    vertical was eased and an extra drop added sqrt(2h/g)); closed form in
    elapsed time, so 60 Hz tick and --fps 144 agree exactly.
  - Up onto a top (rise > STEP): the clips' root rises in a straight line
    and reaches the top only at the end, so the feet cut the edge (0.52 m
    deep on the parapet). Instead the vertical is a ballistic arc to 0.1 m
    over the top (apex time split by g between rise and drop) and the
    horizontal is `u^lag` with lag = ln(frac)/ln(u_over), frac = (near face
    - 0.1 m toe) / distance, u_over = when the feet are 2 cm over the top
    (no retries). `Flight::sink` = deepest feet/toe point under a floor
    above the start step; > 5 cm rejects the flight. Not retail-exact
    (warp shape unknown), physically clean.
  - Free fall: g = 9.81 (HumanInAir GetDropTime).
  - Targets from collision floors (not guidance edges): a floor profile
    every 5 cm on three lanes (0, +-0.25 m); up = a rise > STEP within 2 m,
    gap = first floor past the drop (else down onto the first floor below,
    not deeper than 3.2 m). Tops narrower than 0.6 m are centred
    (`Ground::settle_top`, 1 cm probes from the profile sample itself).
    Bug fixed: it probed from 5 cm past the sample, so a 5-6 cm railing
    was stepped over and the target fell to the floor behind it (then
    "blocked path"/headroom against whatever was there): the cause of the
    earlier railing rejections. Path checked at 0.2 / 0.9 / 1.6 m above
    the feet (feet not in the last 0.5 m). Headroom 1.8 m from 0.3 m over
    the top on the axis and 0.12 m around it (`Ground::headroom`).
  - Beams: `Ground::beam` (top < 0.6 m across, 8 directions, 2 cm probes)
    and `Ground::balance` (player.rs hook before move_on): a move within
    60 deg of the beam axis is projected on it and pulled to the centre
    line; across it, or on a post, the move is untouched (step/jump off).
    Beam clip set (locomotion.rs `BeamClips`, retail HumanNarrowObjectBeam
    `xx_l/h_beam_*`): crouchwait per foot, crouchwalk/jog/sprint cycles
    (root 1.38 / 3.72 / 5.79 m/s), wait->walk/jog starts, jog stop + settle
    per foot, landing onto the beam, 180 turn on the spot (no run turns).
    `Locomotion::set_beam` cross-fades sets; `ACBMP_TRACE=pose` tags beam
    ticks. No climb entry (wall run / mount) from a beam. The probe allows
    floor rise 0.85 m/m (ramps read as beams before: -11.3,48 next to a
    mount spot, which also blocked the climb entry there).
  - Gate (San Donato, viewer camera now follows --at yaw): 5 cm stairwell
    balustrade (38.60,-54.4, top 1.01, 2.36 m clear) via
    `--at=34.5,-54.38,0.1,-90 --input "0=w,shift 1.1="`: up 300cm 2.03 m
    ahead +1.04 m in 0.363 s, lands centred and stays; parapet via
    `--at=-13,0,6,-97 --input "0=w,shift 0.9="`: up 050cm +1.20 m in 0.277 s
    to (-10.29,-0.33,6.59) (true centre; was -10.15 with the 5 cm probe),
    lag 1.22, sink 0. Identical at --fps 144. The old "bollard" (22.9,
    -22.44) is a 10 cm corner of a larger object with a 1.75 m face 0.15 m
    away: now rejected by the body headroom (the body would be inside it).
    The fenced square rail at x=-34.19 has a 1.9 m canopy 2 cm beside it:
    rejected too. tests/jump_map.rs: `thin_railing_landing` (World, 60 Hz:
    landing tick count = ceil(dur/DT), stays, then walks 7 deg off the rail
    axis and stays on its centre line), `thin_railings_sweep` (all < 10 cm
    tops with body room, approached over level floor: 11 of 15 land; the
    rest have an arch lintel or clutter over the path).
  - Debug: ACBMP_TRACE=1 prints jump/landed lines, ACBMP_JUMPDBG=1 the
    profile and rejected candidates. Examples: `jumpclips` (clip lengths,
    root displacement), `narrow` (narrow tops / thin triangles / profile).
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

## Netcode (ours)

Retail PRUDP/Net-Z is not kept; `crates/acb-net` is our own UDP protocol
with a server-authoritative sim.

- Wire: every datagram `"AB"`, version byte (1), type byte, little-endian
  hand-packed fields (`proto.rs`, <= 1400 bytes). Client -> server:
  `Hello{name,character,map}`, `Ping`, `Ready`, `Input{snapshot_ack,
  client_ms, last, inputs[]}` (every input the server has not acked, up
  to 32: redundancy against loss), `Bye`. Server -> client: `Welcome`,
  `Reject`, `Lobby`, `Start{map, snapshot_every, slots}`, `Snapshot`,
  `Over`. An input is 4 bytes (9 button bits, camera yaw in 1/65536 turns);
  both sides simulate the quantized input.
- Snapshot (every 2 ticks = 30 Hz): tick, delta baseline (the snapshot the
  client acked), input ack, the client's lead, echo/hold ms for the RTT,
  a changed-mask + `NetPos` per changed player (retail PlayerNetPos-style:
  int16 cm x/y, 15-bit z + moving bit, yaw byte, flags = airborne /
  attacking / climbing / victim, int16 cm/s velocity when moving; 8 or 12
  bytes), the inputs the server applied for every player since the ack
  (run-length coded) and the decided events (kill/stun started, death,
  rejection with reason, respawn). ~55 bytes for 2 players.
- Server (`acbmp serve <map> [--port --players --at x,y,z,yaw --netsim
  --log --duration]`): lobby until N joined, everyone builds the same
  world (`game::load_world`, slot i on player spawn i), then
  `acb_game::World` at 60 Hz. Per-client `InputBuffer`: inputs by tick, a
  missing one repeats the last (counted). Before each tick
  `game::authoritative_step` asks the rules hook
  `World::validate_action(player, input, rules, relation)`
  (`acb-game/src/action.rs`, calls `rules::best_victim`/`rules::check`);
  a refused kill/stun is cleared from the applied (and relayed) input and
  reported as `KillRejected`. Contracts are not hooked in yet: relation is
  `Npc` (free play) for everyone.
- Client (`acb_net::client`, viewer `--connect host:port [--netsim]`):
  the sim is deterministic, so the client rebuilds the server's state
  from the relayed applied inputs: a confirmed `Saved` (`World::save`/
  `restore`, `acb-game/src/state.rs`) is stepped through every tick it has
  all inputs for and checked against the snapshot's `NetPos` (counted
  exact/desync; on a desync the server's position wins). Then rewind to
  it and replay the own unacked inputs (remote players hold their last
  applied input) up to the predicted tick. A kill the server refused, or
  one predicted against a remote that did something else, disappears in
  that replay (logged `rollback: predicted kill undone`). Remote bodies
  are drawn from the confirmed ticks 6 ticks (100 ms) behind the newest,
  placement and pose interpolated. The clock runs `lead` 2..5 ticks ahead
  of the server: +-3 % rate from the server's lead report, a jump when
  far off (ignored until the server has inputs past the jump).
- Tests: `proto`/`game`/`link` unit tests (round trips, quantization,
  delta, redundancy under 50 % loss), `tests/match_map.rs`: in-process
  server + 2 clients over localhost with loss 5 %, delay 150 ms, jitter
  10 ms each: all confirmed ticks exact, each sees the other where it is,
  the predicted out-of-range kill is rejected (`out of range`) and rolled
  back. Viewer gate (fps 60 with delay 120 ms + fps 144): 201/207 checked
  ticks exact, the two clients' confirmed positions identical on all 26
  traced ticks and equal to the server log.
- Lag compensation: see "Kill variants" below (`view_lag` in the input).

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
- A standing mount waits 0.3 s in the hold wait before a held stick
  starts the first move (`MOUNT_SETTLE`). Shimmy: the next piece of a
  split edge is searched along the line ahead (wall dot > 0.9, -0.3..0.6 m
  on, < 0.25 m in/out, < 0.3 m up/down), and a dead end finishes the last
  < 0.3 m to the 0.25 m margin. San Donato survey (dead-end shimmies off
  2 m+ edges): 28/354 had a reachable next piece, now 24 left, all real
  hold-graph gaps (next piece 0.75 m on, or 0.22 m out and 0.3 m down).
- Viewer orbit camera backs off 1.75 m while climbing (eased, 0.4 s).

### Freerun moves (wall jumps, ledges, falls)
- Jump button: `PlayerInput.jump`, viewer key F. On the wall (in hold
  wait): F (+S) back eject = retail rebound (see "Hang rebound" below; it
  was a plain 1.71 m/s fall off the wall until 2026-11-03);
  F+W leap to a hold 0.9-2.6 m up (`xx_h_climbing_{hangwall,climb1m}_jump_
  straight_{120,180,240}cm_jump_targeted[_start|_tr_hangwall]`, root rises
  exactly 1.2/1.8/2.4 m; the nearest size is used); F+A/D, or Shift+A/D at a
  shimmy dead end, side jump to a hold 0.5-2.6 m along the wall
  (`xx_h_climbing_{hangwall,hangfree,climb1m}_tr_hangwall_{left,right}_
  {2,3}_{a,b,c}`: the b clip moves the root 1.50 / 2.25 m sideways). Target
  search `Ground::jump_hold` (same wall plane, |out| < 0.35 m, hands 0.25 m
  in from the target's ends). San Donato (`examples/walljumps.rs`): 248 dead
  ends on 2 m+ hang edges, 50 bridged by a side jump; 12 holds only a leap
  reaches. Retail's targeted rebound sets (`xx_h_rebound_{dir}_{front,up,
  down}_{050,300,550}cm_*_to_air`) are not used yet.
- Low profile (no Shift) walking at a drop deeper than 2.5 m (retail SetBreakHard, see "Low-profile edge brake"),
  or off a beam end at any drop: stops at the edge (retail `xx_l_ledge_
  lookdown_*`, `xx_l_beam_edge_stop`; clips not played yet). F there lowers
  onto the wall below (`Ground::ledge_hold`, `Climb::pulldown`: `xx_l_ledge_
  pulldown_soft_to_climbing_2m_{a,b}` + `_tr_climbing_2m`, turning to face
  the wall) or, with no hold, steps off. High profile runs off / jumps as
  before. Dropping from a hang onto a lower hold = drop + catch while falling.
- Fall damage, retail data: DataPC.forge `HealthSettings` 0x16bc5031
  `PlayerDamageFall` (Height, Damage % of max life, LandingType): 8 m 30 %
  SmallDamage, 12 m 50 % HeavyDamage, 16 m 75 % HeavyDamage, 20 m 200 %
  Fatal (NPCs: 7 m 200 %). `HealthSettings::LookupFallDamage` (0x00b9304e)
  takes the highest row <= the fall height; without settings
  `HumanInAir::LookupFallDamageWithoutHealthSettings` (0x012ccdcc) uses the
  same 8/12/16/20 m. `HumanInAir::CheckForLanding` (0x01310742): fatal ->
  ApplyDeath; ragdoll hurt landing from 5.25 m. `Player.landing` keeps
  (height, damage %, type) of the last touchdown; ACBMP_TRACE prints `land:`.
  Health/death are not applied to the match yet (mode.rs).
- Gates (tools/gates.sh, identical at 60/144 fps): eject, rebound, sidejump, leap,
  pulldown, beamend, roll (end positions in its header).

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

## Duel mode (`acb-game/src/mode.rs`, proto v2)
- `World.mode: Option<Match>` (in `Saved`, so prediction/replay step it): `Contracts`
  decide relations for `validate_action` (kills only on your target, stuns only on your
  pursuer, crowd kills allowed without points). Kill = `KILL_POINTS` 100
  (`GBrick_ScoreKill.TargetKilled`); stun 200 (see below); respawn delay 5 s, start delay 3 s are
  placeholders. Respawn on the player spawn furthest from the nearest living player,
  exactly `respawn_delay` ticks after the death tick; contract cooldown `MIN_WAIT_TIME`
  counts from the death tick too. Ends on `--time-limit` / `--score-limit`; server prints
  `serve: result after 20.0 s: a 100, b 0 (a wins)` and repeats `Msg::Over`.
- `acbmp serve <map> --mode duel|free --time-limit --score-limit --respawn --start-delay`.
- Wire: `Start.rules`, snapshot `GameState` (ticks_left, per player target/score/respawn/
  cooldown; clients check it against their confirmed world: `game_checked/game_desyncs`),
  events Score/Contract*/Respawn/MatchOver/KillRejected(reason).
- HUD: no text path in the renderer, so `Client::hud(&world)` (target distance + compass
  fill from `rules::locate`, pursuers, scores, m:ss left, last own event) goes to the
  window title and, with ACBMP_TRACE=1, `hud tick N: ...` lines (per sim second / new
  message) plus a final `hud:` line in --capture runs.
- Gates: `acb-net/tests/duel_map.rs` (2 clients, loss 5% delay 150 ms: pre-contract kill
  refused, predicted dodge kill refused+rolled back, kill scores, respawn +180 ticks,
  reassignment, 0 desyncs). Localhost (`--at=31,20.4,0.02,-90 --at=41,20.4,0.02,90
  --time-limit 20`, A `--input "0.5=e 0.8= 4=w,shift 6.2=w 6.8=e 7.2="`, B idle, netsim
  loss 2% 40 ms): reject tick 30, contracts 180, kill 427, respawn 727, A reassigned 727,
  B 1026, over 1200 "a 100, b 0"; events/final positions identical with A/B at 60/144
  and 144/60 fps, 589/589 snapshots exact.

## Kill variants, stun scoring, lag compensation
- `KillClips.sets` (kill.rs): WALK 0/1, STUN 2/3, RUN 4/5 (`{id}_G2G_*_Run2All_Kill_ass`/`_Killed_Npc`),
  AIR 6/7 (`{id}_Air2G_*_Kill_ass`/`_Killed_npc`), DROP 8/9 (`{id}_Drop2G_*`), HAY_GRAB 10/11
  (`{id}_Hayground2Walk_grab_{front,back}_kill_ass`/`_killed_npc`), TO_HAY 12/13 (`{id}_Walk2Hayground_Kill_ass`
  + `*_Walk2HayGround_Kill_Out_npc`, only ID10 has it). Missing kinds fall back to the walking pair.
  Air2G/Drop2G are authored with both References at the origin (the assassin lands on the victim,
  pelvis z 0.6-0.8); Run2All starts 0.4 m apart; the hay grab 0.6 m.
- Situations (rules.rs): Run2Ground (moving >= 3 m/s, `Player::moved` = real horizontal m/s; before,
  the rules got the gait level 0..3, so only sprinting counted as running), AirDrop2Ground (airborne or
  in a freerun flight), Drop2Ground (climbing/hanging), both from >= 0.5 m above within Dist_kill_droptech
  8 m (3D); Ground2Haystack (hidden victim), Haystack2Ground (hidden killer, 2.5 m; no stun from hiding).
  `Player::hidden` exists but nothing sets it: hiding spots are not modelled. Ledge kills: only
  `idxx_roof2ledge_*_ass` / `Narrow2Ledge_Kick*` assassin clips found, no victim pair: not wired.
- Non-walking kinds close the gap first (`player::Lead`, whole ticks): air/drop a ballistic leap
  (g 9.81, t = max(sqrt(2h/g), gap/8) in 0.2..1 s), run/haystack a straight lunge (gap/max(speed,3)
  in 0.1..0.4 s), pose blending into the clip's first frame; the victim holds its clip's first frame
  until then (`Scripted.start` = now + lead). Gate `tools/kills.sh` (expected lines in its header,
  identical at 60/144 fps): air kill from 5 m onto the NPC at 38,20, running kill from 31,20.4.
- Scoring (mode.rs, brick XML defaults since the mode XMLs are not shipped): kill 100
  (`ScoreTargetKilled`), + `BonusAcrobatic` 100 for air/drop, + `BonusHidden` 200 from hiding
  (GBrick_ScoreKill 0x0103832c); stun 200 = `GBrick_ScoreChase.ScoreHumiliation` (0x010381ec;
  `GBrick_Escape.ScoreOnHumiliation` enables it). Respawn/start delay stay placeholders: `GBrick_Respawn`
  `RespawnTime` (0x00ec1870) and `GBrick_Delay` `DelayDuration` (0x00d3d70a) default to 0 and come
  only from the XML; GameParameters/Wanted2 have no such field (GlobalParameters: KOTimeAfterKick 1.0,
  LowScoreKill 100, HighScoreKill 750).
- Rejections: `not your target` (kill on a player without the contract), `not your pursuer` (stun),
  wire reasons 7/8; "not a valid victim" is left for protected NPCs.
- Lag compensation: `World.seen` (in `Saved`) keeps every player's rules actor for the last 60 ticks.
  The client puts `view_lag` = input tick - the tick it draws remotes at (`interp`) into the top 7
  bits of the input's button word (no size change); the sim (both server validation and the step
  itself, so prediction/replay agree) checks player victims `min(view_lag, 12)` ticks back (200 ms),
  NPCs and anyone being killed now at present; the kill itself starts on the present victim.
  Test `acb-game/tests/kill_map.rs::rewound_kill`: a victim running away is out of range now but in
  range 9 ticks ago: lag 0 refused (Range), lag 9 granted.
- Stun test `acb-net/tests/stun_map.rs`: 3 clients (loss 3 %, 60 ms): the pursuer of player 0 walks
  up, player 0 stuns it at tick 350: StunStarted + Stunned + Score 200 + pursuer's ContractLost in the
  same tick, all clients exact (0 desync).

### Ground locomotion vs retail (tests/loco_map.rs)

- Speeds: retail HumanGround is root-motion driven (its speed table is
  built from the anim set, `HumanSharedData::AddSpeedTableForAnimSet`), no
  speed tuning in data (GamePlaySettings has none). Steady speeds = cycle
  root speeds, asserted: walk 1.90, run 5.12, sprint 6.28 m/s. The 0..3
  speed ease (2.5/s) is still ours.
- Heading: `HumanGround::UpdateOrientation` turns by
  `RotateVectorToVectorWithAngularSpeed` with Min/MaxRotationSpeed and
  Min/MaxRotationAngle (HumanGround +0xa08..+0xa14, set through the vtable;
  ctor zeroes them). The only setter call found, in
  `UpdateMovementIntention` (networked player heading): 1.5 / 6.0 rad/s,
  10 / 40 degrees (flagged variant 0.975 / 3.9). We use that curve
  (`locomotion::turn_toward`) instead of our old 7 rad/s clamp; gates
  unchanged. The local player's own setter values are not found yet.
- Edge stop: `TestMoveToEdgeStop..UnbalanceOnEdge` needs a narrow edge,
  gap depth > 2.0 m and pull-down clearance: `LEDGE_STOP` = 2.0 (was 1.5).
- Soft/hard landing at 5.25 m is retail's
  `HumanInAir::IsHurtFallLandingWithoutRagdoll` (height < 5.25).
- Player yaw is kept in (-180, 180] (`locomotion::norm_yaw`).
- Body: retail BipedComponent (AC2MP_Generic_PlayerEntity) radius 0.4,
  height 1.8, mass 65 = `ground::BODY_*`. Walls swept at 0.4, people kept
  2 x 0.4 apart (`PERSONAL`, was 0.6), landing headroom 1.8. Gates, kills
  and duel positions unchanged by the switch.
- Move-cycle speed (`UpdateMoveCycleMain`): normalized anim speed, walk
  0.2 / run 0.8 / sprint 1.0 (`locomotion::anim_speed`), up 1.0/s, down by
  the `LocalInit` curve (0,1)(0.333,1)(0.4,0.3)(0.666,0.2)(1,1) of the
  current speed; release above 0.4 plays a stop; a running turn beyond
  120 degrees; sprint heading 0.975 / 3.9 rad/s. Tests: loco_map
  `anim_speed_ramp`, `sprint_turn_rate`. kills `run` start 1 cm earlier.
- Standing turn-on-spot threshold (60 deg) is still ours: the
  `TestWaitToTurnStart/OnSpot` conditions read player-flag fields we have
  not identified.
- Slopes/steps: no player value. The player has no CharacterController
  (MaxSlopeAngle is reflection-only); `HumanInAir::LandedOnSlope` counts a
  landing contact 44..65 degrees as a slope; `CanStepUp/Down` are FSM
  transitions. Still ours: |nz| >= 0.6 floor, 0.6 m step.

## Wanted mechanics: hiding, blending, escape, falls (proto v3)
- Hiding spots (`hide.rs`): map Entities with a `HayStackComponent` (top-level or inside EntityGroups; San
  Donato 20: `HayStackType` 0, `DetectionBoxMin/Max` ~+-1.4 x +-0.9 x 0..1.6 m in entity space). Benches
  (`RestObjectAttributeComponent`, 3 `RestSpot`s, 34 on San Donato) are not modelled. Clips: in
  `xx_h_freestep_footr_to_haystack_01` (1.0 m fwd), from a fall `xx_h_air_to_haystack` (no fall damage),
  loop `xx_l_haystack_wait`, out `xx_l_haystack_hop_out` + `_tr_l_wait` (2 m). Entry (our reading): high
  profile moving into the box from <= 1.2 m outside, or descending over it <= 1.5 m above the top; any move
  key leaves (camera relative). `Player::hidden` only while fully in. A hidden killer has no aim cone
  (our reading); kill from hiding = HAY_GRAB set + BonusHidden 200.
- `--at` takes further x,y,z,yaw groups for bodies 1, 2, ... (scripted scenes). kills.sh `hay`: run into
  haystack 13 (24.62,7), NPC at 24.62,9.4, E: Haystack2Ground, identical at 60/144 fps.
- Blending (`rules::blended`, our reading; retail uses crowd group polygons, `GSrvGroupPolygon`, not traced):
  on the ground, <= 2 m/s, >= 2 NPCs within 2 m (|dz| <= 1.8). Crowd NPCs are the persona characters
  (round-robin over `--character`), i.e. lookalikes. `locate_concealed`: a hidden/blended target still fills
  the compass but is only `in_view` within Dist_gap 3.5 m (our reading). Net matches run without crowd.
- Escape (`Match::chase`, `GBrick_Escape` 0x00c17d32/0x00c0f710/0x00c0f794/0x00c5f4fc): phases in sight /
  in sight hidden / out of sight / out of sight hidden with TimeToEscape 0/0/12/7 s (0 = meter resets);
  meter += dt/time, > 1 = escaped: target +100 (`ScoreEscape`), hunter loses the contract, redrawn after
  MinWait. Hidden = haystack or blended. In sight = head-to-head `Ground::ray` within 30 m. Chase start: per-hunter
  threat meter (`Match::threat`, on the wire since proto v6), `GameInfo::UpdateLocalPlayerThreatMeter`
  0x011857d0 with the `GamemodeParameters` data rates (Wanted2): seen running +0.1/s x awareness, walking
  -0.1, concealed -0.132; out of sight -0.0495/-0.1155/-0.165; seen beyond 35 m -0.25; falling rates x
  max(1, 2 - awareness). Awareness, facing cones, speed ramp and InAir rates: see "Threat meter" at the
  end of this file (superseding the first version here). Full meter (1.0) starts the chase
  (StartChaseAtFullThreat). Missing: lock-on 0.99 (we have no target lock).
- Falls: `Player::landing` is now this tick's touchdown; `Match::on_fall` takes the `HealthSettings` damage %
  off `health` (100); 0 = death without killer (pursuers lose the contract, nobody scores: no suicide penalty
  in data, `NetPlayer::M2All_Suicide` 0x0112125c only applies the death); full life on respawn.
- Wire v3: `PlayerGame` + health u8, chase phase u8 (NONE), escape u16 (coef*65535); flags F_HIDDEN 16,
  F_BLENDED 32; events ChaseStarted 12, Escaped 13, Fall 14, FatalFall 15. HUD: "(concealed)", "chased,
  escape N%", hidden/blended, health.
- Tests: `acb-game/tests/hide_map.rs` (enter/exit, haystack kill, blend, fatal fall), mode tests
  `escape_scores_and_reassigns`, `falls_hurt_and_kill`, rules `blend_rules`, `hidden_killer_all_around`.

## Online crowd and retail blend groups (proto v4)
- `acbmp serve --crowd N [--crowd-seed S]`: `Start` carries `CrowdConfig { count u16, seed u32 }` (wire v4);
  `game::load_world(.., crowd)` builds N wanderers after the players and `World::seed_crowd(seed)` reseeds
  them (NPC i from seed+i+1; 0 = what `World::load` builds). The crowd is in `Saved`, so the client's
  confirmed world and rollback replay simulate it from the applied inputs; snapshots carry no crowd.
  `acb-net/tests/crowd_map.rs`: 40 NPCs, 3% loss, slot 1 runs to the nearest blend group and blends
  (180 ticks), 0 desync, both clients' confirmed crowds identical every common tick.
- Blend groups (retail, Mac): `GSrvGroupPolygon::CanUse::Method_IsEntityPositionInsideGroupPolygon`
  0x01088556: 2D convex hull of each member's position plus `ModifyFormationPolygon` 0x01088206
  (pos +- r along the entity X and Y axes), `GroupConvexHull::IsPointInsideConvexHull` 0x0108ad3e
  (crossing number, XY only); r from the caller (untraced; the FX polygon `UpdateGroupPolygon` 0x010887c4
  uses 0.8, we use 0.8). `IsGroupDispersed` 0x01079faa: a member > 6 m from the first. Groups are
  blendable AI groups (`AIEntityGroupUtils::IsGroupBlendable` 0x01123abe, `FindBlendGroups` 0x01189530,
  `NetPlayer::AddToBlendingGroup` 0x0114de98). Our crowd has no AI groups, so `rules::blend_groups`
  chains NPCs within 2 m (our reading) and caps at the 6 m dispersion; <= 2 m/s on the ground stays ours.
- Benches (not done): sit clips `xx_rest_sit_{left,right}_{000,090,180}_to_sitting`,
  `xx_rest_sit_sitting_tr_sit_idle_040cm_01`, loop `xx_rest_sit_idle_040cm_01`, out
  `xx_rest_sit_idle_040cm_01_tr_l_wait_footl`; kills exist (`IDxx_Bench2Bench_{Left,Right}_Kill_*`,
  `ID05_Walk2Bench_*`, `ID05_Bench2Walk_*`, `xx_h_assassina_sit_bench_close_*`), all in DataPC.forge.

## Benches, deterministic net tests (proto v5)
- Seats (`hide.rs`): `RestObjectAttributeComponent.RestSpots[]` (`RestSpotType` 0/1/2, `LocalPosition` ~(0.7,
  +1/0/-1, 0) entity space): 102 seats (34 benches) on San Donato, appended after the 20 haystacks as
  `Spot { seat: Some(type) }`. Facing = entity +X (our reading, not checked visually). Sit (our reading of
  retail): jump button (F) at low profile within 0.6 m; clip = `xx_rest_sit_{left,right}_{000,090,180}_to_sitting`
  (turn 0/+-90/180, picked to reach the seat facing) + `_sitting_tr_sit_idle_040cm_01`, loop
  `_idle_040cm_01`, out `_idle_040cm_01_tr_l_wait_footl` straight ahead. A fresh move key stands up.
  Seated = `Stance::Seated`, `Player::blended` (retail benches are blend spots), not hidden.
- Bench kills: `Ground2Bench` (`kill::TO_BENCH` 14, `{id}_Walk2Bench_Front_kill_ass`/`_killed_npc`) and
  `Bench2Ground` (`FROM_BENCH` 16, `{id}_Bench2Walk_{Front,Back}_100_kill_ass`/`_killed_npc`), ID05's for
  personas without their own (ID07 has only cameras), DIST_KILL_RUN reach, all-around from the bench. These
  clips have no Reference track, so `KillClips::plan` places them in the assassin's frame facing the victim
  (stand-in). kills.sh `bench`. Wire v5: flag `F_SEATED` 64.
- Net tests: duel_map's script is a pure function of the sim tick (it read remote views / arrived events);
  the winner check is by slot (5% loss can delay "a"'s join so "b" gets slot 0); B turns at 3.4 s so A
  always mispredicts the dodge. 20/20 passes, kill at tick 516 every run. tools/duel.sh still feeds viewer
  keys in real time (kill tick varies there).

## Threat meter finished (2026-10-22, threat)
- `mode::awareness` = GameInfo::GetTargetAwearness 011850aa: 1/sqrt(1 + far + to + behind). far =
  ThreatDistanceDowning 4 * ramp(dist, 10, 35); to = ThreatToTargetDowning 3 * speedramp * cone(hunter
  facing . dir-to-target, 45.57/154.16 deg) when the dot > 0; behind = ThreatBehindTargetDowning 2 *
  speedramp * (1 - cone(target facing . dir hunter->target, 25.84/134.43 deg)); speedramp = hunter speed
  ThreatLowSpeed 2 .. ThreatHighSpeed 4 m/s. The decompile shows the cone ramps as (cosA - dot)/(cosA - cosB);
  which vectors are dotted (+0x10 of the 0xb0 behaviour table) is our reading. Beyond 35 m: FarSpeed.
- In air (Presence.in_air = loco airborne or freerun flight): InAirFast 0.65 / InAirSlow 0.25 seen,
  -0.066 / -0.0825 OOS; fast = running.
- New target -> meter 0 (`Match::threat_of`). Lock-on 0.99 (THREAT_DISCOVERED) NOT wired: no target
  lock/selection exists in our code.
- Wire v6: PlayerGame.threat u16 (*65535), client applies it with the server state. HUD: " | threat NN%"
  while holding a contract and not chasing.

## Retail Human* state graph (Phase 3)
Spec: `~/.cache/acbmp-re/findings/human-states.md` (state list, transition table with
values, sources, our equivalent and status; full name list in `human-states-graph.txt`).
- Retail FSM `...Main` tests only read flags or "anim finished"; the decisions live in
  the states' Update/CheckFor* functions and in the player brain
  `AssassinTaskGround::UpdateGroundMovement` (Mac 0166fb1c), which issues interface
  commands (FallOffEdge, UnbalanceOnEdge, JumpToTarget, EnterClimb, EnterHayStack, ...).
- Tuning is code constants: HumanGroundData/HumanInAirData/ClimbData/NarrowObjectData
  are runtime structs with no reflected roots in DataPC.forge.
- Walking off: FallOffEdge needs gap depth >= 0.53 m (ours: drop > STEP 0.6); smaller
  drops are StepDown. EdgeStop / pull-down need > 2.0 m (matches LEDGE_STOP).
- In air, per tick: water well > targeted ledge grab > default grasp > boats >
  haystacks > step-off > horses > landing. Ours checks the haystack before the catch.
- Default grasp only while the fall is < 2.0 m (unless flagged), 1.2 m guidance probe
  at hand height; ours needs a fall > 0.3 m and vz < -1 instead.
- Landing contact classes: normal 0-45 deg ground, 45-70 slope, < 135 wall; a 0.3 m
  landing delay after take-off. We have no slope state.
- Haystack from the ground: a physical contact, stick within 50 deg of the contact
  normal, facing within 100 deg. Ours: 1.2 m proximity in high profile.
- Jumps: the brain's jump flag needs a button held (or pressed within 0.5 s) and the
  stick within 80 deg of facing. Ours jumps on speed >= 1.5 m/s with no button.
- Missing here: standing JumpImpulsion, targeted ledge grabs, StepOff, TurnOnWall,
  ObstacleCollision, NarrowObject FreeRun as a state, beam entry anim.
- Port order for B2: in-air order + landing + grasp gate; edge fall/step/pull-down;
  jump input + JumpImpulsion + hang targets; haystack contact, obstacle state, beam entry.
## Retail traces: acb-trace (2026-10-27)
- crates/acb-trace reads retail ACBMP.exe (Wine) via /proc/<pid>/mem; offsets in
  crates/acb-trace/offsets.toml (shipped next to the binary, editable). Objects are identified
  by class id from their GetClassID stub (`B8 id C3`), so each chain link is validated and
  unknown offsets (world->player array, entity components, entity matrix) are scanned for.
- Chain: [base+0x22623f0] EngineContextImp -> +0x4c World -> (scan) pad manager -> +4 -> [0]
  entity -> components -> AIActor -> +0x18 current ActorContext (Human* state) -> +0x10
  HumanXData -> class id -> name. Details/confidence: ~/.cache/acbmp-re/findings/retail-mem.md.
- Not live-tested (no retail here): first diag.txt from the Wine machine pins the scanned offsets.
- Ours: ACBMP_TRACE=csv:<path> writes the same columns per tick (findings/trace-format.md).

## Human* port, B2a (InAir order, landing classes, grasp gate, edges)
- `crates/acb-game/src/human/`: decisions only, constants with retail addresses. player.rs asks it; motion stays in jump/climb/locomotion/Ground.
- InAir order (01328022): grasp is tested before haystack entry (hide::entry is skipped when a hold is in reach).
- Grasp (0132017a): only in the first 2.0 m of a fall (from the peak), probe 1.2 m ahead (catch_hold reach; side 0.7 m and hand window 1.3-2.2 m stay ours). Old gate (fell > 0.3 m, vz < -1) gone. Effect: the climb gate's S-drop now catches the awning beam below (hangfree, ends -10.61 41.07 3.72).
- Landing (0130fea6): contact classes 45/70/135 deg via Ground::floor_hit normal; within 0.3 m below the air start a landing waits for the delay timer (retail: length of the starting action; ours: eject 0.467 s, walk-off/drop 0). Our floors stop at 53 deg, so the slope class (45-53) is only logged; no slide state.
- Edges (00fc3ed2): drops >= 0.53 m are fallen off (was 0.6), smaller ones stepped down (Ground::move_on_edge). Edge stop > 2.0 m kept; Player.edge remembers the stop so F pulls down while standing too (00fd226a), and while walking into it (00fd215e).

## Human* port, B2c (haystack contact, obstacle collision, turn on wall)
- Haystack (00fe030c): entered on contact (body radius + 0.12 m skin, ours: our haystacks are solid boxes the slide stops at 0.4 m), facing within 100 deg and the stick within 50 deg of the way in, any profile (human::haystack). The 1.2 m high-profile proximity is gone; kills.sh hay holds the run to 1.0 s (contact is reached ~0.85 s in).
- ObstacleCollision (00ceee20 entry, 00fd0fd2 clip choice): wall contact within 45 deg of the stick, |vz| <= 0.2, contact >= 0.5 m up (our sweeps are at 0.55/1.3 m). Player.obstacle keeps angle and side blend; no bump clip yet, the cycle eases out (loco.blocked). Contact = the stick swept 0.05 m (Ground::slide_hit), so it holds at zero speed. The < 30 % step test stays for no-floor refusals.
- TurnOnWall (01315f90): in the air, wall hit face first (normal within 45 deg of -facing), into it > 0.17 m/s, > 2 m/s horizontal, heading within 60 deg of the facing, > 0.2 s airborne, passes twice -> yaw turns away from the wall, horizontal velocity 0 (no reception clip).
- Not ported: StepOff (00cef704: pushes off a contact too steep to stand on, clearance ray >= 0.8 m; our floors stop at 53 deg so nothing is stood on there), beam entry clip, TestIsEdgeValidForFall, walk-off landing delay.

## Escort (teamvip) RE, Phase 4 prep (2026-10-27, escort-re)
Full findings with addresses and the Phase 4 task plan: `~/.cache/acbmp-re/findings/mode-escort.md`.
- Engine name "teamvip" (`DataPC.forge` 0x6a0ac994 `TeamVIP GamemodeParameters`), values identical to Wanted2
  (kill reach, threat meter). Map `DataBlock_gamemode_teamvip` blocks are empty.
- VIPs are crowd NPCs, not players: `GBrick_SpawnVIPs` NbVIPs 2, all dead -> 5 s -> both respawn, new VIPs become
  hunter targets after 10 s, way start >= 35 m from hunters. Hunters target every VIP; protectors target every
  hunter and are its pursuers (stun/escape as in Wanted); no contract lottery; no friendly fire (our reading).
- VIP paths (`AdditionalWorldData_TeamVIP` / `TeamVIPNavflowPath`) are NOT in any shipped PC forge: we author them.
- Score defaults: ScoreVIPKilled 100, ScoreEnemyKilled 100, ScoreVIP base 200 / bonus 10 / max 500
  (protect/checkpoint), kill bonuses as Wanted. Rounds: ChangeAttackerTeam 1 (roles swap), Intro and recap 8 s.
- Every AC2MP map has 4 team spawn groups (SpawnType 2, TeamIndex 1..4) of 4 spawns.
- Round length, respawn delay and what the VIP score counts come from unshipped XML: placeholders.
- Jump button (B2b, 0166fb1c): every jump test calls ButtonPressed / ButtonJustPressedWithDelay(.., 0.5) with
  PadButton 0 (call sites 167105c/1671084, 1671591/16715b9, 1671764/1671784). PadButton order in the binary's
  name table is Button1..4, PadUp.., so 0 = Button1 = spacebar in multi/DefaultBindings.map = Legs (kills use 1 =
  Button2 = left mouse, weapon hand, ButtonWantsToKill b16a76). Ours: PlayerInput::sprint (Space) via
  human::jump::Legs (held or pressed within 0.5 s, counted in ticks) + stick within 80 deg (cos 0.1736).
  The run-speed >= 1.5 test stays (ours). Scripted runs tap Space for one tick before the jump.
- No manual jump (user, 2026-10-27): the player never jumps on a button press alone. Jumps only come
  out of freerunning (HP + Legs) at an edge or gap. B2b's standing JumpImpulsion port was removed; the
  retail state exists, but MP players never reach it by pressing a button.
- Jump to a hang (TargetedLedgeGrab 0131c1e4): jump::hang_target picks a wall edge facing us within the
  detection range when the gap has no floor within 1.75 m lower; Flight.grab -> Climb::catch on arrival.
  Feet 1.8 m below / 0.35 m out from the grab point are ours. Map test jump_map::hang_targets (471/637 edges).

## Escort teams and roles, E1 (2026-10-27, escort-teams)
- `mode::Mode {Duel (default), Escort(rules::Teams)}` on `Match`; `Match::escort(teams, ..)` builds one. Duel path
  unchanged (contracts). Escort skips `Contracts::update`, so no contracts, chases or threat meter run yet.
- `rules::Teams {team: Vec<Team>, attacking_team, vips: Vec<usize>}`, `Role {Hunter, Protector}` from
  `team == attacking_team`, `Kind {Player(team), Vip, Npc}`; `team_relation`: hunter->VIP Target, protector->hunter
  Target, hunter->protector Pursuer (hunter may stun it), teammates and protector->VIP Other (no friendly fire),
  crowd Npc. `Match::on_action` now scores through `Match::relation`, which handles both modes.
- Hooks for later tasks: `Teams::vips` (body indices, E3 fills it), `attacking_team` (E4 swaps per round),
  `Teams::alternating(n)` (slot parity: placeholder until E5 sends teams on the wire). No net or proto changes.

## Escort VIP ways, E2 (2026-10-27, escort-ways)
- Retail reads the ways from `AdditionalWorldData_TeamVIP` (not shipped on PC), so `acb_game::vip` generates them:
  `ways_for(&Nav, hunter_spawns) -> Vec<Way { points, checkpoints }>` (up to `WAYS` 4), `team_spawns(&Archive)`
  (SpawnType 2, (TeamIndex, pos)), `Walker` (follows a way exactly, faces turns at the crowd's 4 rad/s, reports
  reached waypoints; E3 can drive VIP bodies with it), `clear`/`height` (on-mesh checks).
- Generation: candidates = CrowdFlow lane points every 5 m (triangle centroids without lanes), snapped to the main
  navmesh component, >= 35 m (`MIN_DIST_FROM_KILLERS`) from every hunter spawn and 0.3 m from the mesh border. Fixed
  seed (crowd `Rng`); a way = up to 6 A* legs (crowd `Nav::path`) between candidates, each greedily string-pulled with
  a 0.2 m clearance either side, until 150..400 m; leg goals stay 30 m from earlier ones, way starts 40 m apart.
  Checkpoints every 25 m plus the goal (our reading; retail flags hand-placed waypoints). Only `+ - * / sqrt` in the
  generator, no HashMap iteration: same output everywhere.
- Navmesh tiles have mm seams (e.g. x = -32 on San Donato): `height` bridges 5 cm, else a walker on a clear
  segment drops off the mesh at the seam.
- Small maps can't keep all 16 team spawns 35 m away (Firenze, SanMarco, Siena: 0 ways). Per hunter group (1 or 2)
  every AC2MP map gets 3-4 ways, so E3 should call `ways_for` with the attacking team's spawn group.
- San Donato (tests/vip_map.rs): all groups 3 ways 155-183 m; hunters 1 / 2: 4 ways 155-217 m each, 6-9 checkpoints,
  starts 36-105 m from hunters, walked at 1.4 m/s inside 1.25x length/pace + 5 s. `acbmp ways` lists them.

## Escort VIP bodies, E3 (2026-10-28, escort-vips)
- vip.rs: `Vips` cycle in ticks (TIME_BEFORE_RESPAWN_ALL 5 s, HUNTERS_DELAY 10 s, both GBrick_SpawnVIPs defaults);
  `choose_way` (ChooseBestVipWay, our reading: nearest start >= 35 m from every hunter, else farthest); `WaySet` holds
  each way and its reverse: a VIP at its way's end turns round (our choice, retail end-of-way behaviour unknown).
- World::escort(archive, builtins, Teams, config, spawns) adds NB_VIPS bodies (lookalikes of loaded personas), builds
  ways per team (avoid TeamIndex team+1 spawns), Match.vips. World::step: a hunter kill that scripts a VIP body is
  recorded, then step_vips (kills, timers, spawns, walk at Crowd::pace) keeps Teams::vips = targetable bodies;
  dead/not-yet-targetable VIPs are Kind::Escorted (Relation::Other for everyone). First spawn: targets at once.
- tests/escort_map.rs (San Donato): VIPs walk 8.0 / 9.5 m in 5 s, a running hunter kills both via the walk-in,
  respawn exactly 300 ticks after the second kill (42 / 63 m from the killer), Targetable 600 ticks later.
  ACBMP_TRACE=1 prints `vip:` events and per-second VIP positions.
- Open: kill from a standing stop behind a walking VIP times out (walk-in at walk pace never closes); no net sync of
  Vips (E5); no attacking-team swap/score (E4); VIP skins/reaction pack not modelled.

## Escort scoring and rounds, E4 (2026-10-28, escort-score)
- rounds.rs: `Rounds` (intro/play/recap in ticks, round, phase, team_score, round_score), `RoundEvent {Phase, Swap,
  Over}`, `checkpoint_points(n)` = min(n*10, 300) and `protect_points(n)` = min(200 + n*10, 500) (GBrick_ScoreVIP
  defaults, GetScoreVIPCheckpoint / GetScoreProtectVIP), VIP_KILLED_POINTS / ENEMY_KILLED_POINTS 100, ROUNDS 2,
  INTRO_DURATION / RECAP_DURATION 8 s (GBrick_Round defaults).
- PLACEHOLDERS (not in shipped data; need a retail capture): ROUND_LENGTH 300 s (Match::escort uses config.time_limit
  per round), what `n` counts (ours: checkpoints that VIP passed), that WayEnd pays protect_points, checkpoint/protect
  points go to the protecting team only (no personal share), totals summed over both rounds (CumulativeScores unclear).
- Match: `rounds: Option<Rounds>` (Escort only), `award` (personal + team while playing), `on_vip_events` (Killed ->
  killer, Checkpoint / WayEnd -> protecting team, `MatchEvent::TeamScore`), `tick_rounds` (no time/score limit in
  Escort; Play start resets `Vips` so each round starts a fresh cycle on the new attacker's ways; Swap flips
  `Teams::attacking_team`; Over sets `over`). Intro/recap: relation Other for players and VIPs (no kills), VIPs do not
  spawn or walk. `VipEvent::WayEnd` gained `n`. Duel path unchanged (`rounds` None).
- Gate: mode::tests::escort_round_pair_scores_and_swaps (team totals 210 / 450, swap at 6 s + 1 tick, winner team 1);
  escort_map: hunter 0 / team 0 = 200 after two VIP kills. Not done: players re-placed at team spawns at round start,
  ResetAbilityCD (no abilities), round end on VIP escape, net (E5), HUD (E6).

## Human state machine part 4 (2026-10-28, phase3-human-d)
- Fall-off-edge clearance (retail `TestIsEdgeValidForFall` 00fc38ca -> `TestFallOffEdgeClearance` 00d01166): upright box 0.5 m past the edge, centre 0.7 m up, half 0.25/0.3/1.0; blocked = the move is refused (stay on the edge). `Ground::box_clear` (exact triangle/box SAT), `human::ground::fall_box`. No gate exercises a blocked edge.
- StepOff (00cef704, push 00d129e8: 0.4 m out, 0.4 m up, 0.2 s; clearance ray down >= 0.8 m): only contacts with collision material type 2 (no-stand) step off; default material never does. Our collision has no materials, so `human::in_air::step_off` is a pure decision, not wired.
- ForwardFromIdle (00f6720c): freerun jumps may leave from standing (or a start action < 0.75), no 1.5 m/s needed. Blocked at a wall counts as standing (speed 0), so the parapet gate jumps at once; its stick release moved to 0.78 s (ending -10.09 -0.36 6.59, on the top).
- JumpImpulsion exits after 0.2 s anticipation (00f80e88; 00f3159c/00f31634/00fddfdc/00fde04a): `human::jump::impulsion_exit`; unreachable while there is no standing jump.

## Escort on the network, E5 (2026-10-28, phase4-escort-net)
- Proto v7. `Start.escort: Option<EscortRules{team per slot, attacking, intro, recap}>` (play length =
  `MatchConfig::time_limit`); `GameState.escort: Option<EscortState>`: round, phase, ticks left, attacking
  team, team totals, VIP respawn/target timers, spawn count, per VIP slot alive/targetable/back/way/
  checkpoints/next waypoint/pos (f32). Events 16..24: TeamScore, RoundPhase, RoundSwap, EscortOver,
  VipSpawned, VipTargetable, VipKilled, VipCheckpoint, VipWayEnd (VIP events read from `Vips::events`).
- Clients build the same Escort world (`game::load_match`), so the VIP cycle is simulated, not sent; the
  state is compared (match desync) and the server's wins (`apply_escort`; a VIP the server has alive and
  we do not is not recreated, its walk state is not fully on the wire: yaw, way pool).
- Team spawns (our reading): team t's k-th player on the k-th spawn of TeamIndex t + 1, at `World::escort`
  (intro) and on every round's Play start (`World::place_teams`); same group VIP ways avoid when t hunts.
- `acbmp serve --mode escort [--teams 0,1,0,1] [--attacking 0] [--intro 8] [--recap 8]`.
  Viewer test input `vip` (hunter runs at nearest targetable VIP with kill). `tools/escort.sh`: 2v2,
  5 % loss, 75 ms each way: 3 VIP kills, each once (server and all clients), swap after round 1,
  0 desync / 0 match desync on 4 clients.
- Open: Escort HUD (E6), respawn near own VIP / far for hunters (GetBestSpawnPoint_VIPrule), round length.

## Escort HUD, E6 (2026-10-28, phase4-escort-hud)
- `Client::hud` branches on `Mode::Escort` + `Rounds` into `escort_hud`: `team T hunter|protector`, respawn
  timer or nearest VIP distance (hunters: targetable `Teams::vips`; protectors: any living `Vips` slot; else
  `VIPs in N s` / `VIPs targetable in N s` / `no VIP`), health, team totals (own bracketed),
  `round R/N phase m:ss` (Rounds::left), last message. Duel path untouched (duel.sh output unchanged).
  The window title uses the same line, so no duel clock in Escort any more.
- VIP highlight: `Scene::set_skinned_tint(handle, Some(rgb))` adds the colour to ambient cube and sun colour.
  Retail character programs light in the PS, so each skinned draw got a second bind group with its own PS
  frame buffer (`SkinnedConsts::tinted`), written only while tinted; no new pipeline. Viewer tints living
  VIP bodies (`living_vips`) with `VIP_TINT` (0.45, 0.32, 0) = gold.
- tools/escort.sh prints 3 `hud tick .*round` lines and the final `hud:` per client.

## Escort HUD on screen (2026-10-28, phase4-escort-hud-text)
- crates/acb-viewer/src/hud.rs: `ClientNet::hud` line drawn every frame at the top of the screen. 5x7 bitmap
  font baked in code (uppercase, digits, punctuation; lowercase drawn uppercase, unknown `?`), one quad per
  lit horizontal run, black 55% backing per line, fields wrapped on ` | `, scale = round(width/640) clamped 1..4.
- Hunters (`vip_chase` = attacking team, alive): ring + gold arrow under the text, bearing = VIP yaw minus
  `orbit.yaw` (CCW positive = left, so screen dir (-sin b, -cos b)).
- One untextured vertex-colour pipeline, no bind groups, LoadOp::Load pass after `Renderer::render`; colours
  sRGB, converted to linear on upload for sRGB surfaces. Captures use the same triangles rasterised on the CPU
  into the `render_to_rgba` buffer. Render-only: gates unchanged at 60/144.

## Human state machine part 5 (2026-10-28, phase3-human-e)
- InAir->HayStack (`CheckForHayStacks` 01319100): air entry on contact or when the feet..feet+1.8 m segment crosses the detection box (`DetectHayStack` 01318f9a), rising or falling: `human::haystack::air_enters`. (Ignore timer, +0x3b0 veto, leap ray, deep target: see "Phase 3 leftovers".)
- InAir->NarrowObject (`CheckForDefaultGrasp` 0132017a, `GetBeamAnchor` 0130fb08, box half 0.55 side / 0.4 sight / 0.35 up at +0.15): `Ground::beam_anchor`, gated by the 2 m grasp rule and a 0.3 m drop (ours). Drop at 38.25,-54.6,2.2 catches the railing at 38.60 -54.60 1.01 (60/144 fps). Pilotis and TargetedLedgeGrab beam landing: see "Phase 3 leftovers"; retail probe point offset unknown (ours feet).
- StepOff wired: CollisionMaterial layout (FastLoad 00344b40 = reflected order) +8 SoundType, +0xc GraphicalType, +0x10 AIType, +0x14 Friction, +0x18 Restitution. AIType 2 = `AIType_StepOff`. `Archive::material_ai_type`, `Ground` per-triangle no_stand, `Ground::step_contacts` (feet sphere r 0.4), fall branch after the beam check. San Donato: only `Restricted_BounceOff_WoodSolid_NoCam`, 11 triangles at ~14.9 22.1 3.0-3.7; a drop there steps off along (-0.57, 0.82) and lands below (60/144 fps). Clearance ray from our feet (retail origin unverified).

## Escort rules, E7 (2026-10-28, phase4-escort-rules)
- Respawn (retail, confirmed): `GetBestSpawnPoint_VIPrule` 0x011750e4 (rule 6 of `Server_GetBestSpawnPoint`
  0x011770c4) -> `mode::vip_rule` + `Match::pick_spawn` in Escort. Per free spawn: nearest enemy and nearest mate
  (2000 m when none, `BuildSpawnPointListFromEntities` 0x01174c46); `SortByVIPdistance` 0x0110fad8: if either spawn
  has an enemy within 35 m (`VIP_RULE_ENEMY_RADIUS`), the one further from enemies first, else the one nearer to
  mates. Entities: living other players (teammates mates, other team enemies, `AddPlayersInEntityInfo` 0x0116f1fc);
  VIPs (`AddVIPsInEntityInfo` 0x0116f55c) are mates when the player's team is GameInfo+0x514, which
  `IsAProtector` 0x0113c264 shows is the protecting team, else enemies. Spawns already taken the same tick are
  skipped (retail IsCurrentlyUsedForSpawning). Ours: linear scan for the first instead of std::sort (ties: lowest
  index). PLACEHOLDER: the SpawnType asked for (XML): we use the standard spawns as before; type 2 would limit it to
  the own team group. Duel respawn unchanged. escort.sh unchanged ([270, 130]): its respawns all fall on round 2's
  play start, where `place_teams` re-places everyone.
- Round end on VIP escape: PLACEHOLDER (`rounds::ROUND_ENDS_ON_ESCAPE`). No caller in the binary sets a round victory
  condition (GBrick_Round::Update 0x00fa3d52 passes 0; ConvertToMPEngineMessageType 0x00f2e514 maps conditions 1..10
  to MPEMT 6..15, WIN/LOSE_VIPEscaped among them); SpawnVIPs::Update has no way-end case, and
  `Quazal::VIPdata::OnUpdate` 0x0125c33a searches the waypoint modulo the way length (retail ways may be loops).
  Ours: a VIP's WayEnd still pays protect_points to the protectors, then `Rounds::escape` ends the play on the next
  tick (`RoundEvent::Escaped {round, team}`, net event 25 VipEscaped, shown to every client). escort.sh rounds are
  too short for a VIP to reach a way's end (155+ m at 1.4 m/s).
- VIP re-create (retail model confirmed): VIPdata::OnUpdate re-adds a VIP the master's replicated data has and the
  local list lacks (and removes one it has dropped). Ours: proto v8 adds `VipState::yaw` and `EscortState::pool`;
  `game::apply_escort` re-creates a missing VIP slot from the snapshot (way, back, checkpoints, waypoint, pos, yaw;
  walk phase our spread) and `revive_vips` puts living VIP bodies that are still scripted (lost or mispredicted kill)
  back on their feet; `ClientStats::vip_recreated`, log `net: VIP re-created`. Test game::lost_vip_is_recreated.
- Noted, not changed: VIPdata::OnUpdate sends the checkpoint score (U2A_NotifySpecialScore) to every member of the
  protecting team, i.e. retail gives each protector a personal share (E4 placeholder says team only).

## Assassinate RE, Phase 4 prep (2026-10-28, phase4-assassinate-spec)
Full findings, a values table (C = confirmed with address, R = our reading, P = placeholder), and the
retail-vs-duel table are in `~/.cache/acbmp-re/findings/mode-assassinate.md`. The repo has no findings/ dir.
- Engine name "assassinate": `DataPC.forge` 0xebec9a91 UnlockableGameMode (PlayNowMinimumLevel 5) and
  0x71e41bec GamemodeParameters. The parameters are identical to Wanted2 except the name, so kill reach 2.5/8 m
  and the threat meter carry over. Selection is by name through `GameModeManager::SelectCurrentGameMode(char
  const*)` 0x00cabbac. The bricks come from `game:multi\gamemode%s_%s.xml`, which is not shipped.
- No map has a `gamemode_assassinate` data block, and there is no AdditionalWorldData_Assassinate instance, so
  Assassinate needs no map objects and uses the standard spawns (R).
- Candidate target rule (R): `GBrick_ContractInitiator` (0x012d8f7a defaults, Update 0x0130d708). A player
  acquires a target by looking at it for SelectingTime 3 s (0 s when the target is in High Profile). The target
  is warned (blink), and NetPlayer SelectingEntity is replicated. After a focus change an acquired target is kept
  for ChangeTargetFocusDelay 2 s. Other values: OutOfLosTime 1 s, BlinkAfterAcquiredTime 3 s.
  - The alternative `GBrick_SelectTargetSimpleLoop` 0x016d4d68 is a single ring (each player hunts the next) and
    fits dual/metadual (R).
  - `GBrick_SimpleRadar::GetRadarRange` 0x013064f8 gives blip scale 1.0 under 6 m, then a falloff curve with a
    floor of 0.075.
  - BloodBath: a chain of 3 kills within 10 s.
- Scores: ScoreKill 100, NPC kill 0 (UI strings say an innocent kill breaks the contract, R), stun 200, escape
  100, Wanted bonuses. Round length, respawn delay and start delay are P (XML missing): we keep the duel's
  300/5/3 s.
- Already matching in our duel: kill and stun validity, reach, bands, threat meter, chase/escape, kill 100 /
  stun 200 / escape 100, contract loss on death or stun, spawns.
- Differs: how targets are acquired (lottery vs looking at a player), the compass (retail probably uses a radar,
  R), the wrong-kill contract loss, and the missing kill bonuses.
- A-steps (each needs its gate green before the next):
  - A0 rules-from-data: decode CompressedLocalizationData (DataPC 0x176ce0f5) for UIStrings 0xebec9a93..98, the
    official mode description. Gate: the text quoted here and the R items confirmed or dropped.
  - A1 `Mode::Assassinate` skeleton plus `serve --mode assassinate`. Gate: acb-game tests green, duel.sh output
    unchanged, every player kill refused before any acquisition.
  - A2 acquisition in rules.rs: 180 ticks (0 in High Profile), 120-tick memory, acquired -> Target/Pursuer
    relation. Gate: unit tests with exact ticks (a kill at tick 179 is refused, at 180 granted).
  - A3 contract events: death fulfils or loses, a stun by the target loses, an NPC kill loses (R). Gate:
    per-event unit tests, and the prediction replay matches.
  - A4 the missing kill bonuses (Silent, Unspotted, FirstBlood, Revenge, Overkill 10 s, Incognito, Proximity) and
    a BloodBath message. Gate: a table test against the 0x0103832c defaults.
  - A5 net proto bump: acquiring and acquired state in GameState, plus Acquiring/Acquired/AcquireLost events.
    Gate: 4 clients at 5% loss / 150 ms, acquire then kill scores once, a kill before acquiring is rolled back,
    0 desyncs.
  - A6 HUD: acquire progress, a "being acquired" blink, the radar curve; README Playtesting. Gate: the --capture
    hud line.

## Phase 3 leftovers (2026-10-28, phase3-leftovers)
- InAir->HayStack prologue (`CheckForHayStacks` 01319100): ignore timer (`GetHayStackIgnoreTimer` 00ec45f0, InAirData +0x228) lazily starts at the clock's now and no other code in the Mac build writes its end (objdump scan of +0x230/+0x234), so it never blocks: `human::haystack::ignore_blocks`, ours never armed. InAirData +0x3b0 == 9 is the respawn drop (`BhvAssassin::PostRespawn` 010dad1b, `Human::PlayerLeaving`): `Air::respawn` (set by `Player::new`, cleared on the ground) vetoes air haystack entry.
- The prev-pos ray branch runs only in a leap of faith (HumanInAir vtable +0x88 = `IsInLeapOfFaith` 012e64a0): ray from the previous position along the step, 3x its length (none <= 0.0005), layer 5; a hit plus `IsNearDeepHayStackTarget` (0130aa50: |z - target z| < 2.0, target HumanInAir +0x420) enters. Ours (no leap of faith): a targeted jump aimed into a haystack box (`hide::deep_target`) enters it when its next step's ray crosses the box (`hide::leap_entry`); other targeted jumps still land first. Contact/box fallbacks to the deep target not ported (no deep target outside flights).
- Pilotis (`Human::CheckForPilotis` 00d1109a, from `CheckForDefaultGrasp` before `GetBeamAnchor`): box 0.6 side / 0.5 sight / -0.3..1.1 up, four top edges (-sight, +sight, then +-side from their midpoint), nearest edge <= 0.375 m from the probe, <= 0.7 m wide both ways, centre = mean, clearance 0.6..1.4 m (r 0.4) over it, then a 0.2 s displacement (entry type 4). `human::in_air::pilotis`, `Ground::pilotis` (posts = beams with no long axis, edges probed in 2 cm steps), fall snaps onto the centre in the beam clip set. Gate `pilotis` (-41.81,10.30,7.5 -> -41.71 10.61 6.90, 60/144). San Donato has many post-like tops (188 by a scan); retail origin is a gap point at hand height (ours feet), retail also requires no valid freestep target and a gap ahead (Human vtable +0x2c0, 0.5): not ported.
- TargetedLedgeGrab arrival (0131c1e4, < 0.01 m): dispatch on InAirData +0x2d0 foothold type: hang 1/0x20000, ladder 0x2000, beam 0x100 (NarrowObject entry 8, landing clip by `GetFrontFoot` 00cbb628: left if the left foot is ahead), 0x200 unknown. `human::in_air::arrival`, `beam_landing_left`; ours traces the beam arrival, behaviour unchanged (no beam landing clips, no foot bones read).
- Pilotis cost broke escort (merge with master): the scan ran the full `Ground::beam` post test on every floor point of its 5 cm grid (up to 525) each falling tick: ~1 ms average, up to 17 ms per call in escort, and a client's prediction replays every falling tick after each snapshot. Clients sent inputs late (50-110 late per client instead of ~10, once 3645), the server repeated old inputs, and the bots' outcome turned on timing: [270, 110] (VIP 1 killed by player 2 at tick 1638 instead of player 0 at 1878) or [290, 10] with 1103 corrections, instead of master's [270, 130]. No catch, haystack veto or beam landing happened in those runs (traced). Fix, same answers (sample hash and gates identical): a candidate's cheap tests first (`Ground::beam_top`: not a wide top; the edges, 0.70 m probe reach, width), the full post test last; `beam` stops a width probe at NARROW (only narrower widths decide); `Ground::pilotis` keeps its last 256 answers by exact inputs, so replays are free. Random-point bench 514 -> 117 us, escort late inputs back to 0-21, 0 corrections.

## Assassinate A0: localization (2026-10-28, phase4-assassinate-a0)
- Decoder: `anvil_forge::loc` (format in its module doc: big-endian; a pair grammar of u16 nodes, sections of
  string tables, 1/2/3-byte node codes; read from CompressedLocalizationData::DecodeHeader 0x00326e60 and
  GetLocalizedStringRaw 0x0033f140). `Archive::localization(package)` in acb-world, CLI `acbmp loc DataPC.forge
  [0x<id>|text]`. `UIString.OasisLineID` is the string id. English: 10703 strings. Tests: loc unit test,
  acb-world/tests/localization.rs (map). No dump is committed; regenerate with `acbmp loc`.
- GameMode Assassinate: Name 0x4bd31, Description 0x4e102 (shipped). The text says FFA 6-8 players, 10 minutes,
  "Potential targets are marked on your compass", "To secure a contract, you must lock on a target".
  Unreferenced drafts 0x4dbc4/0x4dea8/0x4deac: acquiring a contract "takes time and warns the target", the target
  may stun the acquirer meanwhile, "Don't rush, or the contract will be filled instantly" (= 0 s in High
  Profile), and killing an Informer NPC reveals a target (no Informer in the shipped text). The drafts disagree on
  who is immune during the acquisition (0x4dbc4: the target cannot kill the acquirer; 0x4dea8: the target cannot
  be killed).
- Verdicts on the spec's R items: acquire-by-lock confirmed (0x4e102), compass kept (radar reading
  contradicted, 0x4e102), round length 600 s (not the duel's 300), the 0xED5A0706 flag marks DLC modes
  (0x4e053: Alhambra DLC = Escort + Assassinate; vip also has it). Wrong-kill contract loss: no text; 0x4e125
  "Civilian kills give +#ScoreOrBonus# points to each of your opponents" (mode not named). Pursuer cap, focus
  memory 2 s, spawns: no text, still open. "RELEASE YOUR CONTRACT" 0x4e260 = CancelWithY.
- Escort (teamvip): Description 0x4bd3d: hunters "can only stun the PROTECTORS"; protectors "score each time a VIP
  reaches a Checkpoint" (team wording; does not settle team vs per-protector, so the E7 note stands). No escape
  round end in the text, but round banners exist: 0x4be8e all VIPs killed, 0x4be8f protectors killed all hunters,
  0x4be90 "DEFENDERS HAVE PROTECTED THE VIP". The escape placeholder stays; 0x4be90 fits an escape or a time-out.

## Assassinate A1-A2 (+A3) (2026-10-29, phase4-assassinate-a1)
- `Mode::Assassinate` (mode.rs) beside Duel/Escort, `Match::assassinate`, `acbmp serve --mode assassinate`
  (`--time-limit` default 600 s = `ASSASSINATE_TIME`, C: "10 minutes" UIString 0x4e102). Free-for-all: no lottery
  (`Contracts::update` runs only in Duel); `hunters[h].target` is set only by acquisition, so every kill is refused
  (`Reject::NotTarget`) until the killer acquired its victim. Chase/threat/escape, kill/stun scores, respawns and
  spawns are the duel's (spec section 6).
- Acquisition (`rules::Contracts::acquire`, `rules::Acquire`, `rules::looked_at`), every tick after the start delay:
  - candidate = living other in the camera view (`PlayerInput::cam_yaw`, now `Presence::look`), nearest the view
    centre, ties lowest index. Cone 20 deg (`LOOK_CONE_COS`) and range 30 m (`LOOK_RANGE` = compass DetectionRange):
    P (retail uses the camera lock/highlight through 0x0130d596, not read). LOS = the match's head-to-head ray.
  - `ACQUIRE_TICKS` 180 = SelectingTime 3.0 s (C 0x012d8f7a +0x86c); 0 when the candidate runs (`Presence::running`
    = High Profile, C `IsTargetInHighProfile` 0x012e0a66, text draft 0x4dbc4) or is in memory. Look counts ticks in
    a row; at 180 the candidate becomes the target (`ContractEvent::Assigned`). Tests: tick 179 refused, 180 granted.
  - `MEMORY_TICKS` 120 = ChangeTargetFocusDelay 2.0 s (C 0x012d8f7a +0x894 / 0x012f7c2e). An acquired player out of
    focus is remembered 120 ticks (counted from the first tick away); back in focus within that = still held / at
    once; at 120 the held contract is `Lost`. Reading R: retail's list drops entries after 2 s; that the *contract*
    also drops then is ours (CancelTargetDelay 0 / OutOfLosTime 1 s meanings not traced).
  - Warning: `MatchEvent::Acquire(AcquireEvent::Acquiring|Stopped)` (net events 26/27), `Match::acquirers(p)`; HUD
    "being acquired by pN" (retail blink, C 0x012f59ae/0x012f5b30). While acquiring, the target sees the acquirer as
    Pursuer (may stun it; draft text 0x4dbc4/0x4dea8, R). Nobody is immune during acquisition (drafts disagree: open).
  - Compass: `Match::potential_targets(p)` = every other living player (0x4e102 "Potential targets are marked on your
    compass"; which players retail marks is R); HUD text only ("compass p1 12 m").
- A3 events (tests `assassinate_event_*`): target death fulfils the killer's contract and loses everybody else's
  (and is forgotten by every acquisition); a stun by the target (or an escape) loses the contract and resets the
  acquisition; a crowd NPC kill loses the killer's contract (`NPC_KILL_LOSES_CONTRACT`, P: strings only; 0x4e125
  "civilian kills give points to each of your opponents" not ported, mode unnamed). `World::step` reports NPC kills
  to `Match::on_npc_kill` (no effect outside Assassinate). Contract cooldown (MinWait 5 s) is ignored here (P).
- Net proto v9: `Start.assassinate`, `GameState.acquire` (focus, look, memory per player), events Acquiring /
  AcquireStopped. `tools/assassinate.sh` (port 7952, duel inputs): both acquire at 180 (start delay), B acquires
  running A at once (~tick 256), A acquires B at 359, A kills at 408: a 100, b 0, 0 desync.
- Gates on this branch: duel.sh a 100 b 0 0 desync; escort.sh [270, 130] 3 VIP kills 0 desync; gates.sh unchanged;
  kills.sh 8 granted.
- Still placeholders: start delay 3 s, respawn 5 s, score limit 1000, look cone/range. Open: A4 kill bonuses, A5 net
  test with 4 clients at loss, A6 HUD (progress blink, compass markers on screen), lock button / CancelWithY.

## Assassinate HUD on screen (A6) (2026-10-29, phase4-assassinate-hud)
- Viewer only (`hud::Assassin`, `hud::Mark`, `assassin_widgets`; `main.rs` `assassin_hud`), same overlay pass and CPU
  capture rasteriser as the Escort HUD; text drawing moved into `hud::label`.
- Compass strip under the HUD text: +-180 deg, ahead centre, ticks at +-90 and the ends. Marks = `potential_targets(me)`
  at bearing atan2(d) - `orbit.yaw` (positive = left, same as the Escort arrow): white bar = potential, gold triangle
  = `acquire[me].focus` (not yet the target), red diamond = `hunters[me].target`.
- Acquire bar: `look / ACQUIRE_TICKS` while a focus that is not the target exists. Warning: `acquirers(me)` non-empty,
  red box "BEING ACQUIRED!" at 68% height, 2x text; blink on `(world.tick / 8) % 2` (sim tick, so 60/144 identical;
  retail blink rate not read, ours). Hidden when dead / not Assassinate.
- Visual check: serve on port 7965, two viewers (60/144) captured at 4.4/4.6 s (bar ~50%, warning) and 6 s
  (diamond centred). Gates unchanged. Open: retail widget art/placement (compass/radar), blink rate.

## Assassinate A4-A5 (2026-10-29, phase4-assassinate-a4)
- `crates/acb-game/src/bonus.rs`: values = `GBrick_ScoreKill::ReadFromXML` defaults (C 0x0103832c): Silent 200,
  Unspotted 50, Incognito 300, FirstBlood 50 (AllowFirstBlood 1), Revenge 50, Proximity 150, Overkill 200 / 10 s.
  Order (C `HandleConflictResolvedEvent` 0x0103beae): Proximity (= the conflict's "focus" flag, in-game name FOCUS),
  then exactly one of Incognito > Silent > Unspotted (Unspotted only when the killer is not chasing,
  `IsChasing`), then FirstBlood, Revenge. Overkill: `AddBonusOverkill` 0x01111d18 has no caller in the Mac build
  (xref), so retail never awards it and neither do we. Flags come from `GameInfo::ComputeReducedField`
  0x00c5b7b0 (only partly read).
- Conditions: Focus = "Stay close to a target for at least 3 seconds before killing" (C text 0x4d0cd): 180 ticks
  with the victim as the killer's nearest living player within `FOCUS_RANGE` 10 m (P: retail compares to a
  GameInfo float at +0x5c4, not found; 10 m = Wanted2 ThreatClose). Silent = "Kill your target while remaining out
  of high profile" (C text 0x4d0d2); "remaining" = no running for `SILENT_TICKS` 180 (P). Incognito: no shipped
  description; ours = the killer is not on the victim's threat meter (meter 0 or on someone else) (R). Unspotted:
  `Match::chases[killer]` is None (C order; text 0x4bd47 "DISCREET: Enter High Profile just before
  assassinating" fits). FirstBlood = first scoring kill of the match (C text 0x4bd59). Revenge = "Kill the Templar
  who killed you last before anyone else" (C text 0x4bd57): any death of X clears everyone's last_killer == X.
- Assassinate only (`Match::bonus` is `Some` only there). Retail's ScoreKill brick is shared by all modes, but the
  duel and Escort gates keep their scores (a 100; [270, 130]); porting the bonuses to the duel/Escort is open.
- BloodBath (C `GBrick_BloodBath::HandleKillEvent` 0x01594ad8, `GetBloodBath` 0x015638b8, defaults NbKills 3 /
  Duration 10 s at 0x0157093e): a memory of (killer, victim) per kill, each dropped after 600 ticks; on a kill the
  longest chain from the killer (killer -> victim, who killed -> ...) reaching 3 announces it (retail: a unique
  voice) and clears the memory. It is a chain, not one player's 3 kills. `MatchEvent::BloodBath`, no points.
- Net proto v10: `GameState.bonus` (first blood, per player quiet ticks / focus player + ticks / last killer, kill
  memory), events 28 Bonus (b = mask `1 << Bonus`; one event per kill because server events dedup by (tick, kind,
  player)) and 29 BloodBath (shown to everyone). Client message `bonus: FOCUS, INCOGNITO, FIRST BLOOD`.
- Tests: `bonus::tests::table` (situation -> bonuses -> points vs the defaults), focus/silent ticks, revenge/first
  blood, bloodbath chain and window; mode tests assassinate_* now expect the bonus points.
- A5 `tools/assassinate4.sh` (port 7952, 4 viewers, default netsim loss=5%,delay=150ms,jitter=5ms; A 60 fps, B 144,
  C/D idle on standard spawns): A's kill at tick 90 is refused ("not your target") on every client log, A
  acquires B at 359 and kills at 408: "player 0 scores 600" once on all four clients and the server (a 600, b 0,
  c 0, d 0), 0 desync / 0 match desync everywhere, PASS. The client also refuses the early kill in its own
  prediction, so there is nothing to roll back (0 rollbacks).
- The flaky tick-30 rejection (2-player script): the client starts by jumping ahead by its lead (rtt at join is
  inflated by loading, 213-481 ms = 16-32 ticks) and the viewer script is keyed to client ticks, so the 0.5 s press
  was sent for ticks the server had often already played (late inputs, "ticks without input" at the start) and
  never reached it. Both scripts now press at 1.5 s (tick 90); the rejection event is resent until acked, so a
  press that reaches the server always shows in the log.
- `tools/assassinate.sh` now gives a 600 (100 + Focus 150 + Incognito 300 + FirstBlood 50), b 0, 0 desync.
- Still open: lock button / CancelWithY "release contract" (0x4e260), A6 HUD (progress blink, compass markers),
  the focus range and Incognito meaning (trace `ComputeReducedField` arguments), bonuses outside Assassinate.

## Assassinate A7: highlight, lock, release (2026-10-29, phase4-assassinate-a7)
- Look = the player targeting pick `AssassinTask::GetBestNearbyTarget` 0x00b4f648: horizontal range
  `GlobalParameters.Dist_visibility` 9 m (C), |dz| <= 1.6 (C literal), cone cos 0.5 = 60 deg (C literal). That the
  camera highlight used by `GBrick_ContractInitiator::Update` 0x0130d708 is this pick is our reading (R; the
  highlight setter was not traced). Replaces the A1 placeholders 20 deg / 30 m.
- Line of sight: `ValidateTargetForCameraWithNavAndRaycast` 0x0130d596 casts to the target's feet + 2.0 / 1.0 /
  0.0 m (C), seen when any ray is clear; we cast from the acquirer's head, not the camera (P). A locked target
  skips the test (C, 0x0130d708).
- Lock (R key, `BUTTON_LOCK`): a press locks the highlight, a second press unlocks; the lock holds while the
  target lives and is within `LOCK_RANGE` = 30 m compass range (P, retail break distance not read).
- Release (Y key, CancelWithY, text "RELEASE YOUR CONTRACT" 0x4e260): drops the contract unless in a kill/stun
  conflict (`IsInAnyActiveConflictWithPlayer`, C) and once `CancelTargetDelay` 0 s has run (C default, 0x012d8f7a
  +0x890); a chase on it ends as an escape. `RELEASE_COOLDOWN_TICKS` 300 (5 s = Wanted MinWaitTime) before the
  released player can be acquired again (P: retail's release path sets no cooldown we found).
- Net proto v11: input buttons 11 bits (lock, release); acquisition state carries the locked player, last
  buttons and the released player + cooldown; event 30 Released (a = player, b = target).
- Gate scripts: assassinate.sh / assassinate4.sh walk a forward 2-3 s so B is inside the 9 m range before the
  acquisition; results unchanged (a 600, b 0; 4-client PASS), 0 desync.

## Escort VIP marker (2026-10-29, phase4-vip-marker)
- Retail (findings/mode-escort.md): `GBrick_VIPFeedback` ReadFromXML 0x00b7fe18 draws an arrow over each VIP,
  ArrowFarScale 0.6 at ArrowFarDistance 30 m, ArrowCloseScale 1.0 at ArrowCloseDistance 5 m; icon per viewer
  (GetTextureID(Entity*), texture not looked up). Offscreen behaviour unknown (VipAssassinFeedback has
  ShowArrowOffscreen 0, a different brick).
- Ours (crates/acb-viewer): `vip_marks` picks the VIPs as `escort_hud` does (hunter: living and in `Teams::vips`;
  else every living slot), point = interpolated body root + 2.1 m (ours), clip = `Camera::view_proj`, distance
  root to player (3D, as the HUD line). `hud::VipMark` drawn first in `hud::build` (under the text): gold diamond
  with black outline, chevron down to the head, `NNm` label; scale lerp per the retail constants.
- `hud::place`: on screen inside a 26*s px inset -> projected point; else clamped to the inset edge along the
  undivided clip x/y from the centre (keeps the side behind the camera; behind forced to the lower half, ours)
  with an outward wedge. Same overlay pass and CPU rasteriser, so captures show it. Render-only: gates unchanged
  at 60/144 (pilotis -41.71 10.61 6.90). Open: markers on the same edge spot overlap.

## Playability pass (2026-10-29, phase3-playability)
User report: sprint start stood still, no real jumps (only falls), many walls not climbable or the climb ending
inside a wall, no way off beams/ledges. Measured with `examples/parkour_sweep.rs` (every San Donato wall with a
guidance edge 1.0-4.5 m over the floor in front of it, deduplicated on a 3 m grid: 493 walls; 10 s Shift+W,
`SWEEP_LEGS=1` adds Space; 15 s for the map) and `examples/probe.rs` (holds, floor and headroom round a point).
`examples/tickbench.rs`: one player costs 0.01-0.03 ms per sim tick (idle, walk, sprint, climb, fall).
- Starts: a standing start more than 60 deg off the facing played the low profile turn on the spot and its
  settle into the wait (`Cancel::Never`): 1.3 s in place before any move, also when sprinting. Now high profile
  uses retail's `h_` turn and both continue into their moving exits (NOTES "Player control"): moving 0.4 s after
  the press (HP), 0.4 s turn + walk-out (LP). A stick released during the turn still settles.
- Pull-up (hang + up): needed the hold flagged top and floor exactly 0.7 m behind; now
  `Ground::passover_spot` tries 0.7, 0.5, 0.9, 0.35, 0.25, 0.15, 1.2 m behind the edge (floor within 0.5 m of
  the edge height, any hold), centres on narrow tops (`settle_top`) and needs headroom. `Then::Stand` keeps
  the Reference re-root only where it is clear floor at the planned height (else the planned spot). Sweep,
  Shift+W: standing on top after a climb 78 -> 308 of 493 (with the hold ranking below; Shift+Space+W 304),
  standing inside geometry 25 -> 0.
- Hold choice (ours): a hold tucked behind another object (San Donato 26.4 -13.1: a roof edge 4.00 m behind
  an eave beam 3.89 m) was grabbed with the body inside the beam, and nothing moved from there.
  `Ground::hang_clear` (nothing 0.35-0.9 m out from the hold, 0.35 m and 1 m under the hands) now ranks
  clear holds first in every hang query (wall run, fall grasp, hang target, next hold, wall jumps); a blocked
  one is still taken when nothing else is in reach (rejecting them outright lost 22 walls whose only top
  is behind a canopy). The wall run's hold may stand 0.7 m out from the wall face it hits (was 0.4 m either
  way), for eaves over recessed walls.
- Leap: replaced by retail FallOffEdge, see "Falling off an edge" below.
- Step off (ours, as NOTES "Freerun moves" already described): F at a low profile edge stop with no wall below
  to lower onto steps over the edge (to where the floor drops, up to 0.8 m out, wall and fall-box checked)
  and falls at 1.2 m/s. Before, F there did nothing, so a beam end or roof edge without a hold had no way down
  but walking back.
- Open: guidance-edge dead ends (a lintel under an overhang with no hold above) stay unclimbable; retail
  freerun up a 1.2 m wall needs Legs (Shift alone bumps), as before.

## Falling off an edge, targeted-jump ballistics (2026-11-03, phase3-fall-off-edge)
Retail never walks off an edge: `AssassinTaskGround::UpdateGroundMovement` (0166fb1c) calls
IHumanGround slot 0x137/0x138 `CanFallOffEdge(gap)` / `DoFallOffEdge(gap, f)` with f = 1.0 when its
high-profile flag (+0x1f64, the flag that also sets `SetMovementType(1)`) is set, else 0.0, when no jump target
was taken (`GetBestEdgeJumpTargetIndex` -1; the detector `GetJumpTargetListInDirection` 0122626a adds no
fallback target). IHumanGround slots, from the HumanGround vtable 02c2ab60 (IHumanGround thunk block from byte +0xbc8, entry 754):
4/6 Can/DoJumpToTarget, 8/9 Can/DoOverHeadJumpToTarget, 0xc/0xd Can/DoJumpImpulsion, 0x18/0x19
Can/DoEnterWallingVertical, 0x137/0x138 Can/DoFallOffEdge, 0x139 CanUnbalanceOnEdge, 0x13b IsInEdgeStop,
0x143 CanPullDownOnEdge, 0x22e/0x22f Can/DoEnterLookDown, 0x23b/0x23c Can/DoEnterHayStack.
- `HumanGround::ExecuteFallOffEdge` (00fd2406): angle a between the gap direction (Gap +0x20) and the facing
  (both flat). |a| >= 120 deg: backward fall action 0x935ce89e, landing-test delay 12000 ticks (0.4 s), no jump.
  Else direction = facing (|a| <= 60 deg) or the gap direction turned 60 deg toward the facing; target = feet +
  dir * (1 + 2 * HumanGround+0x904 * f) - 3 m up; `Human::DoTargetedJump(target, source 0 ground, landing
  intention 1, front foot)`. +0x904 is the move cycle's normalized anim speed (written 0.2/0.6/1.0 by the
  Exec*ToMoveCycle transitions, ramped in `UpdateMoveCycleMain` 00fc151a): our `locomotion::anim_speed`.
  So a sprint lands 3 m out, a run 2.6 m, low profile 1 m, each 3 m down: the `down 300cm` / `down 050cm`
  clip reaches.
- Ballistics after the takeoff action (`HumanInAir::UpdateMainMain` 01328022, ~l.1740-1970), only when
  InAirData +0x1fa is set. Writers of +0x1fa = 1 (disassembly scan): `Human::ComputeDeepFootHold` (leap of faith
  into a deep target), `HumanLedge::ReleaseToInAir` 00d86b02, `HumanLedge::ReleaseToTarget` 00d8cabb,
  `HumanLadder::ExecuteReleaseToTarget`; cleared by `HumanInAir::ClearTargetedJumpData`. A ground
  `DoTargetedJump` (00d058de, also the fall-off) does not set it: those jumps warp their takeoff + air clips onto
  the target (progress (t + len1) / (len1 + len2), `Action::GetTotalDisplacementMatrix`, ~l.640-1000), the
  model `jump::Flight` approximates. Ballistic: velocity = the
  character controller's (takeoff root motion) when the takeoff item finishes (vz forced to -1 for action
  0x1120e17b); then each tick, above the target's height: T = `GetDropTime(z, target z, vz, -9.81)` (012cf475:
  (vz + sqrt(vz^2 + 2 g max(z - tz, 0))) / g), a = `GetTargettingAcceleration` (01327fdd: 2 (target - pos -
  T v) / T^2, horizontal), dt' = min(dt, T), v += a dt', vz -= 9.81 dt', pos += v dt' + a dt'^2 / 2. At or
  below the target height the horizontal velocity is zeroed: straight down.
- In the warp model the target is re-read per tick (foothold entity matrix; +0x1fc: offset by half of
  +0x2b0), the correction toward it clamped to 1 m (10 m with +0x1fc).
- Ours (`human::ground::fall_off`, `human::in_air::home`, `jump::fall_off`): target, cones and back fall are
  retail; the path to the target is not: it flies the ballistic homing (retail's ledge-release model) instead
  of the clip warp, from the edge with the ground velocity (no takeoff root motion, so no rise); the takeoff
  clip of the jump kind for (reach, -3 m) plays, then the fall pose. The gap direction is the move direction.
  The homing runs inside
  the ordinary fall, so grasp, wall reception (which drops the target), haystacks, beams and landing classes
  all apply. Low profile stops at drops over 2.5 m (was 2 m, see "Low-profile edge brake"), so its 1 m fall-off is seen on 0.53-2.5 m
  drops. Replaces `jump::leap` (LEAP_MIN_SPEED 3.5 m/s, LEAP_UP 3.0 m/s) and the plain walk-off fall.
- Test: `tests/jump_map.rs` `fall_off_an_edge_by_profile` (San Donato roof edge x -13.1): sprint lands
  3.0 m out, Shift run 2.6 m, no rise. Gates, kills, duel, escort unchanged.
- Next: the hang back eject / release (`HumanLedge::ReleaseToInAir` 00d86b02, `ReleaseToTarget` 00d8cabb) are
  retail ballistic jumps: port them onto `human::in_air::home` (ours: a fixed 1.71 m/s fall off the wall, NOTES
  "Freerun moves"); `Flight` keeps the warp model (its warp shape is ours).

## Hang rebound (back eject) (2026-11-03, phase3-ledge-release)
Retail's back eject is the rebound, a ground-style targeted jump. It is not one of the ballistic ledge releases.
- Brain `AssassinTaskLedge::UpdateLedge` (012a8298, ~l.615-745, 1430-1600):
  - The stick (dead zone 0.35: none = straight away from the wall) is measured against "away from the wall".
  - With high profile (+0x1f64) and the jump press (+0x1f5e), no stick or the stick more than 120 deg from away
    first tries the climb leap up (ClimbLeap ability).
  - Then, if the stick is within 135 deg of away: inside 60 deg the jump direction is the stick, beyond that
    away turned 89 deg (0x3fc6d3f2 rad) to the stick's side. More than 135 deg (into the wall): no rebound.
  - Target: `Human::GetJumpTargetListInDirection` 00d078f8 (origin 0.15 m along the direction and 6 m down,
    search 9 m x ForwardJumpDetectionScale, jump source = Human +0x960: 0 on a hang, so the ground polygon; 3 in
    a swing, 4 in a hand pass-over), then `GetBestEdgeJumpTargetIndex` 0129cc3a. For plain floors that picks
    the highest one under 45 deg slope and no more than 3 m below.
  - No target: IHuman slot +0x78 = `Human::GetFarthestFootHoldInDirection` 00d14144 (detection type 0, no gap):
    position + dir x the detection curve's far point (slot 1, point n-2 = 8.0 m x scale), 3 m down. Swing:
    4.5 m out, 2.3 down; pass-over or type 2: 3 m.
  - Then `IHumanLedge::DoRebound` (sub-vtable 02c10cfc, slot +0x1c; CanRebound +0x18).
- `HumanLedge::ProcessReboundTransition` 00d8d890: when the preparation action ends,
  `Human::DoTargetedJumpOrFarthestJump` 00d06d30 (source 2, direction = -entity forward). A foothold whose entity
  handle (+0x5c) is invalid (positions, farthest footholds) goes straight to `DoTargetedJump`. A valid but dead
  handle falls back to the farthest foothold (Human vtable +0x1dc).
- Not the rebound: `HumanLedge::ReleaseToInAir` 00d86ac2 is a drop. It takes action 0x1120e17b on a wall hang,
  0x1120e179 when hanging free, with a 0.2 s blend, sets InAirData +0x1f4 = 2/3 and +0x190 = position + 1.1 / 2.4 m
  up (not the fall height: `ComputeFallHeight` 0130f790 reads +0xd8), and is ballistic (+0x1fa) only when given a
  foothold. `ReleaseToTarget` 00d8ca6e drops onto a target below. It blends two drop actions by
  (hands - 1 m - target z) / 16 m and sets +0x1fa.
- Ours (`human::ledge`, `Climb::plan_jump`, player.rs `Outcome::Eject`):
  - Direction and leap-first are retail. The stick is the wall-relative W/A/S/D, so no stick = straight away,
    S = away, W = into the wall (leap only), A/D = along the wall (89 deg).
  - Ours: A/D try our side jump to a hold first.
  - Ours: the target search is our freerun gap search (`jump::target` off the edge, a hang across via
    `jump::hang_target` beating a floor more than 1.75 m lower), not retail's highest-target pick.
  - A target is flown with `jump::Flight` (the warp model, on the ground jump clips: retail's
    `xx_h_rebound_{dir}_{front,up,down}_{050,300,550}cm_*_to_air` sets have the same kinds but are not loaded).
  - No target: the farthest foothold, flown like a fall off an edge (ballistic homing stand-in, starting at
    1.71 m/s off the wall).
  - Ours: none from a free hang. The 0.467 s catch block is kept.
- Probe `examples/rebounds.rs`: of 983 San Donato wall hangs, 333 rebound to a floor, 186 to a hang across and
  464 to the farthest foothold.
- Gates:
  - `eject` moves -14.86 43.99 2.25 -> -14.03 43.99 2.21: the street at the wall's foot, 2.8 m down, is now the
    rebound's target, so you land there instead of 1.28 m out.
  - New gate `rebound` (10 m hang on the west tower, F) ends at -46.79 13.18 -0.01: 8 m out, then straight
    down, 30 % fall damage.
  - Test `tests/jump_map.rs` `rebound_targets`.
- Check in Wave C: how far a retail rebound off a tall wall really goes (8 m by this reading).
- Next: `Flight` warp fidelity (PROMPT target 2).

## Flight warp (2026-11-03, phase3-ledge-release)
- Retail `Human::ComputeTargetedJumpData` (00d04b86):
  - InAirData +0x1e4 / +0x1e8 = the takeoff and air action lengths; +0x1ec / +0x1f0 are the second pair.
  - +0x50 = the start matrix; +0x90 = the air action's displacement.
  - +0xe0 = target - (start x takeoff total displacement x air total displacement), i.e. where the clips end.
  - +0x1fb = rotate toward the target's orientation.
- `HumanInAir::UpdateMainMain` (01328022, l.640-1110):
  - The root is start x the clips' displacement so far, plus progress x (+0xe0 + correction).
  - progress = t / (len1 + len2) in the takeoff and (t + len1) / (len1 + len2) in the air clip, capped at 1.
  - So the end error is spread linearly in time, on every axis, and the flight lasts exactly the clips.
  - The correction is the foothold entity's movement since takeoff (target re-read from its matrix), clamped to
    1 m per tick (10 m with +0x1fc, which also offsets the target by half of +0x2b0). San Donato has no moving
    footholds, so ours has none.
  - With +0x1fb the orientation slerps toward the target's by the action's progress.
- Ours (`jump::Flight::new`): the vertical error is now linear in time like the horizontal, with no extra fall
  time (was: ease-out up / ease-in down, plus sqrt(2h/g) when ending lower). Still ours: the ballistic arc and
  `u^lag` hold-back for jumps up onto a top (`edge`). Retail warps the up clips straight through the edge, while
  our collision checks (`Flight::sink`) need the feet over it.
- Gates unchanged (flights end on their targets). Sweep, Shift+W: identical. Shift+Space+W: 3 walls end "beam
  below top" where they ended "partway up" or "still on wall".

## Low-profile edge brake (2026-11-03, phase3-ledge-release)
- The player brain never sends `UnbalanceOnEdge`, so a player never enters Ground EdgeStop. The disassembly has
  one `DoUnbalanceOnEdge` caller (IHumanGround slot 0x13a, call *0x4e8): `GroundNavigationTask::CheckForFallOffEdge`
  01538746, the NPC navigation. The brain uses `CanUnbalanceOnEdge(gap, 0.0225)` only to gate the high-profile
  pull-down. So `IsNarrowEdge` 00fd1e26 (EdgeStop vs NarrowObject) is NPC-only too.
- What stops a walking player: `AssassinTaskGround::UpdateGroundMovement` 0166fb1c ~l.995-1070.
  - Gaps come from `Human::GetBestGapInDirectionInList` (IHuman slot 0x2f): within 0.3 m, 100 deg of the
    move, then 180 deg.
  - The brake fires when there is a gap ahead, the stick is above 0.35, button 0 is not held, the gap is deeper
    than 2.5 m (Gap +0x30), within 70 deg of the stick, and Gap +0x34 < 0.16 and > 0.1 m of something (fields
    not identified).
  - In low profile (+0x1f64 clear) it then calls `IHumanGround::SetBreakHard(true)`, slot 0x1f; the IHumanGround
    block's zero vtable entries resolve through the bind table, `llvm-objdump --macho --bind`.
  - `HumanGround::TestMoveCycleToMoveStopMain` reads the flag (HumanGroundData +0x122): the move cycle stops.
  - Facing within 70 deg of the gap starts a timer. Standing there over 0.25 s, gap > 2.5 m and the LookDown
    ability enter LookDown (slots 0x22e/0x22f).
- Otherwise the brain's `CanFallOffEdge(gap)` (FSM test 0x57, depth >= 0.53 m, no profile gate) leads to
  `DoFallOffEdge`: the 1 m hop in low profile.
- Ours: the low-profile stop moves from 2.0 m (UnbalanceOnEdge's value) to 2.5 m (`human::ground::break_hard`,
  `BREAK_HARD_DEPTH`). The pull-down with F stays at 2.0 m (`CanPullDownOnEdge`). Beam ends still stop at any
  drop (HumanNarrowObjectBeam EdgeStop). Drops of 0.53-2.5 m are now walked off in low profile.
  - Ours: no LookDown pose; the brake is our `loco.blocked`, not a MoveStop clip.
  - Ours: the gap direction is the move direction, so the 70 deg cone always holds.
- Gates, tests, kills, duel, escort unchanged.

## Phase 3 review: release, pilotis gate, beams (2026-11-03, phase3-ledge-release)
- Hang release (player brain `AssassinTaskLedge::UpdateLedge` 012a8298 ~l.760-850, IHumanLedge slots +0x28
  CanRelease, +0x2c DoRelease, +0x30/+0x34 Can/DoReleaseToTarget):
  - With the LeapOfFaith ability and a deep target below (IHuman +0x84), `DoRelease(foothold, 0x800)`: a leap
    of faith, which also sends `SendLeapOfFaithEvent` and switches the collision layer to 0x1c.
  - Otherwise `DoRelease(null, 0x80000000)`: a plain drop.
  - `ReleaseToTarget` is never called by the player brain. Our drop then catch while falling is the player
    path. A drop over a haystack enters it through the in-air haystack contact; ours has no leap-of-faith
    clip.
- Pilotis gate (`HumanInAir::CheckForDefaultGrasp` 0132017a ~l.1430-1465): retail tries `CheckForPilotis` only
  when:
  - `IsFreeStepTargetValidAndAccessible(0x90000)` is false;
  - `Human::IsNearGap(gap, 0.5)` (Human vtable +0x2c0) is true;
  - Gap +0x38 is clear;
  - and the knee sub-shape cast passed (< 20).
  The probe direction is the gap's direction, negated. Ours: not ported. Our post search already needs a post top
  within 0.375 m of the probe, and another per-tick gap query costs sim time (escort replays falls).
- NarrowObject FreeRun: freerun jumps off beam ends already come out of the ground jump search (a 0.5 m gap
  jump off the stairwell balustrade's end at 38.60 -55.07 1.01, Shift+Space+W). What remains is retail's beam
  clip sets (entry, run, jumps from a beam), which are visuals and parked.
- Slopes (`examples/slopes.rs`, San Donato collision by face angle):
  - 0-45 deg: 19822 m2; 45-53: 138 m2; 53-70: 321 m2; 70-90: 20653 m2.
  - The 45-70 deg patches are 2-2.5 m2 bevels at street level (z 0.1-0.4) and two 61 deg strips at
    -39 41..49 4.9.
  - So retail's slope class (45-70 deg, a slide) barely shows on this map. Not ported.
- Ladders: none handled (no ladder entities read). Retail `HumanLadder::ExecuteReleaseToTarget` is ballistic.

## Manhunt (catsmice) (2026-11-03, phase4-manhunt)
Full findings: `~/.cache/acbmp-re/findings/mode-manhunt.md`.
- Data: DataPC 0x9f959f8c ManHunt GamemodeParameters, engine "catsmice". Kill reach and threat meter are the same
  as Escort and Wanted2. Spawn distances differ: CatchupSpawnDistanceFromOthers 10 m, FromNoContracts 0,
  CatchupSpawnDistanceMax 60 m (catch-up spawns are not used by our team spawn rule). Rules text loc 0x4db3f: 2 teams,
  6-8 players, 2 rounds of 5 minutes, roles switch, hunters kill targets, the prey hide (more points near
  teammates) and humiliate (stun) pursuers.
- Contracts: `GameInfo::SetupTeamLoop_CatsMice` 0117f9d8 gives every hunter a killer loop on every member of the
  target team (round 1 team 0 hunts, round 2 team 1). `GBrick_SelectBestTargetCatsMice` (Start 012828ce, Update
  01263db0, SwapTargetsForLoop 01263c5c) re-adds a kill's loop after TimeWithoutTarget 3 s, through
  `AddKillerLoop` 0117eec8's pending target. The teammate switches (SwitchToTeammate, SwitchMyTeammate,
  ChangeTarget*OnNPCKill) default off.
  - Ours: `rules::TeamGame::Manhunt` and `manhunt_relation`: hunter -> enemy player = Target, prey -> hunter =
    Pursuer (stun only), teammates Other.
  - The 3 s pending loop is moot while the respawn delay (5 s, placeholder) is longer; not modelled.
- Hide bonus `GBrick_BlendedScore` (ReadFromXML 012d1822 defaults):
  - StartInterval 1 s, TimeInterval 5 s, CooldownInterval 7 s, Radius 20 m.
  - BlendedBonus 10 / 20 / 30 / 50 by the number of hidden prey together (`UpdateHiddenTargets` 0130f0d2).
  - "Hidden" (`ComputePBHIForPlayer` 011864af, +0x8d) = blended in a crowd group, a hiding spot, or Disguise.
  - Ours: `manhunt::Hide` (state machine from `Update` 0130f28e, `AwardBonus` 012fd80a, `StopBonus` 012ffad6).
    `Match::hide_tick` runs while playing, for a living prey, `Presence.hidden || blended`, not in a kill or stun.
    The timer counts whole sim ticks and rides in `GameState.hide` (state, ticks) so a rollback restores it.
- Ours, the plumbing: Manhunt reuses Escort's machinery (`Mode::Escort(Teams)` with `Teams.game`, rounds, team
  scores, team spawn groups, net team state); `World::manhunt` makes no VIPs. Wire: the Start escort byte is 2
  (proto v12). Server `--mode manhunt`; HUD role "prey" and the nearest enemy's distance. Result line
  `manhunt result ...`.
- [R], ours, our reading: the brick list did not ship. Kills score `KILL_POINTS` with the acrobatic and haystack
  bonuses, stuns `STUN_POINTS` (ScoreChase humiliation); escape scoring is not wired (no single-target chase).
  Kill bonuses: the Assassinate set (`bonus::Tracker`; loc 0x4db3f "execute quality kills"), without Incognito
  (ours: team modes have no threat meter); `tools/manhunt.sh` round 1: 4 kills, teams [800, 0].
  Round length 300 s matches the text. Kill bonuses beyond those (retail ScoreKill types) are not ported.
- Tests: `manhunt::tests` (bonus timing, mates), `mode::tests::manhunt_hunters_kill_prey_hide_and_stun`;
  `tools/manhunt.sh`: 2 rounds, roles swap, 0 desync on 4 clients, round 1 scores 4 kills (teams [400, 0]); its
  `vip` bot follows the navmesh (`Nav::path`, `World::manhunt` loads the crowd) to the nearest prey.

## Crowd of every persona, 60 by default (2026-11-04, phase4-crowd)
- Characters: the crowd now uses `crowd_characters`, every `AC2MP_IDxx_MainTemplate_NPC` builder in DataPC (14:
  ID07, 08, 10, 14, 15, 17, 18, 20, 22-27), sorted by name and assigned round-robin, instead of the players'
  `--character` list. All 14 load (+1.9 s startup with 60 NPCs). Retail ties the crowd to the players' skins
  (`GBrick_PlayerReady::SetSkinsAndCrowdReadyCallback` 013c2ed8, `SkinInCrowdInfo`; its builder has no direct
  caller in the Mac build, so it is probably filled from the network). Ours: every persona, not only the players'.
- Count: `acb_game::DEFAULT_CROWD` = 60 for the viewer and `acbmp serve`. PLACEHOLDER: retail's count is not
  traced. Sim cost (`tickbench MAP [CROWD]`): 0.38 ms per tick with 60, 0.76 with 120 (0.02 with none).
  4-client escort on one PC: 30 NPCs normal, 60 raises rtt to 300+ ms (five renderers on one machine), 0 desync
  either way.
- The crowd nudges the player, e.g. the tickbench climb ends at 44.23 instead of 43.99. So gates.sh, kills.sh,
  duel.sh, escort.sh and assassinate(4).sh pin `--crowd 0` to keep their baselines. `acb-net/tests/crowd_map.rs`
  plays 10 s (was 7) because 14 characters load per peer.

## Movement feedback (2026-11-04, phase3-movement-feedback)

Nine reports from the user's playtest of master. Where this overrides a retail value it is on purpose: the user plays
the real game, and these are their calls on how it feels. All of it is "Ours" unless marked retail. Open question for
Wave C: traces could show how retail gets its instant feel with the ramp below (a guess: root motion and the ramp don't
mean what the port took them to mean).

Measured: `examples/parkour_sweep.rs` on San Donato against a build of master, `[stood ...]` labels (`W` stays held for
10 s, so the end state says little now that the character keeps going):
| | master (493 walls) | now (483) |
|---|---|---|
| stood on top | 308 | 345 |
| never stood | 157 | 98 |
| never climbed | 72 | 21 |
`SWEEP_ANGLE=D` (new) comes at each wall D degrees off square; never climbed 67 / 85 / 56 / 97 of 455 at 0 / 30 / 45 / 60
before the mount fix below (not re-measured after it). Eight walls that used to stand no longer do and ten have left
the sweep with their holds: not looked at one by one.

1. **Leap off a building.** A freerun (Legs) off an edge with no target used FallOffEdge, 3 m out at most. Now
   `jump::leap`: `human::ledge::farthest` (retail `GetFarthestFootHoldInDirection` 00d14144, 8 m out, 3 m down), a
   ballistic arc rising at 3 m/s (0.46 m; ours), which keeps going past the target's height. The caller from a ground
   edge is our reading: only the rebound's use of it was read in the binary. No Legs: FallOffEdge as before.
2. **Feet, not a hang, across a gap.** `jump::hang_target`: a hold with floor on top no higher than the polygon's 1.3 m
   flies to the spot behind it (`feet_on_top`) instead of hanging. In the air `Ground::top_hold`: moving 2.5 m/s or more
   and running up against a top edge, feet below it by up to 2.2 m, it hops on top (`FEET_ON_TOP_SPEED`), before the wall
   reception can turn the character round. A soft landing with the stick held goes straight into the cycle.
3. **No wind-up.** `Locomotion::play_cycle` takes the gait's speed the tick it's asked for, and standing it faces the
   stick at once. Retail's `ramp_anim_speed` (still there, tested, not driving anything), start, stop and on-the-spot turn
   clips are gone: they were the wind-up (0.8 s to a run, 0.73 s start clip, 0.4 s turn). The cross-fade smooths the pose
   only. Kept: the run turn (a clip of its own, not reported); hard and damage landings. `Locomotion::blocked` now stops
   and arms a one-tick `stalled`: a stopped cycle has no root motion to test, so `player.rs` tries the stick's way
   (`STALL_PROBE`) to tell a move that's still refused from one that's free again.
4. **Pull-up.** Top-outs played `hangwall_tr_passover` (retail's vault, `HumanLedge` HandPassOver). Now retail's Pullup
   clips: `hangwall_tr_hangknee_a/b` or `hangfree_tr_hangwaist_a/b` + `hangwaist_tr_hangknee`, then
   `hangknee_tr_freestep_entry`. Retail still vaults some thin tops (`CheckForPassover` 00d984cc, not read); we don't.
5. **Swing.** Guidance sub type 4 is `GuidanceObjectSubTypePole` (schema enum; 26 edges on San Donato, thin bars in pairs
   0.1 m apart, one per side). Retail's start test wants both hands on sub type 4 (+0xc30, +0xc34 == 4 in 00da2a0e; that
   those fields are the sub type is our reading). They used to be plain holds. Now `Hold::bar`; `jump::bar_target` beats
   any other freerun target, a catch on one swings (`Phase::Swing`, the four `xx_h_swing_cycle_*` clips, entity fixed at
   the free hang). W at the forward peak: `Outcome::SwingJump`, the floor in reach or a leap. S or two cycles: the
   `swing_stop` clips into the hang. Starts at the forward peak so a held stick flies straight through. Ours: the exit
   timing, `SWING_ROOM`, and the plain jump clips (retail has `xx_h_air_*_to_swing` and `swing_cycle_*_to_air`, whose root
   motion doesn't fit our entity yet). `examples/poles.rs` lists poles and spots to run at them from.
6. **Holds that aren't.** Guidance marks every edge where a wall meets another face. Of 3655 wall holds, audited with
   `examples/guidance_kinds.rs` on the first version of the tests: 399 at a wall's foot (concave), 901 with no collision
   at them, 123 with a surface over 0.35 m proud (buried: a body there is inside it), 36 undersides. `Ground::load` now
   drops them (`Ground::hold_fault`, not bars; a long edge stays if good at a quarter, the middle or three quarters).
   Concave wants floor outward and wall above the edge: floor alone also matched roof edges with a lip. The straight-on
   mount was using concave edges as foot holds, so `climb_entry` now takes the floor at the wall's foot for one, which is
   where most of the never climbed drop came from. Retail's real test is `ValidateAnchorPoints`, not read; the user should
   say if perches remain. `examples/wallprobe.rs X Y Z YAW` prints what the entry sees at a spot.
7. **Drop button.** F did three different things, none of them reliably letting go. On a wall a fresh press with no stick
   now releases at once in any phase (`Outcome::Release`; F + stick stays the wall jumps, and F alone no longer leaps
   or rebounds). A held F doesn't count (`Player::drop_held`), or lowering onto a wall would let go on its next tick. The
   fall grabs nothing (`RELEASE_NO_CATCH`, cleared on the ground). On foot no edge stop is needed any more, nor low
   profile: `drop_off` tries the stick's way, or standing all four ways. Separately `Outcome::Drop` (S) never set
   `catch_block` and re-caught the hold it had just left; it now has the eject's 0.47 s (gate `climb` no longer catches
   the awning beam below as a result: retail's grasp would, see Wave C).
8. **Instant climbing.** `speed >= 1.5` for a wall run fought `blocked()` (the ramp could never win once against the
   wall): gone. A mount plays in `MOUNT_TIME` 0.2 s (retail clip 0.87 s, at rate) and its 0.3 s settle is gone. A wall
   run from under 0.7 m skips the 0.27 s run-up clip. Instant speed took away the delay that used to let a mount win over
   a wall run, so the order is now: wall run with a target, mount, empty wall run.
9. **Wider angles.** My first suspicion was wrong: the 20-29 deg wall-run cone is what retail does (`angle < 1 - curve`,
   012204d2, in radians). Ours is now `CLIMB_CONE` 60 deg, stick against the wall (`Ground::climb_wall`, looking along the
   stick and the facing, at 1.3 m and 0.7 m), and every entry then runs squarely into the wall found.

Tests: `jump_map.rs` swinging_on_a_pole, only_real_edges_are_holds, gait_changes_are_instant, climbing_starts_at_once,
the_drop_button_drops; updated fall_off_an_edge_by_profile, rebound_targets, hang_targets, thin_railing_landing. Timed
scripts moved for the lost ramp: gates inputs and header (`rebound` uses S+F), `duel_map` and `match_map` start B's run
0.45 s later for the same distance at the dodge, `steady_speeds` runs 2 s. 60 and 144 fps gates agree; kills 8; duel 100/0;
escort no desyncs. NOTES "Climbing"'s `-11.31,46.8,6.5,0` walk-in mount gate was already stale on master (a ledge stop).
Not done: the swing's own air and exit clips; retail's thin-top vault; whether retail lets a wall run start at these
angles at all. Checked on two frames only (pull-up, pole); how it plays is for the user to judge.

## Abilities, perks, streaks (2026-11-04)

`crates/acb-game/src/abilities.rs`: logic only, no input, HUD or wire form yet. Empty loadouts by default, so a match
without `Match::set_loadout` plays as before.

Data ships: `AbilityManagerMulti` (DataPC.forge 0x639b4f79) lists 75 objects: 48 actives (12 families x base + 3 upgrades),
8 perks, 4 kill streaks, 4 loss streaks, 6 slots, 5 sets. `acbmp dump DataPC.forge 0x<uid> --depth 2`; floats print as raw
bits. ACTIVES was checked field by field against the dump. "Perk" has no symbol: it's the UI word for `PassiveAbility`.

Rules from code:
- Slot: effect, cooldown, ready (`SlotAbility` Update 00f95a3c, CanActivate 00f390d4). Effect length per class: Smoke, Morph,
  Gun inherit 0 (0118c25e); Poison's is its Delay.
- Cooldown (015edf26): a running cooldown streak's `factor` replaces the Overall Cooldowns perk, it doesn't stack.
- `GamePlayEndContract` reasons (enum table 02eff420, read from the binary): 0 killed target, 1 killed NPC, 2 target was
  killed, 3 player was killed, 4 escaped, 5 humiliated, 6 humiliated and escaped, 7 suicide, 8 player was killed with or
  without a contract. `HandleEndContractEvent` 00f94b86: 2/4/5/6 add a loss; 7/8 also end the kill streak; 0/1/3 nothing.
  So being escaped or stunned doesn't end a kill streak, and killing an NPC is not a loss.
- `UpdateKillCounts` 00f94d36: kills++, silent++ if the kill earned Silent or Incognito (015b1c04), losses = 0.
- Kill streaks are periodic (010db168): arm at `count % n == n - 1`, disarm at `count % n == 0`. Every nth kill pays, not just
  the first. A silent streak's bonus is paid on silent kills only (00f51b10). Loss streaks run at every loss from n on
  (0163c210).
- Score: `(base + extra) * multiplier` (01180946).

Retail oddities kept: SILENT HUNT does nothing (`SpeedFactor`, `Activated` have no reader). Base DISGUISE ships 1 s cooldown
and 1 s duration; RAPID RELOAD DISGUISE 10 s against 60 s (every other Rapid Reload saves 10 s). Looks like data, not a port
error; worth a look at a capture.

Acts in `Match`: streak scoring and losses in every mode, Escapist, Overall Cooldowns, cooldown streaks, Score x2,
`ResetAbilityCD` at round start, target and range checks, Poison (kills after its delay, Score on top), Mute (refuses
abilities), `Match::speed_factor` (Sprint Boost, Knives, Chase Expert).

Not done:
- `Match::activate` has no caller: no key, no choice of loadout, no HUD slots, no wire form. `MatchEvent::{KillStreak,
  LossStreak, Ability}` map to no net event, and `State` isn't in the snapshot, so a client would mispredict any non-empty
  loadout. Wire this before enabling loadouts online.
- Speed effects now reach movement through `Match::time_scale` (see "Physics review"); Mute doesn't yet stop attacks; a poisoned body plays no death clip (the match
  kills it, the body stands until the respawn).
- Timers only, no effect: Disguise, Smoke, Gun, Firecrackers, Morph, Vision, Charge, Decoy. Disguise's `GrantsHidden` is in
  `State::effect_hides`, unused.
- Perks with a value but no hook: Resistance (`ms_DecreaseTime` 3 s, reader not found: xrefs stop at the GOT slot 02b98398),
  AutoBash (we have no bash), Blender, Extra Precision (no compass precision). Wall Runner: see "Wall Runner" (2026-11-11).

Our readings (R): dying ends a running effect (`RequestAbilityStop` 00f95426, its death caller not read); `activate` refuses
during a kill or stun (`IsPlayerEntityCanStartAbility` 00f8db75 not read); Chase Expert holds for the whole chase; VIP kills
don't count for streaks (retail: score reasons 0 and 4 do, reason 4 not identified, so Escort hunters may be short-changed).

Tests: abilities::tests (10), mode::tests (kill_streak_pays_in_the_match_and_death_ends_it, score_x2_after_five_deaths...,
a_stun_is_a_loss..., a_fatal_fall..., killing_anothers_target..., escapist_shortens_the_escape, poison, mute, knives, chase).

## Physics review: running, climbing, ejecting, jumping (2026-11-10)

A pass over every movement constant against the binary and DataPC.forge, multiplayer only. Ghidra's decompiler
answered nothing this session, so this was read from the disassembly (`llvm-objdump --macho -d`, cached at
`~/.cache/acbmp-re/dis.txt`). `acbmp census` now prints type names when `ACBMP_SCHEMAS` is set. `examples/jumpclips.rs`
now prints each clip's end velocity.

DataPC.forge: the movement tuning lives in code, as found before. These objects have values that matter for movement:
- `GameSetting` 0x816: `JumpDetectionShape` (mesh `AC1_ Hang_free_Zone3D`), `NPCMaxSpeedOnBeam` 3.5 (NPCs only),
  velocity thresholds 1/2/3/6 for physics-object hits.
- `GameParameters.GlobalParameters` 0xbc9: `Boost_*`. Nothing in our code read these before (see below).
- `HealthSettings`: fall damage, already ported.
- `GamePlaySettings`: AI and detection only.

| Area | Retail | Ours before | Now |
|---|---|---|---|
| Chase boost | `AssassinTask::UpdateAspiration` 00b55022, called from `UpdateGroundMovement` 01670407. In high profile with the stick past 0.35 (and no knife slow), a target closer than `fDistMaxAspi` 10 m on the flat (`Length2` = xy length) and more than 0.707 m ahead along the facing sets `ActionComponent::SetTimeScale(GetAspirationCoeff(d))`. The coefficient (00af44f6) runs linearly from `Boost_ValueAtMinDist` 1.1 at `Boost_MinDist` 0 m to `Boost_ValueAtMaxDist` 1.05 at `Boost_MaxDist` 10 m, clamped to [1.05, 1.1]. | none | `Match::time_scale`, `mode::aspiration_coeff`. `Player::time_scale` scales the ground cycle's dt, so pose and root motion both speed up. |
| Chase Expert | Its `SpeedFactor` 1.15 replaces `Boost_ValueAtMinDist` (`ms_BoostFactor`, written on activation at 00dc4044). It is not a flat factor. | ×1.15 for the whole chase, not applied to movement | 1.15 → 1.05 over 10 m, through the boost |
| Sprint Boost | `BhvAssassin::GetSpeedUpCoeff` replaces the chase boost, high profile only | in `speed_factor`, not applied | applied the same way |
| Knives | No boost while `ThrowingKnifeTimerIsElapsed` is false | not applied | their factor scales the cycle |
| Wall Runner | Wrong at first: the function and net-flag readers only feed accomplishments, but the real path goes through the static `AbilityWallRunner::Activated`. See "Wall Runner" below. | no effect | ported (2026-11-11) |
| Hang drop (S) | `ReleaseToInAir` plays `xx_h_hangwall_tr_fall_a`: 0.30 m out, 1.0 m down in 0.333 s, ending at (0, -4.98) m/s with no outward speed. `xx_h_hangfree_tr_fall_a` has no root motion. | after the clip, the fall restarted at vz 0 with 1.0 m/s outward | continues at -4.98 m/s from a wall hang and from rest from a free hang. Gate `climb` ends 0.61 m nearer the wall (40.28). |
| Drop button (F) | same drop clip | 0.5 m/s push, no clip | kept: the user asked for an instant release (playtest #7). A free hang now drops straight down, as the retail clip does. |
| Rebound with no target | `DoTargetedJump` onto the farthest foothold (8 m out, 3 m down) with the clip-warp model | ballistic homing from 1.71 m/s | unchanged (stand-in): it reaches the same point. A clip warp onto a point with no floor has no landing path yet. |
| Gravity | 9.81 (`GetDropTime`) | 9.81 everywhere | agrees |
| Gaits, climb cone, mount time, leap | retail ramp, 20-29 deg cone, 0.87 s mount, no ground leap | "Ours" by the user's playtest | unchanged (feel over retail) |

Not verified (decompiler down): the grasp's hand window (1.3-2.2 m) and side reach (0.7 m), which are ours. The 1.2 m probe
and the 2 m fall limit are retail. The two globals behind the boost's floor are `AbilitySpeed::MovementSpeedFactor` (`0x2de5c08`) and
`AbilitySpeed::Activated` (`0x2de5c05`), per AC RE Memory. `AbilitySpeed` isn't in `AbilityManagerMulti`, so both are off in MP. `GameInfo +0xd4` (the default time scale) is taken as 1.
Retail resets the scale to default while a conflict or one of four Human states is active (`-0xa5`). The conflict test
isn't ported, so ours keeps the boost.

Tests: `mode::tests::chase_boost_closes_on_a_target_ahead` (replaces `chase_expert_holds_while_chasing`). Gates match
their headers at 60 and 144 fps, with `climb` updated. Kills 8, duel 100/0, escort 0 desyncs. `acb-net` `match_map`
(`match_over_lossy_localhost`) fails with or without this change: the lossy run sees a server rejection but no
rollback. This was already failing before this work.

## Wall Runner (2026-11-11)

Source: the AC RE Memory knowledge base (`~/Music/AC-RE-Memory`, notes `acb/abilities/acbmp_wallrunner_investigation.md` and
`acb2_ability_statics_break_fsm_wall.md`). Consult it before re-deriving ability or movement RE: my static search on
2026-11-10 looked only at the ability's functions and `NetPlayer` flag, and wrongly concluded the perk does nothing.

Retail chain:
1. `AbilityWallRunner::Activated` (static 0x02e39165) is read by `BhvAssassin::IsSuperWallingActivated`.
2. That value is the last argument of `DoEnterWallingVertical` (`UpdateGroundMovement` and the beam/free-step tasks) and
   is stored at `HumanWallingData+0x21`.
3. It is read once, in `HumanWalling::UpdateHumanWallingMain` 0122f968.

At the end of the entry (the entry clip has risen 1.0 m):
- The low scan runs first (edges 0.25-1.1 m over the feet, so 1.25-2.1 m over the start). A hold there is grabbed, with or
  without the perk.
- Perk on, with nothing in the low scan: an extra scan 1.5 m back, 3.5 m up, 2.1 m tall. A top edge 0.25-5 m up (4.5-6.0 m
  over the start) plays the perk-only action 0x89587CC9: `IDxx_h_wallingfront_step1_300_footr`, 0.4 s, 2.0 m up. The plain
  step 0x45, `xx_h_wallingfront_step1_footr`, takes 0.133 s for 0.5 m.
- The ledge tests then run from 2.0 m higher (feet +3.0). Tests 1 and 3 stop at the 2.95 m scan box; the perk lifts test
  2 to 3.1 m. Ledges are taken at 3.5-6.0 m over the start, against 4.45 m at most without the perk (ours: 4.4).
- The low scan blocks the perk, but a lower edge in the plain test range does not. Where a high top exists, the perk
  climbs past the lower edges. The AC RE notes also cover losing the eject: no `WallingEnd` on the long climb.

Ours:
- `Ground::wall_run_hold(.., wall_runner)` returns the hold and whether the run is long (`WALL_RUNNER_LOW`, `_SCAN`,
  `_REACH`).
- `Climb::wall_run(.., long, ..)` plays the 2 m step clip, and the grab clip is chosen 1.5 m higher.
- `Player::wall_runner` is set each tick from the match loadout. In free play it keeps the player's own flag:
  `acb-viewer --wall-runner`.
- Our reading: test types 2/3/4 (ledge kinds) are not told apart, and any hold counts in the reach. The "pull-up type"
  of the perk's scan is our `Hold::top`. The perk's run does not lose the eject in ours (we have no `WallingEnd` state).
- Gates `wallrun` (perk: hangs at entity -9.45 -2.03 4.39 from the 5.39 m top) and `wallrun0` (no perk: -9.50 -2.03 1.99,
  the lower edge), identical at 60 and 144 fps. Test `jump_map.rs` `wall_runner_reaches_higher`. On San Donato, 67 tops
  stand 4.7-5.8 m over the floor 1 m out. Other gates, kills 8 and duel 100/0 are unchanged. Loadouts are empty by
  default and online has no loadout choice yet, so a match only gets the perk through `Match::set_loadout`.
