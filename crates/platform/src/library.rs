//! Run-time loading of system libraries (`dlopen` on Unix, `LoadLibraryW` on Windows).

use crate::PlatformError;
use std::ffi::{c_void, CString};

/// A shared library opened at run time. Closed on drop.
pub struct Library {
    handle: *mut c_void,
}

impl Library {
    /// Opens the first library in `names` that loads.
    pub fn open(names: &[&str]) -> Result<Library, PlatformError> {
        for name in names {
            let handle = unsafe { sys::open(name) };
            if !handle.is_null() {
                return Ok(Library { handle });
            }
        }
        Err(PlatformError::new(format!(
            "could not load any of {names:?}"
        )))
    }

    /// Looks up a symbol as a raw pointer, or null when the library has no such symbol.
    pub fn symbol_ptr(&self, name: &str) -> *mut c_void {
        let Ok(name) = CString::new(name) else {
            return std::ptr::null_mut();
        };
        unsafe { sys::symbol(self.handle, &name) }
    }

    /// Looks up a function symbol.
    ///
    /// # Safety
    /// `F` must be a function pointer type matching the symbol's real signature.
    pub unsafe fn get<F: Copy>(&self, name: &str) -> Option<F> {
        assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*mut c_void>());
        let ptr = self.symbol_ptr(name);
        if ptr.is_null() {
            None
        } else {
            Some(std::mem::transmute_copy::<*mut c_void, F>(&ptr))
        }
    }

    /// Like [`Library::get`], but fails with an error naming the missing symbol.
    ///
    /// # Safety
    /// As for [`Library::get`].
    pub unsafe fn require<F: Copy>(&self, name: &str) -> Result<F, PlatformError> {
        self.get(name)
            .ok_or_else(|| PlatformError::new(format!("missing symbol {name}")))
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { sys::close(self.handle) };
    }
}

#[cfg(unix)]
mod sys {
    use std::ffi::{c_char, c_int, c_void, CStr, CString};

    const RTLD_NOW: c_int = 2;

    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
    }

    pub unsafe fn open(name: &str) -> *mut c_void {
        match CString::new(name) {
            Ok(name) => dlopen(name.as_ptr(), RTLD_NOW),
            Err(_) => std::ptr::null_mut(),
        }
    }

    pub unsafe fn symbol(handle: *mut c_void, name: &CStr) -> *mut c_void {
        dlsym(handle, name.as_ptr())
    }

    pub unsafe fn close(handle: *mut c_void) {
        dlclose(handle);
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::{c_char, c_void, CStr};

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }

    pub unsafe fn open(name: &str) -> *mut c_void {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        LoadLibraryW(wide.as_ptr())
    }

    pub unsafe fn symbol(handle: *mut c_void, name: &CStr) -> *mut c_void {
        GetProcAddress(handle, name.as_ptr())
    }

    pub unsafe fn close(handle: *mut c_void) {
        FreeLibrary(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn opens_libc_and_finds_a_symbol() {
        let lib = Library::open(&["libc.so.6"]).unwrap();
        assert!(!lib.symbol_ptr("strlen").is_null());
        assert!(lib.symbol_ptr("definitely_not_a_symbol").is_null());
    }

    #[test]
    fn reports_missing_libraries() {
        assert!(Library::open(&["shallot-missing-library"]).is_err());
    }
}
