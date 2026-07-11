//! `.bundle` container reader, writer, and merger for The Witcher 3.

use std::borrow::Cow;
use std::ffi::{CStr, CString};
use std::fmt::Debug;
use std::io::Read;

use log::debug;

use crate::errors::*;
use crate::util::*;


// -----------------------------------------------------------------------------
// Format constants
// -----------------------------------------------------------------------------

/// Magic bytes at the start of every bundle.
const MAGIC: &[u8; 8] = b"POTATO70";

/// Size of one TOC entry in bytes.
const TOC_ENTRY_SIZE: usize = 0x140;

/// Bundle header size (magic + 3×u32 + mystery bytes).
const HEADER_SIZE: usize = 0x20;

/// Page size used for data alignment. Item data pages start at multiples of this.
const PAGE: usize = 4096;

/// Bytes 20..32 of the bundle header. Undocumented in WolvenKit; preserved as-is.
const MYSTERY_BYTES: [u8; 12] = [0x03, 0x00, 0x01, 0x00, 0x00, 0x13, 0x13, 0x13, 0x13, 0x13, 0x13, 0x13];

// Field offsets within a 0x140-byte TOC entry.
const OFF_NAME: usize = 0x000;
const OFF_HASH: usize = 0x100;
const OFF_EMPTY: usize = 0x110;
const OFF_SIZE: usize = 0x114;
const OFF_ZSIZE: usize = 0x118;
const OFF_OFFSET: usize = 0x11C;
const OFF_DATE: usize = 0x120;
const OFF_TIME: usize = 0x124;
const OFF_ZERO: usize = 0x128;
const OFF_CRC: usize = 0x138;
const OFF_COMPRESSION: usize = 0x13C;


// -----------------------------------------------------------------------------
// Compression
// -----------------------------------------------------------------------------

/// Compression algorithm used for a bundle item's payload.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleCompression {
    None    = 0,
    Zlib    = 1,
    Snappy  = 2,
    Doboz   = 3,
    Lz4     = 4,
    Lz4hc   = 5,
    Unknown = 6,
}
impl From<u32> for BundleCompression {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Zlib,
            2 => Self::Snappy,
            3 => Self::Doboz,
            4 => Self::Lz4,
            5 => Self::Lz4hc,
            _ => Self::Unknown,
        }
    }
}


// -----------------------------------------------------------------------------
// BundleItem
// -----------------------------------------------------------------------------

/// A single file entry in a bundle.
///
/// `data` holds **exactly** the compressed bytes for this item (its length equals
/// [`zsize`](Self::zsize)). This allows items to be moved between bundles
/// (e.g. via [`Bundle::merge`]) without dragging along their source buffer.
#[derive(Clone)]
pub struct BundleItem<'a> {
    pub(crate) data:        Cow<'a, [u8]>,
    pub(crate) name:        CString,
    pub(crate) hash:        u128,
    pub(crate) empty:       u32,
    pub(crate) size:        u32,
    pub(crate) zsize:       u32,
    pub(crate) offset:      u32,
    pub(crate) date:        u32,
    pub(crate) time:        u32,
    pub(crate) zero:        u128,
    pub(crate) crc:         u32,
    pub(crate) compression: u32,
}

