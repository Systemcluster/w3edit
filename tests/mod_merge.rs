//! End-to-end mod-merge integration tests: given N mods (each a bundle +
//! metadata pair), the merged output must be self-consistent — every metadata
//! entry must point at the correct byte range in the merged bundle.

use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::path::PathBuf;

use w3edit::{Bundle, Metadata, ModInput, TextureCache, TextureCacheEntry};

// -----------------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------------

/// All shipped mod fixtures.
const MODS: &[&str] = &["modnmm", "modsbn", "modtest"];

/// Path shared between `modtest` and `modsbn`. Used to test first-wins priority.
const SHARED_PATH: &[u8] = br"items\cutscenes\succubus_cs\succubus_cs.w2ent";

fn asset(mod_name: &str, file: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("test/assets");
    p.push(mod_name);
    p.push(file);
    p
}

fn load_mod(mod_name: &str) -> (Vec<u8>, Vec<u8>) {
    let bundle_bytes = std::fs::read(asset(mod_name, "blob0.bundle"))
        .unwrap_or_else(|e| panic!("cannot read {mod_name}/blob0.bundle: {e}"));
    let meta_bytes = std::fs::read(asset(mod_name, "metadata.store"))
        .unwrap_or_else(|e| panic!("cannot read {mod_name}/metadata.store: {e}"));
    (bundle_bytes, meta_bytes)
}

/// Parse both files and return them along with owned backing buffers.
fn parse_mod(mod_name: &str) -> (Vec<u8>, Bundle<'static>, Metadata) {
    let (bundle_bytes, meta_bytes) = load_mod(mod_name);
    // Use owned Cow so the Bundle is 'static.
    let bundle = Bundle::parse(bundle_bytes.clone()).unwrap_or_else(|e| panic!("{mod_name}: bundle parse failed: {e}"));
    let meta = Metadata::parse(&meta_bytes).unwrap_or_else(|e| panic!("{mod_name}: metadata parse failed: {e}"));
    (bundle_bytes, bundle, meta)
}

fn bundle_name() -> CString {
    CString::new("blob0.bundle").unwrap()
}

fn merge_mods<'data>(mods: &[(&Bundle<'data>, &Metadata)], output_bundle_name: &CStr) -> (Bundle<'data>, Metadata) {
    let inputs: Vec<_> = mods
        .iter()
        .map(|&(bundle, metadata)| ModInput::new(bundle, metadata))
        .collect();
    let merged = w3edit::merge_mods(&inputs, output_bundle_name);
    (merged.bundle, merged.metadata)
}

fn align_up(value: usize, align: usize) -> usize {
    (value + (align - 1)) & !(align - 1)
}

fn push_texture_string(string_table: &mut Vec<u8>, value: &str) -> i32 {
    let offset = string_table.len() as i32;
    string_table.extend_from_slice(value.as_bytes());
    string_table.push(0);
    offset
}

fn push_texture_page(data_pages: &mut Vec<u8>, mip_index: u8, payload: &[u8]) -> u32 {
    const PAGE: usize = 4096;

    let offset = align_up(data_pages.len(), PAGE);
    data_pages.resize(offset, 0);
    data_pages.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    data_pages.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    data_pages.push(mip_index);
    data_pages.extend_from_slice(payload);
    offset as u32
}

fn texture_cache_fixture(names: &[(&str, u32)]) -> TextureCache {
    let mut string_table = vec![0];
    let mut data_pages = Vec::new();
    let mut mip_offsets = Vec::new();
    let mut entries = Vec::new();

    for (idx, &(name, hash)) in names.iter().enumerate() {
        let string_table_offset = push_texture_string(&mut string_table, name);
        let page_offset = push_texture_page(&mut data_pages, 0, &[idx as u8 + 1, 2, 3]);
        let mip_offset = push_texture_page(&mut data_pages, 1, &[idx as u8 + 4, 5]);
        let mip_offset_index = mip_offsets.len() as i32;
        mip_offsets.push(mip_offset);

        entries.push((CString::new(name).unwrap(), TextureCacheEntry {
            hash,
            string_table_offset,
            page_offset,
            compressed_size: 3,
            uncompressed_size: 3,
            base_alignment: 16,
            base_width: 64 + idx as u16,
            base_height: 32,
            mipcount: 2,
            slice_count: 1,
            mip_offset_index,
            num_mip_offsets: 1,
            time_stamp: 10 + idx as i64,
            type1: 1,
            type2: 2,
            is_cube: 0,
            unk1: 9,
        }));
    }

    TextureCache {
        version: 3,
        data_pages,
        mip_offsets,
        string_table,
        entries,
    }
}

