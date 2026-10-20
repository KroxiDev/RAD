//! Test-only delegating VFS. Actual SQLite files and platform I/O, not a mock DB.
//! Registration outlives all connections; each file owns a separately SQLite-allocated
//! native file. File methods delegate using that native pointer, never the wrapper.
use rusqlite::ffi::*;
use std::{
    ffi::{CString, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};
#[derive(Clone, Copy)]
pub enum Event {
    Write = 0,
    Sync = 1,
    Truncate = 2,
    Delete = 3,
    Open = 4,
    Read = 5,
}
pub struct State {
    base: *mut sqlite3_vfs,
    counts: [AtomicUsize; 6],
    event: AtomicUsize,
    nth: AtomicUsize,
    persistent: AtomicUsize,
    after: AtomicUsize,
    pub directory_syncs: AtomicUsize,
}
impl State {
    pub fn arm(&self, event: Event, nth: usize, persistent: bool, after: bool) {
        for n in &self.counts {
            n.store(0, SeqCst);
        }
        self.event.store(event as usize, SeqCst);
        self.nth.store(nth, SeqCst);
        self.persistent.store(persistent as usize, SeqCst);
        self.after.store(after as usize, SeqCst);
    }
    pub fn disarm(&self) {
        self.nth.store(0, SeqCst);
    }
    pub fn count(&self, event: Event) -> usize {
        self.counts[event as usize].load(SeqCst)
    }
    fn hit(&self, event: Event) -> bool {
        let n = self.counts[event as usize].fetch_add(1, SeqCst) + 1;
        let nth = self.nth.load(SeqCst);
        self.event.load(SeqCst) == event as usize
            && nth > 0
            && (n == nth || (n > nth && self.persistent.load(SeqCst) != 0))
    }
    fn invoke(&self, event: Event, operation: impl FnOnce() -> c_int) -> c_int {
        let fail = self.hit(event);
        if fail && self.after.load(SeqCst) == 0 {
            return SQLITE_IOERR;
        }
        let result = operation();
        if fail { SQLITE_IOERR } else { result }
    }
}
pub struct Vfs {
    inner: Box<sqlite3_vfs>,
    name: CString,
    pub state: Box<State>,
}
impl Vfs {
    pub fn new() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(1);
        let name = CString::new(format!("rad-fault-{}", ID.fetch_add(1, SeqCst))).unwrap();
        // SQLite is initialized by rusqlite before this fixture is constructed.
        unsafe {
            let base = sqlite3_vfs_find(ptr::null());
            assert!(!base.is_null());
            let mut state = Box::new(State {
                base,
                counts: std::array::from_fn(|_| AtomicUsize::new(0)),
                event: AtomicUsize::new(0),
                nth: AtomicUsize::new(0),
                persistent: AtomicUsize::new(0),
                after: AtomicUsize::new(0),
                directory_syncs: AtomicUsize::new(0),
            });
            let mut inner = Box::new(*base);
            inner.iVersion = 1;
            inner.szOsFile = std::mem::size_of::<File>() as c_int;
            inner.pNext = ptr::null_mut();
            inner.zName = name.as_ptr();
            inner.pAppData = (&mut *state as *mut State).cast();
            inner.xOpen = Some(open);
            inner.xDelete = Some(delete);
            inner.xAccess = Some(access);
            inner.xFullPathname = Some(full_path);
            inner.xRandomness = Some(random);
            inner.xSleep = Some(sleep);
            inner.xCurrentTime = Some(time);
            inner.xGetLastError = Some(last_error);
            inner.xDlOpen = None;
            inner.xDlError = None;
            inner.xDlSym = None;
            inner.xDlClose = None;
            assert_eq!(sqlite3_vfs_register(&mut *inner, 0), SQLITE_OK);
            Self { inner, name, state }
        }
    }
    pub fn name(&self) -> &str {
        self.name.to_str().unwrap()
    }
}
impl Drop for Vfs {
    fn drop(&mut self) {
        unsafe {
            sqlite3_vfs_unregister(&mut *self.inner);
        }
    }
}
#[repr(C)]
struct File {
    header: sqlite3_file,
    real: *mut sqlite3_file,
    state: *const State,
}
unsafe extern "C" fn open(
    v: *mut sqlite3_vfs,
    name: *const c_char,
    f: *mut sqlite3_file,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    unsafe {
        let state = &*((*v).pAppData.cast::<State>());
        (*f).pMethods = ptr::null();
        let real = sqlite3_malloc64((*state.base).szOsFile as u64).cast::<sqlite3_file>();
        if real.is_null() {
            return SQLITE_NOMEM;
        }
        ptr::write_bytes(real.cast::<u8>(), 0, (*state.base).szOsFile as usize);
        let result = state.invoke(Event::Open, || {
            ((*state.base).xOpen.unwrap())(state.base, name, real, flags, out)
        });
        if result != SQLITE_OK {
            if !(*real).pMethods.is_null() {
                ((*(*real).pMethods).xClose.unwrap())(real);
            }
            sqlite3_free(real.cast());
            return result;
        }
        ptr::write(
            f.cast::<File>(),
            File {
                header: sqlite3_file { pMethods: &METHODS },
                real,
                state,
            },
        );
        SQLITE_OK
    }
}
unsafe extern "C" fn close(f: *mut sqlite3_file) -> c_int {
    unsafe {
        let file = &mut *f.cast::<File>();
        let result = ((*(*file.real).pMethods).xClose.unwrap())(file.real);
        sqlite3_free(file.real.cast());
        file.header.pMethods = ptr::null();
        result
    }
}
macro_rules! delegate {
    ($name:ident,$field:ident,($($arg:ident:$ty:ty),*))=>{
        unsafe extern "C" fn $name(f:*mut sqlite3_file,$($arg:$ty),*)->c_int {unsafe {
            let file=&*f.cast::<File>();((*(*file.real).pMethods).$field.unwrap())(file.real,$($arg),*)
        }}
    };
}
unsafe extern "C" fn read(f: *mut sqlite3_file, b: *mut c_void, n: c_int, o: i64) -> c_int {
    unsafe {
        let file = &*f.cast::<File>();
        (&*file.state).invoke(Event::Read, || {
            ((*(*file.real).pMethods).xRead.unwrap())(file.real, b, n, o)
        })
    }
}
delegate!(file_size,xFileSize,(size:*mut i64));
delegate!(lock,xLock,(level:c_int));
delegate!(unlock,xUnlock,(level:c_int));
delegate!(reserved,xCheckReservedLock,(value:*mut c_int));
delegate!(control,xFileControl,(op:c_int,value:*mut c_void));
delegate!(sector, xSectorSize, ());
delegate!(characteristics, xDeviceCharacteristics, ());
unsafe extern "C" fn write(f: *mut sqlite3_file, b: *const c_void, n: c_int, o: i64) -> c_int {
    unsafe {
        let file = &*f.cast::<File>();
        (&*file.state).invoke(Event::Write, || {
            ((*(*file.real).pMethods).xWrite.unwrap())(file.real, b, n, o)
        })
    }
}
unsafe extern "C" fn sync(f: *mut sqlite3_file, flags: c_int) -> c_int {
    unsafe {
        let file = &*f.cast::<File>();
        (&*file.state).invoke(Event::Sync, || {
            ((*(*file.real).pMethods).xSync.unwrap())(file.real, flags)
        })
    }
}
unsafe extern "C" fn truncate(f: *mut sqlite3_file, size: i64) -> c_int {
    unsafe {
        let file = &*f.cast::<File>();
        (&*file.state).invoke(Event::Truncate, || {
            ((*(*file.real).pMethods).xTruncate.unwrap())(file.real, size)
        })
    }
}
unsafe extern "C" fn delete(v: *mut sqlite3_vfs, name: *const c_char, sync: c_int) -> c_int {
    unsafe {
        let s = &*((*v).pAppData.cast::<State>());
        if sync != 0 {
            s.directory_syncs.fetch_add(1, SeqCst);
        }
        let fail = s.hit(Event::Delete);
        if fail && s.after.load(SeqCst) == 0 {
            return SQLITE_IOERR;
        }
        // Fault-after performs the unlink but withholds requested directory sync.
        // This models its failure, not a successful durable delete with a fake DB.
        let real_sync = if fail { 0 } else { sync };
        let result = ((*s.base).xDelete.unwrap())(s.base, name, real_sync);
        if fail { SQLITE_IOERR } else { result }
    }
}
macro_rules! vdelegate {
    ($name:ident,$field:ident,($($arg:ident:$ty:ty),*))=>{
        unsafe extern "C" fn $name(v:*mut sqlite3_vfs,$($arg:$ty),*)->c_int {unsafe {
            let s=&*((*v).pAppData.cast::<State>());((*s.base).$field.unwrap())(s.base,$($arg),*)
        }}
    };
}
vdelegate!(access,xAccess,(name:*const c_char,flags:c_int,out:*mut c_int));
vdelegate!(full_path,xFullPathname,(name:*const c_char,n:c_int,out:*mut c_char));
vdelegate!(random,xRandomness,(n:c_int,out:*mut c_char));
vdelegate!(sleep,xSleep,(us:c_int));
vdelegate!(time,xCurrentTime,(out:*mut f64));
vdelegate!(last_error,xGetLastError,(n:c_int,out:*mut c_char));
static METHODS: sqlite3_io_methods = sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(close),
    xRead: Some(read),
    xWrite: Some(write),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(file_size),
    xLock: Some(lock),
    xUnlock: Some(unlock),
    xCheckReservedLock: Some(reserved),
    xFileControl: Some(control),
    xSectorSize: Some(sector),
    xDeviceCharacteristics: Some(characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};
