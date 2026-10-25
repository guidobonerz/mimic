//! The OpenCBM library, loaded at run time (nothing links at build time).
//!
//! This is the only module of the crate with `unsafe` code: calling C
//! functions through pointers found in a shared library. Every call
//! passes buffers it owns with their true lengths; the library is kept
//! loaded for as long as the function pointers live (they are fields of
//! the same struct as the [`Library`], which is dropped last).
//!
//! Functions used (OpenCBM's public API, `include/opencbm.h`, all
//! returning 0 on success unless noted): `cbm_driver_open_ex(CBM_FILE *,
//! char *adapter)` (adapter NULL = the configured default),
//! `cbm_driver_close(CBM_FILE)`, `cbm_open(f, dev, secadr, name, len)`,
//! `cbm_close(f, dev, secadr)`, `cbm_listen(f, dev, secadr)`,
//! `cbm_talk(f, dev, secadr)`, `cbm_unlisten(f)`, `cbm_untalk(f)`,
//! `cbm_raw_read(f, buf, size)` and `cbm_raw_write(f, buf, size)` (bytes
//! transferred, < 0 on error), `cbm_exec_command(f, dev, cmd, len)`,
//! `cbm_device_status(f, dev, buf, size)` (the status number, 99 on a
//! driver error), `cbm_identify(f, dev, &type, &type_string)`.
//! `CBM_FILE` is a `HANDLE` on Windows and an `intptr_t` elsewhere: both
//! pointer-sized, passed here as `isize`. Calling convention: `__cdecl` on
//! Windows, the C default elsewhere (`extern "C"`).
//! Source: OpenCBM API documentation (doxygen comments of `opencbm.h` and
//! `lib/cbm.c`, <https://github.com/OpenCBM/OpenCBM>; GPL code, only the
//! interface was used).
#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_uchar, c_void, CString};

use libloading::Library;

use emu_core::media::MediaError;

use super::CbmBus;

type CbmFile = isize;
type DriverOpenEx = unsafe extern "C" fn(*mut CbmFile, *mut c_char) -> c_int;
type DriverClose = unsafe extern "C" fn(CbmFile);
type Addr = unsafe extern "C" fn(CbmFile, c_uchar, c_uchar) -> c_int;
type OpenFile = unsafe extern "C" fn(CbmFile, c_uchar, c_uchar, *const c_void, usize) -> c_int;
type Bus = unsafe extern "C" fn(CbmFile) -> c_int;
type RawRead = unsafe extern "C" fn(CbmFile, *mut c_void, usize) -> c_int;
type RawWrite = unsafe extern "C" fn(CbmFile, *const c_void, usize) -> c_int;
type DeviceStatus = unsafe extern "C" fn(CbmFile, c_uchar, *mut c_void, usize) -> c_int;
type ExecCommand = unsafe extern "C" fn(CbmFile, c_uchar, *const c_void, usize) -> c_int;
type Identify = unsafe extern "C" fn(CbmFile, c_uchar, *mut c_int, *mut *const c_char) -> c_int;

/// Library names tried in order (`MIMIC_OPENCBM` first when set).
fn candidates() -> Vec<String> {
    let mut v: Vec<String> = std::env::var("MIMIC_OPENCBM").into_iter().collect();
    if cfg!(windows) {
        v.push("opencbm.dll".into());
    } else if cfg!(target_os = "macos") {
        v.extend([
            "libopencbm.dylib".into(),
            "/usr/local/lib/libopencbm.dylib".into(),
        ]);
    } else {
        v.extend([
            "libopencbm.so.0".into(),
            "libopencbm.so".into(),
            "/usr/local/lib/libopencbm.so.0".into(),
        ]);
    }
    v
}

struct Api {
    driver_close: DriverClose,
    open: OpenFile,
    close: Addr,
    listen: Addr,
    talk: Addr,
    unlisten: Bus,
    untalk: Bus,
    raw_read: RawRead,
    raw_write: RawWrite,
    device_status: DeviceStatus,
    exec_command: ExecCommand,
    identify: Identify,
}

