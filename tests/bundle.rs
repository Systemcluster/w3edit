//! Integration tests for `Bundle` parse / decompress / write / merge,
//! driven by the fixtures under `test/assets/`.

use std::collections::HashSet;
use std::ffi::CString;
use std::path::PathBuf;

use w3edit::bundle::{Bundle, BundleCompression};

#[test]
fn convert_bundle_preserves_compressed_payloads() {
    use w3edit::{BundleFormat, BundleItem, convert};

    let items = [
        BundleCompression::None,
        BundleCompression::Zlib,
        BundleCompression::Snappy,
        BundleCompression::Lz4,
        BundleCompression::Lz4hc,
    ]
    .into_iter()
    .map(|codec| BundleItem::new(CString::new(format!("{codec:?}.bin")).unwrap(), b"payload", codec).unwrap())
    .collect();
    let original = Bundle::from_items(BundleFormat::Legacy, items);
    let mut bytes = original.write();
    for (format, version) in [
        (BundleFormat::Remastered, 5),
        (BundleFormat::Legacy, 3),
        (BundleFormat::Legacy, 3),
    ] {
        bytes = convert(&bytes, format).unwrap();
        let parsed = Bundle::parse(bytes.as_slice()).unwrap();
        assert_eq!(parsed.version(), version);
        assert_eq!(parsed.items().len(), original.items().len());
        for (actual, expected) in parsed.items().iter().zip(original.items()) {
            assert_eq!(actual.name(), expected.name());
            assert_eq!(actual.raw(), expected.raw());
            assert_eq!(actual.compression(), expected.compression());
            assert_eq!(actual.crc(), expected.crc());
            assert_eq!(actual.decompressed().unwrap(), b"payload");
        }
    }
}

#[test]
fn create_both_bundle_formats_with_supported_codecs() {
    use w3edit::{BundleFormat, BundleItem};
    for format in [BundleFormat::Legacy, BundleFormat::Remastered] {
        for codec in [
            BundleCompression::None,
            BundleCompression::Zlib,
            BundleCompression::Snappy,
            BundleCompression::Lz4,
            BundleCompression::Lz4hc,
        ] {
            let item = BundleItem::new(c"test\\file.bin".into(), b"123456789", codec).unwrap();
            let bundle = Bundle::from_items(format, vec![item]);
            let parsed = Bundle::parse(bundle.write()).unwrap();
            assert_eq!(parsed.version(), if format == BundleFormat::Legacy { 3 } else { 5 });
            assert_eq!(parsed.items()[0].decompressed().unwrap(), b"123456789");
            assert_eq!(parsed.items()[0].crc(), 0xcbf43926);
            assert_eq!(parsed.items()[0].compression(), codec);
        }
    }
}

// -----------------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------------

/// Well-formed bundles used across most tests.
const BUNDLES: &[&str] = &["modnmm/blob0.bundle", "modsbn/blob0.bundle", "modtest/blob0.bundle"];

/// A deliberately truncated stub — bundle header claims 8192 bytes but the file
/// is 352 bytes long with no data pages. Used to verify parse error handling.
const TRUNCATED_BUNDLE: &str = "modtest2/blob0.bundle";

/// Filename shared between `modtest/blob0.bundle` and `modsbn/blob0.bundle`
/// (verified by inspecting the fixtures). Used to test merge priority.
const SHARED_ITEM: &str = r"items\cutscenes\succubus_cs\succubus_cs.w2ent";

fn asset(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("test/assets");
    p.push(rel);
    p
}

fn load(rel: &str) -> Vec<u8> {
    std::fs::read(asset(rel)).unwrap_or_else(|e| panic!("cannot read fixture {rel}: {e}"))
}

// -----------------------------------------------------------------------------
// Parse
// -----------------------------------------------------------------------------

#[test]
fn parse_yields_items_for_every_fixture() {
    for &b in BUNDLES {
        let data = load(b);
        let bundle = Bundle::parse(&*data).expect("valid bundle should parse");
        assert!(!bundle.items().is_empty(), "{b}: expected at least one item");
    }
}

