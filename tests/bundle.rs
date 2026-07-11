//! Integration tests for `Bundle` parse / decompress / write / merge,
//! driven by the fixtures under `test/assets/`.

use std::collections::HashSet;
use std::ffi::CString;
use std::path::PathBuf;

use w3edit::bundle::{Bundle, BundleCompression};

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
