# w3edit — File Format Research Notes
Source: WolvenKit-7 (WolvenKit.Bundles, WolvenKit.Cache)

## Bundle Format (.bundle)
- Magic: `POTATO70` (8 bytes, start of file)
- Header (32 bytes total):
  - magic[8], bundleSize(u32), dummySize(u32), dataOffset(u32), 12 mystery bytes
  - Mystery bytes: `03 00 01 00 00 13 13 13 13 13 13 13` (WolvenKit TODO)
- TOC: starts at offset 0x20, each entry is 0x140 bytes:
  - filename (0x100, null-padded, ISO-8859-1)
  - hash (u128)
  - empty (u32)
  - size (u32) — uncompressed
  - zsize (u32) — compressed
  - pageOffset (u32, sometimes u64 in newer files)
  - date (u32), time (u32)
  - zero (u128)
  - crc (u32) — CRC32 of uncompressed data
  - compression (u32)
- Data: starts at 0x20 + dataOffset, 4096-byte aligned
  - Alignment padding: up to 16 bytes of "AlignmentUnused", then null bytes
- Compression types: 0=None, 1=Zlib, 2=Snappy, 3=Doboz, 4=Lz4, 5=Lz4hc
- bundleSize = 8192 is a placeholder in WolvenKit; needs proper computation

## metadata.store Format
- Magic: `\x03VTM` (4 bytes)
- Header: version(i32), maxFileSizeInBundle(i32), maxFileSizeInMemory(i32), stringTableSize(VLQ i32)
- Body: stringTable(bytes), then TDynArray sections in order:
  1. fileInfos (MetadataFileInfo, 8 fields × u32 = 32 bytes each)
  2. fileEntryInfos (MetadataFileEntryInfo, 5 × u32 = 20 bytes each)
  3. bundleInfos (MetadataBundleInfo, 6 × u32 = 24 bytes each)
  4. buffers count (VLQ) + buffer values (u32 each)
  5. dirInitInfos (MetadataDirectoryInitInfo, 2 × i32 = 8 bytes each)
  6. fileInitInfos (MetadatFileInitInfo, 3 × i32 = 12 bytes each)
  7. hashes (MetadataHash, 2 × i64 = 16 bytes each)
- TDynArray prefix: VLQ i32 count, then serialized entries
- WolvenKit Write() method was never implemented (empty TODO)

## texture.cache Format
- Magic `HCXT` is at the END (last 4 bytes of footer), NOT the start
- Footer (last 32 bytes from EOF): crc(u64), usedPages(u32), entryCount(u32), stringTableSize(u32), mipTableEntryCount(u32), magic=HCXT(u32), version(u32)
- Layout from EOF backwards: footer(32) + entryTable(entryCount×52) + stringTable(stringTableSize) + mipOffsets(mipTableEntryCount×4)
- Entry (52 bytes): hash(u32), stringTableOffset(i32), pageOffset(u32), compressedSize(u32), uncompressedSize(u32), baseAlignment(u32), baseWidth(u16), baseHeight(u16), mipcount(u16), sliceCount(u16), mipOffsetIndex(i32), numMipOffsets(i32), timeStamp(i64), type1(u8), type2(u8), isCube(u8), unk1(u8)
- Data pages: 4096-byte aligned; each page header = zsize(u32) + size(u32) + mipIdx(u8) = 9 bytes, then zlib-compressed image data
- Compression: zlib only (unlike bundles)
- CRC = FNV64 hash over the info tables (string table + entry table)

## VLQ i32 encoding (util.rs)
- Read and write both implemented (see `read_vlq_i32` / `write_vlq_i32`)
- Format: sign bit in high bit of first byte, continuation bit in bit 6 of the
  first byte and bit 7 of subsequent bytes; value bits are [5:0] in the first
  byte and [6:0] in subsequent bytes
- Zero is encoded as `0x80` (sign bit set) to match REDengine's canonical form
- See TDynArray.cs WriteVLQInt32 for reference

## CLI (wcc_lite equivalent) — planned, not implemented
The library is usable programmatically today; no `w3edit` binary CLI exists
yet. Target commands (from README intent + wcc_lite model):
- `bundle list <file>` — list bundle contents
- `bundle unpack <file> [output]` — extract files (code already written, commented out)
- `bundle pack <dir> [output] [--compression lz4hc|lz4|zlib|snappy|none]`
- `metadata list <file>` — show metadata.store contents
- `metadata create <bundle_dir>` — create metadata.store
- `texture list <file>` — list texture.cache contents
- `texture unpack <file> [output]` — extract as DDS
