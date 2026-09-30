//! End-to-end mod merging: combines mod file sets into outputs with all
//! cross-references (offsets, indices, string table) recomputed to be
//! consistent.
//!
//! [`Bundle::merge`] and [`Metadata::merge`] individually produce record-level
//! unions, but the metadata's `offset_in_bundle` values are inherited from the
//! source bundles and go stale as soon as the merged bundle is serialized to a
//! new page layout. [`merge_mods`] closes that gap by re-computing every
//! item's page-aligned offset and emitting fresh outputs.

use std::collections::HashMap;
use std::ffi::CStr;

use crate::build::{PendingBundle, PendingEntry, PendingFile, build_metadata};
use crate::bundle::{Bundle, BundleFormat, BundleItem};
use crate::errors::MetadataError;
use crate::metadata::Metadata;
use crate::texture_cache::TextureCache;

#[derive(Clone, Copy)]
struct FileFields {
    path_hash:        u32,
    size_in_memory:   u32,
    compression_type: u32,
    buffer_size:      Option<u32>,
    hash:             Option<i64>,
}

impl FileFields {
    fn from_bundle_item(item: &BundleItem<'_>) -> Self {
        Self {
            path_hash:        0,
            size_in_memory:   item.size(),
            compression_type: item.compression() as u32,
            buffer_size:      None,
            hash:             None,
        }
    }
}

/// Input files for one mod in an end-to-end merge.
#[derive(Clone, Copy)]
pub struct ModInput<'bundle, 'data> {
    pub bundle:        &'bundle Bundle<'data>,
    pub metadata:      &'bundle Metadata,
    pub texture_cache: Option<&'bundle TextureCache>,
}

impl<'bundle, 'data> ModInput<'bundle, 'data> {
    pub fn new(bundle: &'bundle Bundle<'data>, metadata: &'bundle Metadata) -> Self {
        Self {
            bundle,
            metadata,
            texture_cache: None,
        }
    }

    pub fn with_texture_cache(mut self, texture_cache: &'bundle TextureCache) -> Self {
        self.texture_cache = Some(texture_cache);
        self
    }
}

/// Output files from an end-to-end mod merge.
pub struct MergedMod<'data> {
    pub bundle:        Bundle<'data>,
    pub metadata:      Metadata,
    pub texture_cache: Option<TextureCache>,
}

/// Merge N mods into a single output set, including `texture.cache` when any
/// source supplies one.
///
/// Priority is insertion order — `mods[0]` is the highest-priority mod.
/// Behavior:
///
/// - Bundles are merged via [`Bundle::merge`] (first-wins by filename).
/// - The resulting bundle is used to compute the final page-aligned byte
///   offset for every retained item.
/// - The output metadata is built from scratch to describe **only** the
///   merged bundle: exactly one
///   [`MetadataBundleInfo`](crate::metadata::MetadataBundleInfo) with the
///   given `output_bundle_name`, one
///   [`MetadataFileEntryInfo`](crate::metadata::MetadataFileEntryInfo) per
///   retained item pointing at its fresh offset, and rebuilt string table +
///   directory tree.
/// - For every item, per-file fields (`path_hash`, `size_in_memory`,
///   `compression_type`, buffer, hash) are inherited from the first source
///   metadata that has that file. Items without a matching metadata record
///   ("orphan" items) fall back to the fields derivable from the bundle TOC.
/// - Texture caches are merged independently with the same first-wins mod
///   priority; missing caches are skipped, and the output cache is `None` when
///   no source supplies one.
///
/// The returned bundle is exactly what [`Bundle::merge`] produced (still
/// requires [`Bundle::write`](crate::bundle::Bundle::write) to serialize).
/// The returned metadata is self-consistent and can be written immediately.
pub fn merge_mods<'data>(mods: &[ModInput<'_, 'data>], output_bundle_name: &CStr) -> MergedMod<'data> {
    merge_mods_inner(mods, output_bundle_name, None)
}

/// Merge into an explicit target format, rejecting legacy metadata overflow.
/// Compressed bundle and texture payloads are retained without transcoding.
pub fn merge_mods_for_format<'data>(
    mods: &[ModInput<'_, 'data>],
    output_bundle_name: &CStr,
    format: BundleFormat,
) -> Result<MergedMod<'data>, MetadataError> {
    let merged = merge_mods_inner(mods, output_bundle_name, Some(format));
    merged.metadata.try_write()?;
    Ok(merged)
}

