use crate::DeckError;
pub fn parse(b: &[u8]) -> Result<[bool; 32], DeckError> {
    if b.len() < 36 || b.len() > 512 || b[..4] != [1, 0, 32, 0] || b[4..36].iter().any(|v| *v > 1) {
        return Err(DeckError::Protocol);
    }
    Ok(std::array::from_fn(|i| b[i + 4] == 1))
}
pub fn packets(key: u8, jpeg: &[u8]) -> Result<Vec<[u8; 1024]>, DeckError> {
    if key >= 32 || jpeg.is_empty() || jpeg.len() > 128 * 1024 {
        return Err(DeckError::Protocol);
    }
    Ok(jpeg
        .chunks(1016)
        .enumerate()
        .map(|(i, c)| {
            let mut p = [0; 1024];
            p[..4].copy_from_slice(&[2, 7, key, u8::from((i + 1) * 1016 >= jpeg.len())]);
            p[4..6].copy_from_slice(&(c.len() as u16).to_le_bytes());
            p[6..8].copy_from_slice(&(i as u16).to_le_bytes());
            p[8..8 + c.len()].copy_from_slice(c);
            p
        })
        .collect())
}
pub fn feature(command: u8, value: i32) -> [u8; 32] {
    let mut b = [0; 32];
    b[..2].copy_from_slice(&[3, command]);
    if command == 13 {
        b[2..6].copy_from_slice(&value.to_le_bytes());
    } else if command == 8 {
        b[2] = value as u8;
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xl_report_and_chunk_boundaries_preserve_32_keys_and_final_padding() {
        let mut b = [0; 512];
        b[..4].copy_from_slice(&[1, 0, 32, 0]);
        b[4] = 1;
        b[35] = 1;
        let keys = parse(&b).unwrap();
        assert!(keys[0] && keys[31]);
        assert_eq!(keys.iter().filter(|v| **v).count(), 2);
        b[10] = 2;
        assert_eq!(parse(&b), Err(DeckError::Protocol));
        assert_eq!(parse(&b[..35]), Err(DeckError::Protocol));
        let data = vec![77; 1017];
        let p = packets(31, &data).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(&p[0][..8], &[2, 7, 31, 0, 248, 3, 0, 0]);
        assert_eq!(&p[1][..9], &[2, 7, 31, 1, 1, 0, 1, 0, 77]);
        assert!(p[1][9..].iter().all(|b| *b == 0));
        assert!(packets(32, &data).is_err());
    }
}
