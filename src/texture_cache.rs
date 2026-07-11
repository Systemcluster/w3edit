//! `texture.cache` reader, writer, and merger for The Witcher 3.
//!
//! A texture cache stores page-aligned zlib-compressed texture pages followed by
//! several info tables and an EOF footer. Unlike most formats in this crate, the
//! magic (`HCXT`) lives in the footer rather than at byte zero.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};

use crate::errors::*;
use crate::hash::fnv64;
use crate::util::read;


const MAGIC: &[u8; 4] = b"HCXT";
const FOOTER_SIZE: usize = 32;
const ENTRY_SIZE: usize = 52;
const PAGE: usize = 4096;
const PAGE_HEADER_SIZE: usize = 9;

const OFF_HASH: usize = 0;
const OFF_STRING_TABLE: usize = 4;
const OFF_PAGE: usize = 8;
const OFF_COMPRESSED_SIZE: usize = 12;
const OFF_UNCOMPRESSED_SIZE: usize = 16;
const OFF_BASE_ALIGNMENT: usize = 20;
const OFF_BASE_WIDTH: usize = 24;
const OFF_BASE_HEIGHT: usize = 26;
const OFF_MIPCOUNT: usize = 28;
const OFF_SLICE_COUNT: usize = 30;
const OFF_MIP_OFFSET_INDEX: usize = 32;
const OFF_NUM_MIP_OFFSETS: usize = 36;
const OFF_TIMESTAMP: usize = 40;
const OFF_TYPE1: usize = 48;
const OFF_TYPE2: usize = 49;
const OFF_IS_CUBE: usize = 50;
const OFF_UNK1: usize = 51;


/// One 52-byte texture entry from the cache entry table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextureCacheEntry {
    pub hash:                u32,
    pub string_table_offset: i32,
    pub page_offset:         u32,
    pub compressed_size:     u32,
    pub uncompressed_size:   u32,
    pub base_alignment:      u32,
    pub base_width:          u16,
    pub base_height:         u16,
    pub mipcount:            u16,
    pub slice_count:         u16,
    pub mip_offset_index:    i32,
    pub num_mip_offsets:     i32,
    pub time_stamp:          i64,
    pub type1:               u8,
    pub type2:               u8,
    pub is_cube:             u8,
    pub unk1:                u8,
}

impl TextureCacheEntry {
    fn parse(bytes: &[u8]) -> Result<Self, ReadError> {
        if bytes.len() < ENTRY_SIZE {
            return Err(ReadError(format!(
                "texture entry too short: {} < {ENTRY_SIZE}",
                bytes.len()
            )));
        }
        Ok(Self {
            hash:                read::<u32>(&bytes[OFF_HASH..])?,
            string_table_offset: read::<i32>(&bytes[OFF_STRING_TABLE..])?,
            page_offset:         read::<u32>(&bytes[OFF_PAGE..])?,
            compressed_size:     read::<u32>(&bytes[OFF_COMPRESSED_SIZE..])?,
            uncompressed_size:   read::<u32>(&bytes[OFF_UNCOMPRESSED_SIZE..])?,
            base_alignment:      read::<u32>(&bytes[OFF_BASE_ALIGNMENT..])?,
            base_width:          read::<u16>(&bytes[OFF_BASE_WIDTH..])?,
            base_height:         read::<u16>(&bytes[OFF_BASE_HEIGHT..])?,
            mipcount:            read::<u16>(&bytes[OFF_MIPCOUNT..])?,
            slice_count:         read::<u16>(&bytes[OFF_SLICE_COUNT..])?,
            mip_offset_index:    read::<i32>(&bytes[OFF_MIP_OFFSET_INDEX..])?,
            num_mip_offsets:     read::<i32>(&bytes[OFF_NUM_MIP_OFFSETS..])?,
            time_stamp:          read::<i64>(&bytes[OFF_TIMESTAMP..])?,
            type1:               bytes[OFF_TYPE1],
            type2:               bytes[OFF_TYPE2],
            is_cube:             bytes[OFF_IS_CUBE],
            unk1:                bytes[OFF_UNK1],
        })
    }

