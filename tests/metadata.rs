//! Integration tests for `Metadata` parse / write / merge, driven by fixtures
//! under `test/assets/`.

use std::collections::HashSet;
use std::path::PathBuf;

use w3edit::metadata::Metadata;

// -----------------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------------

/// All shipped fixtures. Each is a real `metadata.store` extracted from a mod.
const FIXTURES: &[&str] = &["modnmm", "modsbn", "modtest", "modtest2"];

/// A path known to be present in both `modtest` and `modsbn` — used to verify
/// first-wins priority on merge.
const SHARED_PATH: &[u8] = br"items\cutscenes\succubus_cs\succubus_cs.w2ent";

fn asset(fixture: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("test/assets");
    p.push(fixture);
    p.push("metadata.store");
    p
}

fn load(fixture: &str) -> Vec<u8> {
    std::fs::read(asset(fixture)).unwrap_or_else(|e| panic!("cannot read fixture {fixture}: {e}"))
}

fn parse(fixture: &str) -> Metadata {
    let bytes = load(fixture);
    Metadata::parse(&bytes).unwrap_or_else(|e| panic!("{fixture}: parse failed: {e}"))
}

// -----------------------------------------------------------------------------
// Parse
// -----------------------------------------------------------------------------

#[test]
fn parse_yields_records_for_every_fixture() {
    for &f in FIXTURES {
        let m = parse(f);
        assert!(
            m.file_infos.len() >= 2,
            "{f}: expected at least the null placeholder + one file"
        );
        assert!(
            m.bundle_infos.len() >= 2,
            "{f}: expected at least the null placeholder + one bundle"
        );
        assert!(!m.dir_init_infos.is_empty(), "{f}: dir tree missing root");
    }
}

#[test]
fn parse_rejects_bad_magic() {
    let mut bytes = load("modtest");
    bytes[0] ^= 0xFF;
    assert!(Metadata::parse(&bytes).is_err(), "corrupted magic must not parse");
}

#[test]
fn parse_rejects_short_input() {
    assert!(Metadata::parse(&[]).is_err(), "empty input must not parse");
    assert!(Metadata::parse(b"\x03VTM").is_err(), "magic-only input must not parse");
}

#[test]
fn null_placeholders_are_preserved() {
    for &f in FIXTURES {
        let m = parse(f);
        let fi0 = &m.file_infos[0].1;
        assert_eq!(fi0.string_table_name_offset, 0, "{f}: file_infos[0] not null");
        assert_eq!(fi0.size_in_bundle, 0);
        assert_eq!(fi0.first_entry, 0);

        let ei0 = &m.entry_infos[0];
        assert_eq!(ei0.file_id, 0, "{f}: entry_infos[0] not null");
        assert_eq!(ei0.bundle_id, 0);

        let bi0 = &m.bundle_infos[0];
        assert_eq!(bi0.string_table_name_offset, 0, "{f}: bundle_infos[0] not null");
        assert_eq!(bi0.num_bundle_entries, 0);
    }
}

#[test]
fn dir_tree_root_has_expected_shape() {
    for &f in FIXTURES {
        let m = parse(f);
        let root = &m.dir_init_infos[0];
        // Root parent is 0 (self-loop) — verified across all four fixtures.
        assert_eq!(root.parent, 0, "{f}: root dir must self-parent");
    }
}

#[test]
fn string_table_starts_with_nul_sentinel() {
    for &f in FIXTURES {
        let m = parse(f);
        assert!(!m.string_table.is_empty(), "{f}: string_table empty");
        assert_eq!(m.string_table[0], 0, "{f}: string table must start with a NUL sentinel");
    }
}

#[test]
fn file_paths_are_reachable_from_string_table() {
    for &f in FIXTURES {
        let m = parse(f);
        for (idx, (name, fi)) in m.file_infos.iter().enumerate().skip(1) {
            assert!(!name.as_bytes().is_empty(), "{f}: file {idx} has empty name");
            let resolved = m.string_at(fi.string_table_name_offset);
            assert_eq!(resolved.to_bytes(), name.as_bytes(), "{f}: file {idx} name mismatch");
        }
    }
}

// -----------------------------------------------------------------------------
// Write / roundtrip
// -----------------------------------------------------------------------------

#[test]
fn write_is_byte_identical_for_every_fixture() {
    for &f in FIXTURES {
        let src = load(f);
        let out = Metadata::parse(&src).unwrap().write();
        assert_eq!(
            src.len(),
            out.len(),
            "{f}: written length {} differs from source {}",
            out.len(),
            src.len()
        );
        assert_eq!(src, out, "{f}: byte-level differences");
    }
}

