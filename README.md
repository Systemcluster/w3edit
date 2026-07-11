# w3edit

**Utility for packing, unpacking and analyzing The Witcher 3 content bundles.**

Implements a subset of features from `wcc_lite` from the official The Witcher 3 mod tools.

## Status

Library only. A `w3edit` CLI is planned but not yet implemented; the shipped
`main.rs` is a debug harness that parses a fixture bundle and metadata store.

## Features

- Reads the content of `.bundle` files
- Unpacks `.bundle` contents
  - Supports unpacking `lz4`, `lz4hc`, `snappy` and `zlib` compressed as well as uncompressed bundles
- Writes `.bundle` files (byte-identical roundtrip against shipped fixtures)
  - Preserves per-item `lz4`, `lz4hc`, `snappy`, `zlib`, and uncompressed payloads without re-encoding
- Reads and writes `metadata.store` files (byte-identical roundtrip against shipped fixtures)
- Merges multiple mods into a single `(bundle, metadata)` pair with first-wins
  priority and self-consistent cross-references
- Reads, writes, and merges `texture.cache` files

Not yet implemented:

- A user-facing CLI

## Credits

Thanks to the team behind [WolvenKit](https://github.com/WolvenKit/WolvenKit) for specifications of some of the file format layouts.
