//! End-to-end mod merging: combines mod file sets into outputs with all
//! cross-references (offsets, indices, string table) recomputed to be
//! consistent.
//!
//! [`Bundle::merge`] and [`Metadata::merge`] individually produce record-level
//! unions, but the metadata's `offset_in_bundle` values are inherited from the
//! source bundles and go stale as soon as the merged bundle is serialized to a
//! new page layout. [`merge_mods`] closes that gap by re-computing every
//! item's page-aligned offset and emitting fresh outputs.

use std::ffi::CStr;

use crate::build::{PendingBundle, PendingEntry, PendingFile, build_metadata};
use crate::bundle::Bundle;
use crate::metadata::Metadata;
use crate::texture_cache::TextureCache;

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
    let texture_sources: Vec<&TextureCache> = mods.iter().filter_map(|m| m.texture_cache).collect();
    let texture_cache = (!texture_sources.is_empty()).then(|| TextureCache::merge(&texture_sources));

    // 1. Merge bundles and compute final page-aligned offsets.
    let bundles: Vec<&Bundle<'data>> = mods.iter().map(|m| m.bundle).collect();
    let merged_bundle = Bundle::merge(&bundles);

    if merged_bundle.items().is_empty() {
        return MergedMod {
            bundle: merged_bundle,
            metadata: Metadata::new(),
            texture_cache,
        };
    }

    let offsets = merged_bundle.compute_offsets();

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

        let source = mods.iter().enumerate().find_map(|(src_idx, input)| {
            input
                .metadata
                .find_file_id_by_name(name_bytes)
                .map(|fid| (src_idx, input.metadata, fid))
        });

        let (path_hash, size_in_memory, compression_type, buffer_size, hash) =
            if let Some((src_idx, meta, fid)) = source {
                let fi = meta.file_infos[fid].1;
                (
                    fi.path_hash,
                    fi.size_in_memory,
                    fi.compression_type,
                    meta.buffer_size_of(&fi),
                    source_hash_lookups[src_idx].get(&(fid as i64)).copied(),
                )
            } else {
                // Orphan: no source metadata knows this file. Fill with what the
                // bundle can tell us; keep hash/buffer absent.
                (0, item.size(), item.compression() as u32, None, None)
            };

        files.push(PendingFile {
            path: name,
            path_hash,
            size_in_bundle: item.zsize(),
            size_in_memory,
            compression_type,
            buffer_size,
            hash,
            entries: vec![PendingEntry {
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
        data_block_size:       merged_bundle.data_block_size(),
        burst_data_block_size: 0,
    }];

    // 5. Emit the metadata.
    let metadata = build_metadata(files, bundles_out);

    MergedMod {
        bundle: merged_bundle,
        metadata,
        texture_cache,
    }
}
