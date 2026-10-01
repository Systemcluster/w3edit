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

Prebuilt binaries for Linux, Windows and macOS are available on the
[snapshot release](https://github.com/Systemcluster/w3edit/releases/tag/snapshot).
Download the archive for your platform and extract it.

<details>
<summary>Running on macOS</summary>

The macOS builds are not signed or notarized, so macOS may refuse to run `w3edit`
after downloading it through a browser. If you trust the release source, remove
the quarantine attribute from the executable:

```sh
xattr -d com.apple.quarantine ./w3edit
```

Alternatively, try to run it once and then approve it under
**System Settings > Privacy & Security > Open Anyway**.

</details>

### Building From Source

Building requires a Rust toolchain and a C compiler for the native LZ4 dependency.

```sh
git clone https://github.com/Systemcluster/w3edit.git
cd w3edit
cargo install --path .
```

This installs `w3edit` into Cargo's binary directory, usually `~/.cargo/bin`.
To build without installing, run `cargo build --release` and use
`./target/release/w3edit` instead.

## Usage

Set `W3_DIR` to the game installation directory. For example, a default Steam
installation on macOS may use:

```sh
export W3_DIR="$HOME/Library/Application Support/Steam/steamapps/common/The Witcher 3"
w3edit metadata list "$W3_DIR/mods/modExample"
w3edit bundle unpack "$W3_DIR/mods/modExample" build/modExample-unpacked/
```

Replace `modExample` with the installed mod's directory name. Container-based
mods normally keep `metadata.store`, one or more `.bundle` files, and an optional
`texture.cache` directly in `mods/<mod-name>/content/`.

Run `w3edit --help` or `w3edit <command> --help` for all commands and options.
Commands that create containers take `--format legacy` or `--format remastered`.
Mod-directory inputs accept either a mod root or its `content/` directory.
`merge` and directory-to-directory `convert` take an output mod root and create
`content/` automatically; an explicit output ending in `content/` is used directly.

Container commands also accept mod roots: `bundle pack`, `metadata create`, and
`texture create` write `content/blob0.bundle`, `content/metadata.store`, and
`content/texture.cache`, respectively. For container outputs, an existing directory
or a new path without an extension means a mod root. An existing file or a new
path with an extension means an explicit file; its parent directory must exist.
Create a directory first when using a new mod name containing a dot.

The `list` commands and `bundle unpack` accept an existing mod root or content
directory in place of a container file. Bundle commands use `blob0.bundle` for
directory inputs; pass an explicit file to select another bundle. `bundle pack`
and `metadata create` resolve input mod roots to `content/` before computing
relative paths. `bundle unpack` still writes raw extracted files directly into
its destination, not into an added `content/` directory.

> [!WARNING]
> w3edit only replaces existing files with `--force`, but doesn't clear existing
> directories, and a failed command can leave partial output behind. Use a fresh
> output directory separate from your inputs, and keep backups of your mods.

### Bundles

```sh
w3edit bundle list "$W3_DIR/mods/modExample"
w3edit bundle unpack "$W3_DIR/mods/modExample" build/modExample-unpacked/
w3edit bundle pack cooked/ build/modMyMod --format remastered --compression zlib
```

Unpacking extracts all files into the given directory, or into `blob0.unpacked`
beside the bundle when no output is given.

Packing recursively includes all files in the resolved input directory and compresses
them with `zlib` by default, or with `none`, `snappy`, `lz4` or `lz4hc`.
Files are packed as they are, so they need to be already cooked for the
target game version.

<details>
<summary>Notes</summary>

- <sup>Stored paths are relative to the input directory with backslash separators.
Names have to be ISO-8859-1 encodable and at most 255 bytes long, and symlinks are rejected.</sup>
- <sup>Keep the output bundle outside of the input directory, otherwise a previous
output will be packed into the new bundle.</sup>
- <sup>Unpacking rejects absolute and parent-traversal paths, but follows existing
symlinks in the output directory.</sup>

</details>

### Metadata

```sh
w3edit metadata list "$W3_DIR/mods/modExample"
w3edit metadata create build/modMyMod build/modMyMod --format remastered
```

Metadata creation recursively indexes all `.bundle` files in the resolved content
directory using their on-disk offsets. Bundle names are relative to that directory,
without an extra `content/` prefix. All bundles have to match the selected format.

The index is built from the bundle records, not from the cooked resources inside
them, so resource-specific hashes and buffer information are not reconstructed.
To keep them for an existing mod, use [`merge`](#merging) with a single input instead.

### Texture Caches

```sh
w3edit texture list "$W3_DIR/mods/modExample"
w3edit texture create modMyMod-textures.json build/modMyMod --format remastered
```

Texture caches are created from a JSON manifest referencing already cooked GPU
texture data. w3edit takes care of compression, streaming headers, page indexes,
mip offsets and checksums.

Each chunk file contains uncompressed cooked GPU data without a DDS or PNG header,
ordered from the largest mip to the smallest. The last chunk can contain multiple
small mips. The descriptor and hash have to match the cooked texture resource;
copy `name`, `hash`, dimensions, mip and slice counts, alignment, and texture type
fields from the tooling that produced that resource. Do not guess or reuse a
descriptor from another texture.

<details>
<summary>Notes</summary>

- <sup>Chunk paths are resolved relative to the manifest. Absolute paths are accepted as well.</sup>
- <sup>Each texture needs between 1 and 256 chunks, and no more chunks than mips.</sup>
- <sup>Texture names have to be unique, relative and ISO-8859-1 encodable.
Dimensions and slice counts have to be nonzero.</sup>
- <sup>`is_cube`, `unk1` and `time_stamp` are optional and default to zero.
Unknown fields are rejected.</sup>

</details>

### Merging

When two mods supply the same file, choose which version should win. Inputs are
ordered from highest to lowest priority; the first mod wins unless a per-file
choice overrides it. For a full merged copy of their content:

```sh
w3edit merge \
  "$W3_DIR/mods/modHighPriority" \
  "$W3_DIR/mods/modLowPriority" \
  --output build/modMerged \
  --format remastered
```

Inputs may be mod roots with a `content/` directory, or content directories
directly. The output above is a mod root with files under `build/modMerged/content/`.
Bundled files are rebuilt into `blob0.bundle` and `metadata.store` with
fresh offsets and the winning files' metadata. All indexed bundles and additional
`.bundle` files under the content directory are included. Texture caches are
rebuilt by texture name, and loose files (including scripts) remain loose.
Script-only mods and bundles without an index are supported.

To create only an override layer for conflicting paths, leaving unique files in
their original mods:

```sh
w3edit merge \
  "$W3_DIR/mods/modHighPriority" \
  "$W3_DIR/mods/modLowPriority" \
  --output build/modConflictOverrides \
  --conflicts-only \
  --prefer 'scripts/game/player.ws=2' \
  --format remastered
```

`--prefer PATH=INPUT` picks a specific file from a numbered input (starting at 1),
overriding the default order for that path only. Repeat it for other files or
textures. Paths are relative to `content/` for loose files, or depot paths for
bundled files and textures. Unknown paths, invalid input numbers, and duplicate
choices are rejected before writing.

A conflict means the same path occurs in at least two input mods, even when its
bytes are identical. Repeated entries within one mod do not count. Path matching
ignores ASCII case and treats `/` and `\` as equivalent. Bundled files, cached
textures, and loose files are selected independently within their own groups.

Install `modConflictOverrides` under the game's `mods/` directory and give it
higher priority than **all source mods** in your mod manager or `mods.settings`.
Keep the originals enabled: conflict-only output does not contain their unique
files. For a full merge, the output replaces the source mods' content; preserve
any required DLC, settings, or installation steps outside that content yourself.
The tool does not change game load order or modify its inputs.

<details>
<summary>Notes</summary>

- <sup>Merging picks whole files. It does not combine edits within scripts, XML,
or cooked assets. Use a script/text merger when both mods' edits are needed.</sup>
- <sup>Only `content/` is read when passing a mod root; files outside it are not
included. Other cache formats are treated as opaque loose files and are not combined.</sup>
- <sup>`--conflicts-only` requires a new or empty output `content/` directory, even
with `--force`, to avoid retaining stale files. No conflicts produce a mod root
with an empty `content/`. Empty bundle/metadata pairs and empty conflict-only caches are omitted.</sup>
- <sup>Output must not overlap any input directory. Existing outputs are protected
unless `--force` is used; this does not bypass the conflict-only empty-directory rule.</sup>
- <sup>Files without source metadata use bundle-derived fields. Without `--format`,
bundle and metadata output use the newest input bundle format.</sup>
- <sup>Without `--format`, the texture cache format is selected independently of the
bundle and metadata format. An explicit `--format` applies to all three, but
doesn't convert cooked assets.</sup>
- <sup>`--bundle-name` changes the name of the output bundle. It has to be a single
ISO-8859-1 filename not ending in a dot or space, and can't be `metadata.store`
or `texture.cache`.</sup>

</details>

### Converting

To target a mod's content at another game format, pass its root on both sides.
Directory conversion uses the single-mod merge path: it combines its bundles,
rebuilds metadata, preserves source metadata fields, converts its texture cache,
and copies loose files:

```sh
w3edit convert \
  "$W3_DIR/mods/modLegacy" \
  build/modLegacy-remastered \
  --format remastered
```

Converting upgrades or downgrades bundles, metadata indexes and texture caches
between Legacy and Remastered layouts. The file type is detected from its contents.

`convert` also accepts an individual container. Use an explicit output filename,
or a mod root to write the default filename for that container type. For an
explicit file, its output parent directory must already exist:

```sh
mkdir -p build/converted/
w3edit convert \
  "$W3_DIR/mods/modExample/content/blob0.bundle" \
  build/converted/blob0.bundle \
  --format remastered
```

Only the container layout is converted. Compressed payloads are carried over
as they are, and cooked resources, scripts and textures are left untouched, so a
converted container is not automatically compatible with the other game version.

An individually converted `metadata.store` keeps its recorded bundle offsets.
After converting individual bundles, recreate their metadata with `metadata create`.
Directory conversion does this rebuilding automatically.

<details>
<summary>Notes</summary>

- <sup>Fields that don't exist in the target layout are dropped, such as legacy
bundle timestamps and metadata burst sizes when upgrading.</sup>
- <sup>Downgrading fails when values exceed the legacy limits instead of truncating them.</sup>
- <sup>Bundles are always rebuilt, even when converting to the same format.</sup>

</details>

### Limitations

w3edit loads containers into memory instead of streaming them, and merging keeps
both the inputs and the merged output in memory. Individual bundle items are
limited to less than 4 GiB, also in version 5 bundles.

## Library

```toml
[dependencies]
w3edit = { git = "https://github.com/Systemcluster/w3edit.git", default-features = false }
```

w3edit can be used as a library with the same functionality as the CLI.
Disabling the default features removes the CLI dependencies.

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

This creates a bundle with a placeholder payload, its metadata and an empty
texture cache in memory. Other entry points include:

- `merge_mods` and `merge_mods_for_format` merge complete mods with automatic or
  explicit format selection. `Bundle::merge` and `Metadata::merge` on their own
  don't update the cross-references between each other.
- `merge_mods_with_options` accepts `MergeOptions` with `conflicts_only`, optional
  `format`, and `preferred_sources` mapping depot paths to **zero-based** mod
  indices. The library operates on containers; loose-file handling is in the CLI.
- `Metadata::from_bundle_files` indexes parsed bundles that won't be rewritten.
- `TextureCache::insert` adds texture descriptors and cooked texture chunks.
- `convert` converts a container between formats.

<details>
<summary>Notes</summary>

- <sup>Bundle item offsets are 64-bit. `compute_offsets()` panics when an offset
exceeds `u32`, use `compute_offsets_u64()` for large version 5 bundles.</sup>
- <sup>`write()` panics on serialization errors, such as values exceeding the legacy
limits. Use `try_write()` to handle them instead.</sup>

</details>

## Testing

```sh
cargo test
cargo test --no-default-features
```

The tests cover parsing, creating, converting and merging all supported containers
as well as the CLI, using the fixtures in [`test/assets`](./test/assets) and generated
Legacy and Remastered data.

Additional tests against an installed copy of the game are ignored by default.
To run them, set `W3_GAME_DIR` to a Remastered installation:

```sh
W3_GAME_DIR='/path/to/The Witcher 3' cargo test --test bundle installed_remastered_bundles -- --ignored --nocapture
W3_GAME_DIR='/path/to/The Witcher 3' cargo test --test metadata installed_remastered_metadata_roundtrip -- --ignored
W3_GAME_DIR='/path/to/The Witcher 3' cargo test --test texture_cache installed_remastered_texture_creation -- --ignored
```

These tests only read the game files, but they shouldn't be modified or updated
while the tests are running. They check all installed bundles, round-trip the
metadata index and rebuild a sample of cached textures.

## License

BSD-2-Clause. See [LICENSE](LICENSE).

## Credits

Thanks to the team behind [WolvenKit](https://github.com/WolvenKit/WolvenKit) for specifications of some of the file format layouts.
