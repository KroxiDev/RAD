//! Structural ELF checks, not a loader or a claim of trust/ABI compatibility.
use crate::Error;

pub(crate) fn validate(bytes: &[u8], target: &str) -> Result<(), Error> {
    if target != "x86_64-unknown-linux-gnu" {
        return Err(Error::UnsupportedTarget);
    }
    if bytes.len() < 64
        || &bytes[..7] != b"\x7fELF\x02\x01\x01"
        || !matches!(bytes[7], 0 | 3)
        || bytes[8..16].iter().any(|&byte| byte != 0)
        || !matches!(u16_at(bytes, 16), 2 | 3)
        || u16_at(bytes, 18) != 62
        || u32_at(bytes, 20) != 1
        || u16_at(bytes, 52) != 64
        || u16_at(bytes, 54) != 56
    {
        return Err(Error::InvalidExecutable);
    }
    let count = u16_at(bytes, 56) as u64;
    let offset = u64_at(bytes, 32);
    if count == 0
        || count > 128
        || offset < 64
        || offset
            .checked_add(count * 56)
            .is_none_or(|end| end > bytes.len() as u64)
    {
        return Err(Error::InvalidExecutable);
    }
    let entry = u64_at(bytes, 24);
    let mut executable_entry = false;
    for index in 0..count {
        let header = &bytes[(offset + index * 56) as usize..][..56];
        let kind = u32_at(header, 0);
        if kind == 0 {
            continue;
        } // PT_NULL has no defined payload.
        let file_offset = u64_at(header, 8);
        let address = u64_at(header, 16);
        let file_size = u64_at(header, 32);
        let memory_size = u64_at(header, 40);
        if file_offset
            .checked_add(file_size)
            .is_none_or(|end| end > bytes.len() as u64)
        {
            return Err(Error::InvalidExecutable);
        }
        if kind == 1 {
            // PT_LOAD
            let alignment = u64_at(header, 48);
            let end = address
                .checked_add(memory_size)
                .ok_or(Error::InvalidExecutable)?;
            // Linux x86-64 user addresses cannot exceed the 57-bit user range.
            if end > (1u64 << 56)
                || file_size > memory_size
                || alignment > 1
                    && (!alignment.is_power_of_two()
                        || address % alignment != file_offset % alignment)
            {
                return Err(Error::InvalidExecutable);
            }
            // The entry must contain supplied instructions, not zero-fill/BSS.
            // file_size <= memory_size and checked address + memory_size above.
            executable_entry |=
                u32_at(header, 4) & 1 != 0 && entry >= address && entry < address + file_size;
        }
    }
    if !executable_entry {
        return Err(Error::InvalidExecutable);
    }
    Ok(())
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
