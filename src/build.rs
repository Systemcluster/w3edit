//! Shared building blocks for constructing a [`Metadata`] from normalized
//! records.
//!
//! Both [`Metadata::merge`] and [`crate::merge::merge_mods`] have the same
//! "emit" step — build a string table, walk paths to construct a directory
//! tree, group entries by bundle, link file entry chains, and emit the seven
//! record arrays with the correct index-based cross-references. The types in
//! this module capture that pipeline once so both callers only need to
//! normalize their input into [`PendingFile`] / [`PendingBundle`] values.

use std::collections::HashMap;
use std::ffi::CString;

use crate::metadata::{
    CURRENT_METADATA_VERSION, Metadata, MetadataBundleInfo, MetadataDirectoryInitInfo, MetadataFileEntryInfo,
    MetadataFileInfo, MetadataFileInitInfo, MetadataHash,
};


// -----------------------------------------------------------------------------
// StringTable
// -----------------------------------------------------------------------------

/// Deduplicating builder for a metadata string table.
///
/// The table always starts with a single NUL byte at offset 0 (matching the
/// convention observed in all shipped fixtures — a sentinel meaning "empty
/// string"). Every subsequent string is appended with a trailing NUL and its
/// starting byte offset is returned.
pub(crate) struct StringTable {
    bytes:   Vec<u8>,
    offsets: HashMap<Vec<u8>, u32>,
}

impl StringTable {
    pub fn new() -> Self {
        let mut offsets = HashMap::new();
        offsets.insert(Vec::new(), 0);
        Self {
            bytes: vec![0u8],
            offsets,
        }
    }

    /// Intern `bytes` and return its byte offset in the table. If the same
    /// byte sequence has already been interned, its earlier offset is reused.
    pub fn intern(&mut self, bytes: &[u8]) -> u32 {
        if let Some(&off) = self.offsets.get(bytes) {
            return off;
        }
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(bytes);
        self.bytes.push(0);
        self.offsets.insert(bytes.to_vec(), off);
        off
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}


// -----------------------------------------------------------------------------
// DirTreeBuilder
// -----------------------------------------------------------------------------

/// Builder for the `dir_init_infos` tree.
///
/// The tree always starts with a root at index 0 (empty name, `parent = 0`).
/// [`add_path`](Self::add_path) walks a `\`- or `/`-separated path from the
/// root, creating intermediate directory nodes as needed and interning their
/// component names into the shared [`StringTable`].
pub(crate) struct DirTreeBuilder {
    dirs:     Vec<MetadataDirectoryInitInfo>,
    /// `children[parent_id][component_bytes] = child_dir_id`
    children: HashMap<u32, HashMap<Vec<u8>, u32>>,
}

impl DirTreeBuilder {
    pub fn new() -> Self {
        Self {
            dirs:     vec![MetadataDirectoryInitInfo {
                string_table_name_offset: 0,
                parent:                   0,
            }],
            children: HashMap::new(),
        }
    }

    /// Split `path` by `\` or `/`, walk from the root creating directory nodes
    /// as needed, and return `(leaf_dir_id, leaf_name_offset)` — the id of the
    /// deepest directory (the file's parent) and the offset in `strings` for
    /// the leaf filename.
    pub fn add_path(&mut self, path: &[u8], strings: &mut StringTable) -> (u32, u32) {
        let components: Vec<&[u8]> = path.split(|&b| b == b'\\' || b == b'/').collect();
        // `split` always yields at least one element.
        let (leaf, dir_components) = components.split_last().unwrap();

        let mut cur: u32 = 0;
        for comp in dir_components {
            if comp.is_empty() {
                continue; // skip leading / duplicated separators
            }
            let entry = self.children.entry(cur).or_default();
            if let Some(&existing) = entry.get(*comp) {
                cur = existing;
                continue;
            }
            let name_off = strings.intern(comp);
            let new_id = self.dirs.len() as u32;
            self.dirs.push(MetadataDirectoryInitInfo {
                string_table_name_offset: name_off as i32,
                parent:                   cur as i32,
            });
            entry.insert(comp.to_vec(), new_id);
            cur = new_id;
        }

        let leaf_off = strings.intern(leaf);
        (cur, leaf_off)
    }

