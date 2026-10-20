use crate::DeckError;
use std::path::Path;
fn locate(sys: &Path) -> Result<String, DeckError> {
    use std::{fs, io::Read};
    fn attribute(p: &Path) -> Result<String, DeckError> {
        let file = std::fs::File::open(p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DeckError::Absent
            } else {
                DeckError::Io
            }
        })?;
        let mut s = String::new();
        file.take(32)
            .read_to_string(&mut s)
            .map_err(|_| DeckError::Io)?;
        Ok(s.trim().into())
    }
    let device = sys.join("bus/usb/devices/1-5");
    if attribute(&device.join("idVendor"))? != "0fd9"
        || attribute(&device.join("idProduct"))? != "006c"
    {
        return Err(DeckError::Identity);
    }
    let interface = sys.join("bus/usb/devices/1-5:1.0");
    let driver = fs::read_link(interface.join("driver")).map_err(|_| DeckError::Absent)?;
    if driver.file_name() != Some(std::ffi::OsStr::new("usbhid")) {
        return Err(DeckError::Held);
    }
    let mut node = None;
    for (i, entry) in fs::read_dir(interface)
        .map_err(|_| DeckError::Absent)?
        .enumerate()
    {
        if i >= 64 {
            return Err(DeckError::Identity);
        }
        let path = entry.map_err(|_| DeckError::Io)?.path().join("hidraw");
        let Ok(entries) = fs::read_dir(path) else {
            continue;
        };
        for entry in entries {
            let name = entry.map_err(|_| DeckError::Io)?.file_name();
            let name = name.to_str().ok_or(DeckError::Identity)?;
            let number = name.strip_prefix("hidraw").ok_or(DeckError::Identity)?;
            if number.is_empty() || !number.bytes().all(|c| c.is_ascii_digit()) || node.is_some() {
                return Err(DeckError::Identity);
            }
            node = Some(name.into());
        }
    }
    node.ok_or(DeckError::Absent)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_port_requires_xl_usbhid_and_one_node_before_open() {
        let root = std::env::temp_dir().join(format!(
            "rad-deck-locate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        assert_eq!(locate(&root), Err(DeckError::Absent));
        let usb = root.join("bus/usb/devices/1-5");
        std::fs::create_dir_all(&usb).unwrap();
        std::fs::write(usb.join("idVendor"), "0fd9\n").unwrap();
        std::fs::write(usb.join("idProduct"), "006c\n").unwrap();
        let interface = root.join("bus/usb/devices/1-5:1.0");
        std::fs::create_dir_all(interface.join("0003:0FD9:006C.1/hidraw/hidraw4")).unwrap();
        std::os::unix::fs::symlink("/fixture/drivers/usbfs", interface.join("driver")).unwrap();
        assert_eq!(locate(&root), Err(DeckError::Held));
        std::fs::remove_file(interface.join("driver")).unwrap();
        std::os::unix::fs::symlink("/fixture/drivers/usbhid", interface.join("driver")).unwrap();
        assert_eq!(locate(&root), Ok("hidraw4".into()));
        std::fs::write(usb.join("idProduct"), "9999").unwrap();
        assert_eq!(locate(&root), Err(DeckError::Identity));
        std::fs::write(usb.join("idProduct"), "006c").unwrap();
        std::fs::create_dir_all(interface.join("0003:0FD9:006C.1/hidraw/hidraw5")).unwrap();
        assert_eq!(locate(&root), Err(DeckError::Identity));
        std::fs::remove_dir_all(root).unwrap();
    }
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
    let node = locate(sys)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(dev.join(&node))
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
    if locate(sys)? != node {
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
mod open_tests {
    use super::*;
    #[test]
    fn regular_fixture_is_never_treated_as_hidraw_or_written() {
        let root = std::env::temp_dir().join(format!(
            "rad-deck-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let device = root.join("bus/usb/devices/1-5");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::write(device.join("idVendor"), "0fd9").unwrap();
        std::fs::write(device.join("idProduct"), "006c").unwrap();
        let interface = root.join("bus/usb/devices/1-5:1.0");
        std::fs::create_dir_all(interface.join("hid/hidraw/hidraw4")).unwrap();
        std::os::unix::fs::symlink("/fixture/usbhid", interface.join("driver")).unwrap();
        let dev = root.join("dev");
        std::fs::create_dir(&dev).unwrap();
        std::fs::write(dev.join("hidraw4"), b"intact").unwrap();
        assert!(matches!(connect_at(&root, &dev), Err(DeckError::Identity)));
        assert_eq!(std::fs::read(dev.join("hidraw4")).unwrap(), b"intact");
        std::fs::remove_dir_all(root).unwrap();
    }
}
