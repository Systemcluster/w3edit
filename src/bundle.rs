//! `.bundle` container reader, writer, and merger for The Witcher 3.

use std::borrow::Cow;
use std::ffi::{CStr, CString};
use std::fmt::Debug;
use std::io::{Read, Write};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum BundleFormat {
    Legacy,
    Remastered,
}

impl BundleFormat {
    fn entry_size(self) -> usize {
        match self {
            Self::Legacy => TOC_ENTRY_SIZE,
            Self::Remastered => 0x130,
        }
    }
}

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
    pub(crate) offset:      u64,
    pub(crate) date:        u32,
    pub(crate) time:        u32,
    pub(crate) zero:        u128,
    pub(crate) crc:         u32,
    pub(crate) compression: u32,
    pub(crate) format:      BundleFormat,
    pub(crate) reserved:    [u8; 8],
}

impl<'a> BundleItem<'a> {
    /// Compress a file for a new bundle. Names must fit the 256-byte TOC field.
    pub fn new(name: CString, data: &[u8], compression: BundleCompression) -> Result<Self, BundleError> {
        if name.as_bytes().is_empty() || name.as_bytes().len() >= OFF_HASH {
            return Err(ReadError("bundle names must contain 1..255 bytes".into()).into());
        }
        let size = u32::try_from(data.len()).map_err(|_| ReadError("file exceeds u32".into()))?;
        let compressed = match compression {
            BundleCompression::None => data.to_vec(),
            BundleCompression::Zlib => {
                let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(data).map_err(|error| ReadError(error.to_string()))?;
                encoder.finish().map_err(|error| ReadError(error.to_string()))?
            },
            BundleCompression::Snappy => snap::raw::Encoder::new()
                .compress_vec(data)
                .map_err(|error| ReadError(error.to_string()))?,
            BundleCompression::Lz4 | BundleCompression::Lz4hc => {
                let mode = if compression == BundleCompression::Lz4hc {
                    lz4::block::CompressionMode::HIGHCOMPRESSION(9)
                } else {
                    lz4::block::CompressionMode::DEFAULT
                };
                lz4::block::compress(data, Some(mode), false).map_err(|error| ReadError(error.to_string()))?
            },
            unsupported => return Err(BundleError::UnsupportedCompression(format!("{unsupported:?}"))),
        };
        let zsize = u32::try_from(compressed.len()).map_err(|_| ReadError("compressed file exceeds u32".into()))?;
        let mut crc = flate2::Crc::new();
        crc.update(data);
        Ok(Self {
            data: Cow::Owned(compressed),
            name,
            hash: 0,
            empty: 0,
            size,
            zsize,
            offset: 0,
            date: 0,
            time: 0,
            zero: 0,
            crc: crc.sum(),
            compression: compression as u32,
            format: BundleFormat::Legacy,
            reserved: [0; 8],
        })
    }

