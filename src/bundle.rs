use std::borrow::Cow;
use std::ffi::{CStr, CString};
use std::fmt::Debug;
use std::io::Read;

use log::{debug, info};

use crate::errors::*;
use crate::util::*;


#[repr(u32)]
#[derive(Debug, Clone)]
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
    pub fn parse<R: Into<Cow<'a, [u8]>>>(data: R, position: usize) -> Result<Self, BundleError> {
        let data = data.into();
        let mut position = position;
        let name = CStr::from_bytes_until_nul(&data[position..position + 0x100])
            .map_err(|e| ReadError(e.to_string()))?
            .to_owned();
        position += 0x100;
        let hash = read::<u128>(&data[position..position + 0x10])?;
        position += 0x10;
        let empty = read::<u32>(&data[position..position + 0x4])?;
        position += 0x4;
        let size = read::<u32>(&data[position..position + 0x4])?;
        position += 0x4;
        let zsize = read::<u32>(&data[position..position + 0x4])?;
        position += 0x4;
        let offset = read::<u32>(&data[position..position + 0x4])?;
        position += 0x4;
        let date = read::<u32>(&data[position..position + 0x4])?;
        position += 0x4;
        let time = read::<u32>(&data[position..position + 0x4])?;
        position += 0x4;
        let zero = read::<u128>(&data[position..position + 0x10])?;
        position += 0x10;
        let crc = read::<u32>(&data[position..position + 0x4])?;
        position += 0x4;
        let compression = read::<u32>(&data[position..position + 0x4])?;
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

    pub fn header(&self) -> Vec<u8> {
        let mut header = Vec::with_capacity(0x140);

        header.extend(self.name.as_bytes());
        header.resize(0x100, 0);
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

        header
    }

    pub fn data(&self) -> Result<Vec<u8>, BundleError> {
        match self.compression.into() {
            BundleCompression::None => {
                Ok(self.data[self.offset as usize..(self.offset + self.zsize) as usize].to_vec())
            },
            BundleCompression::Zlib => {
                let mut decoder = flate2::read::ZlibDecoder::new(
                    &self.data[self.offset as usize..(self.offset + self.zsize) as usize],
                );
                let mut data = Vec::with_capacity(self.size as usize);
                decoder.read_to_end(&mut data).map_err(|e| ReadError(e.to_string()))?;
                Ok(data)
            },
            BundleCompression::Snappy => {
                let mut decoder = snap::read::FrameDecoder::new(
                    &self.data[self.offset as usize..(self.offset + self.zsize) as usize],
                );
                let mut data = Vec::with_capacity(self.size as usize);
                decoder.read_to_end(&mut data).map_err(|e| ReadError(e.to_string()))?;
                Ok(data)
            },
            BundleCompression::Doboz => Err(BundleError::UnsupportedCompression("Doboz".to_string())),
            BundleCompression::Lz4 | BundleCompression::Lz4hc => {
                let data = lz4::block::decompress(
                    &self.data[self.offset as usize..(self.offset + self.zsize) as usize],
                    Some(self.size as _),
                )
                .map_err(|e| ReadError(e.to_string()))?;
                Ok(data.to_vec())
            },
            BundleCompression::Unknown => Err(BundleError::UnsupportedCompression(format!("{:?}", self.compression))),
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
            .field("compression", &self.compression)
            .finish()
    }
}

#[derive(Clone)]
pub struct Bundle<'a> {
    pub(crate) data:        Cow<'a, [u8]>,
    pub(crate) bundle_size: u32,
    pub(crate) dummy_size:  u32,
    pub(crate) data_offset: u32,
    pub(crate) items:       Vec<BundleItem<'a>>,
}

impl<'a> Bundle<'a> {
    pub fn new() -> Bundle<'static> {
        Bundle {
            data:        Cow::Owned(Vec::new()),
            bundle_size: 0,
            dummy_size:  0,
            data_offset: 0,
            items:       Vec::new(),
        }
    }

    pub fn parse<R: Into<Cow<'a, [u8]>>>(data: R) -> Result<Self, BundleError> {
        let data = data.into();

        let magic = read::<[u8; 8]>(&data[0..8])?;
        if magic != *b"POTATO70" {
            return Err(BundleError::InvalidBytes(ReadError(format!(
                "no potato magic: {}",
                String::from_utf8_lossy(&magic)
            ))));
        }

        let bundle_size = read::<u32>(&data[8..12])?;
        let dummy_size = read::<u32>(&data[12..16])?;
        let data_offset = read::<u32>(&data[16..20])?;

        let mut position = 0x20;
        let mut items = Vec::new();
        while position < data_offset as usize + 0x20 {
            let item = BundleItem::parse(data.clone(), position)?;
            items.push(item);
            position += 0x140;
        }

        debug!("bundle: read {} out of {} bytes", position, data.len());

        Ok(Self {
            data,
            bundle_size,
            dummy_size,
            data_offset,
            items,
        })
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
