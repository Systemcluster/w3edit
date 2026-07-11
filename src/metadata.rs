//! `metadata.store` container reader, writer, and merger for The Witcher 3.
//!
//! A `metadata.store` file is an index alongside one or more `.bundle` files.
//! It records, for every packed file: its full path, the bundle it lives in, its
//! offset and size within that bundle, plus a directory tree and hash lookup.
//!
//! # Format
//! Roughly (see [`w3edit-formats.md`](../../../w3edit-formats.md) for details):
//! - Header: magic `\x03VTM`, `version: u32`, `max_size_in_bundle: u32`,
//!   `max_size_in_memory: u32`, `string_table_size: vlq_i32`.
//! - String table: raw bytes; offsets in later records are relative to the start
//!   of this table. Offset 0 is always a NUL sentinel (empty string).
//! - Seven [`DynamicArray`]-encoded sections in order: file_infos,
//!   entry_infos, bundle_infos, buffers, dir_init_infos, file_init_infos,
//!   hashes.
//!
//! # Index-0 conventions
//! Observed in all shipped fixtures:
//! - `file_infos[0]`, `entry_infos[0]`, `bundle_infos[0]` are all-zero
//!   *null placeholders*. Real records start at index 1.
//! - `dir_init_infos[0]` is the *root* directory (empty name, self as parent).
//!   Real subdirectories start at index 1.
//! - `file_init_infos` and `hashes` have no placeholder — index 0 is a real
//!   record.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};

use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::build::{PendingBundle, PendingEntry, PendingFile, build_metadata};
use crate::errors::*;
use crate::util::*;


// -----------------------------------------------------------------------------
// Format constants
// -----------------------------------------------------------------------------

/// Magic bytes at the start of every `metadata.store`.
const MAGIC: &[u8; 4] = b"\x03VTM";

/// Fixed-size portion of the header (magic + 3 × u32). The `string_table_size`
/// that follows is a VLQ and thus variable-width.
const HEADER_FIXED_SIZE: usize = 16;

/// Serialised size of a [`MetadataFileInfo`] record.
const FILE_INFO_SIZE: usize = 32;
/// Serialised size of a [`MetadataFileEntryInfo`] record.
const ENTRY_INFO_SIZE: usize = 20;
/// Serialised size of a [`MetadataBundleInfo`] record.
const BUNDLE_INFO_SIZE: usize = 24;
/// Serialised size of a [`MetadataDirectoryInitInfo`] record.
const DIR_INIT_INFO_SIZE: usize = 8;
/// Serialised size of a [`MetadataFileInitInfo`] record.
const FILE_INIT_INFO_SIZE: usize = 12;
/// Serialised size of a [`MetadataHash`] record.
const HASH_SIZE: usize = 16;
/// Size of one buffer entry (single `u32`).
const BUFFER_ENTRY_SIZE: usize = 4;

pub(crate) const CURRENT_METADATA_VERSION: u32 = 6;


// -----------------------------------------------------------------------------
// Records
// -----------------------------------------------------------------------------

/// A single file's metadata: path (via `string_table_name_offset`), sizes,
/// compression, and a link into the entry chain.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataFileInfo {
    pub string_table_name_offset: u32,
    pub path_hash:                u32,
    pub size_in_bundle:           u32,
    pub size_in_memory:           u32,
    /// Index into `entry_infos`; start of this file's entry chain. 0 = none.
    pub first_entry:              u32,
    pub compression_type:         u32,
    /// Index into `buffers` (when `has_buffer != 0`).
    pub buffer_id:                u32,
    /// Boolean-ish flag. 0 = no buffer, non-zero = uses `buffer_id`.
    pub has_buffer:               u32,
}

/// One entry in the (file × bundle) chain: points to a bundle and records the
/// file's offset/size inside that bundle. `next_entry` links to the next entry
/// for the same file (0 terminates).
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataFileEntryInfo {
    /// Index into `file_infos`.
    pub file_id:          u32,
    /// Index into `bundle_infos`.
    pub bundle_id:        u32,
    pub offset_in_bundle: u32,
    pub size_in_bundle:   u32,
    /// Index into `entry_infos`; next entry for the same file. 0 = end.
    pub next_entry:       u32,
}

/// One `.bundle` file referenced by this metadata.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataBundleInfo {
    pub string_table_name_offset: u32,
    /// Index into `entry_infos`; start of this bundle's entry group.
    pub first_file_entry:         u32,
    pub num_bundle_entries:       u32,
    pub data_block_size:          u32,
    pub data_block_offset:        u32,
    pub burst_data_block_size:    u32,
}

