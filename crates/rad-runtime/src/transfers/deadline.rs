use std::{
    io::{self, Read, Write},
    time::Instant,
};
pub(super) struct Timed<'a, T> {
    pub inner: &'a mut T,
    pub end: Instant,
}
impl<T> Timed<'_, T> {
    fn check(&self) -> io::Result<()> {
        if Instant::now() >= self.end {
            Err(io::Error::from(io::ErrorKind::TimedOut))
        } else {
            Ok(())
        }
    }
}
impl<T: Read> Read for Timed<'_, T> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        let n = b.len().min(65536);
        let r = self.inner.read(&mut b[..n]);
        self.check()?;
        r
    }
}
impl<T: Write> Write for Timed<'_, T> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.check()?;
        let r = self.inner.write(&b[..b.len().min(65536)]);
        self.check()?;
        r
    }
    fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.inner.flush()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deadline_stops_file_stream_before_more_bytes() {
        let mut bytes = Vec::new();
        let mut timed = Timed {
            inner: &mut bytes,
            end: Instant::now(),
        };
        assert_eq!(
            timed.write(b"late").unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(bytes.is_empty());
    }
}