// -----------------------------------------------------------------------------
// Basic properties
// -----------------------------------------------------------------------------

#[test]
fn merge_mods_empty_input_yields_empty_pair() {
    let (bundle, meta) = merge_mods(&[], &bundle_name());
    assert!(bundle.items().is_empty());
    assert_eq!(meta.file_infos.len(), 1, "expected only the null placeholder");
    assert_eq!(meta.bundle_infos.len(), 1);
    assert_eq!(meta.entry_infos.len(), 1);
}

#[test]
fn merge_mods_single_mod_preserves_file_set() {
    for &m in MODS {
        let (_, bundle, meta) = parse_mod(m);
        let (out_bundle, out_meta) = merge_mods(&[(&bundle, &meta)], &bundle_name());

        let expected: HashSet<Vec<u8>> = bundle.items().iter().map(|i| i.name().to_bytes().to_vec()).collect();
        let actual_bundle: HashSet<Vec<u8>> = out_bundle
            .items()
            .iter()
            .map(|i| i.name().to_bytes().to_vec())
            .collect();
        let actual_meta: HashSet<Vec<u8>> = out_meta
            .file_infos
            .iter()
            .skip(1)
            .map(|(n, _)| n.as_bytes().to_vec())
            .collect();

        assert_eq!(actual_bundle, expected, "{m}: bundle item set changed");
        assert_eq!(actual_meta, expected, "{m}: metadata file set differs from bundle");
    }
}

#[test]
fn merge_mods_two_disjoint_mods_yield_union() {
    let (_, a_bundle, a_meta) = parse_mod("modnmm");
    let (_, b_bundle, b_meta) = parse_mod("modsbn");
    let (out_bundle, out_meta) = merge_mods(&[(&a_bundle, &a_meta), (&b_bundle, &b_meta)], &bundle_name());

    let a_paths: HashSet<Vec<u8>> = a_bundle.items().iter().map(|i| i.name().to_bytes().to_vec()).collect();
    let b_paths: HashSet<Vec<u8>> = b_bundle.items().iter().map(|i| i.name().to_bytes().to_vec()).collect();
    let expected: HashSet<Vec<u8>> = a_paths.union(&b_paths).cloned().collect();

    let out_paths: HashSet<Vec<u8>> = out_bundle
        .items()
        .iter()
        .map(|i| i.name().to_bytes().to_vec())
        .collect();
    let meta_paths: HashSet<Vec<u8>> = out_meta
        .file_infos
        .iter()
        .skip(1)
        .map(|(n, _)| n.as_bytes().to_vec())
        .collect();
    assert_eq!(out_paths, expected, "merged bundle path set != union");
    assert_eq!(meta_paths, expected, "merged metadata path set != union");
}

#[test]
fn merge_mods_without_texture_cache_has_no_texture_cache() {
    let (_, a_bundle, a_meta) = parse_mod("modnmm");
    let (_, b_bundle, b_meta) = parse_mod("modsbn");

    let merged = w3edit::merge_mods(
        &[ModInput::new(&a_bundle, &a_meta), ModInput::new(&b_bundle, &b_meta)],
        &bundle_name(),
    );

    assert!(merged.texture_cache.is_none());
    assert!(!merged.bundle.items().is_empty());
    assert!(merged.metadata.file_infos.len() > 1);
}