/// One directory in the directory tree. Index 0 is the root (empty name,
/// self as parent).
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataDirectoryInitInfo {
    pub string_table_name_offset: i32,
    /// Index into `dir_init_infos`; parent directory.
    pub parent:                   i32,
}

/// One file's placement in the directory tree.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataFileInitInfo {
    /// Index into `file_infos`.
    pub file_id:                  i32,
    /// Index into `dir_init_infos`.
    pub directory_id:             i32,
    /// Offset of the leaf filename (without directory components).
    pub string_table_name_offset: i32,
}

/// A `(hash, file_id)` pair used for hash-based lookup.
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataHash {
    pub hash:    i64,
    pub file_id: i64,
}


// -----------------------------------------------------------------------------
// DynamicArray codec
// -----------------------------------------------------------------------------

/// Codec for sections written as `VLQ_i32 count` + `count × T` (little-endian).
pub(crate) struct DynamicArray;

impl DynamicArray {
    /// Read a section and return the parsed items plus the number of bytes
    /// consumed (VLQ + payload).
    pub fn parse<T: FromBytes + Clone>(data: &[u8]) -> Result<(Vec<T>, usize), ReadError> {
        let (count, count_len) = read_vlq_i32(data)?;
        let count = count as usize;
        let item_size = std::mem::size_of::<T>();
        let payload = count_len
            .checked_add(count.checked_mul(item_size).ok_or_else(overflow_read)?)
            .ok_or_else(overflow_read)?;
        if data.len() < payload {
            return Err(ReadError(format!(
                "DynamicArray<{}> needs {payload} bytes, got {}",
                std::any::type_name::<T>(),
                data.len()
            )));
        }
        let mut items = Vec::with_capacity(count);
        let mut position = count_len;
        for _ in 0..count {
            let item: T = read(&data[position..])?;
            items.push(item);
            position += item_size;
        }
        Ok((items, position))
    }

    /// Encode a section: `VLQ_i32(count)` followed by each item's bytes.
    pub fn write<T: IntoBytes + Immutable>(items: &[T]) -> Vec<u8> {
        let mut out = write_vlq_i32(items.len() as i32);
        out.reserve(std::mem::size_of_val(items));
        for item in items {
            out.extend_from_slice(item.as_bytes());
        }
        out
    }
}

fn overflow_read() -> ReadError {
    ReadError("integer overflow computing DynamicArray payload size".to_string())
}


// -----------------------------------------------------------------------------
// Metadata
// -----------------------------------------------------------------------------

/// A parsed `metadata.store` file.
///
/// # Invariants
/// All shipped fixtures satisfy the following invariants, and [`Metadata::write`]
/// assumes they hold. Mutating the public fields is allowed but the caller is
/// responsible for preserving them; violating any of these will produce a store
/// that other parsers cannot read.
///
/// - `string_table[0] == 0` (empty-string sentinel).
/// - `file_infos[0]`, `entry_infos[0]`, `bundle_infos[0]` are all-zero null
///   placeholders. Real records start at index 1.
/// - `dir_init_infos[0]` is the root directory: empty name (offset 0), self as
///   parent (index 0).
/// - Every `string_table_name_offset` points at a NUL-terminated byte range
///   inside `string_table`.
/// - Every cross-index (`first_entry`, `next_entry`, `bundle_id`, `file_id`,
///   `buffer_id`, `parent`, `directory_id`) is either 0 (or negative, where
///   the field type permits) or a valid in-range index into its target array.
/// - Every entry chain reachable from `file_infos[i].first_entry` terminates
///   (`next_entry == 0`).
#[derive(Debug, Clone)]
pub struct Metadata {
    pub version:            u32,
    pub max_size_in_bundle: u32,
    pub max_size_in_memory: u32,
    /// Raw string table bytes. Offsets in records index into this vector.
    pub string_table:       Vec<u8>,
    /// `(name, record)`. The name is cached from `record.string_table_name_offset`
    /// for convenience; index 0 is a null placeholder.
    pub file_infos:         Vec<(CString, MetadataFileInfo)>,
    /// Index 0 is a null placeholder.
    pub entry_infos:        Vec<MetadataFileEntryInfo>,
    /// Index 0 is a null placeholder.
    pub bundle_infos:       Vec<MetadataBundleInfo>,
    pub buffers:            Vec<u32>,
    /// Index 0 is the root directory (empty name, self as parent).
    pub dir_init_infos:     Vec<MetadataDirectoryInitInfo>,
    pub file_init_infos:    Vec<MetadataFileInitInfo>,
    pub hashes:             Vec<MetadataHash>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self::new()
    }
}