    pub fn into_records(self) -> Vec<MetadataDirectoryInitInfo> {
        self.dirs
    }
}


// -----------------------------------------------------------------------------
// Normalized input to build_metadata
// -----------------------------------------------------------------------------

/// One retained file with all fields needed by [`build_metadata`].
///
/// The `bundle_index` inside each [`PendingEntry`] is a 0-based index into
/// the `bundles` slice passed to `build_metadata`.
pub(crate) struct PendingFile {
    pub path:             CString,
    pub path_hash:        u32,
    pub size_in_bundle:   u32,
    pub size_in_memory:   u32,
    pub compression_type: u32,
    pub buffer_size:      Option<u32>,
    pub hash:             Option<i64>,
    /// Zero or more entries (one per bundle this file appears in).
    pub entries:          Vec<PendingEntry>,
}

pub(crate) struct PendingEntry {
    /// 0-based index into `bundles` passed to `build_metadata`.
    pub bundle_index:     usize,
    pub offset_in_bundle: u32,
    pub size_in_bundle:   u32,
}

pub(crate) struct PendingBundle {
    pub name:                  CString,
    pub data_block_offset:     u32,
    pub data_block_size:       u32,
    pub burst_data_block_size: u32,
}


// -----------------------------------------------------------------------------
// build_metadata
// -----------------------------------------------------------------------------

/// Emit a fully cross-referenced [`Metadata`] from a normalized set of files
/// and bundles.
///
/// The emit pipeline is:
/// 1. Rebuild the string table (paths, bundle names, dir components, leaves).
/// 2. Build the directory tree from all retained paths.
/// 3. Emit `entry_infos` grouped by bundle, then link each file's `next_entry`
///    chain across groups.
/// 4. Emit `file_infos` + `buffers` with re-indexed `buffer_id`.
/// 5. Emit `bundle_infos` with computed `first_file_entry`/`num_bundle_entries`.
/// 6. Emit `file_init_infos` + `hashes`.
///
/// Output `file_infos` and `entry_infos` and `bundle_infos` all begin with the
/// standard index-0 null placeholder; `dir_init_infos[0]` is the root.
pub(crate) fn build_metadata(files: Vec<PendingFile>, bundles: Vec<PendingBundle>) -> Metadata {
    let mut strings = StringTable::new();
    let mut dir_tree = DirTreeBuilder::new();

    // 1. Intern all full paths first (matches natural fixture layout: paths
    //    appear before dir-component names in the string table).
    let file_path_offsets: Vec<u32> = files.iter().map(|f| strings.intern(f.path.as_bytes())).collect();

    // 2. Intern bundle names.
    let bundle_name_offsets: Vec<u32> = bundles.iter().map(|b| strings.intern(b.name.as_bytes())).collect();

    // 3. Build dir tree per file (interns dir components and leaf names).
    let mut file_dir_ids = Vec::with_capacity(files.len());
    let mut file_leaf_offsets = Vec::with_capacity(files.len());
    for f in &files {
        let (dir_id, leaf_off) = dir_tree.add_path(f.path.as_bytes(), &mut strings);
        file_dir_ids.push(dir_id);
        file_leaf_offsets.push(leaf_off);
    }

    // 4. Emit entry_infos, grouped by bundle. Track (a) the range of entries
    //    per bundle for bundle_infos, and (b) each file's new entry indices
    //    so we can link the file's next_entry chain across bundles afterwards.
    let mut entry_infos: Vec<MetadataFileEntryInfo> = vec![MetadataFileEntryInfo::default()];
    let mut per_bundle_first = vec![0u32; bundles.len()];
    let mut per_bundle_count = vec![0u32; bundles.len()];
    let mut per_file_entries: Vec<Vec<u32>> = vec![Vec::new(); files.len()];

    for out_bundle_idx in 0..bundles.len() {
        let first_id = entry_infos.len() as u32;
        let mut count: u32 = 0;
        for (file_idx, f) in files.iter().enumerate() {
            for e in &f.entries {
                if e.bundle_index != out_bundle_idx {
                    continue;
                }
                let new_eid = entry_infos.len() as u32;
                entry_infos.push(MetadataFileEntryInfo {
                    file_id:          (file_idx as u32) + 1,
                    bundle_id:        (out_bundle_idx as u32) + 1,
                    offset_in_bundle: e.offset_in_bundle,
                    size_in_bundle:   e.size_in_bundle,
                    next_entry:       0, // patched below
                });
                per_file_entries[file_idx].push(new_eid);
                count += 1;
            }
        }
        per_bundle_first[out_bundle_idx] = if count > 0 { first_id } else { 0 };
        per_bundle_count[out_bundle_idx] = count;
    }

    // Patch next_entry within each file's chain.
    for chain in &per_file_entries {
        for window in chain.windows(2) {
            entry_infos[window[0] as usize].next_entry = window[1];
        }
    }

    // 5. Emit file_infos (with buffer_id dedup + remap) + buffers array.
    //    Multiple files may share the same buffer size; the shipped fixtures
    //    deduplicate these so several file_infos can point at the same
    //    `buffer_id`. We match that shape.
    let mut buffers: Vec<u32> = Vec::new();
    let mut buffer_index: HashMap<u32, u32> = HashMap::new();
    let mut file_infos: Vec<(CString, MetadataFileInfo)> = Vec::with_capacity(files.len() + 1);
    file_infos.push((CString::default(), MetadataFileInfo::default()));
    for (idx, f) in files.iter().enumerate() {
        let (buffer_id, has_buffer) = if let Some(sz) = f.buffer_size {
            let bid = *buffer_index.entry(sz).or_insert_with(|| {
                let new_id = buffers.len() as u32;
                buffers.push(sz);
                new_id
            });
            (bid, 1)
        } else {
            (0, 0)
        };
        let first_entry = per_file_entries[idx].first().copied().unwrap_or(0);
        file_infos.push((f.path.clone(), MetadataFileInfo {
            string_table_name_offset: file_path_offsets[idx],
            path_hash: f.path_hash,
            size_in_bundle: f.size_in_bundle,
            size_in_memory: f.size_in_memory,
            first_entry,
            compression_type: f.compression_type,
            buffer_id,
            has_buffer,
        }));
    }

    // 6. Emit bundle_infos.
    let mut bundle_infos: Vec<MetadataBundleInfo> = Vec::with_capacity(bundles.len() + 1);
    bundle_infos.push(MetadataBundleInfo::default());
    for (idx, b) in bundles.iter().enumerate() {
        bundle_infos.push(MetadataBundleInfo {
            string_table_name_offset: bundle_name_offsets[idx],
            first_file_entry:         per_bundle_first[idx],
            num_bundle_entries:       per_bundle_count[idx],
            data_block_size:          b.data_block_size,
            data_block_offset:        b.data_block_offset,
            burst_data_block_size:    b.burst_data_block_size,
        });
    }

    // 7. file_init_infos + hashes.
    let mut file_init_infos: Vec<MetadataFileInitInfo> = Vec::with_capacity(files.len());
    let mut hashes: Vec<MetadataHash> = Vec::new();
    for (idx, f) in files.iter().enumerate() {
        let file_id_out = (idx as i32) + 1;
        file_init_infos.push(MetadataFileInitInfo {
            file_id:                  file_id_out,
            directory_id:             file_dir_ids[idx] as i32,
            string_table_name_offset: file_leaf_offsets[idx] as i32,
        });
        if let Some(h) = f.hash {
            hashes.push(MetadataHash {
                hash:    h,
                file_id: file_id_out as i64,
            });
        }
    }

    // 8. Scalars.
    let max_size_in_bundle = files.iter().map(|f| f.size_in_bundle).max().unwrap_or(0);
    let max_size_in_memory = files.iter().map(|f| f.size_in_memory).max().unwrap_or(0);

    Metadata {
        version: CURRENT_METADATA_VERSION,
        max_size_in_bundle,
        max_size_in_memory,
        string_table: strings.into_bytes(),
        file_infos,
        entry_infos,
        bundle_infos,
        buffers,
        dir_init_infos: dir_tree.into_records(),
        file_init_infos,
        hashes,
    }
}
