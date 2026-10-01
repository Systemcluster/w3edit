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
        let merged_dir = merged_dir.join("content");
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
fn mod_roots_work_across_container_entry_points() {
    use std::fs;
    use w3edit::{Bundle, Metadata, TextureCache};

    let root = std::env::temp_dir().join(format!("w3edit-root-entrypoints-{}", std::process::id()));
    let cooked = root.join("cookedMod");
    fs::create_dir_all(cooked.join("content/nested")).unwrap();
    fs::write(cooked.join("content/nested/file.bin"), b"payload").unwrap();
    fs::write(cooked.join("outside.txt"), b"not content").unwrap();
    let manifest = root.join("textures.json");
    fs::write(
        &manifest,
        serde_json::to_vec(&serde_json::json!({"textures": [{
            "name": "textures\\test.xbm", "hash": 42, "width": 4, "height": 4,
            "mipcount": 1, "slice_count": 1, "base_alignment": 16, "type1": 7, "type2": 4,
            "chunks": ["cookedMod/content/nested/file.bin"]
        }]}))
        .unwrap(),
    )
    .unwrap();

    for explicit_content in [false, true] {
        let mod_root = root.join(format!("modCreated-{explicit_content}"));
        let content = mod_root.join("content");
        let destination = if explicit_content { &content } else { &mod_root };
        let result = command()
            .args(["bundle", "pack"])
            .arg(&cooked)
            .arg(destination)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let bundle = Bundle::parse(fs::read(content.join("blob0.bundle")).unwrap()).unwrap();
        assert_eq!(bundle.items().len(), 1);
        assert_eq!(bundle.items()[0].name(), c"nested\\file.bin");
        for (subcommand, input) in [("metadata", destination), ("texture", &manifest)] {
            let result = command()
                .args([subcommand, "create"])
                .arg(input)
                .arg(destination)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
        }
        let metadata = Metadata::parse(&fs::read(content.join("metadata.store")).unwrap()).unwrap();
        assert_eq!(metadata.bundle_name(1).unwrap(), c"blob0.bundle");
        assert_eq!(metadata.entry_infos[1].offset_in_bundle, bundle.items()[0].offset());
        for input in [&mod_root, &content] {
            for subcommand in ["bundle", "metadata", "texture"] {
                let result = command().args([subcommand, "list"]).arg(input).output().unwrap();
                assert!(result.status.success(), "{result:?}");
            }
        }
        let unpacked = root.join(format!("unpacked-{explicit_content}"));
        let result = command()
            .args(["bundle", "unpack"])
            .arg(destination)
            .arg(&unpacked)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(fs::read(unpacked.join("nested/file.bin")).unwrap(), b"payload");
        assert!(!unpacked.join("content").exists());
        for (subcommand, action, input, filename) in [
            ("bundle", "pack", &cooked, "blob0.bundle"),
            ("metadata", "create", destination, "metadata.store"),
            ("texture", "create", &manifest, "texture.cache"),
        ] {
            let original = fs::read(content.join(filename)).unwrap();
            let result = command()
                .args([subcommand, action])
                .arg(input)
                .arg(destination)
                .output()
                .unwrap();
            assert!(!result.status.success(), "{result:?}");
            assert_eq!(fs::read(content.join(filename)).unwrap(), original);
            let result = command()
                .args([subcommand, action])
                .arg(input)
                .arg(destination)
                .arg("--force")
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
        }
        fs::create_dir_all(content.join("scripts")).unwrap();
        fs::write(content.join("scripts/test.ws"), b"script").unwrap();
        let converted_root = root.join(format!("modConverted-{explicit_content}"));
        let converted_content = converted_root.join("content");
        let converted_output = if explicit_content {
            &converted_content
        } else {
            &converted_root
        };
        let result = command()
            .arg("convert")
            .arg(destination)
            .arg(converted_output)
            .args(["--format", "remastered"])
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let converted_bundle = Bundle::parse(fs::read(converted_content.join("blob0.bundle")).unwrap()).unwrap();
        let converted_metadata = Metadata::parse(&fs::read(converted_content.join("metadata.store")).unwrap()).unwrap();
        let converted_cache = TextureCache::parse(&fs::read(converted_content.join("texture.cache")).unwrap()).unwrap();
        assert_eq!(converted_bundle.version(), 5);
        assert_eq!(converted_metadata.version, 7);
        assert_eq!(converted_cache.version, 7);
        assert_eq!(
            converted_metadata.entry_infos[1].offset_in_bundle,
            converted_bundle.items()[0].offset()
        );
        assert_eq!(fs::read(converted_content.join("scripts/test.ws")).unwrap(), b"script");
        assert!(!converted_content.join("content").exists());
        let result = command()
            .arg("convert")
            .arg(destination)
            .arg(converted_output)
            .args(["--format", "legacy"])
            .output()
            .unwrap();
        assert!(!result.status.success(), "{result:?}");
        let result = command()
            .arg("convert")
            .arg(destination)
            .arg(converted_output)
            .args(["--format", "legacy", "--force"])
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let result = command()
            .arg("convert")
            .arg(destination)
            .arg(destination)
            .args(["--format", "remastered", "--force"])
            .output()
            .unwrap();
        assert!(!result.status.success(), "{result:?}");
        let merged_root = root.join(format!("modRoundtrip-{explicit_content}"));
        let result = command()
            .arg("merge")
            .arg(&converted_root)
            .arg("--output")
            .arg(&merged_root)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(
            fs::read(merged_root.join("content/scripts/test.ws")).unwrap(),
            b"script"
        );
        for filename in ["blob0.bundle", "metadata.store", "texture.cache"] {
            let container_mod = root.join(format!("single-{filename}-{explicit_content}"));
            fs::create_dir_all(&container_mod).unwrap();
            let container_output = if explicit_content {
                container_mod.join("content")
            } else {
                container_mod.clone()
            };
            let result = command()
                .arg("convert")
                .arg(content.join(filename))
                .arg(container_output)
                .args(["--format", "remastered"])
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            assert!(container_mod.join("content").join(filename).is_file());
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn merge_includes_all_bundles_referenced_by_metadata() {
    use std::fs;
    use w3edit::{Bundle, BundleCompression, BundleFormat, BundleItem, Metadata};

    let root = std::env::temp_dir().join(format!("w3edit-multi-bundle-{}", std::process::id()));
    let input = root.join("input");
    let output_dir = root.join("output");
    fs::create_dir_all(input.join("content")).unwrap();

    let first = Bundle::from_items(BundleFormat::Legacy, vec![
        BundleItem::new(c"first.bin".into(), b"first", BundleCompression::Zlib).unwrap(),
    ]);
    let second = Bundle::from_items(BundleFormat::Legacy, vec![
        BundleItem::new(c"second.bin".into(), b"second", BundleCompression::Zlib).unwrap(),
    ]);
    fs::write(input.join("blob0.bundle"), first.write()).unwrap();
    fs::write(input.join("content/blob1.bundle"), second.write()).unwrap();
    let metadata = Metadata::from_bundle_files(BundleFormat::Legacy, &[
        (c"blob0.bundle", &first),
        (c"content\\blob1.bundle", &second),
    ])
    .unwrap();
    fs::write(input.join("metadata.store"), metadata.write()).unwrap();

    let result = command()
        .arg("merge")
        .arg(&input)
        .arg("--output")
        .arg(&output_dir)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");

    let output_dir = output_dir.join("content");
    let merged = Bundle::parse(fs::read(output_dir.join("blob0.bundle")).unwrap()).unwrap();
    let merged_metadata = Metadata::parse(&fs::read(output_dir.join("metadata.store")).unwrap()).unwrap();
    assert_eq!(merged.items().len(), 2);
    for (name, payload) in [
        (c"first.bin", b"first".as_slice()),
        (c"second.bin", b"second".as_slice()),
    ] {
        let item = merged.items().iter().find(|item| item.name() == name).unwrap();
        assert_eq!(item.decompressed().unwrap(), payload);
        let file_id = merged_metadata.find_file_id_by_name(name.to_bytes()).unwrap();
        let entry_id = merged_metadata.file_infos[file_id].1.first_entry as usize;
        assert_eq!(merged_metadata.entry_infos[entry_id].offset_in_bundle, item.offset());
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn merge_resolves_real_mod_layouts_and_conflict_only_overrides() {
    use std::fs;
    use w3edit::{Bundle, BundleCompression, BundleFormat, BundleItem, Metadata, TextureCache, TextureCacheEntry};

    let root = std::env::temp_dir().join(format!("w3edit-merge-workflow-{}", std::process::id()));
    let first = root.join("modFirst");
    let second = root.join("modSecond/content");
    for (content, payload, unique, hash) in [
        (first.join("content"), b"first".as_slice(), c"first.bin", 11),
        (second.clone(), b"second choice".as_slice(), c"second.bin", 22),
    ] {
        fs::create_dir_all(content.join("scripts/game")).unwrap();
        let bundle = Bundle::from_items(BundleFormat::Legacy, vec![
            BundleItem::new(c"shared.bin".into(), payload, BundleCompression::Zlib).unwrap(),
            BundleItem::new(unique.into(), payload, BundleCompression::None).unwrap(),
        ]);
        let metadata = Metadata::from_bundles(BundleFormat::Legacy, &[(c"blob0.bundle", &bundle)]).unwrap();
        fs::write(content.join("blob0.bundle"), bundle.write()).unwrap();
        fs::write(content.join("metadata.store"), metadata.write()).unwrap();
        fs::write(content.join("scripts/game/player.ws"), payload).unwrap();
        fs::write(content.join(unique.to_str().unwrap()), payload).unwrap();
        let mut cache = TextureCache::new();
        for name in [c"textures\\shared.xbm", unique] {
            cache
                .insert(
                    name.into(),
                    TextureCacheEntry {
                        hash,
                        base_width: 4,
                        base_height: 4,
                        mipcount: 1,
                        slice_count: 1,
                        ..TextureCacheEntry::default()
                    },
                    &[payload],
                )
                .unwrap();
        }
        fs::write(content.join("texture.cache"), cache.write()).unwrap();
    }
    let unindexed = Bundle::from_items(BundleFormat::Legacy, vec![
        BundleItem::new(c"unindexed.bin".into(), b"extra", BundleCompression::None).unwrap(),
        BundleItem::new(
            c"first.bin".into(),
            b"duplicate within one mod",
            BundleCompression::None,
        )
        .unwrap(),
    ]);
    fs::write(first.join("content/extra.bundle"), unindexed.write()).unwrap();
    let original_bundle = fs::read(first.join("content/blob0.bundle")).unwrap();
    for format in ["legacy", "remastered"] {
        for (mode, conflicts_only, prefer_second) in
            [("full", false, false), ("patch", true, false), ("choices", true, true)]
        {
            let output = root.join(format!("{format}-{mode}/content"));
            let mut merge = command();
            merge
                .arg("merge")
                .arg(&first)
                .arg(&second)
                .arg("--output")
                .arg(if mode == "choices" {
                    output.as_path()
                } else {
                    output.parent().unwrap()
                })
                .args(["--format", format]);
            if conflicts_only {
                merge.arg("--conflicts-only");
            }
            if prefer_second {
                merge.args([
                    "--prefer",
                    "SHARED.bin=2",
                    "--prefer",
                    "scripts/game/player.ws=2",
                    "--prefer",
                    "textures/shared.xbm=2",
                ]);
            }
            let result = merge.output().unwrap();
            assert!(result.status.success(), "{result:?}");
            let bundle = Bundle::parse(fs::read(output.join("blob0.bundle")).unwrap()).unwrap();
            let metadata = Metadata::parse(&fs::read(output.join("metadata.store")).unwrap()).unwrap();
            let cache = TextureCache::parse(&fs::read(output.join("texture.cache")).unwrap()).unwrap();
            let expected = if prefer_second {
                b"second choice".as_slice()
            } else {
                b"first".as_slice()
            };
            assert_eq!(bundle.items().len(), if conflicts_only { 1 } else { 4 });
            assert_eq!(
                bundle
                    .items()
                    .iter()
                    .find(|item| item.name() == c"shared.bin")
                    .unwrap()
                    .decompressed()
                    .unwrap(),
                expected
            );
            assert_eq!(fs::read(output.join("scripts/game/player.ws")).unwrap(), expected);
            assert_eq!(output.join("first.bin").exists(), !conflicts_only);
            assert_eq!(output.join("second.bin").exists(), !conflicts_only);
            assert_eq!(cache.entries.len(), if conflicts_only { 1 } else { 3 });
            assert_eq!(cache.entries[0].1.hash, if prefer_second { 22 } else { 11 });
            assert_eq!(metadata.file_infos.len(), bundle.items().len() + 1);
            for item in bundle.items() {
                let file_id = metadata.find_file_id_by_name(item.name().to_bytes()).unwrap();
                let entry_id = metadata.file_infos[file_id].1.first_entry as usize;
                assert_eq!(metadata.entry_infos[entry_id].offset_in_bundle, item.offset());
            }
            assert_eq!(bundle.version(), if format == "legacy" { 3 } else { 5 });
            assert_eq!(cache.version, metadata.version);
        }
    }
    let reversed = root.join("reversed");
    let result = command()
        .arg("merge")
        .arg(&second)
        .arg(&first)
        .arg("--output")
        .arg(&reversed)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let reversed = reversed.join("content");
    assert_eq!(
        fs::read(reversed.join("scripts/game/player.ws")).unwrap(),
        b"second choice"
    );
    let bundle = Bundle::parse(fs::read(reversed.join("blob0.bundle")).unwrap()).unwrap();
    assert_eq!(bundle.items()[0].decompressed().unwrap(), b"second choice");
    assert_eq!(fs::read(first.join("content/blob0.bundle")).unwrap(), original_bundle);
    assert_eq!(
        fs::read(first.join("content/scripts/game/player.ws")).unwrap(),
        b"first"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn merge_script_only_mods_validates_choices_and_protects_sources() {
    use std::fs;

    let root = std::env::temp_dir().join(format!("w3edit-merge-scripts-{}", std::process::id()));
    let first = root.join("modFirst");
    let second = root.join("modSecond");
    for (input, payload) in [(&first, b"first".as_slice()), (&second, b"second".as_slice())] {
        fs::create_dir_all(input.join("content/scripts")).unwrap();
        fs::write(input.join("content/scripts/player.ws"), payload).unwrap();
    }
    let output = root.join("merged/content");
    let result = command()
        .arg("merge")
        .arg(&first)
        .arg(&second)
        .args(["--conflicts-only", "--prefer", "scripts/player.ws=2"])
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(fs::read(output.join("scripts/player.ws")).unwrap(), b"second");
    assert!(!output.join("blob0.bundle").exists());
    assert!(!output.join("metadata.store").exists());
    for duplicate in [&first, &first.join("content")] {
        let destination = root.join("duplicate");
        let result = command()
            .arg("merge")
            .arg(&first)
            .arg(duplicate)
            .arg("--conflicts-only")
            .arg("--output")
            .arg(&destination)
            .output()
            .unwrap();
        assert!(!result.status.success(), "{result:?}");
        assert!(
            String::from_utf8(result.stderr)
                .unwrap()
                .contains("duplicate mod input")
        );
        assert!(!destination.exists());
    }
    for preference in ["missing.ws=1", "scripts/player.ws=0", "scripts/player.ws=3", "invalid"] {
        let destination = root.join("invalid");
        let result = command()
            .arg("merge")
            .arg(&first)
            .arg(&second)
            .arg("--prefer")
            .arg(preference)
            .arg("--output")
            .arg(&destination)
            .output()
            .unwrap();
        assert!(!result.status.success(), "{preference}: {result:?}");
        assert!(!destination.exists());
    }
    for destination in [&first, &first.join("content/nested"), &root, &output] {
        let result = command()
            .arg("merge")
            .arg(&first)
            .arg(&second)
            .args(["--conflicts-only", "--force"])
            .arg("--output")
            .arg(destination)
            .output()
            .unwrap();
        assert!(!result.status.success(), "{result:?}");
    }
    let empty = root.join("no-conflicts");
    let result = command()
        .arg("merge")
        .arg(&first)
        .arg("--conflicts-only")
        .arg("--output")
        .arg(&empty)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(fs::read_dir(empty.join("content")).unwrap().count(), 0);
    assert_eq!(fs::read(first.join("content/scripts/player.ws")).unwrap(), b"first");
    assert_eq!(fs::read(second.join("content/scripts/player.ws")).unwrap(), b"second");
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
