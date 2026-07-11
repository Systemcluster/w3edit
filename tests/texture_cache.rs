//! Integration tests for `TextureCache` parse / write / merge.

use std::collections::HashSet;
use std::ffi::CString;

use w3edit::texture_cache::{TextureCache, TextureCacheEntry};

const PAGE: usize = 4096;

fn align_up(value: usize, align: usize) -> usize {
    (value + (align - 1)) & !(align - 1)
}

fn push_string(string_table: &mut Vec<u8>, value: &str) -> i32 {
    let offset = string_table.len() as i32;
    string_table.extend_from_slice(value.as_bytes());
    string_table.push(0);
    offset
}

fn push_page(data_pages: &mut Vec<u8>, mip_index: u8, payload: &[u8]) -> u32 {
    let offset = align_up(data_pages.len(), PAGE);
    data_pages.resize(offset, 0);
    data_pages.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    data_pages.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    data_pages.push(mip_index);
    data_pages.extend_from_slice(payload);
    offset as u32
}

fn fixture(prefix: &str) -> TextureCache {
    let mut string_table = vec![0];
    let mut data_pages = Vec::new();
    let mut mip_offsets = Vec::new();
    let mut entries = Vec::new();

    for (idx, name) in [
        format!(r"textures\{prefix}\shared.xbm"),
        format!(r"textures\{prefix}\unique.xbm"),
    ]
    .into_iter()
    .enumerate()
    {
        let string_table_offset = push_string(&mut string_table, &name);
        let base_offset = push_page(&mut data_pages, 0, &[idx as u8 + 1, 2, 3]);
        let mip_offset = push_page(&mut data_pages, 1, &[idx as u8 + 4, 5]);
        let mip_offset_index = mip_offsets.len() as i32;
        mip_offsets.push(mip_offset);

        entries.push((CString::new(name).unwrap(), TextureCacheEntry {
            hash: idx as u32 + 100,
            string_table_offset,
            page_offset: base_offset,
            compressed_size: 3,
            uncompressed_size: 3,
            base_alignment: 16,
            base_width: 64,
            base_height: 32,
            mipcount: 2,
            slice_count: 1,
            mip_offset_index,
            num_mip_offsets: 1,
            time_stamp: 1234 + idx as i64,
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

fn names(cache: &TextureCache) -> HashSet<Vec<u8>> {
    cache.entries.iter().map(|(name, _)| name.as_bytes().to_vec()).collect()
}

#[test]
fn parse_write_parse_is_stable() {
    let cache = fixture("a");
    let bytes = cache.write();
    let parsed = TextureCache::parse(&bytes).expect("synthetic cache should parse");
    assert_eq!(parsed.version, 3);
    assert_eq!(parsed.entries.len(), 2);
    assert_eq!(bytes, parsed.write(), "texture.cache write should be stable");
}

#[test]
fn parse_rejects_bad_footer_magic() {
    let mut bytes = fixture("bad").write();
    let len = bytes.len();
    bytes[len - 8] ^= 0xFF;
    assert!(TextureCache::parse(&bytes).is_err());
}

#[test]
fn string_table_offsets_resolve_names() {
    let parsed = TextureCache::parse(&fixture("resolve").write()).unwrap();
    for (idx, (name, entry)) in parsed.entries.iter().enumerate() {
        assert_eq!(parsed.string_at(entry.string_table_offset), name.as_c_str());
        assert_eq!(parsed.find_entry_by_name(name.as_bytes()), Some(idx));
    }
}

#[test]
fn merge_empty_yields_empty_cache() {
    let merged = TextureCache::merge(&[]);
    assert!(merged.entries.is_empty());
    assert!(merged.data_pages.is_empty());
    assert!(merged.mip_offsets.is_empty());
    assert_eq!(merged.string_table, vec![0]);
    let reparsed = TextureCache::parse(&merged.write()).expect("empty merged cache should parse");
    assert!(reparsed.entries.is_empty());
}

#[test]
fn merge_disjoint_sources_yields_union_of_names() {
    let a = fixture("a");
    let b = fixture("b");
    let merged = TextureCache::merge(&[&a, &b]);
    let union: HashSet<_> = names(&a).union(&names(&b)).cloned().collect();
    assert_eq!(names(&merged), union);
    assert_eq!(merged.write(), TextureCache::parse(&merged.write()).unwrap().write());
}

#[test]
fn merge_first_wins_and_repacks_offsets() {
    let a = fixture("winner");
    let mut b = fixture("winner");
    b.entries[0].1.hash = 999;
    b.entries[0].1.base_width = 512;

    let merged = TextureCache::merge(&[&a, &b]);
    assert_eq!(merged.entries.len(), 2);
    assert_eq!(merged.entries[0].1.hash, a.entries[0].1.hash);
    assert_eq!(merged.entries[0].1.base_width, a.entries[0].1.base_width);

    for (_, entry) in &merged.entries {
        assert_eq!(entry.page_offset as usize % PAGE, 0);
        assert!(entry.page_offset as usize + 9 + entry.compressed_size as usize <= merged.data_pages.len());
        assert!(entry.mip_offset_index >= 0);
        let mip_offset = merged.mip_offsets[entry.mip_offset_index as usize];
        assert_eq!(mip_offset as usize % PAGE, 0);
        assert!(mip_offset as usize + 9 <= merged.data_pages.len());
    }
}
