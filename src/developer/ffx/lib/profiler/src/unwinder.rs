// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::CString;
use std::os::raw::c_char;

// LINT.IfChange
#[repr(C)]
pub struct FfiFrame {
    pub pc: u64,
    pub sp: u64,
}
// LINT.ThenChange(src/developer/ffx/lib/profiler/sys/unwinder_wrapper.h)

// LINT.IfChange
#[repr(C)]
pub struct FfiUnwinder {
    _private: [u8; 0],
}
// LINT.ThenChange(src/developer/ffx/lib/profiler/sys/unwinder_wrapper.h)

unsafe extern "C" {
    fn ffi_unwinder_new() -> *mut FfiUnwinder;
    fn ffi_unwinder_free(unwinder: *mut FfiUnwinder);
    fn ffi_unwinder_add_memory(unwinder: *mut FfiUnwinder, base: u64, data: *const u8, size: usize);
    fn ffi_unwinder_clear_memory(unwinder: *mut FfiUnwinder);
    fn ffi_unwinder_add_module(
        unwinder: *mut FfiUnwinder,
        load_address: u64,
        file_path: *const c_char,
        file_path_len: usize,
    );
    fn ffi_unwinder_unwind(
        unwinder: *mut FfiUnwinder,
        regs_data: *const u8,
        regs_size: usize,
        output_frames: *mut FfiFrame,
        max_depth: usize,
    ) -> usize;
}

pub struct Unwinder {
    inner: *mut FfiUnwinder,
}

impl Unwinder {
    pub fn new() -> Self {
        let inner = unsafe { ffi_unwinder_new() };
        assert!(!inner.is_null());
        Self { inner }
    }

    pub fn add_module(&self, load_address: u64, file_path: &str) {
        let c_path = CString::new(file_path).unwrap();
        unsafe {
            ffi_unwinder_add_module(
                self.inner,
                load_address,
                c_path.as_ptr(),
                c_path.as_bytes().len(),
            );
        }
    }

    pub fn add_memory(&self, base: u64, data: &[u8]) {
        unsafe {
            ffi_unwinder_add_memory(self.inner, base, data.as_ptr(), data.len());
        }
    }

    pub fn clear_memory(&self) {
        unsafe {
            ffi_unwinder_clear_memory(self.inner);
        }
    }

    /// Unwinds the stack using the configured modules and memory, and the given registers.
    /// `regs_data` must be a valid representation of `zx_thread_state_general_regs_t`.
    pub fn unwind(&self, regs_data: &[u8], max_depth: usize) -> Vec<FfiFrame> {
        let mut frames = Vec::with_capacity(max_depth);
        let count = unsafe {
            ffi_unwinder_unwind(
                self.inner,
                regs_data.as_ptr(),
                regs_data.len(),
                frames.as_mut_ptr(),
                max_depth,
            )
        };
        unsafe {
            frames.set_len(count);
        }
        frames
    }

    /// Decodes packed memory buffers into memory chunks, configured for 1 sample context.
    /// The `sample_memory` buffer is structured as:
    ///   - `regs_size`: 8 bytes
    ///   - `regs_data`: `regs_size` bytes (e.g. 144 bytes for x86_64 zx_thread_state_general_regs_t)
    ///   - A list of chunks, where each chunk is:
    ///       - `base`: 8 bytes
    ///       - `size`: 8 bytes
    ///       - `data`: `size` bytes
    ///
    /// It populates the unwinder memory and returns a reference to the extracted `regs_data` slice.
    pub fn set_sample_context<'a>(
        &self,
        sample_memory: &'a [u8],
    ) -> Result<&'a [u8], &'static str> {
        self.clear_memory();
        if sample_memory.len() < 8 {
            return Err("Sample memory is too small to contain registers size");
        }
        let regs_size = u64::from_le_bytes(sample_memory[0..8].try_into().unwrap()) as usize;
        let mut offset = 8;
        if sample_memory.len() < offset + regs_size {
            return Err("Sample memory is too small to contain registers");
        }

        let regs_data = &sample_memory[offset..offset + regs_size];
        offset += regs_size;

        while offset < sample_memory.len() {
            if offset + 16 > sample_memory.len() {
                return Err("Sample memory chunk header incomplete");
            }
            let base = u64::from_le_bytes(sample_memory[offset..offset + 8].try_into().unwrap());
            offset += 8;
            let size =
                u64::from_le_bytes(sample_memory[offset..offset + 8].try_into().unwrap()) as usize;
            offset += 8;
            if offset + size > sample_memory.len() {
                return Err("Sample memory chunk data incomplete");
            }
            self.add_memory(base, &sample_memory[offset..offset + size]);
            offset += size;
        }

        Ok(regs_data)
    }
}

impl Drop for Unwinder {
    fn drop(&mut self) {
        unsafe {
            ffi_unwinder_free(self.inner);
        }
    }
}
