#![cfg(feature = "cli")]

use std::path::Path;
use std::process::Command;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_w3edit"));
    command.current_dir(std::env::temp_dir()).env_remove("RUST_LOG");
    command
}

#[test]
fn convert_containers_in_both_directions_with_output_protection() {
    use std::fs;
    use w3edit::{Bundle, BundleCompression, BundleFormat, BundleItem, Metadata, TextureCache, convert};

    let root = std::env::temp_dir().join(format!("w3edit-convert-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let bundle = Bundle::from_items(BundleFormat::Legacy, vec![
        BundleItem::new(c"file.bin".into(), b"payload", BundleCompression::Zlib).unwrap(),
    ]);
    let metadata = Metadata::from_bundles(BundleFormat::Legacy, &[(c"blob0.bundle", &bundle)]).unwrap();
    for (name, original) in [
        ("bundle", bundle.write()),
        ("metadata", metadata.write()),
        ("texture", TextureCache::new().write()),
    ] {
        let input = root.join(format!("{name}.input"));
        let output_path = root.join(format!("{name}.output"));
        fs::write(&input, &original).unwrap();
        let output = command()
            .arg("convert")
            .arg(&input)
            .arg(&output_path)
            .args(["--format", "remastered"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(fs::read(&input).unwrap(), original);
        assert_eq!(
            fs::read(&output_path).unwrap(),
            convert(&original, BundleFormat::Remastered).unwrap()
        );
        let output = command()
            .arg("convert")
            .arg(&output_path)
            .arg(&input)
            .args(["--format", "legacy"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read(&input).unwrap(), original);
        let output = command()
            .arg("convert")
            .arg(&output_path)
            .arg(&input)
            .args(["--format", "legacy", "--force"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(fs::read(&input).unwrap(), original);
    }
    let input = root.join("invalid");
    let output_path = root.join("protected");
    fs::write(&output_path, b"keep me").unwrap();
    for invalid in [b"".as_slice(), b"unknown", b"POTATO70", b"\x03VTM", b"HCXT\x06\0\0\0"] {
        fs::write(&input, invalid).unwrap();
        let output = command()
            .arg("convert")
            .arg(&input)
            .arg(&output_path)
            .args(["--format", "legacy", "--force"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(fs::read(&output_path).unwrap(), b"keep me");
    }
    let output = command().arg("convert").arg(&input).arg(&output_path).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr).unwrap().contains("--format"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn create_bundle_store_and_cache_for_both_games() {
    use std::fs;
    use w3edit::{Bundle, Metadata, TextureCache};
    let root = std::env::temp_dir().join(format!("w3edit-create-{}", std::process::id()));
    fs::create_dir_all(root.join("input/nested")).unwrap();
    fs::write(root.join("input/nested/file.bin"), b"test payload").unwrap();
    let manifest = serde_json::json!({"textures": [{
        "name": "textures\\test.xbm", "hash": 42, "width": 4, "height": 4,
        "mipcount": 1, "slice_count": 1, "base_alignment": 16, "type1": 7, "type2": 4,
        "chunks": ["input/nested/file.bin"]
    }]});
    fs::write(root.join("textures.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    for (format, bundle_version, companion_version) in [("legacy", 3, 6), ("remastered", 5, 7)] {
        let output_dir = root.join(format);
        fs::create_dir_all(&output_dir).unwrap();
        let bundle_path = output_dir.join("blob0.bundle");
        let output = command()
            .args(["bundle", "pack"])
            .arg(root.join("input"))
            .arg(&bundle_path)
            .args(["--format", format])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let bundle = Bundle::parse(fs::read(&bundle_path).unwrap()).unwrap();
        assert_eq!(bundle.version(), bundle_version);
        assert_eq!(bundle.items()[0].decompressed().unwrap(), b"test payload");
        assert_eq!(bundle.items()[0].name(), c"nested\\file.bin");
        let output = command()
            .args(["metadata", "create"])
            .arg(&output_dir)
            .arg(output_dir.join("metadata.store"))
            .args(["--format", format])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let metadata = Metadata::parse(&fs::read(output_dir.join("metadata.store")).unwrap()).unwrap();
        assert_eq!(metadata.version, companion_version);
        assert_eq!(metadata.entry_infos[1].offset_in_bundle, bundle.items()[0].offset());
        let output = command()
            .args(["texture", "create"])
            .arg(root.join("textures.json"))
            .arg(output_dir.join("texture.cache"))
            .args(["--format", format])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let cache = TextureCache::parse(&fs::read(output_dir.join("texture.cache")).unwrap()).unwrap();
        assert_eq!(cache.version, companion_version);
        assert_eq!(cache.entries.len(), 1);
        let merged_dir = root.join(format!("merged-{format}"));
        let output = command()
            .arg("merge")
            .arg(&output_dir)
            .arg("--output")
            .arg(&merged_dir)
            .args(["--format", format])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let merged_path = merged_dir.join("blob0.bundle");
        let merged_bytes = fs::read(&merged_path).unwrap();
        let merged_bundle = Bundle::parse(&merged_bytes).unwrap();
        let merged_metadata = Metadata::parse(&fs::read(merged_dir.join("metadata.store")).unwrap()).unwrap();
        let merged_cache = TextureCache::parse(&fs::read(merged_dir.join("texture.cache")).unwrap()).unwrap();
        assert_eq!(merged_bundle.version(), bundle_version);
        assert_eq!(merged_metadata.version, companion_version);
        assert_eq!(merged_cache.version, companion_version);
        assert_eq!(
            merged_metadata.entry_infos[1].offset_in_bundle,
            merged_bundle.items()[0].offset()
        );
        let unpacked_dir = root.join(format!("unpacked-{format}"));
        let output = command()
            .args(["bundle", "unpack"])
            .arg(&merged_path)
            .arg(&unpacked_dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let unpacked_path = unpacked_dir.join("nested/file.bin");
        assert_eq!(fs::read(&unpacked_path).unwrap(), b"test payload");
        fs::write(&unpacked_path, b"existing data").unwrap();
        let output = command()
            .args(["bundle", "unpack"])
            .arg(&merged_path)
            .arg(&unpacked_dir)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read(&unpacked_path).unwrap(), b"existing data");
        let output = command()
            .args(["bundle", "unpack"])
            .arg(&merged_path)
            .arg(&unpacked_dir)
            .arg("--force")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(fs::read(&unpacked_path).unwrap(), b"test payload");
        let output = command()
            .args(["bundle", "pack"])
            .arg(root.join("input"))
            .arg(&bundle_path)
            .args(["--format", format])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(
            Bundle::parse(fs::read(&bundle_path).unwrap()).unwrap().version(),
            bundle_version
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn help_lists_commands() {
    let output = command().arg("--help").output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    for name in ["bundle", "metadata", "texture", "merge", "convert"] {
        assert!(stdout.contains(name), "missing command {name}: {stdout}");
    }
}

#[test]
fn bundle_list_works_outside_the_workspace() {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/assets/modnmm/blob0.bundle");
    let output = command().args(["bundle", "list"]).arg(bundle).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("COMPRESSED"));
    assert!(stdout.contains("PATH"));
    assert!(stdout.contains(" files"));
}

#[test]
fn read_failure_reports_error_and_exits_unsuccessfully() {
    let output = command()
        .args(["bundle", "list"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml/missing.bundle"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error: failed to read bundle"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}
