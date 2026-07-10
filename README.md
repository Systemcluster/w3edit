# w3edit

**Utility for packing, unpacking and analyzing The Witcher 3 content bundles.**

Implements a subset of features from `wcc_lite` from the official The Witcher 3 mod tools.

## Features

- Reads the content of `.bundle` files
- Unpacks `.bundle` contents
  - Supports unpacking `lz4`, `lz4hc`, `snappy` and `zlib` compressed as well as uncompressed bundles
- Packs `.bundle` contents
  - Supports packing `lz4`, `lz4hc`, `snappy` and `zlib` compressed as well as uncompressed bundles
- Reads the content of `texture.cache` files
- Reads the content of `metadata.store` files
- Creates `metadata.store` files for bundles

## Credits

Thanks to the team behind [WolvenKit](https://github.com/WolvenKit/WolvenKit) for specifications of some of the file format layouts.