#[test]
fn parse_accepts_borrowed_and_owned_input_equivalently() {
    for &b in BUNDLES {
        let data = load(b);
        let from_borrow = Bundle::parse(&*data).unwrap();
        let from_owned = Bundle::parse(data.clone()).unwrap();
        assert_eq!(from_borrow.items().len(), from_owned.items().len(), "{b}");
        for (l, r) in from_borrow.items().iter().zip(from_owned.items()) {
            assert_eq!(l.name(), r.name(), "{b}");
            assert_eq!(l.hash(), r.hash(), "{b}");
            assert_eq!(l.zsize(), r.zsize(), "{b}");
            assert_eq!(l.raw(), r.raw(), "{b}: compressed bytes differ");
        }
    }
}

#[test]
fn parse_rejects_truncated_bundle() {
    let data = load(TRUNCATED_BUNDLE);
    let result = Bundle::parse(&*data);
    assert!(result.is_err(), "truncated bundle should fail to parse");
}

#[test]
fn parse_rejects_bad_magic() {
    let mut data = load(BUNDLES[0]);
    data[0..8].copy_from_slice(b"NOTBUNDL");
    assert!(Bundle::parse(&*data).is_err());
}

#[test]
fn legacy_offset_does_not_include_timestamp() {
    use w3edit::{BundleFormat, BundleItem};

    let payload = b"legacy bundle payload";
    let item = BundleItem::new(c"test.txt".into(), payload, BundleCompression::None).unwrap();
    let mut bytes = Bundle::from_items(BundleFormat::Legacy, vec![item]).write();
    let date = (2015u32 << 20) | (5 << 15) | (19 << 10);
    let time = (12u32 << 22) | (34 << 16) | (56 << 10);
    bytes[0x140..0x144].copy_from_slice(&date.to_le_bytes());
    bytes[0x144..0x148].copy_from_slice(&time.to_le_bytes());

    for bundle in [Bundle::parse(bytes.as_slice()), Bundle::parse(bytes.clone())] {
        let bundle = bundle.expect("legacy timestamp must not become the offset's high word");
        assert_eq!(bundle.version(), 3);
        assert_eq!(bundle.items()[0].offset(), 4096);
        assert_eq!(bundle.items()[0].raw(), payload);
        assert_eq!(bundle.items()[0].header(), bytes[0x20..0x160]);
        assert_eq!(bundle.write(), bytes);
    }
}

fn remastered_bundle() -> Vec<u8> {
    let payload = b"remastered bundle payload";
    let data_start = 0x20 + 0x130;
    let mut bytes = vec![0; data_start + payload.len()];
    let total_size = bytes.len() as u64;
    bytes[..8].copy_from_slice(b"POTATO70");
    bytes[8..16].copy_from_slice(&total_size.to_le_bytes());
    bytes[16..20].copy_from_slice(&0x130u32.to_le_bytes());
    bytes[20..22].copy_from_slice(&5u16.to_le_bytes());
    bytes[22..30].copy_from_slice(&(data_start as u64).to_le_bytes());
    let entry = &mut bytes[0x20..data_start];
    entry[..8].copy_from_slice(b"test.txt");
    entry[0x100..0x110].copy_from_slice(&123u128.to_le_bytes());
    entry[0x110..0x118].copy_from_slice(&(data_start as u64).to_le_bytes());
    entry[0x118..0x11c].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    entry[0x11c..0x120].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes[data_start..].copy_from_slice(payload);
    bytes
}

#[test]
fn remastered_parse_and_roundtrip() {
    let bytes = remastered_bundle();
    for bundle in [Bundle::parse(bytes.as_slice()), Bundle::parse(bytes.clone())] {
        let bundle = bundle.expect("version 5 bundle should parse");
        assert_eq!(bundle.version(), 5);
        let item = &bundle.items()[0];
        assert_eq!(item.name().to_bytes(), b"test.txt");
        assert_eq!(item.hash(), 123);
        assert_eq!(item.offset(), 0x150);
        assert_eq!(item.compression(), BundleCompression::None);
        assert_eq!(item.decompressed().unwrap(), b"remastered bundle payload");
        assert_eq!(item.header(), bytes[0x20..0x150]);
        let written = bundle.write();
        assert_eq!(&written[20..22], &5u16.to_le_bytes());
        assert_eq!(u64::from_le_bytes(written[22..30].try_into().unwrap()), 0x150);
        let reparsed = Bundle::parse(written.as_slice()).unwrap();
        assert_eq!(reparsed.items()[0].raw(), item.raw());
        assert_eq!(reparsed.write(), written);
    }
}

