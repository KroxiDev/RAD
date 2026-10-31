use crate::DeckError;
use std::path::Path;
/// A hidraw node together with the serial of the USB device that owns it.
#[derive(Debug, PartialEq, Eq)]
struct Located {
    node: String,
    serial: String,
}
/// Finds the XL through `<sys>/class/hidraw`, on any USB port (ADR 0006).
///
/// With several XL connected the one with the lowest serial wins, so the choice is
/// stable across ports, hidraw numbering and reconnections; the others are never
/// touched. A device without a valid serial has no verifiable identity and is not
/// eligible: if no XL has one, or two share the lowest, the result is `Identity`.
fn locate(sys: &Path) -> Result<Located, DeckError> {
    use std::{fs, io::Read, path::PathBuf};
    const MAX_ENTRIES: usize = 256;
    const MAX_SERIAL: usize = 32;
    fn attribute(p: &Path) -> Result<String, DeckError> {
        let file = std::fs::File::open(p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DeckError::Absent
            } else {
                DeckError::Io
            }
        })?;
        let mut s = String::new();
        file.take(64)
            .read_to_string(&mut s)
            .map_err(|_| DeckError::Io)?;
        Ok(s.trim().into())
    }
    struct Candidate {
        usb: PathBuf,
        interface: PathBuf,
        node: String,
    }
    let devices = fs::canonicalize(sys.join("devices")).map_err(|_| DeckError::Absent)?;
    let class = fs::read_dir(sys.join("class/hidraw")).map_err(|_| DeckError::Absent)?;
    let mut candidates = Vec::new();
    for (i, entry) in class.enumerate() {
        if i >= MAX_ENTRIES {
            return Err(DeckError::Identity);
        }
        let entry = entry.map_err(|_| DeckError::Io)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let number = name.strip_prefix("hidraw").unwrap_or("");
        if number.is_empty() || !number.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // <usb device>/<interface>/<hid device>/hidraw/<node>
        let Ok(real) = fs::canonicalize(entry.path()) else {
            continue;
        };
        if !real.starts_with(&devices) {
            continue;
        }
        let mut up = real.ancestors().skip(1);
        let (_, _, Some(interface), Some(usb)) = (up.next(), up.next(), up.next(), up.next())
        else {
            continue;
        };
        let xl = |file, value| match attribute(&usb.join(file)) {
            Ok(v) => Ok(v == value),
            Err(DeckError::Absent) => Ok(false),
            Err(e) => Err(e),
        };
        if xl("idVendor", "0fd9")? && xl("idProduct", "006c")? {
            candidates.push(Candidate {
                usb: usb.into(),
                interface: interface.into(),
                node: name.into(),
            });
        }
    }
    if candidates.is_empty() {
        return Err(DeckError::Absent);
    }
    let mut chosen: Option<(String, &PathBuf)> = None;
    let mut tie = false;
    let mut seen: Vec<&PathBuf> = Vec::new();
    for c in &candidates {
        if seen.contains(&&c.usb) {
            continue;
        }
        seen.push(&c.usb);
        let serial = attribute(&c.usb.join("serial")).unwrap_or_default();
        if serial.is_empty()
            || serial.len() > MAX_SERIAL
            || !serial.bytes().all(|b| b.is_ascii_graphic())
        {
            continue;
        }
        match &chosen {
            Some((best, _)) if serial > *best => {}
            Some((best, _)) if serial == *best => tie = true,
            _ => {
                chosen = Some((serial, &c.usb));
                tie = false;
            }
        }
    }
    let (serial, usb) = chosen.ok_or(DeckError::Identity)?;
    if tie {
        return Err(DeckError::Identity);
    }
    let mut nodes = candidates.iter().filter(|c| &c.usb == usb);
    let (Some(first), None) = (nodes.next(), nodes.next()) else {
        return Err(DeckError::Identity);
    };
    match fs::read_link(first.interface.join("driver")) {
        Ok(d) if d.file_name() == Some(std::ffi::OsStr::new("usbhid")) => {}
        Ok(_) => return Err(DeckError::Held),
        Err(_) => return Err(DeckError::Absent),
    }
    Ok(Located {
        node: first.node.clone(),
        serial,
    })
}
pub(crate) trait Link: Send {
    fn raw_fd(&self) -> std::os::fd::RawFd;
    fn read(&mut self) -> Result<Option<[bool; 32]>, DeckError>;
    fn write(&mut self, packet: &[u8; 1024]) -> Result<bool, DeckError>;
    fn feature(&mut self, packet: [u8; 32]) -> Result<(), DeckError>;
    fn close(&mut self) -> bool;
}
#[repr(C)]
#[derive(Default)]
struct RawInfo {
    bus: u32,
    vendor: i16,
    product: i16,
}
struct Hidraw {
    file: std::fs::File,
    sleep: i32,
    closed: bool,
}
fn fd(file: &std::fs::File) -> std::os::fd::RawFd {
    std::os::fd::AsRawFd::as_raw_fd(file)
}
pub(crate) fn connect_at(sys: &Path, dev: &Path) -> Result<Box<dyn Link>, DeckError> {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
    let located = locate(sys)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(dev.join(&located.node))
        .map_err(|_| DeckError::Io)?;
    if !file
        .metadata()
        .map_err(|_| DeckError::Io)?
        .file_type()
        .is_char_device()
    {
        return Err(DeckError::Identity);
    }
    let mut info = RawInfo::default();
    // SAFETY: owned fd; RawInfo matches linux/hidraw.h's hidraw_devinfo.
    if unsafe { libc::ioctl(fd(&file), libc::_IOR::<RawInfo>(b'H' as u32, 3), &mut info) } < 0 {
        return Err(DeckError::Io);
    }
    if (info.bus, info.vendor, info.product) != (3, 0x0fd9, 0x006c) {
        return Err(DeckError::Identity);
    }
    file.try_lock().map_err(|_| DeckError::Held)?;
    if locate(sys)? != located {
        return Err(DeckError::Identity);
    }
    let mut duration = [0u8; 32];
    duration[0] = 0x0a;
    // SAFETY: live feature buffer and owned, identified, locked hidraw fd.
    let n = unsafe {
        libc::ioctl(
            fd(&file),
            libc::_IOWR::<[u8; 32]>(b'H' as u32, 7),
            duration.as_mut_ptr(),
        )
    };
    if n < 6 || duration[0] != 0x0a || duration[1] < 4 {
        return Err(DeckError::Protocol);
    }
    let sleep = i32::from_le_bytes(duration[2..6].try_into().unwrap());
    if sleep < 0 {
        return Err(DeckError::Protocol);
    }
    let mut link = Hidraw {
        file,
        sleep,
        closed: false,
    };
    link.feature(crate::protocol::feature(13, 0))?;
    Ok(Box::new(link))
}
impl Link for Hidraw {
    fn raw_fd(&self) -> std::os::fd::RawFd {
        fd(&self.file)
    }
    fn read(&mut self) -> Result<Option<[bool; 32]>, DeckError> {
        use std::io::Read;
        let mut b = [0; 512];
        match self.file.read(&mut b) {
            Ok(0) => Err(DeckError::Absent),
            Ok(n) => crate::protocol::parse(&b[..n]).map(Some),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(_) => Err(DeckError::Io),
        }
    }
    fn write(&mut self, p: &[u8; 1024]) -> Result<bool, DeckError> {
        use std::io::Write;
        match self.file.write(p) {
            Ok(1024) => Ok(true),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(false)
            }
            _ => Err(DeckError::Io),
        }
    }
    fn feature(&mut self, mut p: [u8; 32]) -> Result<(), DeckError> {
        // SAFETY: live feature buffer on identified owned fd; only worker calls this.
        let n = unsafe {
            libc::ioctl(
                fd(&self.file),
                libc::_IOWR::<[u8; 32]>(b'H' as u32, 6),
                p.as_mut_ptr(),
            )
        };
        if n == 32 { Ok(()) } else { Err(DeckError::Io) }
    }
    fn close(&mut self) -> bool {
        if self.closed {
            return true;
        }
        self.closed = true;
        let sleep = self
            .feature(crate::protocol::feature(13, self.sleep))
            .is_ok();
        let logo = self.feature(crate::protocol::feature(2, 0)).is_ok();
        sleep && logo
    }
}
impl Drop for Hidraw {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rad-deck-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("devices")).unwrap();
        std::fs::create_dir_all(root.join("class/hidraw")).unwrap();
        root
    }
    /// Builds `devices/.../usb1/<port>/<port>:1.0/<hid>/hidraw/<node>` plus the
    /// relative `class/hidraw/<node>` symlink, as sysfs lays it out.
    fn add(root: &Path, port: &str, ids: (&str, &str), serial: Option<&str>, nodes: &[&str]) {
        let usb = root.join(format!("devices/pci0000:00/0000:00:14.0/usb1/{port}"));
        let interface = usb.join(format!("{port}:1.0"));
        std::fs::create_dir_all(&interface).unwrap();
        std::fs::write(usb.join("idVendor"), format!("{}\n", ids.0)).unwrap();
        std::fs::write(usb.join("idProduct"), format!("{}\n", ids.1)).unwrap();
        if let Some(serial) = serial {
            std::fs::write(usb.join("serial"), format!("{serial}\n")).unwrap();
        }
        symlink("/fixture/drivers/usbhid", interface.join("driver")).unwrap();
        for (i, node) in nodes.iter().enumerate() {
            let dir = interface.join(format!("0003:0FD9:006C.{i:04}/hidraw/{node}"));
            std::fs::create_dir_all(&dir).unwrap();
            let target = dir.strip_prefix(root.join("devices")).unwrap();
            symlink(
                PathBuf::from("../../devices").join(target),
                root.join("class/hidraw").join(node),
            )
            .unwrap();
        }
    }
    fn xl(root: &Path, port: &str, serial: Option<&str>, nodes: &[&str]) {
        add(root, port, ("0fd9", "006c"), serial, nodes);
    }
    fn found(root: &Path) -> Result<(String, String), DeckError> {
        locate(root).map(|l| (l.node, l.serial))
    }
    fn pair(node: &str, serial: &str) -> Result<(String, String), DeckError> {
        Ok((node.into(), serial.into()))
    }

    #[test]
    fn lowest_serial_wins_regardless_of_port_or_numbering() {
        for (low_port, low_node, high_port, high_node) in [
            ("1-5", "hidraw2", "3-2", "hidraw9"),
            ("3-2", "hidraw9", "1-5", "hidraw2"),
            ("1-5", "hidraw9", "3-2", "hidraw2"),
        ] {
            let root = temp_root("two");
            xl(&root, low_port, Some("AL1"), &[low_node]);
            xl(&root, high_port, Some("AL2"), &[high_node]);
            assert_eq!(found(&root), pair(low_node, "AL1"));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn regular_fixture_is_never_treated_as_hidraw_or_written() {
        let root = temp_root("open");
        xl(&root, "1-5", Some("AL1"), &["hidraw4"]);
        let dev = root.join("dev");
        std::fs::create_dir(&dev).unwrap();
        std::fs::write(dev.join("hidraw4"), b"intact").unwrap();
        assert!(matches!(connect_at(&root, &dev), Err(DeckError::Identity)));
        assert_eq!(std::fs::read(dev.join("hidraw4")).unwrap(), b"intact");
        std::fs::remove_dir_all(root).unwrap();
    }
}