#[test]
fn merge_mods_merges_texture_caches_with_mod_priority() {
    const SHARED_TEXTURE: &str = r"textures\shared\diffuse.xbm";

    let (_, a_bundle, a_meta) = parse_mod("modnmm");
    let (_, b_bundle, b_meta) = parse_mod("modsbn");
    let a_cache = texture_cache_fixture(&[(SHARED_TEXTURE, 11), (r"textures\a\only.xbm", 12)]);
    let b_cache = texture_cache_fixture(&[(SHARED_TEXTURE, 99), (r"textures\b\only.xbm", 100)]);

    let merged = w3edit::merge_mods(
        &[
            ModInput::new(&a_bundle, &a_meta).with_texture_cache(&a_cache),
            ModInput::new(&b_bundle, &b_meta).with_texture_cache(&b_cache),
        ],
        &bundle_name(),
    );

    let texture_cache = merged.texture_cache.expect("texture cache should be produced");
    let texture_names: HashSet<Vec<u8>> = texture_cache
        .entries
        .iter()
        .map(|(name, _)| name.as_bytes().to_vec())
        .collect();
    assert_eq!(texture_names.len(), 3);
    assert!(texture_names.contains(SHARED_TEXTURE.as_bytes()));
    assert!(texture_names.contains(br"textures\a\only.xbm" as &[u8]));
    assert!(texture_names.contains(br"textures\b\only.xbm" as &[u8]));

    let shared = texture_cache
        .entries
        .iter()
        .find(|(name, _)| name.as_bytes() == SHARED_TEXTURE.as_bytes())
        .unwrap();
    assert_eq!(shared.1.hash, 11, "first mod's texture entry should win");

    for (_, entry) in &texture_cache.entries {
        assert_eq!(entry.page_offset as usize % 4096, 0);
        assert!(entry.page_offset as usize + 9 + entry.compressed_size as usize <= texture_cache.data_pages.len());
    }
    assert_eq!(
        texture_cache.write(),
        TextureCache::parse(&texture_cache.write()).unwrap().write()
    );
}

#[test]
fn merge_mods_skips_missing_texture_caches() {
    let (_, a_bundle, a_meta) = parse_mod("modnmm");
    let (_, b_bundle, b_meta) = parse_mod("modsbn");
    let b_cache = texture_cache_fixture(&[(r"textures\b\only.xbm", 100)]);

    let merged = w3edit::merge_mods(
        &[
            ModInput::new(&a_bundle, &a_meta),
            ModInput::new(&b_bundle, &b_meta).with_texture_cache(&b_cache),
        ],
        &bundle_name(),
    );

    let texture_cache = merged
        .texture_cache
        .expect("lower-priority cache should still be merged");
    assert_eq!(texture_cache.entries.len(), 1);
    assert_eq!(texture_cache.entries[0].0.as_bytes(), br"textures\b\only.xbm");
}

#[test]
fn merge_mods_first_wins_on_shared_path() {
    let (_, tb, tm) = parse_mod("modtest");
    let (_, sb, sm) = parse_mod("modsbn");

    // Sanity: both mods must contain the shared path.
    let has_path = |b: &Bundle| b.items().iter().any(|i| i.name().to_bytes() == SHARED_PATH);
    assert!(has_path(&tb) && has_path(&sb));

    // modtest first: modtest's item wins.
    let (out1, _) = merge_mods(&[(&tb, &tm), (&sb, &sm)], &bundle_name());
    let winner1 = out1
        .items()
        .iter()
        .find(|i| i.name().to_bytes() == SHARED_PATH)
        .unwrap();
    let modtest_item = tb.items().iter().find(|i| i.name().to_bytes() == SHARED_PATH).unwrap();
    assert_eq!(winner1.zsize(), modtest_item.zsize());
    assert_eq!(winner1.hash(), modtest_item.hash());

    // modsbn first: modsbn's item wins.
    let (out2, _) = merge_mods(&[(&sb, &sm), (&tb, &tm)], &bundle_name());
    let winner2 = out2
        .items()
        .iter()
        .find(|i| i.name().to_bytes() == SHARED_PATH)
        .unwrap();
    let modsbn_item = sb.items().iter().find(|i| i.name().to_bytes() == SHARED_PATH).unwrap();
    assert_eq!(winner2.zsize(), modsbn_item.zsize());
    assert_eq!(winner2.hash(), modsbn_item.hash());
}

// -----------------------------------------------------------------------------
// Cross-consistency: metadata must correctly describe the bundle
// -----------------------------------------------------------------------------

#[test]
fn merge_mods_metadata_offsets_match_bundle_layout() {
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (bundle, meta) = merge_mods(&[(&ab, &am), (&bb, &bm)], &bundle_name());

    let offsets = bundle.compute_offsets();
    for (i, item) in bundle.items().iter().enumerate() {
        // Find the file_info for this item's name.
        let (fid, (_, fi)) = meta
            .file_infos
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, (n, _))| n.as_bytes() == item.name().to_bytes())
            .unwrap_or_else(|| panic!("no metadata entry for bundle item '{:?}'", item.name()));
        assert!(fi.first_entry != 0, "file {fid} has no entry chain");
        let entry = &meta.entry_infos[fi.first_entry as usize];
        assert_eq!(entry.bundle_id, 1, "entry points at wrong bundle");
        assert_eq!(
            entry.offset_in_bundle,
            offsets[i],
            "entry offset for '{:?}' does not match bundle layout",
            item.name()
        );
        assert_eq!(
            entry.size_in_bundle,
            item.zsize(),
            "entry size for '{:?}' does not match bundle item zsize",
            item.name()
        );
    }
}

