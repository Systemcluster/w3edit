//! Utility for packing, unpacking and analyzing The Witcher 3 content bundles.
//!
//! Implements a subset of features from `wcc_lite` from the official The Witcher 3
//! mod tools. See [`bundle`] for the `.bundle` container reader/writer/merger
//! and [`metadata`] for the `metadata.store` reader.

pub mod bundle;
pub mod errors;
pub mod merge;
pub mod metadata;
pub mod texture_cache;

pub(crate) mod build;
pub(crate) mod hash;
pub(crate) mod util;

pub use bundle::{Bundle, BundleCompression, BundleFormat, BundleItem};
pub use errors::ConversionError;
pub use merge::{
    MergeError, MergeOptions, MergedMod, ModInput, merge_mods, merge_mods_for_format, merge_mods_with_options,
};
pub use metadata::Metadata;
pub use texture_cache::{TextureCache, TextureCacheEntry};

/// Upgrade or downgrade a container to the requested game format.
///
/// Detects bundles (versions 3/5), metadata (6/7), and texture caches (6/7)
/// by signature. Compressed payloads are preserved without decoding; cooked
/// assets are not converted. Fields absent from the target layout are lost.
/// Legacy output that exceeds its integer limits is rejected.
///
/// Bundle layouts are rebuilt. Regenerate companion metadata after converting
/// bundles, using [`Metadata::from_bundle_files`]. Converting metadata alone
/// preserves its recorded offsets and does not convert the referenced bundles.
///
/// ```
/// use w3edit::{Bundle, BundleFormat, convert};
///
/// let legacy = Bundle::new().try_write()?;
/// let remastered = convert(&legacy, BundleFormat::Remastered)?;
/// assert_eq!(Bundle::parse(remastered)?.version(), 5);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn convert(data: &[u8], format: BundleFormat) -> Result<Vec<u8>, ConversionError> {
    if data.starts_with(b"POTATO70") {
        let mut bundle = Bundle::parse(data)?;
        bundle.format = format;
        return Ok(bundle.try_write()?);
    }
    let version = if format == BundleFormat::Legacy { 6 } else { 7 };
    if data.starts_with(b"\x03VTM") {
        let mut metadata = Metadata::parse(data)?;
        metadata.version = version;
        return Ok(metadata.try_write()?);
    }
    if data.len() >= 8 && &data[data.len() - 8..data.len() - 4] == b"HCXT" {
        let mut cache = TextureCache::parse(data)?;
        if !matches!(cache.version, 6 | 7) {
            return Err(errors::TextureCacheError::InvalidBytes(errors::ReadError(format!(
                "unsupported texture cache version {}",
                cache.version
            )))
            .into());
        }
        if cache.version != version {
            for (_, entry) in &mut cache.entries {
                entry.num_mip_offsets = entry.mip_offset_count() as i32 | if version == 7 { 1 << 16 } else { 0 };
            }
            cache.version = version;
        }
        return Ok(cache.write());
    }
    Err(ConversionError::UnknownContainer)
}