    fn from_toc(entry: &[u8], data: Cow<'a, [u8]>, format: BundleFormat) -> Result<Self, BundleError> {
        let entry_size = format.entry_size();
        if entry.len() < entry_size {
            return Err(BundleError::InvalidBytes(ReadError(format!(
                "toc entry too short: {} < {}",
                entry.len(),
                entry_size
            ))));
        }
        let name = CStr::from_bytes_until_nul(&entry[OFF_NAME..OFF_HASH])
            .map_err(|e| ReadError(e.to_string()))?
            .to_owned();
        let hash = read::<u128>(&entry[OFF_HASH..OFF_EMPTY])?;
        if format == BundleFormat::Remastered {
            let zsize = read::<u32>(&entry[0x11c..0x120])?;
            if data.len() != zsize as usize {
                return Err(ReadError("item data length does not match zsize".to_string()).into());
            }
            return Ok(Self {
                data,
                name,
                hash,
                empty: 0,
                size: read::<u32>(&entry[0x118..0x11c])?,
                zsize,
                offset: read::<u64>(&entry[0x110..0x118])?,
                date: 0,
                time: 0,
                zero: 0,
                crc: read::<u32>(&entry[0x120..0x124])?,
                compression: read::<u32>(&entry[0x124..0x128])?,
                format,
                reserved: read::<[u8; 8]>(&entry[0x128..0x130])?,
            });
        }
        let empty = read::<u32>(&entry[OFF_EMPTY..OFF_SIZE])?;
        let size = read::<u32>(&entry[OFF_SIZE..OFF_ZSIZE])?;
        let zsize = read::<u32>(&entry[OFF_ZSIZE..OFF_OFFSET])?;
        let offset = u64::from(read::<u32>(&entry[OFF_OFFSET..OFF_DATE])?);
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
            format,
            reserved: [0; 8],
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
    pub fn offset(&self) -> u64 {
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

    /// Serialize this item's TOC entry in its source format with its stored `offset`.
    /// [`Bundle::write`] patches the offset field to the new data-page position.
    pub fn header(&self) -> Vec<u8> {
        self.header_for_format(self.format, self.offset)
    }

    fn header_for_format(&self, format: BundleFormat, offset: u64) -> Vec<u8> {
        let mut header = Vec::with_capacity(format.entry_size());
        header.extend(self.name.as_bytes());
        header.resize(OFF_HASH, 0);
        header.extend(self.hash.to_le_bytes());
        if format == BundleFormat::Remastered {
            header.extend(offset.to_le_bytes());
            header.extend(self.size.to_le_bytes());
            header.extend(self.zsize.to_le_bytes());
            header.extend(self.crc.to_le_bytes());
            header.extend(self.compression.to_le_bytes());
            header.extend(self.reserved);
            return header;
        }
        header.extend(self.empty.to_le_bytes());
        header.extend(self.size.to_le_bytes());
        header.extend(self.zsize.to_le_bytes());
        header.extend(
            u32::try_from(offset)
                .expect("legacy bundle offset exceeds u32")
                .to_le_bytes(),
        );
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
    pub(crate) bundle_size:     u64,
    pub(crate) dummy_size:      u32,
    pub(crate) data_offset:     u32,
    pub(crate) items:           Vec<BundleItem<'a>>,
    pub(crate) format:          BundleFormat,
    pub(crate) header_reserved: [u8; 2],
}

impl Bundle<'static> {
    /// Construct an empty legacy bundle. Use [`Bundle::from_items`] for explicit-format creation.
    pub fn new() -> Self {
        Bundle {
            bundle_size:     0,
            dummy_size:      0,
            data_offset:     0,
            items:           Vec::new(),
            format:          BundleFormat::Legacy,
            header_reserved: [0; 2],
        }
    }
}

impl Default for Bundle<'static> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Bundle<'a> {
    /// Construct a bundle for an explicit game format from compressed entries.
    pub fn from_items(format: BundleFormat, mut items: Vec<BundleItem<'a>>) -> Self {
        for item in &mut items {
            item.format = format;
        }
        Self {
            bundle_size: 0,
            dummy_size: 0,
            data_offset: u32::try_from(items.len() * format.entry_size()).expect("bundle TOC exceeds u32"),
            items,
            format,
            header_reserved: [0; 2],
        }
    }

    /// Parse a bundle from bytes.
    ///
    /// Accepts either a borrowed slice (`&'a [u8]`, zero-copy) or an owned buffer
    /// (`Vec<u8>`, in which case each item's compressed slice is copied out).
    pub fn parse<R: Into<Cow<'a, [u8]>>>(data: R) -> Result<Self, BundleError> {
        let data: Cow<'a, [u8]> = data.into();

        // Header: read fields via a temporary borrow that ends before we consume `data`.
        let (bundle_size, dummy_size, data_offset, format, header_reserved) = {
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
            let format = match read::<u16>(&d[20..22])? {
                3 => BundleFormat::Legacy,
                5 => BundleFormat::Remastered,
                version => return Err(ReadError(format!("unsupported bundle version {version}")).into()),
            };
            let (bundle_size, dummy_size) = match format {
                BundleFormat::Legacy => (u64::from(read::<u32>(&d[8..12])?), read::<u32>(&d[12..16])?),
                BundleFormat::Remastered => (read::<u64>(&d[8..16])?, 0),
            };
            (
                bundle_size,
                dummy_size,
                read::<u32>(&d[16..20])?,
                format,
                read::<[u8; 2]>(&d[30..32])?,
            )
        };

        // TOC layout sanity: the TOC must fit in the file and its size must be a
        // whole multiple of a TOC entry.
        let toc_size = data_offset as usize;
        let entry_size = format.entry_size();
        if !toc_size.is_multiple_of(entry_size) {
            return Err(BundleError::InvalidBytes(ReadError(format!(
                "toc size {toc_size} is not a multiple of {entry_size}"
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
        let mut toc_positions: Vec<usize> = Vec::with_capacity(toc_size / entry_size);
        let mut pos = toc_start;
        while pos < toc_end {
            toc_positions.push(pos);
            pos += entry_size;
        }

        // Extract items. Match on the Cow variant so borrowed input stays zero-copy
        // while owned input performs exactly one small copy per item (not a clone of
        // the entire buffer per item as the previous implementation did).
        let items: Vec<BundleItem<'a>> = match data {
            Cow::Borrowed(bytes) => toc_positions
                .into_iter()
                .map(|toc_pos| -> Result<BundleItem<'a>, BundleError> {
                    let entry = check_entry(bytes, toc_pos, entry_size)?;
                    let (offset, zsize) = entry_data_range(entry, format)?;
                    let slice: &'a [u8] = check_range(bytes, offset, zsize)?;
                    BundleItem::from_toc(entry, Cow::Borrowed(slice), format)
                })
                .collect::<Result<Vec<_>, _>>()?,
            Cow::Owned(vec) => toc_positions
                .into_iter()
                .map(|toc_pos| -> Result<BundleItem<'a>, BundleError> {
                    let entry = check_entry(&vec, toc_pos, entry_size)?;
                    let (offset, zsize) = entry_data_range(entry, format)?;
                    let slice = check_range(&vec, offset, zsize)?;
                    BundleItem::from_toc(entry, Cow::Owned(slice.to_vec()), format)
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        debug!("bundle: parsed {} items", items.len());

        Ok(Self {
            bundle_size,
            dummy_size,
            data_offset,
            items,
            format,
            header_reserved,
        })
    }

    /// The items in this bundle, in TOC order.
    pub fn items(&self) -> &[BundleItem<'a>] {
        &self.items
    }

    /// On-disk bundle version: 3 (legacy) or 5 (Remastered).
    pub fn version(&self) -> u16 {
        match self.format {
            BundleFormat::Legacy => 3,
            BundleFormat::Remastered => 5,
        }
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
    /// Panics if an offset exceeds the legacy metadata format's 32-bit limit;
    /// use [`Self::compute_offsets_u64`] for large version-5 bundles.
    pub fn compute_offsets(&self) -> Vec<u32> {
        self.compute_offsets_u64()
            .into_iter()
            .map(|offset| u32::try_from(offset).expect("bundle offset exceeds legacy metadata's u32 limit"))
            .collect()
    }

    /// Compute serialization offsets without the legacy metadata format's 32-bit limit.
    pub fn compute_offsets_u64(&self) -> Vec<u64> {
        let toc_size = self.items.len() * self.format.entry_size();
        let toc_end = HEADER_SIZE + toc_size;
        let mut offsets = Vec::with_capacity(self.items.len());
        let mut pos = align_up(toc_end, PAGE);
        for item in &self.items {
            offsets.push(pos as u64);
            pos = align_up(pos + item.zsize as usize, PAGE);
        }
        offsets
    }

    /// Byte offset at which the bundle's data region starts — i.e. immediately
    /// after the header and TOC. Matches the `data_block_offset` field written
    /// into `metadata.store::bundle_infos`.
    pub fn data_block_offset(&self) -> u32 {
        u32::try_from(HEADER_SIZE + self.items.len() * self.format.entry_size()).expect("bundle TOC exceeds u32")
    }

    /// Size of the bundle's page-aligned data region (from
    /// [`data_block_offset`](Self::data_block_offset) to the end of the last
    /// item's page). Matches the `data_block_size` field written into
    /// `metadata.store::bundle_infos`.
    pub fn data_block_size(&self) -> u32 {
        u32::try_from(self.data_block_size_u64()).expect("bundle data size exceeds legacy metadata's u32 limit")
    }

    /// Size of the page-aligned data region without the legacy 32-bit limit.
    pub fn data_block_size_u64(&self) -> u64 {
        if self.items.is_empty() {
            return 0;
        }
        let offsets = self.compute_offsets_u64();
        let last = self.items.len() - 1;
        let end = offsets[last] as usize + self.items[last].zsize as usize;
        (align_up(end, PAGE) - self.data_block_offset() as usize) as u64
    }

    /// Serialize the bundle to a byte vector.
    ///
    /// Each item's compressed bytes are copied verbatim and placed at page-aligned offsets.
    /// The output is a valid bundle that can be written to disk and read back by [`Bundle::parse`].
    pub fn write(&self) -> Vec<u8> {
        self.try_write()
            .expect("bundle cannot be represented in its target format")
    }

    /// Serialize, returning an error if the legacy size limit is exceeded.
    pub fn try_write(&self) -> Result<Vec<u8>, BundleError> {
        let entry_size = self.format.entry_size();
        let toc_size = self.items.len() * entry_size;
        let toc_end = HEADER_SIZE + toc_size;
        let item_offsets = self.compute_offsets_u64();

        let total_size = if self.items.is_empty() {
            toc_end
        } else {
            let last = self.items.len() - 1;
            item_offsets[last] as usize + self.items[last].zsize as usize
        };

        if self.format == BundleFormat::Legacy && total_size > u32::MAX as usize {
            return Err(ReadError("legacy bundle size exceeds u32".into()).into());
        }

        // Phase 2: fill a zeroed output buffer (zero bytes serve as page padding).
        let mut out = vec![0u8; total_size];

        // Header (32 bytes).
        out[0..8].copy_from_slice(MAGIC);
        out[16..20].copy_from_slice(&u32::try_from(toc_size).expect("bundle TOC exceeds u32").to_le_bytes());
        match self.format {
            BundleFormat::Legacy => {
                out[8..12].copy_from_slice(
                    &u32::try_from(total_size)
                        .expect("legacy bundle size exceeds u32")
                        .to_le_bytes(),
                );
                out[12..16].copy_from_slice(&self.dummy_size.to_le_bytes());
                out[20..32].copy_from_slice(&MYSTERY_BYTES);
            },
            BundleFormat::Remastered => {
                out[8..16].copy_from_slice(&(total_size as u64).to_le_bytes());
                out[20..22].copy_from_slice(&5u16.to_le_bytes());
                out[22..30].copy_from_slice(&(toc_end as u64).to_le_bytes());
                out[30..32].copy_from_slice(&self.header_reserved);
            },
        }

        // TOC: emit each item's header with its offset field patched to the new page position.
        for (i, item) in self.items.iter().enumerate() {
            let toc_pos = HEADER_SIZE + i * entry_size;
            let entry = item.header_for_format(self.format, item_offsets[i]);
            out[toc_pos..toc_pos + entry_size].copy_from_slice(&entry);
        }

        // Data: copy each item's compressed bytes into its slot. Gaps stay zero.
        for (i, item) in self.items.iter().enumerate() {
            let dst = item_offsets[i] as usize;
            let len = item.zsize as usize;
            out[dst..dst + len].copy_from_slice(&item.data[..len]);
        }

        Ok(out)
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
    /// If any input uses version 5, the output uses version 5 as well.
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
        let format = if bundles.iter().any(|bundle| bundle.format == BundleFormat::Remastered) {
            BundleFormat::Remastered
        } else {
            BundleFormat::Legacy
        };
        let header_reserved = bundles
            .iter()
            .find(|bundle| bundle.format == format)
            .map(|bundle| bundle.header_reserved)
            .unwrap_or([0; 2]);
        let data_offset = u32::try_from(items.len() * format.entry_size()).expect("bundle TOC exceeds u32");

        Bundle {
            bundle_size: 0,
            dummy_size,
            data_offset,
            items,
            format,
            header_reserved,
        }
    }
}

impl Debug for Bundle<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bundle")
            .field("version", &self.version())
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

/// Bounds-checked slice of a single TOC entry.
#[inline]
fn check_entry(bytes: &[u8], toc_pos: usize, entry_size: usize) -> Result<&[u8], BundleError> {
    if toc_pos + entry_size > bytes.len() {
        return Err(BundleError::InvalidBytes(ReadError(format!(
            "toc entry at {:#x} exceeds buffer len {}",
            toc_pos,
            bytes.len()
        ))));
    }
    Ok(&bytes[toc_pos..toc_pos + entry_size])
}

/// Extract `(offset, zsize)` from a TOC entry.
#[inline]
fn entry_data_range(entry: &[u8], format: BundleFormat) -> Result<(usize, usize), BundleError> {
    if format == BundleFormat::Remastered {
        let offset = usize::try_from(read::<u64>(&entry[0x110..0x118])?)
            .map_err(|_| ReadError("bundle offset exceeds usize".to_string()))?;
        let zsize = read::<u32>(&entry[0x11c..0x120])? as usize;
        return Ok((offset, zsize));
    }
    let zsize = read::<u32>(&entry[OFF_ZSIZE..OFF_OFFSET])? as usize;
    let offset = read::<u32>(&entry[OFF_OFFSET..OFF_DATE])? as usize;
    Ok((offset, zsize))
}

/// Bounds-checked slice covering an item's compressed payload.
#[inline]
fn check_range(bytes: &[u8], offset: usize, zsize: usize) -> Result<&[u8], BundleError> {
    let end = offset
        .checked_add(zsize)
        .ok_or_else(|| ReadError("item data range overflows usize".to_string()))?;
    if end > bytes.len() {
        return Err(BundleError::InvalidBytes(ReadError(format!(
            "item data at {:#x}..{:#x} exceeds buffer len {}",
            offset,
            end,
            bytes.len()
        ))));
    }
    Ok(&bytes[offset..end])
}