impl<'a> BundleItem<'a> {
    /// Parse a single 0x140-byte TOC entry, pairing it with the item's compressed data slice.
    pub(crate) fn from_toc(entry: &[u8], data: Cow<'a, [u8]>) -> Result<Self, BundleError> {
        if entry.len() < TOC_ENTRY_SIZE {
            return Err(BundleError::InvalidBytes(ReadError(format!(
                "toc entry too short: {} < {}",
                entry.len(),
                TOC_ENTRY_SIZE
            ))));
        }
        let name = CStr::from_bytes_until_nul(&entry[OFF_NAME..OFF_HASH])
            .map_err(|e| ReadError(e.to_string()))?
            .to_owned();
        let hash = read::<u128>(&entry[OFF_HASH..OFF_EMPTY])?;
        let empty = read::<u32>(&entry[OFF_EMPTY..OFF_SIZE])?;
        let size = read::<u32>(&entry[OFF_SIZE..OFF_ZSIZE])?;
        let zsize = read::<u32>(&entry[OFF_ZSIZE..OFF_OFFSET])?;
        let offset = read::<u32>(&entry[OFF_OFFSET..OFF_DATE])?;
        let date = read::<u32>(&entry[OFF_DATE..OFF_TIME])?;
        let time = read::<u32>(&entry[OFF_TIME..OFF_ZERO])?;
        let zero = read::<u128>(&entry[OFF_ZERO..OFF_CRC])?;
        let crc = read::<u32>(&entry[OFF_CRC..OFF_COMPRESSION])?;
        let compression = read::<u32>(&entry[OFF_COMPRESSION..TOC_ENTRY_SIZE])?;

        if data.len() != zsize as usize {
            return Err(BundleError::InvalidBytes(ReadError(format!(
                "item data length {} does not match zsize {}",
                data.len(),
                zsize
            ))));
        }

        Ok(BundleItem {
            data,
            name,
            hash,
            empty,
            size,
            zsize,
            offset,
            date,
            time,
            zero,
            crc,
            compression,
        })
    }

    /// The item's filename (null-terminated, encoded as ISO-8859-1 in the format).
    pub fn name(&self) -> &CStr {
        &self.name
    }

    /// 128-bit content hash from the TOC.
    pub fn hash(&self) -> u128 {
        self.hash
    }

    /// Uncompressed size in bytes.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Compressed size in bytes (equal to `raw().len()`).
    pub fn zsize(&self) -> u32 {
        self.zsize
    }

    /// Absolute offset of this item's data page within its containing bundle,
    /// as recorded in the TOC. For freshly-merged bundles (before [`Bundle::write`])
    /// this value is stale; [`Bundle::write`] recomputes it at serialization time.
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// CRC32 of the uncompressed data, as recorded in the TOC.
    pub fn crc(&self) -> u32 {
        self.crc
    }

    /// Compression algorithm used for this item.
    pub fn compression(&self) -> BundleCompression {
        self.compression.into()
    }

    /// Raw compressed bytes for this item, exactly as they appear in the bundle.
    pub fn raw(&self) -> &[u8] {
        &self.data
    }

    /// Serialize this item's TOC entry (0x140 bytes) with its currently stored `offset`.
    /// [`Bundle::write`] patches the offset field to the new data-page position.
    pub fn header(&self) -> Vec<u8> {
        let mut header = Vec::with_capacity(TOC_ENTRY_SIZE);
        header.extend(self.name.as_bytes());
        header.resize(OFF_HASH, 0);
        header.extend(self.hash.to_le_bytes());
        header.extend(self.empty.to_le_bytes());
        header.extend(self.size.to_le_bytes());
        header.extend(self.zsize.to_le_bytes());
        header.extend(self.offset.to_le_bytes());
        header.extend(self.date.to_le_bytes());
        header.extend(self.time.to_le_bytes());
        header.extend(self.zero.to_le_bytes());
        header.extend(self.crc.to_le_bytes());
        header.extend(self.compression.to_le_bytes());
        debug_assert_eq!(header.len(), TOC_ENTRY_SIZE);
        header
    }

    /// Decompress and return the item's contents.
    pub fn decompressed(&self) -> Result<Vec<u8>, BundleError> {
        let raw: &[u8] = &self.data;
        match self.compression() {
            BundleCompression::None => Ok(raw.to_vec()),
            BundleCompression::Zlib => {
                let mut decoder = flate2::read::ZlibDecoder::new(raw);
                let mut data = Vec::with_capacity(self.size as usize);
                decoder.read_to_end(&mut data).map_err(|e| ReadError(e.to_string()))?;
                Ok(data)
            },
            BundleCompression::Snappy => snap::raw::Decoder::new()
                .decompress_vec(raw)
                .map_err(|e| BundleError::from(ReadError(e.to_string()))),
            BundleCompression::Doboz => Err(BundleError::UnsupportedCompression("Doboz".to_string())),
            BundleCompression::Lz4 | BundleCompression::Lz4hc => lz4::block::decompress(raw, Some(self.size as _))
                .map_err(|e| BundleError::from(ReadError(e.to_string()))),
            BundleCompression::Unknown => Err(BundleError::UnsupportedCompression(format!("{}", self.compression))),
        }
    }
}