#[test]
fn parse_write_parse_is_stable() {
    for &f in FIXTURES {
        let src = load(f);
        let m1 = Metadata::parse(&src).unwrap();
        let out = m1.write();
        let m2 = Metadata::parse(&out).unwrap();
        // Round two must serialise to the same bytes as round one.
        assert_eq!(out, m2.write(), "{f}: parse/write is not idempotent");
    }
}

// -----------------------------------------------------------------------------
// Merge
// -----------------------------------------------------------------------------

fn real_paths(m: &Metadata) -> HashSet<Vec<u8>> {
    m.file_infos
        .iter()
        .skip(1)
        .map(|(n, _)| n.as_bytes().to_vec())
        .collect()
}

fn real_bundle_names(m: &Metadata) -> HashSet<Vec<u8>> {
    (1..m.bundle_infos.len())
        .map(|i| {
            m.string_at(m.bundle_infos[i].string_table_name_offset)
                .to_bytes()
                .to_vec()
        })
        .collect()
}

#[test]
fn merge_empty_yields_empty_metadata() {
    let m = Metadata::merge(&[]);
    assert_eq!(m.file_infos.len(), 1, "expected only the null placeholder");
    assert_eq!(m.bundle_infos.len(), 1);
    assert_eq!(m.entry_infos.len(), 1);
    assert!(m.hashes.is_empty());
    assert!(m.file_init_infos.is_empty());
    assert!(m.buffers.is_empty());
    assert_eq!(m.dir_init_infos.len(), 1, "expected a root dir");
}

#[test]
fn merge_single_source_preserves_all_paths() {
    for &f in FIXTURES {
        let src = parse(f);
        let merged = Metadata::merge(&[&src]);
        assert_eq!(
            real_paths(&merged),
            real_paths(&src),
            "{f}: merge of single source must preserve all file paths"
        );
        assert_eq!(
            real_bundle_names(&merged),
            real_bundle_names(&src),
            "{f}: merge of single source must preserve all bundle names"
        );
    }
}

#[test]
fn merge_dedups_buffer_sizes() {
    // Each unique buffer size should be interned exactly once, and the
    // `has_buffer`/`buffer_id` fields on file_infos must round-trip: every
    // file with a buffer in the source still resolves to the same size in
    // the output, even though buffer_ids may be renumbered.
    for &f in FIXTURES {
        let src = parse(f);
        let merged = Metadata::merge(&[&src]);

        // Unique-buffer count must match between source and merged output.
        let src_unique: HashSet<u32> = src.buffers.iter().copied().collect();
        let merged_unique: HashSet<u32> = merged.buffers.iter().copied().collect();
        assert_eq!(
            src_unique, merged_unique,
            "{f}: set of unique buffer sizes must be preserved"
        );
        // And the emitted `buffers` array must itself be free of duplicates.
        assert_eq!(
            merged.buffers.len(),
            merged_unique.len(),
            "{f}: merged buffers array contains duplicates"
        );

        // Per-file buffer size must be preserved.
        let src_sizes: std::collections::HashMap<Vec<u8>, Option<u32>> = src
            .file_infos
            .iter()
            .skip(1)
            .map(|(n, fi)| (n.as_bytes().to_vec(), src.buffer_size_of(fi)))
            .collect();
        for (n, fi) in merged.file_infos.iter().skip(1) {
            let got = merged.buffer_size_of(fi);
            let want = src_sizes.get(n.as_bytes()).copied().unwrap_or(None);
            assert_eq!(got, want, "{f}: buffer size mismatch for {n:?}");
        }
    }
}

#[test]
fn merge_disjoint_sources_yields_union_of_paths() {
    let a = parse("modnmm");
    let b = parse("modsbn");
    let merged = Metadata::merge(&[&a, &b]);
    let union: HashSet<_> = real_paths(&a).union(&real_paths(&b)).cloned().collect();
    assert_eq!(real_paths(&merged), union);
}

#[test]
fn merge_first_wins_on_shared_path() {
    let a = parse("modtest");
    let b = parse("modsbn");
    let shared = SHARED_PATH.to_vec();
    assert!(real_paths(&a).contains(&shared), "shared path missing from modtest");
    assert!(real_paths(&b).contains(&shared), "shared path missing from modsbn");

    // Find the file's index in each source, and grab a signature.
    let sig_of = |m: &Metadata| -> (u32, u32) {
        let (_, fi) = m
            .file_infos
            .iter()
            .find(|(name, _)| name.as_bytes() == SHARED_PATH)
            .expect("shared path not found");
        (fi.size_in_memory, fi.size_in_bundle)
    };

    let merged_ab = Metadata::merge(&[&a, &b]);
    let merged_ba = Metadata::merge(&[&b, &a]);

    assert_eq!(sig_of(&merged_ab), sig_of(&a), "a should win when listed first");
    assert_eq!(sig_of(&merged_ba), sig_of(&b), "b should win when listed first");
}

