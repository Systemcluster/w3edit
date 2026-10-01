# w3edit

**Utility for packing, unpacking and analyzing The Witcher 3 content bundles.**

Implements a subset of features from `wcc_lite` from the official The Witcher 3 mod tools.

## Status

The library and CLI support creating, inspecting, unpacking, converting, and merging Witcher 3
mod containers in Legacy and Remastered formats. Bundle packing
accepts already-cooked files; cache creation accepts already-cooked GPU texture
chunks. Asset cooking, image conversion, and DDS extraction are not implemented.

Generated containers are tested by reparsing and comparing data with this library;
in-game acceptance has not been verified. This is not a replacement for the full
official mod toolchain.

## Features

- Reads Legacy (version 3) and Remastered (version 5) `.bundle` files, including version-5 payload offsets above 4 GiB
- Unpacks `.bundle` contents
  - Supports unpacking `lz4`, `lz4hc`, `snappy` and `zlib` compressed as well as uncompressed bundles
- Creates `.bundle` files in either format and preserves versions when rewriting
  - Preserves per-item `lz4`, `lz4hc`, `snappy`, `zlib`, and uncompressed payloads without re-encoding
  - Mixed-version bundle merges emit version 5; legacy-only merges retain version 3
- Creates, reads, and writes legacy version-6 and remastered version-7
  `metadata.store` files, including 64-bit offsets and bundle sizes in version 7
- Merges multiple mods into a single `(bundle, metadata)` pair with first-wins
  priority and self-consistent cross-references
- Creates, reads, writes, and merges version-6 and version-7 `texture.cache` files
- Upgrades or downgrades bundles, metadata indexes, and texture caches between
  Legacy and Remastered container layouts without recompressing payloads
- Provides a `w3edit` CLI for inspecting containers, unpacking bundles, and
  merging a bundle, metadata index, and optional texture cache from each input

Not yet implemented:

- Extracting image data from `texture.cache`
- Doboz compression or decompression

### Format Targets

| Target | `.bundle` | `metadata.store` | `texture.cache` |
| --- | --- | --- | --- |
| `legacy` | Version 3 | Version 6 | Version 6 |
| `remastered` | Version 5 | Version 7 | Version 7 |

Creation defaults to `legacy`; readers detect formats from the input files.
Bundle rewrites preserve compressed payloads, but rebuild their layout and are
not generally byte-identical to the original container.

## Installation

### Release Binaries

