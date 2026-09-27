//! Calls of C functions (spec §21.2, C80). The library is opened with `dlopen`, the
//! function found with `dlsym`, the first time it is called.
//!
//! The call relies on the C calling conventions of Unix on x86-64 and AArch64: the
//! integer and pointer arguments go, in order, to the integer registers, and the
//! `double` arguments, in order, to the floating-point registers, each kind on its own.
//! So one Rust signature, with every integer register first and every floating-point
//! register after, calls any C function whose arguments are of these kinds; the
//! registers that the function does not take are ignored. A variadic function, such as
//! `printf`, is not supported.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use lion_ir::{ForeignFunction, Type};

/// A value given to C or received from it.
pub enum CValue {
    Int(i64),
    Float(f64),
    /// A text, which lives during the call.
    Text(CString),
}

/// What C gave back, read as the declared type says.
pub enum CResult {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(Option<String>),
    None,
}

#[cfg(all(unix, any(target_arch = "x86_64", target_arch = "aarch64")))]
mod platform {
    use super::*;

    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    const RTLD_NOW: c_int = 2;

    /// The file names tried for a library named `libm`: as written, then with the
    /// suffixes of shared libraries.
    fn file_names(library: &str) -> Vec<String> {
        if library.contains('/') || library.contains(".so") || library.ends_with(".dylib") {
            return vec![library.to_string()];
        }
        let mut names = vec![format!("{library}.so"), format!("{library}.so.6"), format!("{library}.dylib")];
        if !library.starts_with("lib") {
            names.push(format!("lib{library}.so"));
            names.push(format!("lib{library}.dylib"));
        }
        // The C library of macOS holds libc and libm.
        if library == "libc" || library == "libm" {
            names.push("libSystem.dylib".to_string());
        }
        names
    }

    pub fn find(library: &str, name: &str) -> Result<*mut c_void, String> {
        let symbol = CString::new(name).map_err(|_| format!("`{name}` is not a C name"))?;
        for file in file_names(library) {
            let Ok(file) = CString::new(file) else { continue };
            // SAFETY: `dlopen` receives a valid C string; a library, once opened, stays.
            let handle = unsafe { dlopen(file.as_ptr(), RTLD_NOW) };
            if handle.is_null() {
                continue;
            }
            // SAFETY: `handle` is a library that `dlopen` opened.
            let function = unsafe { dlsym(handle, symbol.as_ptr()) };
            if function.is_null() {
                return Err(format!("the library `{library}` has no function `{name}`"));
            }
            return Ok(function);
        }
        Err(format!("the library `{library}` cannot be found"))
    }

    type IntCall =
        unsafe extern "C" fn(i64, i64, i64, i64, i64, i64, f64, f64, f64, f64, f64, f64, f64, f64) -> i64;
    type FloatCall =
        unsafe extern "C" fn(i64, i64, i64, i64, i64, i64, f64, f64, f64, f64, f64, f64, f64, f64) -> f64;

    pub fn call(function: *mut c_void, signature: &ForeignFunction, args: &[CValue]) -> CResult {
        let mut integers = [0i64; 6];
        let mut floats = [0f64; 8];
        let (mut next_integer, mut next_float) = (0, 0);
        for arg in args {
            match arg {
                CValue::Int(value) => {
                    integers[next_integer] = *value;
                    next_integer += 1;
                }
                CValue::Text(text) => {
                    integers[next_integer] = text.as_ptr() as i64;
                    next_integer += 1;
                }
                CValue::Float(value) => {
                    floats[next_float] = *value;
                    next_float += 1;
                }
            }
        }
        let [a, b, c, d, e, f] = integers;
        let [g, h, i, j, k, l, m, n] = floats;
        if signature.ret == Type::Float {
            // SAFETY: the declaration gives the kinds of the arguments and of the value;
            // the checker allowed only kinds that this convention passes (C80).
            let function: FloatCall = unsafe { std::mem::transmute(function) };
            return CResult::Float(unsafe { function(a, b, c, d, e, f, g, h, i, j, k, l, m, n) });
        }
        // SAFETY: as above.
        let function: IntCall = unsafe { std::mem::transmute(function) };
        let value = unsafe { function(a, b, c, d, e, f, g, h, i, j, k, l, m, n) };
        match signature.ret {
            Type::Int => CResult::Int(value),
            // A C `int`: only its 32 bits count.
            Type::Bool => CResult::Bool(value as i32 != 0),
            Type::None => CResult::None,
            _ => {
                let pointer = value as *const c_char;
                if pointer.is_null() {
                    CResult::Text(None)
                } else {
                    // SAFETY: C gave a text, which ends with a zero byte.
                    let text = unsafe { CStr::from_ptr(pointer) };
                    CResult::Text(Some(text.to_string_lossy().into_owned()))
                }
            }
        }
    }
}

#[cfg(not(all(unix, any(target_arch = "x86_64", target_arch = "aarch64"))))]
mod platform {
    use super::*;

    pub fn find(_library: &str, _name: &str) -> Result<*mut c_void, String> {
        Err("this version calls C only on Unix, on x86-64 and AArch64".to_string())
    }

    pub fn call(_function: *mut c_void, _signature: &ForeignFunction, _args: &[CValue]) -> CResult {
        unreachable!("no C function is found on this platform")
    }
}

pub use platform::{call, find};
