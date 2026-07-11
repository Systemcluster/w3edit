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

pub use bundle::{Bundle, BundleCompression, BundleItem};
pub use merge::{MergedMod, ModInput, merge_mods};
pub use metadata::Metadata;
pub use texture_cache::{TextureCache, TextureCacheEntry};
