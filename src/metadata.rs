use std::ffi::{CStr, CString};
use std::fmt::Debug;
use std::mem::size_of;

use log::{debug, trace};
use zerocopy::{AsBytes, FromBytes, FromZeroes};

use crate::errors::*;
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
pub struct MetadataFileInitInfo {
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
    pub fn parse(data: &[u8], start: usize) -> Result<Self, MetadataError> {
        let (count, vlq_len) = read_vlq_i32(&data[start..])?;
        if count < 0 {
            return Err(MetadataError::InvalidBytes(ReadError(format!(
                "negative array count: {}",
                count
            ))));
        }
        let mut position = start + vlq_len;
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

#[derive(Debug, Clone)]
pub struct Metadata {
    pub version:            u32,
    pub max_size_in_bundle: u32,
    pub max_size_in_memory: u32,
    pub string_table:       Vec<u8>,
    pub file_infos:         Vec<(CString, MetadataFileInfo)>,
    pub entry_infos:        Vec<MetadataFileEntryInfo>,
    pub bundle_infos:       Vec<MetadataBundleInfo>,
    pub buffers:            Vec<u32>,
    pub dir_init_infos:     Vec<MetadataDirectoryInitInfo>,
    pub file_init_infos:    Vec<MetadataFileInitInfo>,
    pub hashes:             Vec<MetadataHash>,
}

impl Metadata {
    pub fn new() -> Self {
        Metadata {
            version:            0,
            max_size_in_bundle: 0,
            max_size_in_memory: 0,
            string_table:       Vec::new(),
            file_infos:         Vec::new(),
            entry_infos:        Vec::new(),
            bundle_infos:       Vec::new(),
            buffers:            Vec::new(),
            dir_init_infos:     Vec::new(),
            file_init_infos:    Vec::new(),
            hashes:             Vec::new(),
        }
    }

    pub fn parse(data: &[u8]) -> Result<Self, MetadataError> {
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
        trace!("max_size_in_bundle: {:?}", max_size_in_bundle);
        let max_size_in_memory = read::<u32>(&data[12..16])?;
        trace!("max_size_in_memory: {:?}", max_size_in_memory);
        let (string_table_size, vlq_len) = read_vlq_i32(&data[16..])?;
        trace!("string_table_size: {:?}", string_table_size);
        if string_table_size < 0 {
            return Err(MetadataError::InvalidBytes(ReadError(format!(
                "negative string table size: {}",
                string_table_size
            ))));
        }

        let mut position = 16 + vlq_len;

        let string_table = data[position..position + string_table_size as usize].to_vec();
        position += string_table_size as usize;

        trace!("position after string table: {:#x?}", position);

        let file_infos_raw = DynamicArray::<MetadataFileInfo>::parse(data, position)?;
        position += file_infos_raw.length;

        let mut file_infos = Vec::with_capacity(file_infos_raw.entries.len());
        for file_info in &file_infos_raw.entries {
            let name = CStr::from_bytes_until_nul(&string_table[file_info.string_table_name_offset as usize..])
                .map_err(|e| ReadError(e.to_string()))?
                .to_owned();
            file_infos.push((name, *file_info));
        }
        trace!("file_infos ({}): {:#?}", file_infos.len(), file_infos);

        let entry_infos = DynamicArray::<MetadataFileEntryInfo>::parse(data, position)?;
        trace!("entry_infos ({}): {:#?}", entry_infos.entries.len(), entry_infos);
        position += entry_infos.length;

        let bundle_infos = DynamicArray::<MetadataBundleInfo>::parse(data, position)?;
        trace!("bundle_infos ({}): {:#?}", bundle_infos.entries.len(), bundle_infos);
        position += bundle_infos.length;

        let (buffers_count, vlq_len) = read_vlq_i32(&data[position..])?;
        if buffers_count < 0 {
            return Err(MetadataError::InvalidBytes(ReadError(format!(
                "negative buffer count: {}",
                buffers_count
            ))));
        }
        position += vlq_len;
        let mut buffers = Vec::with_capacity(buffers_count as usize);
        for _ in 0..buffers_count {
            buffers.push(read::<u32>(&data[position..position + 4])?);
            position += 4;
        }
        trace!("buffers ({}): {:#?}", buffers.len(), buffers);

        let dir_init_infos = DynamicArray::<MetadataDirectoryInitInfo>::parse(data, position)?;
        trace!(
            "dir_init_infos ({}): {:#?}",
            dir_init_infos.entries.len(),
            dir_init_infos
        );
        position += dir_init_infos.length;

        let file_init_infos = DynamicArray::<MetadataFileInitInfo>::parse(data, position)?;
        trace!(
            "file_init_infos ({}): {:#?}",
            file_init_infos.entries.len(),
            file_init_infos
        );
        position += file_init_infos.length;

        let hashes = DynamicArray::<MetadataHash>::parse(data, position)?;
        trace!("hashes ({}): {:#X?}", hashes.entries.len(), hashes);
        position += hashes.length;

        debug!("metadata: read {} out of {} bytes", position, data.len());

        Ok(Metadata {
            version,
            max_size_in_bundle,
            max_size_in_memory,
            string_table,
            file_infos,
            entry_infos: entry_infos.entries,
            bundle_infos: bundle_infos.entries,
            buffers,
            dir_init_infos: dir_init_infos.entries,
            file_init_infos: file_init_infos.entries,
            hashes: hashes.entries,
        })
    }
}
impl Debug for Metadata<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metadata").finish()
    }
}