impl Metadata {
    /// Construct an empty `Metadata` with the standard index-0 placeholders
    /// (empty string table sentinel, null file/entry/bundle records, root dir).
    pub fn new() -> Self {
        Self {
            version:            CURRENT_METADATA_VERSION,
            max_size_in_bundle: 0,
            max_size_in_memory: 0,
            string_table:       vec![0u8],
            file_infos:         vec![(CString::default(), MetadataFileInfo::default())],
            entry_infos:        vec![MetadataFileEntryInfo::default()],
            bundle_infos:       vec![MetadataBundleInfo::default()],
            buffers:            Vec::new(),
            dir_init_infos:     vec![MetadataDirectoryInitInfo {
                string_table_name_offset: 0,
                parent:                   0,
            }],
            file_init_infos:    Vec::new(),
            hashes:             Vec::new(),
        }
    }

    // -------- parsing --------

    /// Parse a `metadata.store` from a byte slice.
    pub fn parse(data: &[u8]) -> Result<Self, MetadataError> {
        // 1. Header
        if data.len() < HEADER_FIXED_SIZE {
            return Err(ReadError(format!(
                "metadata header needs {HEADER_FIXED_SIZE} bytes, got {}",
                data.len()
            ))
            .into());
        }
        if &data[..4] != MAGIC {
            return Err(ReadError(format!(
                "invalid magic: expected {:02X?}, got {:02X?}",
                MAGIC,
                &data[..4.min(data.len())]
            ))
            .into());
        }
        let version: u32 = read(&data[4..])?;
        let max_size_in_bundle: u32 = read(&data[8..])?;
        let max_size_in_memory: u32 = read(&data[12..])?;

        // 2. String table
        let (string_table_size, vlq_len) = read_vlq_i32(&data[16..])?;
        let string_table_size = string_table_size as usize;
        let mut position = 16 + vlq_len;
        if data.len() < position + string_table_size {
            return Err(ReadError(format!(
                "string table needs {string_table_size} bytes at offset {position}, only {} available",
                data.len().saturating_sub(position)
            ))
            .into());
        }
        let string_table = data[position..position + string_table_size].to_vec();
        position += string_table_size;

        // 3. file_infos
        let (raw_file_infos, consumed) = DynamicArray::parse::<MetadataFileInfo>(&data[position..])?;
        position += consumed;
        let file_infos = raw_file_infos
            .into_iter()
            .map(|fi| (read_cstring(&string_table, fi.string_table_name_offset), fi))
            .collect::<Vec<_>>();

        // 4. entry_infos
        let (entry_infos, consumed) = DynamicArray::parse::<MetadataFileEntryInfo>(&data[position..])?;
        position += consumed;

        // 5. bundle_infos
        let (bundle_infos, consumed) = DynamicArray::parse::<MetadataBundleInfo>(&data[position..])?;
        position += consumed;

        // 6. buffers (VLQ count + u32 payload)
        let (buffer_count, vlq_len) = read_vlq_i32(&data[position..])?;
        position += vlq_len;
        let buffer_count = buffer_count as usize;
        let buffer_bytes = buffer_count * BUFFER_ENTRY_SIZE;
        if data.len() < position + buffer_bytes {
            return Err(ReadError(format!(
                "buffers section needs {buffer_bytes} bytes, only {} available",
                data.len().saturating_sub(position)
            ))
            .into());
        }
        let mut buffers = Vec::with_capacity(buffer_count);
        for _ in 0..buffer_count {
            buffers.push(read::<u32>(&data[position..])?);
            position += BUFFER_ENTRY_SIZE;
        }

        // 7. dir_init_infos
        let (dir_init_infos, consumed) = DynamicArray::parse::<MetadataDirectoryInitInfo>(&data[position..])?;
        position += consumed;

        // 8. file_init_infos
        let (file_init_infos, consumed) = DynamicArray::parse::<MetadataFileInitInfo>(&data[position..])?;
        position += consumed;

        // 9. hashes
        let (hashes, _consumed) = DynamicArray::parse::<MetadataHash>(&data[position..])?;

        Ok(Self {
            version,
            max_size_in_bundle,
            max_size_in_memory,
            string_table,
            file_infos,
            entry_infos,
            bundle_infos,
            buffers,
            dir_init_infos,
            file_init_infos,
            hashes,
        })
    }

