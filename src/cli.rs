use std::error::Error;
use std::ffi::{CStr, CString};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use w3edit::{
    Bundle, BundleCompression, BundleFormat, BundleItem, Metadata, ModInput, TextureCache, TextureCacheEntry,
    merge_mods,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create, inspect or extract a content bundle.
    Bundle(BundleArgs),
    /// Create or inspect a metadata.store index.
    Metadata(MetadataArgs),
    /// Create or inspect a texture.cache file.
    Texture(TextureArgs),
    /// Merge mod directories in first-wins priority order.
    Merge(MergeArgs),
}

#[derive(Debug, Args)]
struct BundleArgs {
    #[command(subcommand)]
    command: BundleCommand,
}

#[derive(Debug, Subcommand)]
enum BundleCommand {
    /// Pack a directory of cooked files into a bundle.
    Pack {
        input:       PathBuf,
        output:      PathBuf,
        #[arg(long, value_enum, default_value = "zlib")]
        compression: Compression,
        #[command(flatten)]
        target:      TargetArgs,
    },
    /// List the files stored in a bundle.
    List {
        /// Bundle file to inspect.
        file: PathBuf,
    },
    /// Decompress all files in a bundle.
    Unpack {
        /// Bundle file to extract.
        file:   PathBuf,
        /// Destination directory. Defaults to <bundle-name>.unpacked.
        output: Option<PathBuf>,
        /// Replace files that already exist.
        #[arg(long)]
        force:  bool,
    },
}

#[derive(Debug, Args)]
struct MetadataArgs {
    #[command(subcommand)]
    command: MetadataCommand,
}

#[derive(Debug, Subcommand)]
enum MetadataCommand {
    /// Index existing bundles under a directory without rewriting them.
    Create {
        input:  PathBuf,
        output: PathBuf,
        #[command(flatten)]
        target: TargetArgs,
    },
    /// List indexed bundles and files.
    List {
        /// metadata.store file to inspect.
        file: PathBuf,
    },
}

#[derive(Debug, Args)]
struct TextureArgs {
    #[command(subcommand)]
    command: TextureCommand,
}

#[derive(Debug, Subcommand)]
enum TextureCommand {
    /// Build a cache from a JSON manifest of cooked GPU texture chunks.
    Create {
        manifest: PathBuf,
        output:   PathBuf,
        #[command(flatten)]
        target:   TargetArgs,
    },
    /// List textures and their dimensions.
    List {
        /// texture.cache file to inspect.
        file: PathBuf,
    },
}

