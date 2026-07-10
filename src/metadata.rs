use std::borrow::Cow;
use std::ffi::CStr;
use std::fmt::Debug;
use std::mem::size_of;

use log::{debug, trace};
use zerocopy::{AsBytes, FromBytes, FromZeroes};

use crate::errors::*;
use crate::hash::fnv64;
use crate::util::{read, read_vlq_i32};


#[repr(C)]
#[derive(FromZeroes, FromBytes, AsBytes, Debug, Clone, Copy)]
pub struct MetadataFileInfo {
    pub(crate) string_table_name_offset: u32,
    pub(crate) path_hash:                u32,
    pub(crate) size_in_bundle:           u32,
    pub(crate) size_in_memory:           u32,
    pub(crate) first_entry:              u32,
    pub(crate) compression_type:         u32,
    pub(crate) bufferid:                 u32,
    pub(crate) hasbuffer:                u32,
}

#[repr(C)]
#[derive(FromZeroes, FromBytes, AsBytes, Debug, Clone, Copy)]
pub struct MetadataFileEntryInfo {
    pub(crate) file_id:          u32,
    pub(crate) bundle_id:        u32,
    pub(crate) offset_in_bundle: u32,
    pub(crate) size_in_bundle:   u32,
    pub(crate) next_entry:       u32,
}

#[repr(C)]
#[derive(FromZeroes, FromBytes, AsBytes, Debug, Clone, Copy)]
pub struct MetadataBundleInfo {
    pub(crate) string_table_name_offset: u32,
    pub(crate) first_file_entry:         u32,
    pub(crate) num_bundle_entries:       u32,
    pub(crate) data_block_size:          u32,
    pub(crate) data_block_offset:        u32,
    pub(crate) burst_data_block_size:    u32,
}

#[repr(C)]
#[derive(FromZeroes, FromBytes, AsBytes, Debug, Clone, Copy)]
pub struct MetadataDirectoryInitInfo {
    pub(crate) string_table_name_offset: i32,
    pub(crate) parent:                   i32,
}

#[repr(C)]
#[derive(FromZeroes, FromBytes, AsBytes, Debug, Clone, Copy)]
pub struct MetadatFileInitInfo {
    pub(crate) file_id:                  i32,
    pub(crate) directory_id:             i32,
    pub(crate) string_table_name_offset: i32,
}

#[repr(C)]
#[derive(FromZeroes, FromBytes, AsBytes, Debug, Clone, Copy)]
pub struct MetadataHash {
    pub(crate) hash:    i64,
    pub(crate) file_id: i64,
}

#[derive(Debug, Clone)]
pub struct DynamicArray<T: Debug + Clone + FromBytes + AsBytes> {
    pub(crate) entries: Vec<T>,
    pub(crate) length:  usize,
}
impl<T: Debug + Clone + FromBytes + AsBytes> DynamicArray<T> {
    pub fn parse<R: AsRef<[u8]>>(data: R, start: usize) -> Result<Self, MetadataError> {
        let data = data.as_ref();
        let (count, length) = read_vlq_i32(&data[start..])?;
        let mut position = start + length;
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            entries.push(read::<T>(&data[position..position + size_of::<T>()])?);
            position += size_of::<T>();
        }
        Ok(Self {
            entries,
            length: position - start,
        })
    }
}

#[derive(Clone)]
pub struct Metadata<'a> {
    pub(crate) data: Cow<'a, [u8]>,
}

impl<'a> Metadata<'a> {
    pub fn new() -> Metadata<'static> {
        Metadata {
            data: Cow::Owned(Vec::new()),
        }
    }

    pub fn parse<R: Into<Cow<'a, [u8]>>>(data: R) -> Result<Self, MetadataError> {
        let data = data.into();

        let magic = read::<[u8; 4]>(&data[0..4])?;
        trace!("magic: {:?}", magic);
        if magic != *b"\x03VTM" {
            return Err(MetadataError::InvalidBytes(ReadError(format!(
                "no vtm magic: {}",
                String::from_utf8_lossy(&magic)
            ))));
        }

        let version = read::<u32>(&data[4..8])?;
        trace!("version: {:?}", version);
        let max_size_in_bundle = read::<u32>(&data[8..12])?;
        trace!("max_size: {:?}", max_size_in_bundle);
        let max_size_in_memory = read::<u32>(&data[12..16])?;
        trace!("max_size_in_memory: {:?}", max_size_in_memory);
        let (string_table_size, length) = read_vlq_i32(&data[16..20])?;
        trace!("string_table_size: {:?}", string_table_size);

        let mut position = 16 + length;

        let string_table = data[position..=position + string_table_size as usize].to_vec();
        trace!("string_table: {:?}", &string_table[string_table.len() - 5..]);
        position += string_table_size as usize;

        trace!("position after string table: {:#x?}", position);

        let file_infos = DynamicArray::<MetadataFileInfo>::parse(data.clone(), position)?;
        position += file_infos.length;

        let mut parsed_file_infos = Vec::with_capacity(file_infos.entries.len());
        for file_info in &file_infos.entries {
            let string_table_name = &string_table[file_info.string_table_name_offset as usize..];
            let string_table_name = CStr::from_bytes_until_nul(string_table_name)
                .map_err(|e| ReadError(e.to_string()))?
                .to_owned();
            parsed_file_infos.push((string_table_name, file_info));
        }
        trace!(
            "parsed_file_info ({}): {:#?}",
            parsed_file_infos.len(),
            parsed_file_infos
        );

        let entry_infos = DynamicArray::<MetadataFileEntryInfo>::parse(data.clone(), position)?;
        trace!("entry_info: ({}): {:#?}", entry_infos.entries.len(), entry_infos);
        position += entry_infos.length;

        let bundle_infos = DynamicArray::<MetadataBundleInfo>::parse(data.clone(), position)?;
        trace!("bundle_info: ({}): {:#?}", bundle_infos.entries.len(), bundle_infos);
        position += bundle_infos.length;

        let (buffers_count, length) = read_vlq_i32(&data[position..position + 4])?;
        let mut position = position + length;
        let mut buffers = Vec::with_capacity(buffers_count as usize);
        for _ in 0..buffers_count {
            buffers.push(read::<u32>(&data[position..position + 4])?);
            position += 4;
        }
        trace!("buffers: ({}): {:#?}", buffers.len(), buffers);

        let dir_init_infos = DynamicArray::<MetadataDirectoryInitInfo>::parse(data.clone(), position)?;
        trace!(
            "dir_init_info: ({}): {:#?}",
            dir_init_infos.entries.len(),
            dir_init_infos
        );
        position += dir_init_infos.length;

        let file_init_infos = DynamicArray::<MetadatFileInitInfo>::parse(data.clone(), position)?;
        trace!(
            "file_init_info: ({}): {:#?}",
            file_init_infos.entries.len(),
            file_init_infos
        );
        position += file_init_infos.length;

        let hashes = DynamicArray::<MetadataHash>::parse(data.clone(), position)?;
        trace!("hashes: ({}): {:#X?}", hashes.entries.len(), hashes);
        position += hashes.length;

        debug!("metadata: read {} out of {} bytes", position, data.len());

        trace!("{:?}", parsed_file_infos[4].0);
        trace!("{:#X?}", fnv64(parsed_file_infos[4].0.as_bytes()));

        Ok(Metadata {
            data,
        })
    }
}
impl Debug for Metadata<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metadata").finish()
    }
}
