//! Private runtime credential vault; never part of exported configuration.
use std::{collections::BTreeMap, fmt, path::Path};
use zeroize::Zeroizing;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    Capacity,
    UnsafePath,
    Unavailable,
    Io,
    Uncertain,
    Busy,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid_input",
            Self::Capacity => "capacity",
            Self::UnsafePath => "unsafe_path",
            Self::Unavailable => "unavailable",
            Self::Io => "io",
            Self::Uncertain => "uncertain",
            Self::Busy => "busy",
        })
    }
}
impl std::error::Error for Error {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Absent,
    Present,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Ready,
    KeyMissing,
    Unknown,
}
pub struct Secret(Zeroizing<String>);
impl Secret {
    pub fn new(value: String) -> Result<Self, Error> {
        let value = Zeroizing::new(value);
        if value.len() > 4096 {
            return Err(Error::Capacity);
        }
        Ok(Self(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}
#[derive(Default)]
pub struct Secrets(BTreeMap<String, Secret>);
impl Secrets {
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Secret)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
}
impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secrets([REDACTED])")
    }
}
mod files;
use std::fs::File;
use std::os::fd::AsRawFd;
pub struct Vault {
    dir: File,
    _lock: File,
    unknown: bool,
}
impl Vault {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let root = files::root(path)?;
        let dir = files::directory(&root, "credentials", true)?.ok_or(Error::Io)?;
        let lock =
            files::child(&dir, "vault.lock", libc::O_RDWR | libc::O_CREAT)?.ok_or(Error::Io)?;
        files::private(&lock, false)?;
        // SAFETY: lock descriptor remains held for entire Vault lifetime.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err(Error::Busy);
        }
        let mut v = Self {
            dir,
            _lock: lock,
            unknown: false,
        };
        if v.key()?.is_none() && !v.has_slots()? {
            v.create_key()?;
        }
        v.reconcile()?;
        Ok(v)
    }
    fn create_key(&mut self) -> Result<(), Error> {
        let mut key = Zeroizing::new(vec![0u8; 40]);
        key[..8].copy_from_slice(b"RADK\0\0\0\x01");
        files::random(&mut key[8..])?;
        let result = files::publish(&self.dir, "master.key", &key, true);
        self.publication(result)?;
        let durable: Result<(), Error> = (|| {
            let file =
                files::child(&self.dir, "master.key", libc::O_RDONLY)?.ok_or(Error::Unavailable)?;
            files::private(&file, false)?;
            files::sync_file(&file)?;
            files::sync_directory(&self.dir)?;
            self.key()?.ok_or(Error::Unavailable)?;
            Ok(())
        })();
        // Publication happened: any subsequent failure must retain Unknown, never generate another key.
        if durable.is_err() {
            self.unknown = true;
            return Err(Error::Uncertain);
        }
        Ok(())
    }
    fn key(&self) -> Result<Option<Zeroizing<Vec<u8>>>, Error> {
        let key = files::read(&self.dir, "master.key", 40, 40)?;
        if key.as_ref().is_some_and(|k| &k[..8] != b"RADK\0\0\0\x01") {
            return Err(Error::Unavailable);
        }
        Ok(key)
    }
}
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
fn id(s: &str) -> Result<(), Error> {
    if s.is_empty()
        || s.len() > 96
        || !s.as_bytes()[0].is_ascii_alphanumeric()
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(Error::InvalidInput);
    }
    Ok(())
}
fn aad(plugin: &str, slot: &str) -> Vec<u8> {
    let mut a = b"RAD credentials v1".to_vec();
    a.extend(1u32.to_be_bytes());
    for s in [plugin, slot] {
        a.extend((s.len() as u16).to_be_bytes());
        a.extend(s.as_bytes());
    }
    a
}
fn envelope(b: &[u8]) -> Result<(), Error> {
    if !(48..=4144).contains(&b.len()) || &b[..8] != b"RADC\0\0\0\x01" {
        return Err(Error::Unavailable);
    }
    Ok(())
}
impl Vault {
    fn has_slots(&self) -> Result<bool, Error> {
        for name in files::entries(&self.dir)? {
            if name == "master.key" || name == "vault.lock" || name.starts_with(".pending-") {
                continue;
            }
            id(&name).map_err(|_| Error::UnsafePath)?;
            let dir = files::directory(&self.dir, &name, false)?.ok_or(Error::Io)?;
            if files::entries(&dir)?
                .iter()
                .any(|n| !n.starts_with(".pending-"))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn state(&self) -> State {
        if self.unknown {
            return State::Unknown;
        }
        if matches!(self.key(), Ok(None)) {
            State::KeyMissing
        } else {
            State::Ready
        }
    }
    fn resolve(&self, plugin: &str, slot: &str) -> Result<Option<Secret>, Error> {
        self.ready()?;
        self.resolve_disk(plugin, slot)
    }
    fn resolve_disk(&self, plugin: &str, slot: &str) -> Result<Option<Secret>, Error> {
        id(plugin)?;
        id(slot)?;
        let Some(dir) = files::directory(&self.dir, plugin, false)? else {
            return Ok(None);
        };
        let Some(b) = files::read(&dir, &format!("{slot}.radc"), 48, 4144)? else {
            return Ok(None);
        };
        envelope(&b)?;
        let k = self.key()?.ok_or(Error::Unavailable)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&k[8..]).map_err(|_| Error::Unavailable)?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&b[8..32]),
                    Payload {
                        msg: &b[32..],
                        aad: &aad(plugin, slot),
                    },
                )
                .map_err(|_| Error::Unavailable)?,
        );
        let value = std::str::from_utf8(&plaintext).map_err(|_| Error::Unavailable)?;
        Secret::new(value.to_owned()).map(Some)
    }
    pub fn status(&self, plugin: &str, slot: &str) -> Result<Status, Error> {
        match self.resolve(plugin, slot) {
            Ok(Some(_)) => Ok(Status::Present),
            Ok(None) => Ok(Status::Absent),
            Err(Error::Unavailable) => Ok(Status::Unavailable),
            Err(e) => Err(e),
        }
    }
    pub fn replace(&mut self, plugin: &str, slot: &str, secret: Secret) -> Result<(), Error> {
        self.ready()?;
        id(plugin)?;
        id(slot)?;
        let dir = files::directory(&self.dir, plugin, true)?.ok_or(Error::Io)?;
        self.quota(&dir, slot, Some(secret.expose().len()))?;
        if self.key()?.is_none() {
            self.create_key()?;
        }
        let k = self.key()?.ok_or(Error::Unavailable)?;
        let mut nonce = [0; 24];
        files::random(&mut nonce)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&k[8..]).map_err(|_| Error::Unavailable)?;
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: secret.expose().as_bytes(),
                    aad: &aad(plugin, slot),
                },
            )
            .map_err(|_| Error::Unavailable)?;
        let mut bytes = b"RADC\0\0\0\x01".to_vec();
        bytes.extend(nonce);
        bytes.extend(ciphertext);
        let result = files::publish(&dir, &format!("{slot}.radc"), &bytes, false);
        self.publication(result)
    }
    fn quota(&self, dir: &File, target: &str, new: Option<usize>) -> Result<(), Error> {
        let mut count = usize::from(new.is_some());
        let mut size = new.unwrap_or(0);
        for name in files::entries(dir)? {
            if name.starts_with(".pending-") {
                continue;
            }
            let slot = name.strip_suffix(".radc").ok_or(Error::Unavailable)?;
            id(slot).map_err(|_| Error::Unavailable)?;
            let bytes = files::read(dir, &name, 48, 4144);
            if slot == target {
                match bytes {
                    Ok(_) | Err(Error::Unavailable) => {}
                    Err(e) => return Err(e),
                }
                continue;
            }
            let bytes = bytes?.ok_or(Error::Unavailable)?;
            envelope(&bytes)?;
            count += 1;
            size += bytes.len() - 48;
        }
        if count > 32 || size > 8192 {
            return Err(Error::Capacity);
        }
        Ok(())
    }
    pub fn remove(&mut self, plugin: &str, slot: &str) -> Result<(), Error> {
        self.ready()?;
        id(plugin)?;
        id(slot)?;
        let Some(dir) = files::directory(&self.dir, plugin, false)? else {
            return Ok(());
        };
        // Explicit deletion cannot increase quota and must remain possible when
        // multiple siblings are corrupt. Validate only the file being removed;
        // replacement still requires safe accounting of every retained sibling.
        if let Some(file) = files::child(&dir, &format!("{slot}.radc"), libc::O_RDONLY)? {
            files::private(&file, false)?;
        }
        let result = files::unlink(&dir, &format!("{slot}.radc"))
            .map_err(|_| Error::Uncertain)
            .and_then(|()| files::sync_directory(&dir));
        self.publication(result)
    }
    pub fn resolve_declared(&self, plugin: &str, slots: &[&str]) -> Result<Secrets, Error> {
        self.ready()?;
        id(plugin)?;
        if slots.len() > 32 {
            return Err(Error::Capacity);
        }
        let mut values = BTreeMap::new();
        let mut size = 0;
        let mut seen = std::collections::BTreeSet::new();
        for slot in slots {
            if !seen.insert(*slot) {
                return Err(Error::InvalidInput);
            }
            match self.resolve(plugin, slot) {
                Ok(Some(secret)) => {
                    size += secret.expose().len();
                    if size > 8192 {
                        return Err(Error::Capacity);
                    }
                    values.insert((*slot).to_owned(), secret);
                }
                Ok(None) | Err(Error::Unavailable) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Secrets(values))
    }
    fn ready(&self) -> Result<(), Error> {
        if self.unknown {
            Err(Error::Uncertain)
        } else {
            Ok(())
        }
    }
    fn publication(&mut self, result: Result<(), Error>) -> Result<(), Error> {
        if result == Err(Error::Uncertain) {
            self.unknown = true
        }
        result
    }
    pub fn reconcile(&mut self) -> Result<(), Error> {
        self.unknown = true;
        for name in files::entries(&self.dir)? {
            if name == "vault.lock" || name.starts_with(".pending-") {
                continue;
            }
            if name == "master.key" {
                if let Some(f) = files::child(&self.dir, &name, libc::O_RDONLY)? {
                    files::private(&f, false)?;
                    files::sync_file(&f)?;
                }
                continue;
            }
            id(&name).map_err(|_| Error::UnsafePath)?;
            let dir = files::directory(&self.dir, &name, false)?.ok_or(Error::Io)?;
            for slot in files::entries(&dir)? {
                if slot.starts_with(".pending-") {
                    continue;
                }
                let slot_id = slot.strip_suffix(".radc").ok_or(Error::Unavailable)?;
                id(slot_id)?;
                if let Some(f) = files::child(&dir, &slot, libc::O_RDONLY)? {
                    files::private(&f, false)?;
                    files::sync_file(&f)?;
                }
                match self.resolve_disk(&name, slot_id) {
                    Ok(_) | Err(Error::Unavailable) => {}
                    Err(e) => return Err(e),
                }
            }
            files::sync_directory(&dir)?;
        }
        files::sync_directory(&self.dir)?;
        // Missing is a recoverable state; an existing invalid key is not.
        self.key()?;
        self.unknown = false;
        Ok(())
    }
}
#[cfg(test)]
mod fault_tests;