    fn write(&self) -> [u8; ENTRY_SIZE] {
        let mut out = [0u8; ENTRY_SIZE];
        out[OFF_HASH..OFF_HASH + 4].copy_from_slice(&self.hash.to_le_bytes());
        out[OFF_STRING_TABLE..OFF_STRING_TABLE + 4].copy_from_slice(&self.string_table_offset.to_le_bytes());
        out[OFF_PAGE..OFF_PAGE + 4].copy_from_slice(&self.page_offset.to_le_bytes());
        out[OFF_COMPRESSED_SIZE..OFF_COMPRESSED_SIZE + 4].copy_from_slice(&self.compressed_size.to_le_bytes());
        out[OFF_UNCOMPRESSED_SIZE..OFF_UNCOMPRESSED_SIZE + 4].copy_from_slice(&self.uncompressed_size.to_le_bytes());
        out[OFF_BASE_ALIGNMENT..OFF_BASE_ALIGNMENT + 4].copy_from_slice(&self.base_alignment.to_le_bytes());
        out[OFF_BASE_WIDTH..OFF_BASE_WIDTH + 2].copy_from_slice(&self.base_width.to_le_bytes());
        out[OFF_BASE_HEIGHT..OFF_BASE_HEIGHT + 2].copy_from_slice(&self.base_height.to_le_bytes());
        out[OFF_MIPCOUNT..OFF_MIPCOUNT + 2].copy_from_slice(&self.mipcount.to_le_bytes());
        out[OFF_SLICE_COUNT..OFF_SLICE_COUNT + 2].copy_from_slice(&self.slice_count.to_le_bytes());
        out[OFF_MIP_OFFSET_INDEX..OFF_MIP_OFFSET_INDEX + 4].copy_from_slice(&self.mip_offset_index.to_le_bytes());
        out[OFF_NUM_MIP_OFFSETS..OFF_NUM_MIP_OFFSETS + 4].copy_from_slice(&self.num_mip_offsets.to_le_bytes());
        out[OFF_TIMESTAMP..OFF_TIMESTAMP + 8].copy_from_slice(&self.time_stamp.to_le_bytes());
        out[OFF_TYPE1] = self.type1;
        out[OFF_TYPE2] = self.type2;
        out[OFF_IS_CUBE] = self.is_cube;
        out[OFF_UNK1] = self.unk1;
        out
    }
}


/// A parsed `texture.cache` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureCache {
    pub version:      u32,
    pub data_pages:   Vec<u8>,
    pub mip_offsets:  Vec<u32>,
    pub string_table: Vec<u8>,
    pub entries:      Vec<(CString, TextureCacheEntry)>,
}

impl Default for TextureCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TextureCache {
    /// Construct an empty cache with a NUL string-table sentinel.
    pub fn new() -> Self {
        Self {
            version:      1,
            data_pages:   Vec::new(),
            mip_offsets:  Vec::new(),
            string_table: vec![0],
            entries:      Vec::new(),
        }
    }

    /// Parse a `texture.cache` from bytes.
    pub fn parse(data: &[u8]) -> Result<Self, TextureCacheError> {
        if data.len() < FOOTER_SIZE {
            return Err(ReadError(format!(
                "texture.cache footer needs {FOOTER_SIZE} bytes, got {}",
                data.len()
            ))
            .into());
        }

        let footer = data.len() - FOOTER_SIZE;
        if &data[footer + 24..footer + 28] != MAGIC {
            return Err(ReadError(format!(
                "invalid texture.cache magic: expected {:02X?}, got {:02X?}",
                MAGIC,
                &data[footer + 24..footer + 28]
            ))
            .into());
        }

        let entry_count = read::<u32>(&data[footer + 12..])? as usize;
        let string_table_size = read::<u32>(&data[footer + 16..])? as usize;
        let mip_count = read::<u32>(&data[footer + 20..])? as usize;
        let version = read::<u32>(&data[footer + 28..])?;

        let entry_bytes = entry_count
            .checked_mul(ENTRY_SIZE)
            .ok_or_else(|| overflow("entry table"))?;
        let mip_bytes = mip_count.checked_mul(4).ok_or_else(|| overflow("mip-offset table"))?;
        let entry_start = footer
            .checked_sub(entry_bytes)
            .ok_or_else(|| layout_error("entry table starts before file"))?;
        let string_start = entry_start
            .checked_sub(string_table_size)
            .ok_or_else(|| layout_error("string table starts before file"))?;
        let mip_start = string_start
            .checked_sub(mip_bytes)
            .ok_or_else(|| layout_error("mip-offset table starts before file"))?;

        let data_pages = data[..mip_start].to_vec();
        let mut mip_offsets = Vec::with_capacity(mip_count);
        for i in 0..mip_count {
            let offset = mip_start + i * 4;
            mip_offsets.push(read::<u32>(&data[offset..])?);
        }
        let string_table = data[string_start..entry_start].to_vec();
        let mut entries = Vec::with_capacity(entry_count);
        for i in 0..entry_count {
            let offset = entry_start + i * ENTRY_SIZE;
            let entry = TextureCacheEntry::parse(&data[offset..])?;
            entries.push((read_cstring(&string_table, entry.string_table_offset), entry));
        }

        Ok(Self {
            version,
            data_pages,
            mip_offsets,
            string_table,
            entries,
        })
    }