#[derive(Debug, Args)]
struct TargetArgs {
    /// Target game format.
    #[arg(long, value_enum, default_value = "legacy")]
    format: BundleFormat,
    /// Replace an existing output file.
    #[arg(long)]
    force:  bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Compression {
    None,
    Zlib,
    Snappy,
    Lz4,
    Lz4hc,
}

impl From<Compression> for BundleCompression {
    fn from(value: Compression) -> Self {
        match value {
            Compression::None => Self::None,
            Compression::Zlib => Self::Zlib,
            Compression::Snappy => Self::Snappy,
            Compression::Lz4 => Self::Lz4,
            Compression::Lz4hc => Self::Lz4hc,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextureManifest {
    textures: Vec<TextureSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextureSource {
    name:           String,
    hash:           u32,
    width:          u16,
    height:         u16,
    mipcount:       u16,
    slice_count:    u16,
    base_alignment: u32,
    type1:          u8,
    type2:          u8,
    #[serde(default)]
    is_cube:        u8,
    #[serde(default)]
    unk1:           u8,
    #[serde(default)]
    time_stamp:     i64,
    chunks:         Vec<PathBuf>,
}

#[derive(Debug, Args)]
struct MergeArgs {
    /// Output format. Defaults to preserving the newest input bundle format.
    #[arg(long, value_enum)]
    format:      Option<BundleFormat>,
    /// Mod directories, from highest to lowest priority.
    #[arg(required = true)]
    inputs:      Vec<PathBuf>,
    /// Directory for the merged files.
    #[arg(short, long)]
    output:      PathBuf,
    /// Name of the output bundle.
    #[arg(long, default_value = "blob0.bundle")]
    bundle_name: String,
    /// Replace output files that already exist.
    #[arg(long)]
    force:       bool,
}

struct LoadedMod {
    bundle:        Bundle<'static>,
    metadata:      Metadata,
    texture_cache: Option<TextureCache>,
}

pub fn run() -> Result<()> {
    run_command(Cli::parse().command)
}

fn run_command(command: Command) -> Result<()> {
    match command {
        Command::Bundle(args) => run_bundle(args.command),
        Command::Metadata(args) => run_metadata(args.command),
        Command::Texture(args) => run_texture(args.command),
        Command::Merge(args) => run_merge(args),
    }
}

fn run_bundle(command: BundleCommand) -> Result<()> {
    match command {
        BundleCommand::Pack {
            input,
            output,
            compression,
            target,
        } => {
            let paths = input_files(&input)?;
            let mut items = Vec::with_capacity(paths.len());
            let absolute_output = std::path::absolute(&output)?;
            for path in paths {
                if std::path::absolute(&path)? == absolute_output {
                    return Err(
                        invalid_data("output bundle must not be included in its input directory".into()).into(),
                    );
                }
                let name = depot_name(path.strip_prefix(&input)?)?;
                let data = fs::read(&path).map_err(|error| contextual_io("read input", &path, error))?;
                items.push(BundleItem::new(name, &data, compression.into())?);
            }
            let bundle = Bundle::from_items(target.format, items);
            write_output(&output, &bundle.try_write()?, target.force)?;
            println!("Packed {} files into {}", bundle.items().len(), output.display());
            Ok(())
        },
        BundleCommand::List {
            file,
        } => {
            let bundle = read_bundle(&file)?;
            println!("{:>12}  {:>12}  {:<8}  PATH", "SIZE", "COMPRESSED", "CODEC");
            for item in bundle.items() {
                println!(
                    "{:>12}  {:>12}  {:<8?}  {}",
                    item.size(),
                    item.zsize(),
                    item.compression(),
                    item.name().to_string_lossy()
                );
            }
            println!("\n{} files", bundle.items().len());
            Ok(())
        },
        BundleCommand::Unpack {
            file,
            output,
            force,
        } => {
            let bundle = read_bundle(&file)?;
            let output = output.unwrap_or_else(|| default_unpack_path(&file));
            fs::create_dir_all(&output)?;

            for item in bundle.items() {
                let relative = safe_relative_path(item.name())?;
                let destination = output.join(relative);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                let data = item.decompressed()?;
                write_output(&destination, &data, force)?;
            }

            println!("Extracted {} files to {}", bundle.items().len(), output.display());
            Ok(())
        },
    }
}

fn run_metadata(command: MetadataCommand) -> Result<()> {
    match command {
        MetadataCommand::Create {
            input,
            output,
            target,
        } => {
            let mut bundles = Vec::new();
            for path in input_files(&input)? {
                if path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("bundle"))
                {
                    bundles.push((depot_name(path.strip_prefix(&input)?)?, read_bundle(&path)?));
                }
            }
            let sources: Vec<_> = bundles.iter().map(|(name, bundle)| (name.as_c_str(), bundle)).collect();
            let metadata = Metadata::from_bundle_files(target.format, &sources)?;
            write_output(&output, &metadata.try_write()?, target.force)?;
            println!("Indexed {} bundles into {}", sources.len(), output.display());
            Ok(())
        },
        MetadataCommand::List {
            file,
        } => {
            let metadata = read_metadata(&file)?;
            println!("Bundles:");
            for bundle_id in 1..metadata.bundle_infos.len() {
                let name = metadata.bundle_name(bundle_id).unwrap_or(c"");
                let info = metadata.bundle_infos[bundle_id];
                println!("  {:>6} files  {}", info.num_bundle_entries, name.to_string_lossy());
            }
            println!("\nFiles:");
            for (name, info) in metadata.file_infos.iter().skip(1) {
                println!("  {:>12}  {}", info.size_in_memory, name.to_string_lossy());
            }
            println!(
                "\n{} files in {} bundles",
                metadata.file_infos.len().saturating_sub(1),
                metadata.bundle_infos.len().saturating_sub(1)
            );
            Ok(())
        },
    }
}

fn run_texture(command: TextureCommand) -> Result<()> {
    match command {
        TextureCommand::Create {
            manifest,
            output,
            target,
        } => {
            let bytes = fs::read(&manifest).map_err(|error| contextual_io("read manifest", &manifest, error))?;
            let source: TextureManifest = serde_json::from_slice(&bytes)?;
            let root = manifest.parent().unwrap_or_else(|| Path::new("."));
            let mut cache = TextureCache::for_format(target.format);
            for texture in source.textures {
                let name = latin1_name(&texture.name)?;
                safe_relative_path(&name)?;
                let mut chunks = Vec::new();
                for chunk in texture.chunks {
                    let path = root.join(chunk);
                    chunks.push(fs::read(&path).map_err(|error| contextual_io("read texture chunk", &path, error))?);
                }
                let chunks: Vec<_> = chunks.iter().map(Vec::as_slice).collect();
                cache.insert(
                    name,
                    TextureCacheEntry {
                        hash: texture.hash,
                        base_width: texture.width,
                        base_height: texture.height,
                        mipcount: texture.mipcount,
                        slice_count: texture.slice_count,
                        base_alignment: texture.base_alignment,
                        type1: texture.type1,
                        type2: texture.type2,
                        is_cube: texture.is_cube,
                        unk1: texture.unk1,
                        time_stamp: texture.time_stamp,
                        ..TextureCacheEntry::default()
                    },
                    &chunks,
                )?;
            }
            write_output(&output, &cache.write(), target.force)?;
            println!("Created {} textures in {}", cache.entries.len(), output.display());
            Ok(())
        },
        TextureCommand::List {
            file,
        } => {
            let cache = read_texture_cache(&file)?;
            println!("{:>7}  {:>7}  {:>5}  {:>6}  PATH", "WIDTH", "HEIGHT", "MIPS", "SLICES");
            for (name, entry) in &cache.entries {
                println!(
                    "{:>7}  {:>7}  {:>5}  {:>6}  {}",
                    entry.base_width,
                    entry.base_height,
                    entry.mipcount,
                    entry.slice_count,
                    name.to_string_lossy()
                );
            }
            println!("\n{} textures", cache.entries.len());
            Ok(())
        },
    }
}

fn run_merge(args: MergeArgs) -> Result<()> {
    if args.bundle_name.contains(['/', '\\']) {
        return Err(invalid_data("bundle name must not contain path separators".to_string()).into());
    }
    let bundle_name = latin1_name(&args.bundle_name)?;
    safe_relative_path(&bundle_name)?;
    if args.bundle_name.eq_ignore_ascii_case("metadata.store")
        || args.bundle_name.eq_ignore_ascii_case("texture.cache")
        || args.bundle_name.ends_with(['.', ' '])
    {
        return Err(invalid_data("bundle name conflicts with a companion file or is not portable".into()).into());
    }
    let loaded = args
        .inputs
        .iter()
        .map(|path| read_mod(path))
        .collect::<Result<Vec<_>>>()?;
    let inputs = loaded
        .iter()
        .map(|source| {
            let input = ModInput::new(&source.bundle, &source.metadata);
            source
                .texture_cache
                .as_ref()
                .map_or(input, |cache| input.with_texture_cache(cache))
        })
        .collect::<Vec<_>>();
    let merged = match args.format {
        Some(format) => w3edit::merge_mods_for_format(&inputs, &bundle_name, format)?,
        None => merge_mods(&inputs, &bundle_name),
    };

    let mut outputs = vec![
        (args.output.join(&args.bundle_name), merged.bundle.try_write()?),
        (args.output.join("metadata.store"), merged.metadata.try_write()?),
    ];
    if let Some(cache) = merged.texture_cache {
        outputs.push((args.output.join("texture.cache"), cache.write()));
    }
    preflight_outputs(&outputs, args.force)?;
    fs::create_dir_all(&args.output)?;
    for (path, bytes) in outputs {
        write_output(&path, &bytes, args.force)?;
    }

    println!(
        "Merged {} mods into {} ({} files)",
        loaded.len(),
        args.output.display(),
        merged.bundle.items().len()
    );
    Ok(())
}

fn input_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| contextual_io("read directory", root, error))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(invalid_data(format!("symlink inputs are not supported: {}", entry.path().display())).into());
        } else if kind.is_dir() {
            files.extend(input_files(&entry.path())?);
        } else if kind.is_file() {
            files.push(entry.path());
        } else {
            return Err(invalid_data(format!("input is not a regular file: {}", entry.path().display())).into());
        }
    }
    files.sort();
    Ok(files)
}