/// An open OpenCBM driver (the ZoomFloppy or another adapter).
pub struct OpenCbm {
    file: CbmFile,
    api: Api,
    // Declared last: dropped after `Drop::drop` closed the driver, and
    // after the function pointers above are gone.
    _lib: Library,
}

fn sym<T: Copy>(lib: &Library, name: &str) -> Result<T, MediaError> {
    let cname = format!("{name}\0");
    // SAFETY: `T` is the function pointer type of `name` as declared in
    // opencbm.h (see the type aliases above); the pointer is only used
    // while `lib` is loaded (both live in `OpenCbm`).
    unsafe { lib.get::<T>(cname.as_bytes()) }
        .map(|s| *s)
        .map_err(|e| MediaError::Backend(format!("OpenCBM: {name} missing: {e}")))
}

impl OpenCbm {
    /// Load the library and open the driver for `adapter` (e.g.
    /// "xum1541"; `None` = OpenCBM's configured default).
    pub fn open(adapter: Option<&str>) -> Result<Self, MediaError> {
        let mut errors = Vec::new();
        let mut lib = None;
        for name in candidates() {
            // SAFETY: loading OpenCBM runs its initialisers, which only set
            // up the library's own state; the library is a plain C DLL.
            match unsafe { Library::new(&name) } {
                Ok(l) => {
                    lib = Some(l);
                    break;
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        let lib = lib.ok_or_else(|| {
            MediaError::Backend(format!(
                "OpenCBM library not found ({}); install OpenCBM or set MIMIC_OPENCBM to the library's path",
                errors.join("; ")
            ))
        })?;
        let driver_open_ex: DriverOpenEx = sym(&lib, "cbm_driver_open_ex")?;
        let api = Api {
            driver_close: sym(&lib, "cbm_driver_close")?,
            open: sym(&lib, "cbm_open")?,
            close: sym(&lib, "cbm_close")?,
            listen: sym(&lib, "cbm_listen")?,
            talk: sym(&lib, "cbm_talk")?,
            unlisten: sym(&lib, "cbm_unlisten")?,
            untalk: sym(&lib, "cbm_untalk")?,
            raw_read: sym(&lib, "cbm_raw_read")?,
            raw_write: sym(&lib, "cbm_raw_write")?,
            device_status: sym(&lib, "cbm_device_status")?,
            exec_command: sym(&lib, "cbm_exec_command")?,
            identify: sym(&lib, "cbm_identify")?,
        };
        let adapter = adapter
            .map(|a| CString::new(a).map_err(|_| MediaError::Control("bad adapter name".into())))
            .transpose()?;
        let mut file: CbmFile = -1;
        let name_ptr = adapter
            .as_ref()
            .map_or(std::ptr::null_mut(), |c| c.as_ptr().cast_mut());
        // SAFETY: `file` is a valid out pointer; the adapter string is
        // NUL-terminated and outlives the call (the library does not keep
        // it: it copies the name when it splits adapter and port).
        let rc = unsafe { driver_open_ex(&mut file, name_ptr) };
        if rc != 0 {
            return Err(MediaError::Backend(format!(
                "OpenCBM: cbm_driver_open_ex failed ({rc}); is the ZoomFloppy connected and \
                 `cbmctrl detect` working?"
            )));
        }
        Ok(Self {
            file,
            api,
            _lib: lib,
        })
    }
}

fn check(rc: c_int, what: &str) -> Result<(), MediaError> {
    if rc == 0 {
        Ok(())
    } else {
        Err(MediaError::Backend(format!(
            "OpenCBM: {what} failed ({rc})"
        )))
    }
}

impl CbmBus for OpenCbm {
    fn exec_command(&mut self, dev: u8, cmd: &[u8]) -> Result<(), MediaError> {
        if cmd.is_empty() {
            // A zero length would make the library look for a NUL.
            return Err(MediaError::Control("empty drive command".into()));
        }
        // SAFETY: pointer and length of a live, non-empty slice.
        let rc = unsafe { (self.api.exec_command)(self.file, dev, cmd.as_ptr().cast(), cmd.len()) };
        check(rc, "cbm_exec_command")
    }

    fn device_status(&mut self, dev: u8) -> Result<(u8, String), MediaError> {
        let mut buf = [0u8; 64];
        // SAFETY: the library writes at most `buf.len()` bytes, NUL
        // terminated.
        let rc =
            unsafe { (self.api.device_status)(self.file, dev, buf.as_mut_ptr().cast(), buf.len()) };
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        let text = String::from_utf8_lossy(&buf[..end]).trim().to_string();
        Ok((u8::try_from(rc).unwrap_or(99), text))
    }

    fn open(&mut self, dev: u8, secondary: u8, name: &[u8]) -> Result<(), MediaError> {
        if name.is_empty() {
            return Err(MediaError::Control("empty file name".into()));
        }
        // SAFETY: pointer and length of a live, non-empty slice.
        let rc =
            unsafe { (self.api.open)(self.file, dev, secondary, name.as_ptr().cast(), name.len()) };
        check(rc, "cbm_open")
    }

    fn close(&mut self, dev: u8, secondary: u8) -> Result<(), MediaError> {
        // SAFETY: plain values.
        check(
            unsafe { (self.api.close)(self.file, dev, secondary) },
            "cbm_close",
        )
    }

    fn talk(&mut self, dev: u8, secondary: u8) -> Result<(), MediaError> {
        // SAFETY: plain values.
        check(
            unsafe { (self.api.talk)(self.file, dev, secondary) },
            "cbm_talk",
        )
    }

    fn untalk(&mut self) -> Result<(), MediaError> {
        // SAFETY: plain values.
        check(unsafe { (self.api.untalk)(self.file) }, "cbm_untalk")
    }

    fn listen(&mut self, dev: u8, secondary: u8) -> Result<(), MediaError> {
        // SAFETY: plain values.
        check(
            unsafe { (self.api.listen)(self.file, dev, secondary) },
            "cbm_listen",
        )
    }

    fn unlisten(&mut self) -> Result<(), MediaError> {
        // SAFETY: plain values.
        check(unsafe { (self.api.unlisten)(self.file) }, "cbm_unlisten")
    }

    fn raw_read(&mut self, buf: &mut [u8]) -> Result<usize, MediaError> {
        // SAFETY: the library writes at most `buf.len()` bytes into `buf`.
        let rc = unsafe { (self.api.raw_read)(self.file, buf.as_mut_ptr().cast(), buf.len()) };
        usize::try_from(rc)
            .map_err(|_| MediaError::Backend(format!("OpenCBM: cbm_raw_read failed ({rc})")))
    }

    fn raw_write(&mut self, data: &[u8]) -> Result<usize, MediaError> {
        // SAFETY: pointer and length of a live slice; the library reads it.
        let rc = unsafe { (self.api.raw_write)(self.file, data.as_ptr().cast(), data.len()) };
        usize::try_from(rc)
            .map_err(|_| MediaError::Backend(format!("OpenCBM: cbm_raw_write failed ({rc})")))
    }

    fn identify(&mut self, dev: u8) -> Result<(i32, String), MediaError> {
        let mut kind: c_int = -1;
        let mut name: *const c_char = std::ptr::null();
        // SAFETY: both are valid out pointers.
        let rc = unsafe { (self.api.identify)(self.file, dev, &mut kind, &mut name) };
        check(rc, "cbm_identify")?;
        let text = if name.is_null() {
            String::new()
        } else {
            // SAFETY: the library returns a pointer to a static,
            // NUL-terminated type name.
            unsafe { std::ffi::CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        };
        Ok((kind, text))
    }
}

impl Drop for OpenCbm {
    fn drop(&mut self) {
        // SAFETY: `file` came from a successful cbm_driver_open_ex and is
        // closed once.
        unsafe { (self.api.driver_close)(self.file) };
    }
}