    /// Encode this texture cache to bytes.
    pub fn write(&self) -> Vec<u8> {
        let entry_table = self.entry_table_bytes();
        let mut info_for_crc = Vec::with_capacity(self.string_table.len() + entry_table.len());
        info_for_crc.extend_from_slice(&self.string_table);
        info_for_crc.extend_from_slice(&entry_table);
        let crc = fnv64(info_for_crc);

        let mut out = Vec::with_capacity(
            self.data_pages.len()
                + self.mip_offsets.len() * 4
                + self.string_table.len()
                + entry_table.len()
                + FOOTER_SIZE,
        );
        out.extend_from_slice(&self.data_pages);
        for &offset in &self.mip_offsets {
            out.extend_from_slice(&offset.to_le_bytes());
        }
        out.extend_from_slice(&self.string_table);
        out.extend_from_slice(&entry_table);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&used_pages(self.data_pages.len()).to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.string_table.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.mip_offsets.len() as u32).to_le_bytes());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out
    }

    /// Union multiple caches with first-wins priority by string-table name.
    pub fn merge(sources: &[&TextureCache]) -> TextureCache {
        if sources.is_empty() {
            return TextureCache::new();
        }

        let mut out = TextureCache {
            version:      sources.iter().map(|s| s.version).max().unwrap_or(1),
            data_pages:   Vec::new(),
            mip_offsets:  Vec::new(),
            string_table: vec![0],
            entries:      Vec::new(),
        };
        let mut seen = HashSet::new();
        let mut copied_pages: HashMap<(usize, u32), u32> = HashMap::new();

        for (source_index, source) in sources.iter().enumerate() {
            for (name, entry) in &source.entries {
                if !seen.insert(name.clone()) {
                    continue;
                }

                let string_table_offset = push_cstring(&mut out.string_table, name) as i32;
                let mut new_entry = *entry;
                new_entry.string_table_offset = string_table_offset;
                new_entry.page_offset = copy_page(
                    source_index,
                    source,
                    &mut out.data_pages,
                    &mut copied_pages,
                    entry.page_offset,
                );

                if entry.mip_offset_index >= 0 && entry.num_mip_offsets > 0 {
                    let start = entry.mip_offset_index as usize;
                    let count = entry.num_mip_offsets as usize;
                    if let Some(slice) = source.mip_offsets.get(start..start.saturating_add(count)) {
                        new_entry.mip_offset_index = out.mip_offsets.len() as i32;
                        new_entry.num_mip_offsets = slice.len() as i32;
                        for &offset in slice {
                            let new_offset =
                                copy_page(source_index, source, &mut out.data_pages, &mut copied_pages, offset);
                            out.mip_offsets.push(new_offset);
                        }
                    } else {
                        new_entry.mip_offset_index = -1;
                        new_entry.num_mip_offsets = 0;
                    }
                } else {
                    new_entry.mip_offset_index = -1;
                    new_entry.num_mip_offsets = 0;
                }

                out.entries.push((name.clone(), new_entry));
            }
        }

        out
    }

    pub fn string_at(&self, offset: i32) -> &CStr {
        if offset < 0 {
            return c"";
        }
        let offset = offset as usize;
        if offset >= self.string_table.len() {
            return c"";
        }
        CStr::from_bytes_until_nul(&self.string_table[offset..]).unwrap_or(c"")
    }

    pub fn find_entry_by_name(&self, name: &[u8]) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, (n, _))| n.as_bytes() == name)
            .map(|(i, _)| i)
    }

    fn entry_table_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.entries.len() * ENTRY_SIZE);
        for (_, entry) in &self.entries {
            out.extend_from_slice(&entry.write());
        }
        out
    }
}

fn copy_page(
    source_index: usize,
    source: &TextureCache,
    out_pages: &mut Vec<u8>,
    copied_pages: &mut HashMap<(usize, u32), u32>,
    source_offset: u32,
) -> u32 {
    if let Some(&offset) = copied_pages.get(&(source_index, source_offset)) {
        return offset;
    }

    let new_offset = align_up(out_pages.len(), PAGE) as u32;
    out_pages.resize(new_offset as usize, 0);

    if let Some(range) = page_range(&source.data_pages, source_offset) {
        out_pages.extend_from_slice(&source.data_pages[range]);
    }

    copied_pages.insert((source_index, source_offset), new_offset);
    new_offset
}

fn page_range(data_pages: &[u8], offset: u32) -> Option<std::ops::Range<usize>> {
    let start = offset as usize;
    let header_end = start.checked_add(PAGE_HEADER_SIZE)?;
    if header_end > data_pages.len() {
        return None;
    }
    let zsize = read::<u32>(&data_pages[start..]).ok()? as usize;
    let end = header_end.checked_add(zsize)?;
    (end <= data_pages.len()).then_some(start..end)
}

fn push_cstring(string_table: &mut Vec<u8>, value: &CString) -> usize {
    let offset = string_table.len();
    string_table.extend_from_slice(value.as_bytes_with_nul());
    offset
}

fn read_cstring(string_table: &[u8], offset: i32) -> CString {
    if offset < 0 {
        return CString::default();
    }
    let offset = offset as usize;
    if offset >= string_table.len() {
        return CString::default();
    }
    match CStr::from_bytes_until_nul(&string_table[offset..]) {
        Ok(cstr) => cstr.to_owned(),
        Err(_) => CString::default(),
    }
}

fn used_pages(data_len: usize) -> u32 {
    if data_len == 0 {
        0
    } else {
        (align_up(data_len, PAGE) / PAGE) as u32
    }
}

fn align_up(value: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (value + (align - 1)) & !(align - 1)
}

fn overflow(section: &str) -> ReadError {
    ReadError(format!("integer overflow computing texture.cache {section} size"))
}

fn layout_error(message: &str) -> ReadError {
    ReadError(message.to_string())
}