#[test]
fn remastered_preserves_reserved_bytes() {
    let mut bytes = remastered_bundle();
    bytes[30..32].copy_from_slice(&[12, 34]);
    bytes[0x148..0x150].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let written = Bundle::parse(bytes.as_slice()).unwrap().write();
    assert_eq!(written[30..32], bytes[30..32]);
    assert_eq!(written[0x148..0x150], bytes[0x148..0x150]);
}

#[test]
fn remastered_decompresses_zlib() {
    use std::io::Write;

    let mut bytes = remastered_bundle();
    let payload = bytes[0x150..].to_vec();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&payload).unwrap();
    let compressed = encoder.finish().unwrap();
    bytes.truncate(0x150);
    bytes.extend(&compressed);
    let size = bytes.len() as u64;
    bytes[8..16].copy_from_slice(&size.to_le_bytes());
    bytes[0x13c..0x140].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
    bytes[0x144..0x148].copy_from_slice(&1u32.to_le_bytes());
    let bundle = Bundle::parse(bytes.as_slice()).unwrap();
    assert_eq!(bundle.items()[0].compression(), BundleCompression::Zlib);
    assert_eq!(bundle.items()[0].decompressed().unwrap(), payload);
    let written = bundle.write();
    let reparsed = Bundle::parse(written.as_slice()).unwrap();
    assert_eq!(reparsed.items()[0].raw(), compressed);
    assert_eq!(reparsed.items()[0].decompressed().unwrap(), payload);
}

#[test]
fn remastered_rejects_truncation_and_64bit_out_of_bounds_offsets() {
    let bytes = remastered_bundle();
    for length in [0, 20, 31, 0x14f, bytes.len() - 1] {
        assert!(Bundle::parse(&bytes[..length]).is_err(), "length {length}");
    }
    for offset in [0x1_0000_0150u64, u64::MAX] {
        let mut invalid = bytes.clone();
        invalid[0x130..0x138].copy_from_slice(&offset.to_le_bytes());
        assert!(Bundle::parse(invalid).is_err(), "offset {offset}");
    }
    let mut invalid = bytes;
    invalid[16..20].copy_from_slice(&0x140u32.to_le_bytes());
    assert!(Bundle::parse(invalid).is_err());
}

#[test]
fn parse_rejects_unknown_versions() {
    for version in [0u16, 4, 6, u16::MAX] {
        let mut bytes = remastered_bundle();
        bytes[20..22].copy_from_slice(&version.to_le_bytes());
        assert!(Bundle::parse(bytes).is_err(), "version {version}");
    }
}

#[test]
fn merge_legacy_and_remastered_uses_version_5() {
    let legacy_bytes = load("modtest/blob0.bundle");
    let legacy = Bundle::parse(legacy_bytes.as_slice()).unwrap();
    let remastered_bytes = remastered_bundle();
    let remastered = Bundle::parse(remastered_bytes.as_slice()).unwrap();
    for sources in [[&legacy, &remastered], [&remastered, &legacy]] {
        let merged = Bundle::merge(&sources);
        assert_eq!(merged.version(), 5);
        assert_eq!(merged.data_block_offset(), (32 + merged.items().len() * 304) as u32);
        let written = merged.write();
        let reparsed = Bundle::parse(written.as_slice()).unwrap();
        assert_eq!(merged.items().len(), reparsed.items().len());
        for ((original, roundtrip), offset) in merged
            .items()
            .iter()
            .zip(reparsed.items())
            .zip(merged.compute_offsets_u64())
        {
            assert_eq!(original.name(), roundtrip.name());
            assert_eq!(original.hash(), roundtrip.hash());
            assert_eq!(original.crc(), roundtrip.crc());
            assert_eq!(original.compression(), roundtrip.compression());
            assert_eq!(original.raw(), roundtrip.raw());
            assert_eq!(roundtrip.offset(), offset);
            assert_eq!(original.decompressed().unwrap(), roundtrip.decompressed().unwrap());
        }
    }
}

