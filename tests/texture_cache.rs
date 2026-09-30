//! Integration tests for `TextureCache` parse / write / merge.

use std::collections::HashSet;
use std::ffi::CString;

use w3edit::texture_cache::{TextureCache, TextureCacheEntry};

const PAGE: usize = 4096;

fn fixture(prefix: &str) -> TextureCache {
    let mut cache = TextureCache::new();

    for (idx, name) in [
        format!(r"textures\{prefix}\shared.xbm"),
        format!(r"textures\{prefix}\unique.xbm"),
    ]
    .into_iter()
    .enumerate()
    {
        cache
            .insert(
                CString::new(name).unwrap(),
                TextureCacheEntry {
                    hash: idx as u32 + 100,
                    base_alignment: 16,
                    base_width: 64,
                    base_height: 32,
                    mipcount: 2,
                    slice_count: 1,
                    time_stamp: 1234 + idx as i64,
                    type1: 1,
                    type2: 2,
                    is_cube: 0,
                    unk1: 9,
                    ..TextureCacheEntry::default()
                },
                &[&[idx as u8 + 1, 2, 3], &[idx as u8 + 4, 5]],
            )
            .unwrap();
    }
    cache
}

fn names(cache: &TextureCache) -> HashSet<Vec<u8>> {
    cache.entries.iter().map(|(name, _)| name.as_bytes().to_vec()).collect()
}

#[test]
fn parse_write_parse_is_stable() {
    let cache = fixture("a");
    let bytes = cache.write();
    let parsed = TextureCache::parse(&bytes).expect("synthetic cache should parse");
    assert_eq!(parsed.version, 6);
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
        assert!(entry.page_offset as usize * PAGE + entry.compressed_size as usize <= merged.data_pages.len());
        assert!(entry.mip_offset_index >= 0);
        let mip_offset = merged.mip_offsets[entry.mip_offset_index as usize];
        assert!(mip_offset + 9 < entry.compressed_size);
    }
}

#[test]
fn create_both_cache_formats_and_decode_chunks() {
    use std::io::Read;
    use w3edit::BundleFormat;
    for format in [BundleFormat::Legacy, BundleFormat::Remastered] {
        let mut cache = TextureCache::for_format(format);
        let descriptor = TextureCacheEntry {
            base_width: 16,
            base_height: 16,
            mipcount: 2,
            slice_count: 1,
            ..TextureCacheEntry::default()
        };
        cache
            .insert(c"textures\\test.xbm".into(), descriptor, &[b"base data", b"mip data"])
            .unwrap();
        let parsed = TextureCache::parse(&cache.write()).unwrap();
        assert_eq!(parsed.version, if format == BundleFormat::Legacy { 6 } else { 7 });
        let entry = parsed.entries[0].1;
        assert_eq!(entry.mip_offset_count(), 1);
        assert_eq!(
            entry.num_mip_offsets >> 16,
            if format == BundleFormat::Legacy { 0 } else { 1 }
        );
        for (index, relative) in [0, parsed.mip_offsets[0]].into_iter().enumerate() {
            let start = entry.page_offset as usize * PAGE + relative as usize;
            let size = u32::from_le_bytes(parsed.data_pages[start..start + 4].try_into().unwrap()) as usize;
            assert_eq!(parsed.data_pages[start + 8], (1 - index) as u8);
            let mut decoded = Vec::new();
            flate2::read::ZlibDecoder::new(&parsed.data_pages[start + 9..start + 9 + size])
                .read_to_end(&mut decoded)
                .unwrap();
            assert_eq!(
                decoded,
                if index == 0 {
                    b"base data".as_slice()
                } else {
                    b"mip data".as_slice()
                }
            );
        }
        assert_eq!(TextureCache::merge(&[&parsed]).write(), parsed.write());
    }
}