fn latin1_name(name: &str) -> Result<CString> {
    let bytes = name
        .chars()
        .map(|character| u8::try_from(character as u32))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| invalid_data(format!("name cannot be encoded as ISO-8859-1: {name}")))?;
    Ok(CString::new(bytes)?)
}

fn depot_name(path: &Path) -> Result<CString> {
    let name = path
        .to_str()
        .ok_or_else(|| invalid_data("input path is not valid Unicode".into()))?;
    let name = latin1_name(&name.replace('/', "\\"))?;
    safe_relative_path(&name)?;
    Ok(name)
}

fn read_bundle(path: &Path) -> Result<Bundle<'static>> {
    let data = fs::read(path).map_err(|error| contextual_io("read bundle", path, error))?;
    Bundle::parse(data).map_err(Into::into)
}

fn read_metadata(path: &Path) -> Result<Metadata> {
    let data = fs::read(path).map_err(|error| contextual_io("read metadata", path, error))?;
    Metadata::parse(&data).map_err(Into::into)
}

fn read_texture_cache(path: &Path) -> Result<TextureCache> {
    let data = fs::read(path).map_err(|error| contextual_io("read texture cache", path, error))?;
    TextureCache::parse(&data).map_err(Into::into)
}

fn read_mod(path: &Path) -> Result<LoadedMod> {
    if !path.is_dir() {
        return Err(invalid_data(format!("mod input is not a directory: {}", path.display())).into());
    }
    let texture_path = path.join("texture.cache");
    Ok(LoadedMod {
        bundle:        read_bundle(&path.join("blob0.bundle"))?,
        metadata:      read_metadata(&path.join("metadata.store"))?,
        texture_cache: texture_path
            .exists()
            .then(|| read_texture_cache(&texture_path))
            .transpose()?,
    })
}

