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
- Next: callers of `StaticPrimitive::StaticPrimitive(..., GfxVertexShaderKey,
  ...)` (0x009682b0) to see how a static mesh's key is built, and the
  meaning of the `GfxVertexFormatDescOptions` slots.

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