    // -------- writing --------

    /// Encode this metadata to bytes. Round-trips byte-for-byte for the shipped
    /// fixtures.
    pub fn write(&self) -> Vec<u8> {
        let file_info_records: Vec<MetadataFileInfo> = self.file_infos.iter().map(|(_, fi)| *fi).collect();

        let capacity = HEADER_FIXED_SIZE
            + 5 // approx VLQ sizes
            + self.string_table.len()
            + 5 + file_info_records.len() * FILE_INFO_SIZE
            + 5 + self.entry_infos.len() * ENTRY_INFO_SIZE
            + 5 + self.bundle_infos.len() * BUNDLE_INFO_SIZE
            + 5 + self.buffers.len() * BUFFER_ENTRY_SIZE
            + 5 + self.dir_init_infos.len() * DIR_INIT_INFO_SIZE
            + 5 + self.file_init_infos.len() * FILE_INIT_INFO_SIZE
            + 5 + self.hashes.len() * HASH_SIZE;
        let mut out = Vec::with_capacity(capacity);

        // Header
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.max_size_in_bundle.to_le_bytes());
        out.extend_from_slice(&self.max_size_in_memory.to_le_bytes());

        // String table
        out.extend_from_slice(&write_vlq_i32(self.string_table.len() as i32));
        out.extend_from_slice(&self.string_table);

        // Seven arrays in order
        out.extend_from_slice(&DynamicArray::write(&file_info_records));
        out.extend_from_slice(&DynamicArray::write(&self.entry_infos));
        out.extend_from_slice(&DynamicArray::write(&self.bundle_infos));

        // buffers: VLQ count + raw u32s
        out.extend_from_slice(&write_vlq_i32(self.buffers.len() as i32));
        for &b in &self.buffers {
            out.extend_from_slice(&b.to_le_bytes());
        }

        out.extend_from_slice(&DynamicArray::write(&self.dir_init_infos));
        out.extend_from_slice(&DynamicArray::write(&self.file_init_infos));
        out.extend_from_slice(&DynamicArray::write(&self.hashes));

        out
    }

    // -------- helpers --------

    /// Resolve a string in the table by offset. Returns an empty `CStr` if the
    /// offset is out of range or not NUL-terminated.
    pub fn string_at(&self, offset: u32) -> &CStr {
        let offset = offset as usize;
        if offset >= self.string_table.len() {
            return c"";
        }
        CStr::from_bytes_until_nul(&self.string_table[offset..]).unwrap_or(c"")
    }

    /// Path of the file at `file_id` (1-based; `0` is the null placeholder).
    pub fn file_path(&self, file_id: usize) -> Option<&CStr> {
        self.file_infos.get(file_id).map(|(name, _)| name.as_c_str())
    }

    /// Name of the bundle at `bundle_id` (1-based; `0` is the null placeholder).
    pub fn bundle_name(&self, bundle_id: usize) -> Option<&CStr> {
        let bi = self.bundle_infos.get(bundle_id)?;
        Some(self.string_at(bi.string_table_name_offset))
    }

    /// Linear-scan lookup: find the `file_id` (1-based index into
    /// `file_infos`) whose cached path matches `name`. Returns `None` if no
    /// match is found. The index-0 null placeholder is skipped.
    pub fn find_file_id_by_name(&self, name: &[u8]) -> Option<usize> {
        self.file_infos
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, (n, _))| n.as_bytes() == name)
            .map(|(i, _)| i)
    }

    /// Look up the hash entry for `file_id` (1-based). Linear scan of
    /// `hashes`; prefer [`hash_lookup`](Self::hash_lookup) when querying many
    /// files.
    pub fn hash_of_file_id(&self, file_id: i64) -> Option<i64> {
        self.hashes.iter().find(|h| h.file_id == file_id).map(|h| h.hash)
    }

    /// Build a `file_id -> hash` map for O(1) repeated lookups.
    pub fn hash_lookup(&self) -> HashMap<i64, i64> {
        self.hashes.iter().map(|h| (h.file_id, h.hash)).collect()
    }

    /// Resolve the buffer size for a file record, honouring `has_buffer`.
    /// Returns `None` when the file has no buffer or when `buffer_id` is out
    /// of range.
    pub fn buffer_size_of(&self, fi: &MetadataFileInfo) -> Option<u32> {
        if fi.has_buffer == 0 {
            return None;
        }
        self.buffers.get(fi.buffer_id as usize).copied()
    }
}

