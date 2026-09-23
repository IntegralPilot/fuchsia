// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bindings;
use std::ffi::{CStr, CString};
use zx::Status;

pub fn find_restricted_symbols(vmo: &zx::Vmo, driver_url: &str) -> Result<Vec<String>, Status> {
    let mut out_symbols: *mut bindings::restricted_symbols = std::ptr::null_mut();
    let mut out_symbols_size: usize = 0;
    let driver_url_c_str = CString::new(driver_url).unwrap();
    // SAFETY: We only look at the out symbols if the status is ok.
    Status::ok(unsafe {
        bindings::restricted_symbols_find(
            vmo.raw_handle(),
            driver_url_c_str.as_ptr(),
            &mut out_symbols,
            &mut out_symbols_size,
        )
    })?;

    let mut symbols = Vec::with_capacity(out_symbols_size);
    for i in 0..out_symbols_size {
        // SAFETY: We can retreieve valid strings for all indicies up until out_symbols_size.
        let symbol = unsafe { CStr::from_ptr(bindings::restricted_symbols_get(out_symbols, i)) };
        symbols.push(symbol.to_str().unwrap().to_string());
    }

    // SAFETY: We can free a pointer retrieved via the find API.
    unsafe {
        bindings::restricted_symbols_free(out_symbols);
    }

    Ok(symbols)
}
