//! Chunked client transfer. The runtime alone validates/normalizes/publishes data.
use crate::{
    Request,
    session::{Error, Port, Session},
};
use serde_json::{Value, json};
use std::io::Read;
pub const CHUNK: usize = 8192;
pub const IMAGE_CAP: u64 = 8 * 1024 * 1024;
pub const PACKAGE_CAP: u64 = 256 * 1024 * 1024;
pub enum Progress {
    Chunk(u64),
    Finished(Value),
}
pub struct Upload<R> {
    reader: R,
    id: String,
    size: u64,
    offset: u64,
}
impl<R: Read> Upload<R> {
    pub fn new(reader: R, id: String, size: u64) -> Self {
        Self {
            reader,
            id,
            size,
            offset: 0,
        }
    }
    pub fn step<P: Port>(&mut self, session: &mut Session<P>) -> Result<Progress, Error> {
        if self.offset == self.size {
            let mut extra = [0];
            if self.reader.read(&mut extra).map_err(|_| io_error())? != 0 {
                return Err(io_error());
            }
            return session
                .request(Request::new(
                    "transfer_finish",
                    json!({"transfer_id":self.id}),
                ))
                .map(|r| Progress::Finished(r.data));
        }
        let mut bytes = [0; CHUNK];
        let n = (self.size - self.offset).min(CHUNK as u64) as usize;
        self.reader
            .read_exact(&mut bytes[..n])
            .map_err(|_| io_error())?;
        session.request(Request::new("transfer_write",json!({"transfer_id":self.id,"offset":self.offset.to_string(),"base64":encode(&bytes[..n])})))?;
        self.offset += n as u64;
        Ok(Progress::Chunk(self.offset))
    }
}

pub fn io_error() -> Error {
    Error {
        code: "file_error".into(),
        cause: None,
        unknown: false,
    }
}
fn encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let a = c[0];
        let b = c.get(1).copied().unwrap_or(0);
        let d = c.get(2).copied().unwrap_or(0);
        out.push(TABLE[(a >> 2) as usize] as char);
        out.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        out.push(if c.len() > 1 {
            TABLE[(((b & 15) << 2) | (d >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            TABLE[(d & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
pub struct Download<W> {
    writer: W,
    id: String,
    offset: u64,
}
impl<W: std::io::Write> Download<W> {
    pub fn new(writer: W, id: String) -> Self {
        Self {
            writer,
            id,
            offset: 0,
        }
    }
    pub fn into_inner(self) -> W {
        self.writer
    }
    pub fn step<P: Port>(&mut self, session: &mut Session<P>) -> Result<Progress, Error> {
        let data = session
            .request(Request::new(
                "transfer_read",
                json!({"transfer_id":self.id,"offset":self.offset.to_string()}),
            ))?
            .data;
        let encoded = data["base64"].as_str().ok_or_else(Error::protocol)?;
        let bytes = decode(encoded)?;
        let eof = data["eof"].as_bool().ok_or_else(Error::protocol)?;
        if bytes.is_empty() && !eof {
            return Err(Error::protocol());
        }
        self.offset = self
            .offset
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= PACKAGE_CAP)
            .ok_or_else(Error::protocol)?;
        self.writer.write_all(&bytes).map_err(|_| io_error())?;
        if eof {
            self.writer.flush().map_err(|_| io_error())?;
            Ok(Progress::Finished(json!({"size":self.offset.to_string()})))
        } else {
            Ok(Progress::Chunk(self.offset))
        }
    }
}

pub(crate) fn decode(encoded: &str) -> Result<Vec<u8>, Error> {
    if encoded.len() > CHUNK.div_ceil(3) * 4 || !encoded.len().is_multiple_of(4) {
        return Err(Error::protocol());
    }
    let digit = |c: u8| -> Result<u8, Error> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            b'=' => Ok(0),
            _ => Err(Error::protocol()),
        }
    };
    let mut bytes = Vec::with_capacity(encoded.len() / 4 * 3);
    for c in encoded.as_bytes().as_chunks::<4>().0 {
        let a = digit(c[0])?;
        let b = digit(c[1])?;
        let d = digit(c[2])?;
        let e = digit(c[3])?;
        bytes.push((a << 2) | (b >> 4));
        if c[2] != b'=' {
            bytes.push((b << 4) | (d >> 2));
        }
        if c[3] != b'=' {
            bytes.push((d << 6) | e);
        }
    }
    if bytes.len() > CHUNK || encode(&bytes) != encoded {
        return Err(Error::protocol());
    }
    Ok(bytes)
}