#[test]
fn merge_output_roundtrips_through_write() {
    let a = parse("modnmm");
    let b = parse("modsbn");
    let merged = Metadata::merge(&[&a, &b]);
    let bytes = merged.write();
    let reparsed = Metadata::parse(&bytes).expect("merged output must reparse");
    // Roundtripping the bytes must be stable.
    assert_eq!(bytes, reparsed.write(), "merged output not stable under write");
    // File set is preserved.
    assert_eq!(real_paths(&reparsed), real_paths(&merged));
}

#[test]
fn merge_output_preserves_null_placeholders() {
    let merged = Metadata::merge(&[&parse("modnmm"), &parse("modsbn")]);
    assert_eq!(merged.file_infos[0].1.string_table_name_offset, 0);
    assert_eq!(merged.entry_infos[0].file_id, 0);
    assert_eq!(merged.bundle_infos[0].string_table_name_offset, 0);
    assert_eq!(merged.dir_init_infos[0].parent, 0);
    assert_eq!(merged.dir_init_infos[0].string_table_name_offset, 0);
    assert_eq!(merged.string_table[0], 0);
}

#[test]
fn merge_entry_chains_terminate_and_stay_within_bundle() {
    let merged = Metadata::merge(&[&parse("modnmm"), &parse("modsbn")]);
    for fid in 1..merged.file_infos.len() {
        let mut eid = merged.file_infos[fid].1.first_entry as usize;
        let mut hops = 0;
        while eid != 0 {
            assert!(eid < merged.entry_infos.len(), "entry id {eid} out of range");
            let e = &merged.entry_infos[eid];
            assert_eq!(
                e.file_id as usize, fid,
                "entry {eid} claims file {} but was reached from file {fid}",
                e.file_id
            );
            assert!(
                (e.bundle_id as usize) < merged.bundle_infos.len(),
                "entry {eid} refers to nonexistent bundle {}",
                e.bundle_id
            );
            eid = e.next_entry as usize;
            hops += 1;
            assert!(hops < merged.entry_infos.len(), "entry chain does not terminate");
        }
    }
}

#[test]
fn merge_bundle_first_entry_ranges_are_consistent() {
    let merged = Metadata::merge(&[&parse("modnmm"), &parse("modsbn")]);
    for bid in 1..merged.bundle_infos.len() {
        let b = &merged.bundle_infos[bid];
        let first = b.first_file_entry as usize;
        let count = b.num_bundle_entries as usize;
        if count == 0 {
            continue;
        }
        for i in 0..count {
            let idx = first + i;
            assert!(idx < merged.entry_infos.len(), "bundle {bid} range out of bounds");
            assert_eq!(
                merged.entry_infos[idx].bundle_id as usize, bid,
                "bundle {bid} first_entry range covers foreign entry at {idx}"
            );
        }
    }
}

#[test]
fn merge_dir_parents_are_reachable_from_root() {
    let merged = Metadata::merge(&[&parse("modnmm"), &parse("modsbn")]);
    for did in 1..merged.dir_init_infos.len() {
        let mut cur = did as i32;
        let mut hops = 0;
        while cur != 0 {
            let parent = merged.dir_init_infos[cur as usize].parent;
            assert!(
                parent >= 0 && (parent as usize) < merged.dir_init_infos.len(),
                "dir {cur} has invalid parent {parent}"
            );
            cur = parent;
            hops += 1;
            assert!(hops < merged.dir_init_infos.len(), "dir chain does not terminate");
        }
    }
}

#[test]
fn merge_file_init_info_directories_are_valid() {
    let merged = Metadata::merge(&[&parse("modnmm"), &parse("modsbn")]);
    for fii in &merged.file_init_infos {
        assert!(
            fii.directory_id >= 0 && (fii.directory_id as usize) < merged.dir_init_infos.len(),
            "file_init_info directory_id {} out of range",
            fii.directory_id
        );
        assert!(
            fii.file_id > 0 && (fii.file_id as usize) < merged.file_infos.len(),
            "file_init_info file_id {} out of range",
            fii.file_id
        );
    }
}
