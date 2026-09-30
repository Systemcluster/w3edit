use std::any::type_name;

use zerocopy::FromBytes;

use crate::errors::ReadError;


pub fn read<T: FromBytes>(resource: &[u8]) -> Result<T, ReadError> {
    T::read_from_prefix(resource)
        .map(|(value, _)| value)
        .map_err(|_| ReadError(type_name::<T>().to_string()))
}

pub fn read_vlq_i32(resource: &[u8]) -> Result<(i32, usize), ReadError> {
    let mut position = 0;
    let mut byte = *resource
        .get(position)
        .ok_or_else(|| ReadError("vlq_i32: unexpected end of input".to_string()))?;
    let sign = (byte & 0x80) != 0;
    let mut next = (byte & 0x40) != 0;
    let mut value = (byte & 0x3F) as i32;
    let mut shift = 6;
    while next {
        position += 1;
        if position >= 5 {
            return Err(ReadError("vlq_i32: encoding exceeds 5 bytes".to_string()));
        }
        byte = *resource
            .get(position)
            .ok_or_else(|| ReadError("vlq_i32: unexpected end of input".to_string()))?;
        let bits = (byte & 0x7F) as i64;
        let candidate = i64::from(value) | (bits << shift);
        if candidate > i64::from(i32::MAX) {
            return Err(ReadError("vlq_i32: value exceeds i32 range".to_string()));
        }
        value = candidate as i32;
        next = (byte & 0x80) != 0;
        shift += 7;
    }
    if sign {
        value = -value;
    }
    Ok((value, position + 1))
}

/// Encode a signed 32-bit integer as a variable-length quantity.
///
/// Encoding (matches WolvenKit / REDengine):
/// - First byte: sign in bit 7, continuation in bit 6, value bits [5:0] in bits [5:0].
/// - Subsequent bytes: continuation in bit 7, value bits [6:0] in bits [6:0].
///
/// Zero is encoded with the sign bit set (`0x80`) to match REDengine's `WriteVLQInt32`
/// convention. Both `0x00` and `0x80` decode to `0`, but round-tripping REDengine
/// files requires this specific canonical form.
pub fn write_vlq_i32(value: i32) -> Vec<u8> {
    // Treat zero as if it were signed so the sign bit is set (REDengine convention).
    let negative = value <= 0;
    let mut abs_val = (value as i64).unsigned_abs() as u32;
    let mut out = Vec::with_capacity(5);

    let mut b = (abs_val & 0x3F) as u8;
    abs_val >>= 6;
    if negative {
        b |= 0x80;
    }
    let mut cont = abs_val != 0;
    if cont {
        b |= 0x40;
    }
    out.push(b);

    while cont {
        b = (abs_val & 0x7F) as u8;
        abs_val >>= 7;
        cont = abs_val != 0;
        if cont {
            b |= 0x80;
        }
        out.push(b);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlq_roundtrips_i32_extremes() {
        for value in [i32::MIN + 1, -1, 0, 1, i32::MAX] {
            let encoded = write_vlq_i32(value);
            assert_eq!(read_vlq_i32(&encoded).unwrap().0, value);
        }
    }

    #[test]
    fn vlq_rejects_overlong_encoding() {
        assert!(read_vlq_i32(&[0x40, 0x80, 0x80, 0x80, 0x80, 0x00]).is_err());
    }

    #[test]
    fn vlq_rejects_out_of_range_value() {
        assert!(read_vlq_i32(&[0x40, 0x80, 0x80, 0x80, 0x10]).is_err());
    }
}