#[test]
fn world_world_startup_parses_decompresses_and_roundtrips() {
    let bytes = load("world_world_startup.bundle");
    for bundle in [Bundle::parse(bytes.as_slice()), Bundle::parse(bytes.clone())] {
        let bundle = bundle.expect("real Remastered startup bundle should parse");
        assert_eq!(bundle.version(), 5);
        assert_eq!(bundle.items().len(), 1657);

        let written = bundle.write();
        let reparsed = Bundle::parse(written.as_slice()).expect("rewritten startup bundle should parse");
        assert_eq!(reparsed.version(), 5);
        assert_eq!(reparsed.items().len(), bundle.items().len());

        for ((original, roundtrip), offset) in bundle
            .items()
            .iter()
            .zip(reparsed.items())
            .zip(bundle.compute_offsets_u64())
        {
            assert_eq!(original.name(), roundtrip.name());
            assert_eq!(original.hash(), roundtrip.hash());
            assert_eq!(original.size(), roundtrip.size());
            assert_eq!(original.zsize(), roundtrip.zsize());
            assert_eq!(original.crc(), roundtrip.crc());
            assert_eq!(original.compression(), roundtrip.compression());
            assert_eq!(original.raw(), roundtrip.raw(), "{:?}", original.name());
            assert_eq!(roundtrip.offset(), offset);

            let payload = original
                .decompressed()
                .unwrap_or_else(|error| panic!("{:?}: {error}", original.name()));
            assert_eq!(payload.len() as u32, original.size(), "{:?}", original.name());
            assert_eq!(roundtrip.decompressed().unwrap(), payload, "{:?}", original.name());
        }
        assert_eq!(reparsed.write(), written);
    }
}

#[test]
#[ignore = "requires W3_GAME_DIR pointing at a local game installation; do not modify it during the test"]
fn installed_remastered_bundles() {
    let root = PathBuf::from(std::env::var_os("W3_GAME_DIR").expect("set W3_GAME_DIR"));
    let mut directories = vec![root];
    let mut bundle_count = 0;
    let mut item_count = 0;
    let mut wide_offsets = 0;
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                directories.push(path);
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("bundle") {
                continue;
            }
            let file = std::fs::File::open(&path).unwrap();
            let bytes = unsafe { memmap2::MmapOptions::new().map(&file).unwrap() };
            let bundle = Bundle::parse(&bytes[..]).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let mut checked_codecs = HashSet::new();
            for item in bundle.items() {
                assert_eq!(item.raw().len() as u32, item.zsize());
                if item.offset() > u64::from(u32::MAX) {
                    wide_offsets += 1;
                }
                if checked_codecs.insert(item.compression() as u32) || bytes.len() < 1_000_000 {
                    let payload = item
                        .decompressed()
                        .unwrap_or_else(|error| panic!("{}: {:?}: {error}", path.display(), item.name()));
                    assert_eq!(
                        payload.len() as u32,
                        item.size(),
                        "{}: {:?}",
                        path.display(),
                        item.name()
                    );
                }
            }
            if bytes.len() < 1_000_000 {
                let written = bundle.write();
                let reparsed = Bundle::parse(written.as_slice()).unwrap();
                assert_eq!(bundle.version(), reparsed.version());
                assert_eq!(bundle.items().len(), reparsed.items().len());
                for (original, roundtrip) in bundle.items().iter().zip(reparsed.items()) {
                    assert_eq!(original.name(), roundtrip.name());
                    assert_eq!(original.hash(), roundtrip.hash());
                    assert_eq!(original.crc(), roundtrip.crc());
                    assert_eq!(original.raw(), roundtrip.raw());
                }
            }
            bundle_count += 1;
            item_count += bundle.items().len();
        }
    }
    assert!(bundle_count > 0, "no bundles found");
    println!("Validated {bundle_count} bundles, {item_count} entries, {wide_offsets} offsets above 4 GiB");
}

// -----------------------------------------------------------------------------
// Item accessors and payload
// -----------------------------------------------------------------------------