#[test]
fn real_legacy_cache_merge_preserves_entire_streams() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/test/assets/modsbn/texture.cache")).unwrap();
    let cache = TextureCache::parse(&bytes).unwrap();
    let merged = TextureCache::merge(&[&cache]);
    for ((_, source), (_, target)) in cache.entries.iter().zip(&merged.entries) {
        let source_start = source.page_offset as usize * PAGE;
        let target_start = target.page_offset as usize * PAGE;
        let size = source.compressed_size as usize;
        assert_eq!(
            &cache.data_pages[source_start..source_start + size],
            &merged.data_pages[target_start..target_start + size]
        );
        let source_mips = source.mip_offset_index as usize;
        let target_mips = target.mip_offset_index as usize;
        assert_eq!(
            &cache.mip_offsets[source_mips..source_mips + source.mip_offset_count()],
            &merged.mip_offsets[target_mips..target_mips + target.mip_offset_count()]
        );
    }
}

#[test]
#[ignore = "requires W3_GAME_DIR pointing at a local game installation"]
fn installed_remastered_texture_creation() {
    use std::io::{Read, Seek, SeekFrom};
    use std::path::PathBuf;
    use w3edit::BundleFormat;

    let root = PathBuf::from(std::env::var_os("W3_GAME_DIR").expect("set W3_GAME_DIR"));
    let mut file = std::fs::File::open(root.join("content/content0/texture.cache")).unwrap();
    file.seek(SeekFrom::End(-32)).unwrap();
    let mut footer = [0u8; 32];
    file.read_exact(&mut footer).unwrap();
    let entries = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as u64;
    let strings = u32::from_le_bytes(footer[16..20].try_into().unwrap()) as u64;
    let mips = u32::from_le_bytes(footer[20..24].try_into().unwrap()) as u64;
    let table_size = entries * 52 + strings + mips * 4 + 32;
    file.seek(SeekFrom::End(-(table_size as i64))).unwrap();
    let mut tables = Vec::new();
    file.read_to_end(&mut tables).unwrap();
    let source = TextureCache::parse(&tables).unwrap();
    assert_eq!(source.version, 7);
    let mut created = TextureCache::for_format(BundleFormat::Remastered);
    for (name, descriptor) in source.entries.iter().take(2) {
        let mut compressed = vec![0; descriptor.compressed_size as usize];
        file.seek(SeekFrom::Start(u64::from(descriptor.page_offset) * 4096))
            .unwrap();
        file.read_exact(&mut compressed).unwrap();
        let first = descriptor.mip_offset_index as usize;
        let relative_offsets = std::iter::once(0).chain(
            source.mip_offsets[first..first + descriptor.mip_offset_count()]
                .iter()
                .copied(),
        );
        let mut chunks = Vec::new();
        for relative in relative_offsets {
            let start = relative as usize;
            let size = u32::from_le_bytes(compressed[start..start + 4].try_into().unwrap()) as usize;
            let mut chunk = Vec::new();
            flate2::read::ZlibDecoder::new(&compressed[start + 9..start + 9 + size])
                .read_to_end(&mut chunk)
                .unwrap();
            chunks.push(chunk);
        }
        assert_eq!(
            chunks.iter().map(Vec::len).sum::<usize>(),
            descriptor.uncompressed_size as usize
        );
        let chunk_refs: Vec<_> = chunks.iter().map(Vec::as_slice).collect();
        created.insert(name.clone(), *descriptor, &chunk_refs).unwrap();
        let output = created.entries.last().unwrap().1;
        assert_eq!(output.num_mip_offsets, descriptor.num_mip_offsets);
        let mut start = output.page_offset as usize * PAGE;
        for expected in &chunks {
            let size = u32::from_le_bytes(created.data_pages[start..start + 4].try_into().unwrap()) as usize;
            let mut decoded = Vec::new();
            flate2::read::ZlibDecoder::new(&created.data_pages[start + 9..start + 9 + size])
                .read_to_end(&mut decoded)
                .unwrap();
            assert_eq!(&decoded, expected);
            start += 9 + size;
        }
    }
    assert_eq!(created, TextureCache::parse(&created.write()).unwrap());
    assert_eq!(created.write(), TextureCache::merge(&[&created]).write());
}
