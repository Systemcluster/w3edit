# w3edit — File Format Research Notes
Source: WolvenKit-7 (WolvenKit.Bundles, WolvenKit.Cache)

## Bundle Format (.bundle)
- Magic: `POTATO70` (8 bytes, start of file)
- Version: little-endian u16 at byte 20; supported values are 3 and 5

### Legacy (version 3)
- Header (32 bytes total):
  - magic[8], bundleSize(u32), dummySize(u32), dataOffset(u32), 12 mystery bytes
  - Mystery bytes: `03 00 01 00 00 13 13 13 13 13 13 13` (WolvenKit TODO)
- TOC: starts at offset 0x20, each entry is 0x140 bytes:
  - filename (0x100, null-padded, ISO-8859-1)
  - hash (u128)
  - empty (u32)
  - size (u32) — uncompressed
  - zsize (u32) — compressed
  - pageOffset (u32)
  - date (u32), time (u32)
  - zero (u128)
  - crc (u32) — CRC32 of uncompressed data
  - compression (u32)
- Data: starts at 0x20 + dataOffset, 4096-byte aligned
  - Alignment padding: up to 16 bytes of "AlignmentUnused", then null bytes
- Compression types: 0=None, 1=Zlib, 2=Snappy, 3=Doboz, 4=Lz4, 5=Lz4hc
- bundleSize = 8192 is a placeholder in WolvenKit; needs proper computation

### Remastered (version 5)
Verified against the locally installed game: 31 version-5 bundles alongside
10 legacy mod bundles. Both formats retain the same magic and 32-byte header.

- Header:
  - `0x00`: magic (8 bytes)
  - `0x08`: bundle size (u64)
  - `0x10`: TOC byte size (u32)
  - `0x14`: version (u16 = 5)
  - `0x16`: absolute end of TOC / data-region start (u64)
  - `0x1e`: reserved (2 bytes, preserved)
- TOC starts at `0x20`; each entry is `0x130` (304) bytes:
  - `0x000`: filename (256 bytes, null-terminated)
  - `0x100`: hash (u128)
  - `0x110`: absolute payload offset (u64)
  - `0x118`: uncompressed size (u32)
  - `0x11c`: compressed size (u32)
  - `0x120`: CRC32 (u32)
  - `0x124`: compression (u32, same IDs as legacy)
  - `0x128`: reserved (8 bytes, preserved)
- Payloads need not be 4096-byte aligned. Follow each entry's offset, not the
  header's data-region start; initial padding can exist (e.g. bumpers.bundle).
- Real movies.bundle and buffers.bundle exceed 4 GiB. Neither the bundle size
  nor payload offsets may be truncated to u32.
- Current serialization preserves version and payload bytes, uses page-aligned
  output, and recomputes offsets. Mixed-version merges emit version 5.
- Verification covers parsing all installed entries, sampled decompression,
  and small-bundle round-trips, not in-game acceptance of rewritten bundles.

## metadata.store Format
- Legacy version 6 and remastered version 7 are supported.
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

### Remastered (version 7)
Verified by a byte-identical round-trip of the installed game's metadata.store.
The header and all arrays except entry and bundle records retain the legacy layout.
- Entry records (24 bytes): offsetInBundle(u64), sizeInBundle(u32), nextEntry(u32),
  fileId(u32), bundleId(u32).
- Bundle records (24 bytes): dataBlockSize(u64), dataBlockOffset(u32),
  stringTableNameOffset(u32), firstFileEntry(u32), numBundleEntries(u32).
- There is no burstDataBlockSize field in version 7. It is represented as zero
  in memory. Real movie bundle sizes and entry offsets exceed 4 GiB.
- In-memory offsets and bundle sizes use u64. Version-6 serialization checks
  that these fit u32 rather than truncating them.

## texture.cache Format
- Magic `HCXT` is at the END (last 4 bytes of footer), NOT the start
- Footer (last 32 bytes from EOF): crc(u64), usedPages(u32), entryCount(u32), stringTableSize(u32), mipTableEntryCount(u32), magic=HCXT(u32), version(u32)
- Layout from EOF backwards: footer(32) + entryTable(entryCount×52) + stringTable(stringTableSize) + mipOffsets(mipTableEntryCount×4)
- Entry (52 bytes): hash(u32), stringTableOffset(i32), pageOffset(u32), compressedSize(u32), uncompressedSize(u32), baseAlignment(u32), baseWidth(u16), baseHeight(u16), mipcount(u16), sliceCount(u16), mipOffsetIndex(i32), numMipOffsets(i32), timeStamp(i64), type1(u8), type2(u8), isCube(u8), unk1(u8)
- Versions: legacy 6, remastered 7. Both use the same 52-byte entry layout.
- `pageOffset` is a 4096-byte page index, not a byte offset.
- Each texture is a contiguous stream of chunks, aligned only at its start.
  Each chunk header is zsize(u32), size(u32), remainingChunkCount(u8), followed
  by zlib data. `compressedSize` includes every chunk header and payload.
- Mip offsets are relative byte offsets from the start of the texture stream,
  not absolute page addresses. The final chunk can contain multiple small mips.
- The low 16 bits of `numMipOffsets` are the count; the high 16 bits contain
  flags (1 in sampled remastered textures, 0 in legacy fixtures).
- Compression: zlib only (unlike bundles)
- CRC = FNV64 hash over the info tables (string table + entry table)

## VLQ i32 encoding (util.rs)
- Read and write both implemented (see `read_vlq_i32` / `write_vlq_i32`)
- Format: sign bit in high bit of first byte, continuation bit in bit 6 of the
  first byte and bit 7 of subsequent bytes; value bits are [5:0] in the first
  byte and [6:0] in subsequent bytes
- Zero is encoded as `0x80` (sign bit set) to match REDengine's canonical form
- See TDynArray.cs WriteVLQInt32 for reference

## CLI
Creation commands accept `--format legacy|remastered` (default: legacy).
Merge accepts the same option; when omitted it preserves automatic selection.
- `bundle list <file>` — list bundle contents
- `bundle unpack <file> [output]` — extract files (code already written, commented out)
- `bundle pack <dir> <output> [--compression lz4hc|lz4|zlib|snappy|none]`
- `metadata list <file>` — show metadata.store contents
- `metadata create <bundle_dir> <output>` — index existing bundle layouts
- `texture list <file>` — list texture.cache contents
- `texture create <manifest.json> <output>` - build a cache from cooked GPU chunks
- `merge <mods>... --output <directory>` - merge bundles, metadata and caches
- DDS extraction and asset cooking are not implemented.