#[test]
fn every_item_reports_zsize_matching_raw_len() {
    for &b in BUNDLES {
        let data = load(b);
        let bundle = Bundle::parse(&*data).unwrap();
        for item in bundle.items() {
            assert_eq!(item.raw().len() as u32, item.zsize(), "{b}: {:?}", item.name());
        }
    }
}

#[test]
fn every_item_decompresses_to_its_declared_size() {
    for &b in BUNDLES {
        let data = load(b);
        let bundle = Bundle::parse(&*data).unwrap();
        for item in bundle.items() {
            let decompressed = item
                .decompressed()
                .unwrap_or_else(|e| panic!("{b}: {:?}: decompress failed: {e}", item.name()));
            assert_eq!(decompressed.len() as u32, item.size(), "{b}: {:?}", item.name());
        }
    }
}

#[test]
fn fixtures_use_lz4hc_compression() {
    for &b in BUNDLES {
        let data = load(b);
        let bundle = Bundle::parse(&*data).unwrap();
        for item in bundle.items() {
            assert_eq!(item.compression(), BundleCompression::Lz4hc, "{b}: {:?}", item.name());
        }
    }
}

// -----------------------------------------------------------------------------
// Write
// -----------------------------------------------------------------------------

#[test]
fn write_produces_valid_bundle_that_reparses() {
    for &b in BUNDLES {
        let data = load(b);
        let bundle = Bundle::parse(&*data).unwrap();
        let written = bundle.write();
        let reparsed = Bundle::parse(&*written).expect("rewritten bundle should parse");
        assert_eq!(bundle.items().len(), reparsed.items().len(), "{b}");
        for (orig, round) in bundle.items().iter().zip(reparsed.items()) {
            assert_eq!(orig.name(), round.name());
            assert_eq!(orig.hash(), round.hash());
            assert_eq!(orig.size(), round.size());
            assert_eq!(orig.zsize(), round.zsize());
            assert_eq!(orig.crc(), round.crc());
            assert_eq!(orig.compression(), round.compression());
            assert_eq!(orig.raw(), round.raw(), "{b}: {:?}", orig.name());
        }
    }
}

#[test]
fn write_is_deterministic_after_roundtrip() {
    for &b in BUNDLES {
        let data = load(b);
        let first = Bundle::parse(&*data).unwrap().write();
        let second = Bundle::parse(&*first).unwrap().write();
        assert_eq!(first, second, "{b}: parse→write should be a fixed point");
    }
}

// -----------------------------------------------------------------------------
// Merge
// -----------------------------------------------------------------------------

#[test]
fn merge_empty_yields_empty_bundle() {
    let bundle = Bundle::merge(&[]);
    assert!(bundle.items().is_empty());
    let bytes = bundle.write();
    let reparsed = Bundle::parse(&*bytes).unwrap();
    assert!(reparsed.items().is_empty());
}

#[test]
fn merge_single_preserves_all_items() {
    let data = load(BUNDLES[0]);
    let src = Bundle::parse(&*data).unwrap();
    let merged = Bundle::merge(&[&src]);
    assert_eq!(merged.items().len(), src.items().len());
    for (s, m) in src.items().iter().zip(merged.items()) {
        assert_eq!(s.name(), m.name());
        assert_eq!(s.hash(), m.hash());
        assert_eq!(s.raw(), m.raw());
    }
}

#[test]
fn merge_disjoint_union_equals_unique_names() {
    let a_data = load("modnmm/blob0.bundle");
    let b_data = load("modsbn/blob0.bundle");
    let a = Bundle::parse(&*a_data).unwrap();
    let b = Bundle::parse(&*b_data).unwrap();

    let unique: HashSet<CString> = a
        .items()
        .iter()
        .chain(b.items().iter())
        .map(|i| i.name().to_owned())
        .collect();

    let merged = Bundle::merge(&[&a, &b]);
    assert_eq!(merged.items().len(), unique.len());

    let merged_names: HashSet<CString> = merged.items().iter().map(|i| i.name().to_owned()).collect();
    assert_eq!(merged_names, unique);
}