impl Debug for BundleItem<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleItem")
            .field("name", &self.name)
            .field("hash", &self.hash)
            .field("empty", &self.empty)
            .field("size", &self.size)
            .field("zsize", &self.zsize)
            .field("offset", &self.offset)
            .field("date", &self.date)
            .field("time", &self.time)
            .field("zero", &self.zero)
            .field("crc", &self.crc)
            .field("compression", &BundleCompression::from(self.compression))
            .finish()
    }
}


// -----------------------------------------------------------------------------
// Bundle
// -----------------------------------------------------------------------------

/// A parsed or constructed `.bundle` container.
#[derive(Clone)]
pub struct Bundle<'a> {
    pub(crate) bundle_size: u32,
    pub(crate) dummy_size:  u32,
    pub(crate) data_offset: u32,
    pub(crate) items:       Vec<BundleItem<'a>>,
}

impl Bundle<'static> {
    /// Construct an empty bundle. Use [`Bundle::merge`] to combine items from existing bundles.
    pub fn new() -> Self {
        Bundle {
            bundle_size: 0,
            dummy_size:  0,
            data_offset: 0,
            items:       Vec::new(),
        }
    }
}

impl Default for Bundle<'static> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Bundle<'a> {
    /// Parse a bundle from bytes.
    ///
    /// Accepts either a borrowed slice (`&'a [u8]`, zero-copy) or an owned buffer
    /// (`Vec<u8>`, in which case each item's compressed slice is copied out).
    pub fn parse<R: Into<Cow<'a, [u8]>>>(data: R) -> Result<Self, BundleError> {
        let data: Cow<'a, [u8]> = data.into();

        // Header: read fields via a temporary borrow that ends before we consume `data`.
        let (bundle_size, dummy_size, data_offset) = {
            let d: &[u8] = &data;
            if d.len() < HEADER_SIZE {
                return Err(BundleError::InvalidBytes(ReadError(format!(
                    "bundle too short: {} < {}",
                    d.len(),
                    HEADER_SIZE
                ))));
            }
            let magic = read::<[u8; 8]>(&d[0..8])?;
            if &magic != MAGIC {
                return Err(BundleError::InvalidBytes(ReadError(format!(
                    "no potato magic: {}",
                    String::from_utf8_lossy(&magic)
                ))));
            }
            (
                read::<u32>(&d[8..12])?,
                read::<u32>(&d[12..16])?,
                read::<u32>(&d[16..20])?,
            )
        };

        // TOC layout sanity: the TOC must fit in the file and its size must be a
        // whole multiple of a TOC entry.
        let toc_size = data_offset as usize;
        if !toc_size.is_multiple_of(TOC_ENTRY_SIZE) {
            return Err(BundleError::InvalidBytes(ReadError(format!(
                "toc size {toc_size} is not a multiple of {TOC_ENTRY_SIZE}"
            ))));
        }
        let toc_end = HEADER_SIZE
            .checked_add(toc_size)
            .ok_or_else(|| ReadError("toc size overflows usize".to_string()))?;
        if toc_end > data.len() {
            return Err(BundleError::InvalidBytes(ReadError(format!(
                "toc ends at {toc_end} but buffer is {} bytes",
                data.len()
            ))));
        }

        // Collect TOC entry positions.
        let toc_start = HEADER_SIZE;
        let mut toc_positions: Vec<usize> = Vec::with_capacity(toc_size / TOC_ENTRY_SIZE);
        let mut pos = toc_start;
        while pos < toc_end {
            toc_positions.push(pos);
            pos += TOC_ENTRY_SIZE;
        }