Download an archive for your platform from the
[snapshot release](https://github.com/Systemcluster/w3edit/releases/tag/snapshot)
and extract it. macOS builds are available for Apple Silicon (`aarch64`) and
Intel (`x86_64`).

The macOS snapshots are not Developer ID-signed or notarized. Browser downloads
can be quarantined, causing macOS to report that Apple could not verify `w3edit`
is free of malware. If you trust the release source, run these commands from the
extracted directory to remove quarantine from this executable only:

```sh
xattr -d com.apple.quarantine ./w3edit
./w3edit --version
```

If `xattr` reports that the attribute does not exist, no removal is needed.
Alternatively, after attempting to run the executable, approve it under
**System Settings > Privacy & Security > Open Anyway**. Do not disable Gatekeeper
globally. This approval is local to your copy and may be needed for each new
download; it does not sign or notarize the release.

### Build From Source

Build from a checkout with Rust and Cargo installed. A C compiler is also needed
for the native LZ4 dependency.

```sh
git clone https://github.com/Systemcluster/w3edit.git
cd w3edit
```

Install the CLI into Cargo's binary directory (normally `~/.cargo/bin`):

```sh
cargo install --path .
```

Ensure that directory is on your `PATH`. Alternatively, build without installing:

```sh
cargo build --release
./target/release/w3edit --help
```

The CLI is enabled by default. The examples below use the installed `w3edit`
command; substitute `./target/release/w3edit` when using a local build.

## CLI

Inspect or unpack a bundle:

```sh
w3edit bundle list path/to/blob0.bundle
w3edit bundle unpack path/to/blob0.bundle path/to/output
```

Omit the unpack output path to create `<bundle-name>.unpacked` beside the bundle
(for example, `blob0.bundle` becomes `blob0.unpacked`).

Inspect companion indexes:

```sh
w3edit metadata list path/to/metadata.store
w3edit texture list path/to/texture.cache
```

Upgrade or downgrade an existing container with an explicit target:

```sh
w3edit convert legacy/blob0.bundle remastered/blob0.bundle --format remastered
w3edit convert remastered/texture.cache legacy/texture.cache --format legacy
w3edit convert input/metadata.store output/metadata.store --format remastered
```

`convert` detects the file type by signature, not filename. It accepts bundle
versions 3/5 and metadata/cache versions 6/7. The output parent directory must
exist; replacing an existing file requires `--force`. The library equivalent is
`w3edit::convert(data: &[u8], format: BundleFormat) -> Result<Vec<u8>, ConversionError>`.

Conversion changes container layouts, not cooked resources, scripts, or texture
formats, and does not establish compatibility with the target game. Compressed
payload bytes are preserved; fields absent from the target layout are lost
(for example, legacy bundle timestamps and metadata burst sizes when upgrading).
Downgrades that exceed legacy integer limits fail instead of truncating values.
Even same-target bundle conversion rebuilds the layout.

Converting metadata alone preserves its recorded bundle offsets; it does not
convert or reindex the referenced bundles. After converting bundles, regenerate
their metadata, for example:

```sh
w3edit metadata create remastered/ remastered/metadata.store --format remastered --force
```

As with all metadata creation, this does not reconstruct resource-specific hashes
or buffer information. To preserve those fields for a single-bundle mod, use
`merge` with one input directory and an explicit `--format` instead.

Create containers with an explicit target (the default is `legacy`):

```sh
mkdir -p output
w3edit bundle pack cooked/ output/blob0.bundle --format remastered --compression zlib
w3edit metadata create output/ output/metadata.store --format remastered
w3edit texture create textures.json output/texture.cache --format remastered
```

The output parent directory must already exist for these creation commands.
Packing recursively includes regular files in sorted order, stores paths relative
to the input directory with backslash separators, and rejects symlinks and names
outside ISO-8859-1. Stored names must fit in 255 bytes. Supported encoders are
`none`, `zlib` (the default), `snappy`, `lz4`, and `lz4hc`. Packing compresses files;
it does not convert legacy cooked assets to remastered assets or vice versa.
Keep the output bundle outside the input tree to avoid packing an earlier output.

Metadata creation recursively indexes `.bundle` files using their actual on-disk
offsets without rewriting them. Every bundle must match the selected target;
mixed-format input is rejected. It builds an index from bundle records, not from
cooked resource internals: resource-specific hashes and buffer information are
not reconstructed.

`texture create` reads a JSON manifest. Relative chunk paths are resolved against
the manifest's directory; absolute chunk paths are also accepted. Each file
contains uncompressed, already-cooked GPU data, not a PNG or DDS header.
Chunks run from largest mip to smallest; the final chunk may hold several small
mips. The descriptor and texture hash must match the cooked texture resource:

```json
{
  "textures": [{
    "name": "textures\\example.xbm",
    "hash": 894555473,
    "width": 2048,
    "height": 2048,
    "mipcount": 12,
    "slice_count": 1,
    "base_alignment": 16,
    "type1": 7,
    "type2": 4,
    "chunks": ["example-base.bin", "example-remaining-mips.bin"]
  }]
}
```

Optional `is_cube`, `unk1`, and `time_stamp` fields default to zero. The writer
generates compression, streaming headers, page indexes, mip offsets, and checksums.
Unknown manifest fields are rejected. Texture names must be unique, relative,
ISO-8859-1 paths; dimensions and slice counts must be nonzero. Each texture needs
1 to 256 chunks, with no more chunks than its mip count. The example descriptor
is illustrative; it does not supply usable texture data or determine the correct
format bytes and hash for another resource.

Merge container directories into one output set with rebuilt metadata offsets and
cross-references. Each input directory must directly contain `blob0.bundle` and
`metadata.store`; `texture.cache` is included when present. Point at the directory
containing these files, not an enclosing mod directory. Additional bundles,
scripts outside the bundle, and other companion files are not included.
Inputs are ordered from highest to lowest priority, so the first mod wins
filename conflicts. This selects whole files; it does not resolve script or
asset-level conflicts. Texture caches are merged independently in the same
priority order.

```sh
w3edit merge path/to/mod-a path/to/mod-b --output path/to/merged
w3edit merge path/to/mod-a path/to/mod-b --output path/to/merged --format remastered
```

Merge creates the output directory if needed. `--bundle-name NAME` changes the
output bundle name (default: `blob0.bundle`), not the input bundle name. It must
be a single ISO-8859-1 filename, must not end in a dot or space, and must not be
`metadata.store` or `texture.cache` (case-insensitive). Without
`--format`, the newest input bundle format determines the output bundle and
metadata formats; the texture cache independently uses the newest input cache
version. An explicit target sets all three output formats, but does not convert
cooked assets.

### Output Safety and Limits

Creation, conversion, extraction, and merge refuse to replace existing files unless `--force` is
passed. Existing directories are not cleared, and failures can leave partial
output. Use a fresh output directory, separate from your inputs, and keep backups.

Bundle extraction rejects absolute and parent-traversal paths, but is not a
filesystem sandbox: existing symlinks in the destination can redirect writes.
Extract trusted bundles into a fresh directory without symlinks.

Use `w3edit --help` or command-specific help such as `w3edit bundle pack --help`
for arguments and options.

The CLI loads containers into memory rather than streaming them. Merging holds
inputs and serialized outputs in memory, so allow substantially more memory than
the size of a single archive. Individual bundle item sizes, both compressed and
uncompressed, remain limited to `u32` (less than 4 GiB), even in version 5.

## Library

The library is exported as `w3edit`. Disable CLI dependencies with
`default-features = false` in your Cargo dependency declaration; from a checkout,
use `cargo build --lib --no-default-features`.

This example serializes a bundle, its metadata, and an empty texture cache in
memory. The payload is a placeholder, not a cooked game asset:

```rust
use w3edit::{Bundle, BundleCompression, BundleFormat, BundleItem, Metadata, TextureCache};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let format = BundleFormat::Remastered;
    let item = BundleItem::new(c"example.bin".into(), b"cooked data", BundleCompression::Zlib)?;
    let bundle = Bundle::from_items(format, vec![item]);
    let store = Metadata::from_bundles(format, &[(c"blob0.bundle", &bundle)])?;
    let bundle_bytes = bundle.try_write()?;
    let store_bytes = store.try_write()?;
    let cache_bytes = TextureCache::for_format(format).write();
    println!(
        "Serialized {} bundle bytes, {} metadata bytes, {} cache bytes",
        bundle_bytes.len(), store_bytes.len(), cache_bytes.len()
    );
    Ok(())
}
```

Use `Metadata::from_bundle_files` when indexing parsed files that will not be
rewritten, and `TextureCache::insert` to add descriptors and cooked texture chunks.
`merge_mods_for_format` selects all three output formats explicitly;
`merge_mods` retains automatic format selection. Use these end-to-end merge APIs
when the output metadata must reference a newly serialized merged bundle;
independent `Bundle::merge` and `Metadata::merge` calls do not recompute those
cross-references together.

The bundle API exposes 64-bit item offsets and `compute_offsets_u64()` for large
version-5 bundles. `compute_offsets()` panics if an offset exceeds `u32`.
Metadata entry offsets and bundle sizes are `u64`; legacy serialization rejects
overflow instead of truncating. Prefer `Bundle::try_write()` and
`Metadata::try_write()` for fallible serialization; their `write()` wrappers panic
on serialization errors.

## Testing

Run tests from a repository checkout, including the fixtures under `test/assets`:

```sh
cargo test
cargo test --no-default-features
```

The suite covers creation, parsing, serialization, conversion, merging, and CLI workflows,
using legacy fixtures and synthetic legacy/remastered cases. CLI tests run only
with the `cli` feature enabled (the default). Tests and fixtures are not included
in the Cargo publication package.

Three installed-game tests are ignored by default. To run them, point `W3_GAME_DIR`
at a local Remastered installation root containing `content/metadata.store` and
`content/content0/texture.cache`:

```sh
W3_GAME_DIR='/path/to/The Witcher 3' cargo test --test bundle installed_remastered_bundles -- --ignored --nocapture
W3_GAME_DIR='/path/to/The Witcher 3' cargo test --test metadata installed_remastered_metadata_roundtrip -- --ignored
W3_GAME_DIR='/path/to/The Witcher 3' cargo test --test texture_cache installed_remastered_texture_creation -- --ignored
```

These tests read the installation without modifying it. The bundle test recursively
memory-maps `.bundle` files, checks entry ranges, samples each codec per bundle,
and decompresses and round-trips bundles smaller than 1 MB. Do not modify or update
the installation while it runs. The metadata test checks a byte-identical version-7
round-trip and the presence of 64-bit values. The texture test rebuilds the first
two cached textures and compares decompressed data without reading the full cache.
These checks do not establish in-game acceptance of generated containers.

## License

BSD-2-Clause. See [LICENSE](LICENSE).

## Credits

Thanks to the team behind [WolvenKit](https://github.com/WolvenKit/WolvenKit) for specifications of some of the file format layouts.