fn preflight_outputs(outputs: &[(PathBuf, Vec<u8>)], force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    if let Some((path, _)) = outputs.iter().find(|(path, _)| path.exists()) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "output already exists: '{}' (use --force to replace it)",
                path.display()
            ),
        )
        .into());
    }
    Ok(())
}

fn default_unpack_path(bundle: &Path) -> PathBuf {
    let name = bundle.file_stem().unwrap_or(bundle.as_os_str());
    bundle.with_file_name(format!("{}.unpacked", name.to_string_lossy()))
}

fn safe_relative_path(name: &CStr) -> Result<PathBuf> {
    let bytes = name.to_bytes();
    if bytes.is_empty() || matches!(bytes.first(), Some(b'/' | b'\\')) {
        return Err(invalid_data(format!("unsafe bundle path: {}", name.to_string_lossy())).into());
    }

    let mut path = PathBuf::new();
    for component in bytes.split(|byte| matches!(byte, b'/' | b'\\')) {
        if component.is_empty() || component == b"." || component == b".." || component.contains(&b':') {
            return Err(invalid_data(format!("unsafe bundle path: {}", name.to_string_lossy())).into());
        }
        let component: String = component.iter().map(|&byte| char::from(byte)).collect();
        path.push(component);
    }
    Ok(path)
}

fn write_output(path: &Path, data: &[u8], force: bool) -> Result<()> {
    if force {
        fs::write(path, data).map_err(|error| contextual_io("write file", path, error))?;
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| contextual_io("create file (use --force to replace it)", path, error))?;
    file.write_all(data)
        .map_err(|error| contextual_io("write file", path, error))?;
    Ok(())
}

fn contextual_io(action: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("failed to {action} '{}': {error}", path.display()),
    )
}

fn invalid_data(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn merge_rejects_unsafe_or_conflicting_bundle_names_before_loading_inputs() {
        for name in [
            "",
            ".",
            "..",
            "../blob0.bundle",
            r"C:\blob0.bundle",
            "metadata.store",
            "METADATA.STORE",
            "texture.cache",
            "TEXTURE.CACHE",
            "metadata.store.",
            "metadata.store ",
        ] {
            for force in [false, true] {
                let error = run_merge(MergeArgs {
                    format: None,
                    inputs: vec![PathBuf::from("nonexistent-input")],
                    output: PathBuf::from("unused-output"),
                    bundle_name: name.into(),
                    force,
                })
                .unwrap_err();
                let error = error.downcast_ref::<io::Error>().unwrap();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData, "accepted {name:?}");
                assert!(!error.to_string().contains("mod input"), "accepted {name:?}");
            }
        }
    }

    #[test]
    fn bundle_path_normalizes_witcher_separators() {
        let name = CString::new("content\\quests/main.ws").unwrap();
        assert_eq!(
            safe_relative_path(&name).unwrap(),
            PathBuf::from("content/quests/main.ws")
        );
    }

    #[test]
    fn bundle_path_rejects_traversal_and_absolute_paths() {
        for name in ["../secret", "content/../../secret", "/absolute", r"C:\absolute"] {
            let name = CString::new(name).unwrap();
            assert!(safe_relative_path(&name).is_err(), "accepted {name:?}");
        }
    }
}