        // Extract items. Match on the Cow variant so borrowed input stays zero-copy
        // while owned input performs exactly one small copy per item (not a clone of
        // the entire buffer per item as the previous implementation did).
        let items: Vec<BundleItem<'a>> = match data {
            Cow::Borrowed(bytes) => toc_positions
                .into_iter()
                .map(|toc_pos| -> Result<BundleItem<'a>, BundleError> {
                    let entry = check_entry(bytes, toc_pos)?;
                    let (offset, zsize) = entry_data_range(entry)?;
                    let slice: &'a [u8] = check_range(bytes, offset, zsize)?;
                    BundleItem::from_toc(entry, Cow::Borrowed(slice))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Cow::Owned(vec) => toc_positions
                .into_iter()
                .map(|toc_pos| -> Result<BundleItem<'a>, BundleError> {
                    let entry = check_entry(&vec, toc_pos)?;
                    let (offset, zsize) = entry_data_range(entry)?;
                    let slice = check_range(&vec, offset, zsize)?;
                    BundleItem::from_toc(entry, Cow::Owned(slice.to_vec()))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        debug!("bundle: parsed {} items", items.len());

        Ok(Self {
            bundle_size,
            dummy_size,
            data_offset,
            items,
        })
    }

    /// The items in this bundle, in TOC order.
    pub fn items(&self) -> &[BundleItem<'a>] {
        &self.items
    }

    /// The value of the `dummy_size` header field (preserved as-is from the source).
    pub fn dummy_size(&self) -> u32 {
        self.dummy_size
    }

    /// Compute the absolute byte offset at which each item's data page will be
    /// placed when the bundle is serialized. Data pages start at the first
    /// [`PAGE`](self)-aligned boundary after the TOC and each item is padded
    /// up to the next page boundary.
    ///
    /// This is the same layout [`Bundle::write`] uses. It's public so that
    /// callers (for example, the `metadata.store` writer) can align metadata
    /// entries to the same byte positions without re-serializing the bundle.
    pub fn compute_offsets(&self) -> Vec<u32> {
        let toc_size = self.items.len() * TOC_ENTRY_SIZE;
        let toc_end = HEADER_SIZE + toc_size;
        let mut offsets = Vec::with_capacity(self.items.len());
        let mut pos = align_up(toc_end, PAGE);
        for item in &self.items {
            offsets.push(pos as u32);
            pos = align_up(pos + item.zsize as usize, PAGE);
        }
        offsets
    }

    /// Byte offset at which the bundle's data region starts — i.e. immediately
    /// after the header and TOC. Matches the `data_block_offset` field written
    /// into `metadata.store::bundle_infos`.
    pub fn data_block_offset(&self) -> u32 {
        (HEADER_SIZE + self.items.len() * TOC_ENTRY_SIZE) as u32
    }

    /// Size of the bundle's page-aligned data region (from
    /// [`data_block_offset`](Self::data_block_offset) to the end of the last
    /// item's page). Matches the `data_block_size` field written into
    /// `metadata.store::bundle_infos`.
    pub fn data_block_size(&self) -> u32 {
        if self.items.is_empty() {
            return 0;
        }
        let offsets = self.compute_offsets();
        let last = self.items.len() - 1;
        let end = offsets[last] as usize + self.items[last].zsize as usize;
        (align_up(end, PAGE) - self.data_block_offset() as usize) as u32
    }

    /// Serialize the bundle to a byte vector.
    ///
    /// Each item's compressed bytes are copied verbatim and placed at page-aligned offsets.
    /// The output is a valid bundle that can be written to disk and read back by [`Bundle::parse`].
    pub fn write(&self) -> Vec<u8> {
        let toc_size = self.items.len() * TOC_ENTRY_SIZE;
        let toc_end = HEADER_SIZE + toc_size;
        let item_offsets = self.compute_offsets();

        let total_size = if self.items.is_empty() {
            toc_end
        } else {
            let last = self.items.len() - 1;
            item_offsets[last] as usize + self.items[last].zsize as usize
        };

        // Phase 2: fill a zeroed output buffer (zero bytes serve as page padding).
        let mut out = vec![0u8; total_size];

        // Header (32 bytes).
        out[0..8].copy_from_slice(MAGIC);
        out[8..12].copy_from_slice(&(total_size as u32).to_le_bytes());
        out[12..16].copy_from_slice(&self.dummy_size.to_le_bytes());
        out[16..20].copy_from_slice(&(toc_size as u32).to_le_bytes());
        out[20..32].copy_from_slice(&MYSTERY_BYTES);

        // TOC: emit each item's header with its offset field patched to the new page position.
        for (i, item) in self.items.iter().enumerate() {
            let toc_pos = HEADER_SIZE + i * TOC_ENTRY_SIZE;
            let mut entry = item.header();
            entry[OFF_OFFSET..OFF_OFFSET + 4].copy_from_slice(&item_offsets[i].to_le_bytes());
            out[toc_pos..toc_pos + TOC_ENTRY_SIZE].copy_from_slice(&entry);
        }

        // Data: copy each item's compressed bytes into its slot. Gaps stay zero.
        for (i, item) in self.items.iter().enumerate() {
            let dst = item_offsets[i] as usize;
            let len = item.zsize as usize;
            out[dst..dst + len].copy_from_slice(&item.data[..len]);
        }

        out
    }

    /// Merge multiple bundles into one with order-based prioritization.
    ///
    /// Bundles are treated as a priority list: the bundle at index `0` has the
    /// **highest** priority. When the same filename appears in more than one
    /// input bundle, the item from the earliest bundle wins and later
    /// occurrences are discarded. The output preserves the order in which
    /// unique filenames were first encountered.
    ///
    /// No compressed data is decoded or recompressed — each retained item
    /// keeps its original compression and its reference (or copy) of the source
    /// bundle's bytes. Call [`Bundle::write`] on the result to obtain a
    /// serialized bundle.
    ///
    /// The `dummy_size` field is inherited from the first (highest-priority)
    /// bundle; when the input is empty, an empty bundle is returned.
    pub fn merge(bundles: &[&Bundle<'a>]) -> Bundle<'a> {
        use std::collections::HashSet;

        let mut items: Vec<BundleItem<'a>> = Vec::new();
        let mut seen: HashSet<CString> = HashSet::new();
        for bundle in bundles {
            for item in &bundle.items {
                if seen.insert(item.name.clone()) {
                    items.push(item.clone());
                }
            }
        }

        let dummy_size = bundles.first().map(|b| b.dummy_size).unwrap_or(0);
        let data_offset = (items.len() * TOC_ENTRY_SIZE) as u32;

        Bundle {
            bundle_size: 0,
            dummy_size,
            data_offset,
            items,
        }
    }
}

impl Debug for Bundle<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bundle")
            .field("bundle_size", &self.bundle_size)
            .field("dummy_size", &self.dummy_size)
            .field("data_offset", &self.data_offset)
            .field("items", &self.items)
            .finish()
    }
}


// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

#[inline]
fn align_up(value: usize, align: usize) -> usize {
    value.div_ceil(align) * align
}

/// Bounds-checked slice of a single 0x140-byte TOC entry.
#[inline]
fn check_entry(bytes: &[u8], toc_pos: usize) -> Result<&[u8], BundleError> {
    if toc_pos + TOC_ENTRY_SIZE > bytes.len() {
        return Err(BundleError::InvalidBytes(ReadError(format!(
            "toc entry at {:#x} exceeds buffer len {}",
            toc_pos,
            bytes.len()
        ))));
    }
    Ok(&bytes[toc_pos..toc_pos + TOC_ENTRY_SIZE])
}

/// Extract `(offset, zsize)` from a TOC entry.
#[inline]
fn entry_data_range(entry: &[u8]) -> Result<(usize, usize), BundleError> {
    let zsize = read::<u32>(&entry[OFF_ZSIZE..OFF_OFFSET])? as usize;
    let offset = read::<u32>(&entry[OFF_OFFSET..OFF_DATE])? as usize;
    Ok((offset, zsize))
}

/// Bounds-checked slice covering an item's compressed payload.
#[inline]
fn check_range(bytes: &[u8], offset: usize, zsize: usize) -> Result<&[u8], BundleError> {
    if offset.saturating_add(zsize) > bytes.len() {
        return Err(BundleError::InvalidBytes(ReadError(format!(
            "item data at {:#x}..{:#x} exceeds buffer len {}",
            offset,
            offset + zsize,
            bytes.len()
        ))));
    }
    Ok(&bytes[offset..offset + zsize])
}