#[test]
fn merge_first_bundle_wins_on_conflict() {
    // modtest contains exactly one item that also exists in modsbn. Placing
    // modtest first should yield merged.len() == modsbn.len() and the
    // conflicting entry should carry modtest's hash, not modsbn's.
    let modsbn_data = load("modsbn/blob0.bundle");
    let modtest_data = load("modtest/blob0.bundle");
    let modsbn = Bundle::parse(&*modsbn_data).unwrap();
    let modtest = Bundle::parse(&*modtest_data).unwrap();

    // Sanity: the fixture assumption we depend on.
    let shared = CString::new(SHARED_ITEM).unwrap();
    assert!(modtest.items().iter().any(|i| i.name() == shared.as_c_str()));
    assert!(modsbn.items().iter().any(|i| i.name() == shared.as_c_str()));

    let merged = Bundle::merge(&[&modtest, &modsbn]);

    // No duplicates were introduced.
    assert_eq!(merged.items().len(), modsbn.items().len());

    // The overlapping name comes from modtest (higher priority).
    let winner = merged
        .items()
        .iter()
        .find(|i| i.name() == shared.as_c_str())
        .expect("shared item must appear in merged bundle");
    let modtest_source = modtest.items().iter().find(|i| i.name() == shared.as_c_str()).unwrap();
    assert_eq!(winner.hash(), modtest_source.hash());
    assert_eq!(winner.raw(), modtest_source.raw());
}

#[test]
fn merge_reverse_priority_uses_last_bundle_for_conflict() {
    // Same inputs as above but with modsbn first — the shared item should
    // now come from modsbn.
    let modsbn_data = load("modsbn/blob0.bundle");
    let modtest_data = load("modtest/blob0.bundle");
    let modsbn = Bundle::parse(&*modsbn_data).unwrap();
    let modtest = Bundle::parse(&*modtest_data).unwrap();

    let merged = Bundle::merge(&[&modsbn, &modtest]);
    assert_eq!(merged.items().len(), modsbn.items().len());

    let shared = CString::new(SHARED_ITEM).unwrap();
    let winner = merged.items().iter().find(|i| i.name() == shared.as_c_str()).unwrap();
    let modsbn_source = modsbn.items().iter().find(|i| i.name() == shared.as_c_str()).unwrap();
    assert_eq!(winner.hash(), modsbn_source.hash());
    assert_eq!(winner.raw(), modsbn_source.raw());
}

#[test]
fn merge_output_serializes_and_reparses_with_intact_payloads() {
    let a_data = load("modnmm/blob0.bundle");
    let b_data = load("modsbn/blob0.bundle");
    let c_data = load("modtest/blob0.bundle");
    let a = Bundle::parse(&*a_data).unwrap();
    let b = Bundle::parse(&*b_data).unwrap();
    let c = Bundle::parse(&*c_data).unwrap();

    let merged = Bundle::merge(&[&a, &b, &c]);
    let bytes = merged.write();
    let reparsed = Bundle::parse(&*bytes).unwrap();

    assert_eq!(reparsed.items().len(), merged.items().len());
    for (m, r) in merged.items().iter().zip(reparsed.items()) {
        assert_eq!(m.name(), r.name());
        assert_eq!(m.hash(), r.hash());
        assert_eq!(m.raw(), r.raw());
    }
    // Every retained item must still decompress to its declared size.
    for item in reparsed.items() {
        let decompressed = item.decompressed().unwrap();
        assert_eq!(decompressed.len() as u32, item.size());
    }
}

#[test]
fn merged_bundle_from_owned_inputs_is_independent_of_source_lifetime() {
    // Verifies the parse-owned path: after dropping the source buffers the
    // merged bundle must still be usable (its items own their compressed bytes).
    let merged_bytes = {
        let a_data = load("modtest/blob0.bundle");
        let b_data = load("modnmm/blob0.bundle");
        let a = Bundle::parse(a_data).unwrap(); // owned
        let b = Bundle::parse(b_data).unwrap(); // owned
        let merged = Bundle::merge(&[&a, &b]);
        merged.write()
        // a and b drop here
    };
    let reparsed = Bundle::parse(&*merged_bytes).unwrap();
    assert!(!reparsed.items().is_empty());
    for item in reparsed.items() {
        let d = item.decompressed().unwrap();
        assert_eq!(d.len() as u32, item.size());
    }
}