fn read_cstring(string_table: &[u8], offset: u32) -> CString {
    let offset = offset as usize;
    if offset >= string_table.len() {
        return CString::default();
    }
    match CStr::from_bytes_until_nul(&string_table[offset..]) {
        Ok(cstr) => cstr.to_owned(),
        Err(_) => CString::default(),
    }
}


// -----------------------------------------------------------------------------
// Merge
// -----------------------------------------------------------------------------

impl Metadata {
    /// Union of records across multiple metadatas, with first-wins priority on
    /// name conflicts.
    ///
    /// Priority is `sources[0]` (highest) through `sources[N-1]` (lowest).
    /// - Bundles are deduplicated by their string-table name.
    /// - Files are deduplicated by their full path.
    /// - When a file is retained from source *S*, its entry chain from *S* is
    ///   preserved; each entry's `bundle_id` is translated to the output's
    ///   bundle numbering.
    /// - The output's `string_table`, directory tree, `buffers`, and all
    ///   index-based cross-references are rebuilt from scratch.
    ///
    /// # Semantics
    /// This is a *record-level* union: the returned metadata is a valid
    /// `metadata.store` that describes the union of files across sources, but
    /// each referenced bundle still needs to be shipped alongside for the
    /// entries' `offset_in_bundle` / `size_in_bundle` to point at real data.
    /// If two sources ship distinct bundles under the same filename, only the
    /// first-source's bundle is retained and later-source entries mapped to
    /// that same name may resolve to wrong data — keep bundle names unique.
    pub fn merge(sources: &[&Metadata]) -> Metadata {
        if sources.is_empty() {
            return Metadata::new();
        }

        // ---- 1. Collect unique bundles (first-wins by name). ----
        // Also build per-source translation: source_bundle_id -> output_bundle_idx (0-based),
        // or `None` for the null placeholder / never-referenced entries.
        let mut bundles: Vec<PendingBundle> = Vec::new();
        let mut bundle_index: HashMap<CString, usize> = HashMap::new();
        let mut per_source_bundle_map: Vec<Vec<Option<usize>>> = Vec::with_capacity(sources.len());

        for src in sources {
            let mut map = vec![None; src.bundle_infos.len()];
            for (sbid, bi) in src.bundle_infos.iter().enumerate().skip(1) {
                let name = read_cstring(&src.string_table, bi.string_table_name_offset);
                let out_idx = if let Some(&existing) = bundle_index.get(&name) {
                    existing
                } else {
                    let new_idx = bundles.len();
                    bundle_index.insert(name.clone(), new_idx);
                    bundles.push(PendingBundle {
                        name,
                        data_block_offset: bi.data_block_offset,
                        data_block_size: bi.data_block_size,
                        burst_data_block_size: bi.burst_data_block_size,
                    });
                    new_idx
                };
                map[sbid] = Some(out_idx);
            }
            per_source_bundle_map.push(map);
        }

        // ---- 2. Collect unique files (first-wins by path). ----
        let mut files: Vec<PendingFile> = Vec::new();
        let mut file_seen: HashSet<CString> = HashSet::new();

        for (src_idx, src) in sources.iter().enumerate() {
            let hash_by_file = src.hash_lookup();

            for (sfid, (name, fi)) in src.file_infos.iter().enumerate().skip(1) {
                if !file_seen.insert(name.clone()) {
                    continue; // higher-priority source already claimed this path
                }

                // Walk this file's entry chain and translate bundle indices.
                let mut entries = Vec::new();
                let mut eid = fi.first_entry as usize;
                let mut safety = src.entry_infos.len() + 1;
                while eid != 0 && eid < src.entry_infos.len() && safety > 0 {
                    let e = &src.entry_infos[eid];
                    if let Some(bundle_index) = per_source_bundle_map[src_idx]
                        .get(e.bundle_id as usize)
                        .copied()
                        .flatten()
                    {
                        entries.push(PendingEntry {
                            bundle_index,
                            offset_in_bundle: e.offset_in_bundle,
                            size_in_bundle: e.size_in_bundle,
                        });
                    }
                    eid = e.next_entry as usize;
                    safety -= 1;
                }

                files.push(PendingFile {
                    path: name.clone(),
                    path_hash: fi.path_hash,
                    size_in_bundle: fi.size_in_bundle,
                    size_in_memory: fi.size_in_memory,
                    compression_type: fi.compression_type,
                    buffer_size: src.buffer_size_of(fi),
                    hash: hash_by_file.get(&(sfid as i64)).copied(),
                    entries,
                });
            }
        }

        build_metadata(files, bundles)
    }
}