#[test]
fn merge_mods_bundle_infos_has_exactly_one_real_bundle() {
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (bundle, meta) = merge_mods(&[(&ab, &am), (&bb, &bm)], &bundle_name());

    assert_eq!(meta.bundle_infos.len(), 2, "expected null placeholder + one bundle");
    let bi = &meta.bundle_infos[1];
    assert_eq!(
        meta.string_at(bi.string_table_name_offset).to_bytes(),
        bundle_name().as_bytes()
    );
    assert_eq!(bi.first_file_entry, 1);
    assert_eq!(bi.num_bundle_entries as usize, bundle.items().len());
    assert_eq!(bi.data_block_offset, bundle.data_block_offset());
    assert_eq!(bi.data_block_size, bundle.data_block_size());
}

#[test]
fn merge_mods_output_bundle_serializes_and_roundtrips() {
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (bundle, _meta) = merge_mods(&[(&ab, &am), (&bb, &bm)], &bundle_name());

    let bytes = bundle.write();
    let reparsed = Bundle::parse(&*bytes).expect("output bundle must reparse");

    assert_eq!(reparsed.items().len(), bundle.items().len());
    for (a, b) in bundle.items().iter().zip(reparsed.items().iter()) {
        assert_eq!(a.name(), b.name());
        assert_eq!(a.zsize(), b.zsize());
        assert_eq!(a.hash(), b.hash());
    }
}

#[test]
fn merge_mods_output_metadata_serializes_and_roundtrips() {
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (_bundle, meta) = merge_mods(&[(&ab, &am), (&bb, &bm)], &bundle_name());

    let bytes = meta.write();
    let reparsed = Metadata::parse(&bytes).expect("output metadata must reparse");

    assert_eq!(reparsed.file_infos.len(), meta.file_infos.len());
    assert_eq!(reparsed.bundle_infos.len(), meta.bundle_infos.len());
    assert_eq!(reparsed.entry_infos.len(), meta.entry_infos.len());
    // Writing again must be stable.
    assert_eq!(reparsed.write(), bytes);
}

#[test]
fn merge_mods_reparsed_metadata_offsets_match_reparsed_bundle() {
    // The strictest end-to-end invariant: after writing both artifacts and
    // parsing them back from bytes, every metadata entry still resolves to
    // valid data in the bundle.
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (bundle, meta) = merge_mods(&[(&ab, &am), (&bb, &bm)], &bundle_name());
    let bundle_bytes = bundle.write();
    let meta_bytes = meta.write();

    let rb = Bundle::parse(bundle_bytes.clone()).unwrap();
    let rm = Metadata::parse(&meta_bytes).unwrap();

    for (name, fi) in rm.file_infos.iter().skip(1) {
        let entry = &rm.entry_infos[fi.first_entry as usize];
        // Locate the corresponding item in the reparsed bundle.
        let item = rb
            .items()
            .iter()
            .find(|i| i.name() == name.as_c_str())
            .unwrap_or_else(|| panic!("reparsed bundle missing '{name:?}'"));

        // The entry's byte range must correspond to actual data in the bundle.
        let start = entry.offset_in_bundle as usize;
        let end = start + entry.size_in_bundle as usize;
        assert!(end <= bundle_bytes.len(), "entry for '{name:?}' out of bundle range");
        assert_eq!(
            &bundle_bytes[start..end],
            item.raw(),
            "entry byte range for '{name:?}' does not match bundle item bytes"
        );
    }
}

// -----------------------------------------------------------------------------
// Metadata structural invariants
// -----------------------------------------------------------------------------

#[test]
fn merge_mods_metadata_preserves_index_zero_placeholders() {
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (_, meta) = merge_mods(&[(&ab, &am), (&bb, &bm)], &bundle_name());

    assert_eq!(meta.file_infos[0].1.string_table_name_offset, 0);
    assert_eq!(meta.entry_infos[0].file_id, 0);
    assert_eq!(meta.bundle_infos[0].string_table_name_offset, 0);
    assert_eq!(meta.dir_init_infos[0].parent, 0);
    assert_eq!(meta.dir_init_infos[0].string_table_name_offset, 0);
    assert_eq!(meta.string_table[0], 0);
}

