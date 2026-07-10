use std::any::type_name;

use zerocopy::FromBytes;

use crate::errors::ReadError;


pub fn read<T: FromBytes>(resource: &[u8]) -> Result<T, ReadError> {
    T::read_from_prefix(resource).ok_or_else(|| ReadError(type_name::<T>().to_string()))
}

pub fn read_vlq_i32(resource: &[u8]) -> Result<(i32, usize), ReadError> {
    let mut position = 0;
    let mut byte = resource[position];
    let sign = (byte & 0x80) != 0;
    let mut next = (byte & 0x40) != 0;
    let mut value = (byte % 0x80 % 0x40) as i32;
    let mut shift = 6;
    while next {
        position += 1;
        byte = resource[position];
        value |= ((byte % 0x80) as i32) << shift;
        next = (byte & 0x80) != 0;
        shift += 7;
    }
    if sign {
        value = -value;
    }
    Ok((value, position + 1))
}