fn merge_mods_inner<'data>(
    mods: &[ModInput<'_, 'data>],
    output_bundle_name: &CStr,
    format: Option<BundleFormat>,
) -> MergedMod<'data> {
    let texture_sources: Vec<&TextureCache> = mods.iter().filter_map(|m| m.texture_cache).collect();
    let mut texture_cache = (!texture_sources.is_empty()).then(|| TextureCache::merge(&texture_sources));
    if let (Some(format), Some(cache)) = (format, &mut texture_cache) {
        cache.version = if format == BundleFormat::Legacy { 6 } else { 7 };
        for (_, entry) in &mut cache.entries {
            entry.num_mip_offsets =
                entry.mip_offset_count() as i32 | if format == BundleFormat::Remastered { 1 << 16 } else { 0 };
        }
    }

    // 1. Merge bundles and compute final page-aligned offsets.
    let bundles: Vec<&Bundle<'data>> = mods.iter().map(|m| m.bundle).collect();
    let mut merged_bundle = Bundle::merge(&bundles);
    if let Some(format) = format {
        merged_bundle = Bundle::from_items(format, merged_bundle.items().to_vec());
    }

    if merged_bundle.items().is_empty() {
        let mut metadata = Metadata::new();
        metadata.version = if merged_bundle.version() == 5 { 7 } else { 6 };
        return MergedMod {
            bundle: merged_bundle,
            metadata,
            texture_cache,
        };
    }

    let offsets = merged_bundle.compute_offsets_u64();

    // 2. Precompute a `path -> hash` lookup for each source (one HashMap per
    //    mod). Later we'll look up each merged item's fields in these.
    let source_hash_lookups: Vec<_> = mods.iter().map(|m| m.metadata.hash_lookup()).collect();

    // 3. Normalize every merged bundle item into a PendingFile, inheriting
    //    fields from the highest-priority source metadata that carries that
    //    path.
    let mut files: Vec<PendingFile> = Vec::with_capacity(merged_bundle.items().len());
    for (i, item) in merged_bundle.items().iter().enumerate() {
        let name = item.name().to_owned();
        let name_bytes = name.as_bytes();
        let fields = metadata_fields_for(mods, &source_hash_lookups, name_bytes)
            .unwrap_or_else(|| FileFields::from_bundle_item(item));

        files.push(PendingFile {
            path:             name,
            path_hash:        fields.path_hash,
            size_in_bundle:   item.zsize(),
            size_in_memory:   fields.size_in_memory,
            compression_type: fields.compression_type,
            buffer_size:      fields.buffer_size,
            hash:             fields.hash,
            entries:          vec![PendingEntry {
                bundle_index:     0,
                offset_in_bundle: offsets[i],
                size_in_bundle:   item.zsize(),
            }],
        });
    }

    // 4. Single output bundle, inheriting the merged bundle's data-block layout.
    let bundles_out = vec![PendingBundle {
        name:                  output_bundle_name.to_owned(),
        data_block_offset:     merged_bundle.data_block_offset(),
        data_block_size:       merged_bundle.data_block_size_u64(),
        burst_data_block_size: 0,
    }];

    // 5. Emit the metadata.
    let mut metadata = build_metadata(files, bundles_out);
    metadata.version = if merged_bundle.version() == 5 { 7 } else { 6 };

    MergedMod {
        bundle: merged_bundle,
        metadata,
        texture_cache,
    }
}

fn metadata_fields_for(
    mods: &[ModInput<'_, '_>],
    hash_lookups: &[HashMap<i64, i64>],
    name: &[u8],
) -> Option<FileFields> {
    mods.iter().enumerate().find_map(|(source_index, input)| {
        let file_id = input.metadata.find_file_id_by_name(name)?;
        let info = input.metadata.file_infos[file_id].1;
        Some(FileFields {
            path_hash:        info.path_hash,
            size_in_memory:   info.size_in_memory,
            compression_type: info.compression_type,
            buffer_size:      input.metadata.buffer_size_of(&info),
            hash:             hash_lookups[source_index].get(&(file_id as i64)).copied(),
        })
    })
}