#[test]
fn merge_mods_dir_tree_reconstructs_file_paths() {
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (_, meta) = merge_mods(&[(&ab, &am), (&bb, &bm)], &bundle_name());

    for fii in &meta.file_init_infos {
        // Walk dir_id → parent chain, collecting the directory names.
        let mut components: Vec<Vec<u8>> = Vec::new();
        let mut cur = fii.directory_id;
        let mut hops = 0;
        while cur != 0 {
            let dir = &meta.dir_init_infos[cur as usize];
            let name = meta.string_at(dir.string_table_name_offset as u32);
            components.push(name.to_bytes().to_vec());
            cur = dir.parent;
            hops += 1;
            assert!(hops < meta.dir_init_infos.len(), "dir chain does not terminate");
        }
        components.reverse();

        let leaf = meta.string_at(fii.string_table_name_offset as u32);
        components.push(leaf.to_bytes().to_vec());

        let reconstructed = components.join(&b'\\');
        let recorded = meta.file_infos[fii.file_id as usize].0.as_bytes().to_vec();
        assert_eq!(
            reconstructed, recorded,
            "reconstructed path for file_id {} != recorded path",
            fii.file_id
        );
    }
}

#[test]
fn merge_mods_hashes_are_carried_over_from_sources() {
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (_, meta) = merge_mods(&[(&ab, &am), (&bb, &bm)], &bundle_name());

    // Every hash in the output must refer to a real file.
    for h in &meta.hashes {
        assert!(
            h.file_id > 0 && (h.file_id as usize) < meta.file_infos.len(),
            "hash refers to invalid file_id {}",
            h.file_id
        );
    }

    // At least all files that had hashes in modnmm should still have them.
    let name_to_hash: std::collections::HashMap<Vec<u8>, i64> = am
        .hashes
        .iter()
        .filter_map(|h| {
            am.file_infos
                .get(h.file_id as usize)
                .map(|(n, _)| (n.as_bytes().to_vec(), h.hash))
        })
        .collect();

    for h in &meta.hashes {
        let (name, _) = &meta.file_infos[h.file_id as usize];
        if let Some(&expected) = name_to_hash.get(name.as_bytes()) {
            assert_eq!(h.hash, expected, "hash for '{name:?}' differs from source");
        }
    }
}

#[test]
fn merge_mods_all_three_mods_produce_valid_output() {
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (_, tb, tm) = parse_mod("modtest");

    let (bundle, meta) = merge_mods(&[(&ab, &am), (&bb, &bm), (&tb, &tm)], &bundle_name());

    let bundle_bytes = bundle.write();
    let meta_bytes = meta.write();

    // Both must parse.
    let _ = Bundle::parse(&*bundle_bytes).expect("merged bundle must parse");
    let _ = Metadata::parse(&meta_bytes).expect("merged metadata must parse");

    // Every metadata file must be extractable from the bundle.
    for (name, _) in meta.file_infos.iter().skip(1) {
        assert!(
            bundle.items().iter().any(|i| i.name() == name.as_c_str()),
            "metadata refers to file '{name:?}' not present in bundle"
        );
    }
    // And vice versa: every bundle item must appear in the metadata.
    let meta_paths: HashSet<&[u8]> = meta.file_infos.iter().skip(1).map(|(n, _)| n.as_bytes()).collect();
    for item in bundle.items() {
        assert!(
            meta_paths.contains(item.name().to_bytes()),
            "bundle item '{:?}' not present in metadata",
            item.name()
        );
    }
}

#[test]
fn merge_mods_extracted_data_matches_source_compressed_bytes() {
    // Merge the three mods and verify that every retained item's compressed
    // bytes in the OUTPUT bundle are byte-identical to the corresponding item's
    // bytes in the SOURCE bundle it came from (first-wins).
    let (_, ab, am) = parse_mod("modnmm");
    let (_, bb, bm) = parse_mod("modsbn");
    let (_, tb, tm) = parse_mod("modtest");
    let sources = [(&ab, &am), (&bb, &bm), (&tb, &tm)];

    let (bundle, _) = merge_mods(&[(&ab, &am), (&bb, &bm), (&tb, &tm)], &bundle_name());

    for item in bundle.items() {
        // Find first source containing this item — that's the one Bundle::merge
        // chose (first-wins).
        let expected = sources
            .iter()
            .find_map(|(b, _)| b.items().iter().find(|i| i.name() == item.name()))
            .expect("item must come from some source");
        assert_eq!(
            item.raw(),
            expected.raw(),
            "compressed bytes for '{:?}' differ from source",
            item.name()
        );
    }
}
