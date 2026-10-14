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
- Next: `StaticPrimitive::StaticPrimitive(..., GfxVertexShaderKey, ...)`
  (0x009682b0) for the other key bits; sky/time-of-day constants; shadows.

## Maps

- Out-of-bounds walls: entity with `OutOfBoundsComponent`, material
  `AC2MP_Oob` (no diffuse). Hidden in the renderer.
- `Material.MaterialDisabled` marks gameplay helpers (e.g.
  `AC2MP_Hay_Lock_disable_01`). Hidden.
- Maps reference TextureSets / MaterialTemplates in the core `DataPC.forge`.

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
  (`UA_Ezio_Head` -> `UA_Ezio_Head_Set`); `_Custom_Colors` columns use
  property 0xdf1ed286 (colour, not applied yet).
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
