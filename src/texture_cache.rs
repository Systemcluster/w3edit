//! `texture.cache` reader, writer, and merger for The Witcher 3.
//!
//! A texture cache stores page-aligned zlib-compressed texture pages followed by
//! several info tables and an EOF footer. Unlike most formats in this crate, the
//! magic (`HCXT`) lives in the footer rather than at byte zero.

use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::io::Write;

use crate::errors::*;
use crate::hash::fnv64;
use crate::util::read;


const MAGIC: &[u8; 4] = b"HCXT";
const FOOTER_SIZE: usize = 32;
const ENTRY_SIZE: usize = 52;
const PAGE: usize = 4096;

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
    /// Number of relative mip offsets; version 7 stores flags in the upper half.
    pub fn mip_offset_count(&self) -> usize {
        (self.num_mip_offsets as u32 & 0xffff) as usize
    }

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
            version:      6,
            data_pages:   Vec::new(),
            mip_offsets:  Vec::new(),
            string_table: vec![0],
            entries:      Vec::new(),
        }
    }

    /// Create a legacy (version 6) or remastered (version 7) texture cache.
    pub fn for_format(format: crate::BundleFormat) -> Self {
        Self {
            version: if format == crate::BundleFormat::Legacy { 6 } else { 7 },
            ..Self::new()
        }
    }

    /// Add already-cooked GPU texture data, split into streaming chunks from
    /// largest mip to smallest. The last chunk may contain several small mips.
    /// Descriptor dimensions, format bytes and hash are supplied by the caller;
    /// sizes, offsets and streaming headers are generated here.
    pub fn insert(
        &mut self,
        name: CString,
        mut descriptor: TextureCacheEntry,
        chunks: &[&[u8]],
    ) -> Result<(), TextureCacheError> {
        if !matches!(self.version, 6 | 7) {
            return Err(layout_error("creation requires texture cache version 6 or 7").into());
        }
        if name.as_bytes().is_empty() || self.find_entry_by_name(name.as_bytes()).is_some() {
            return Err(layout_error("texture name is empty or duplicated").into());
        }
        if chunks.is_empty()
            || chunks.len() > 256
            || chunks.len() > usize::from(descriptor.mipcount)
            || descriptor.base_width == 0
            || descriptor.base_height == 0
            || descriptor.slice_count == 0
        {
            return Err(layout_error("invalid texture dimensions or streaming chunk count").into());
        }
        let mut stream = Vec::new();
        let mut offsets = Vec::new();
        let mut uncompressed_size = 0u32;
        for (index, chunk) in chunks.iter().enumerate() {
            if index > 0 {
                offsets.push(u32::try_from(stream.len()).map_err(|_| overflow("texture stream"))?);
            }
            let size = u32::try_from(chunk.len()).map_err(|_| overflow("texture chunk"))?;
            uncompressed_size = uncompressed_size
                .checked_add(size)
                .ok_or_else(|| overflow("texture size"))?;
            let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(chunk).map_err(|error| ReadError(error.to_string()))?;
            let compressed = encoder.finish().map_err(|error| ReadError(error.to_string()))?;
            let zsize = u32::try_from(compressed.len()).map_err(|_| overflow("compressed chunk"))?;
            stream.extend_from_slice(&zsize.to_le_bytes());
            stream.extend_from_slice(&size.to_le_bytes());
            stream.push((chunks.len() - index - 1) as u8);
            stream.extend_from_slice(&compressed);
        }
        descriptor.page_offset =
            u32::try_from(align_up(self.data_pages.len(), PAGE) / PAGE).map_err(|_| overflow("page index"))?;
        descriptor.compressed_size = u32::try_from(stream.len()).map_err(|_| overflow("texture stream"))?;
        descriptor.uncompressed_size = uncompressed_size;
        descriptor.string_table_offset = i32::try_from(self.string_table.len()).map_err(|_| overflow("strings"))?;
        descriptor.mip_offset_index = i32::try_from(self.mip_offsets.len()).map_err(|_| overflow("mip index"))?;
        descriptor.num_mip_offsets = offsets.len() as i32 | if self.version == 7 { 1 << 16 } else { 0 };
        self.data_pages.resize(descriptor.page_offset as usize * PAGE, 0);
        self.data_pages.extend_from_slice(&stream);
        self.data_pages.resize(align_up(self.data_pages.len(), PAGE), 0);
        self.mip_offsets.extend(offsets);
        push_cstring(&mut self.string_table, &name);
        self.entries.push((name, descriptor));
        Ok(())
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
        Self::merge_selected(sources, |_, _| true)
    }

    pub(crate) fn merge_selected(
        sources: &[&TextureCache],
        mut include: impl FnMut(usize, &CStr) -> bool,
    ) -> TextureCache {
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
        for (source_index, source) in sources.iter().enumerate() {
            for (name, entry) in &source.entries {
                if !include(source_index, name) || !seen.insert(name.clone()) {
                    continue;
                }

                let string_table_offset = push_cstring(&mut out.string_table, name) as i32;
                let mut new_entry = *entry;
                new_entry.string_table_offset = string_table_offset;
                let start = entry.page_offset as usize * PAGE;
                let end = start + entry.compressed_size as usize;
                let target = align_up(out.data_pages.len(), PAGE);
                out.data_pages.resize(target, 0);
                out.data_pages.extend_from_slice(&source.data_pages[start..end]);
                out.data_pages.resize(align_up(out.data_pages.len(), PAGE), 0);
                new_entry.page_offset = u32::try_from(target / PAGE).expect("texture page index exceeds u32");

                if entry.mip_offset_index >= 0 && entry.mip_offset_count() > 0 {
                    let start = entry.mip_offset_index as usize;
                    let count = entry.mip_offset_count();
                    if let Some(slice) = source.mip_offsets.get(start..start.saturating_add(count)) {
                        new_entry.mip_offset_index = out.mip_offsets.len() as i32;
                        out.mip_offsets.extend_from_slice(slice);
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
