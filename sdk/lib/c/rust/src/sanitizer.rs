// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ffi::{CStr, c_char, c_size_t};
use zx::sys::{ZX_LOG_RECORD_DATA_MAX, zx_handle_t};
use zx::{NullableHandle, Vmo};

// <zircon/sanitizer.h>
unsafe extern "C" {
    fn __sanitizer_log_write(string: *const c_char, len: c_size_t) -> ();

    fn __sanitizer_publish_data(sink_name: *const c_char, vmo: zx_handle_t) -> zx_handle_t;

    fn __sanitizer_fast_backtrace(pc_buffer: *mut usize, max_frames: c_size_t) -> c_size_t;
}

/// Write logging information from the sanitizer runtime.  The string is
/// expected to be printable text with '\n' ending each line.  Timestamps and
/// globally unique identifiers of the calling process and thread (zx::Koid)
/// are attached to all messages, so there is no need to include those details
/// in the text.  The log of messages written with this call automatically
/// includes address and ELF build ID details of the program and all shared
/// libraries sufficient to translate raw address values into program symbols
/// or source locations via a post-processor that has access to the original
/// ELF files and their debugging information.  The text can contain markup
/// around address values that should be resolved symbolically.
pub fn log(string: &str) {
    // SAFETY: Basic ffi call.
    unsafe { __sanitizer_log_write(string.as_ptr() as *const c_char, string.len() as c_size_t) }
}

/// Runtimes that have binary data to publish (e.g. coverage) use this
/// interface.  The name describes the data sink that will receive this blob of
/// data; the string is not used after this call returns.  The caller creates a
/// VMO and passes it in.  Each particular data sink has its own conventions
/// about both the format of the data in the VMO and the protocol for when data
/// must be written there.  For some sinks, the VMO's data is used immediately.
/// For other sinks, the caller is expected to have the VMO mapped in and be
/// writing more data there throughout the life of the process, to be analyzed
/// only after the process terminates.  Yet others might use an asynchronous
/// shared memory protocol between producer and consumer.  The return value is
/// either the null handle or a Zircon handle whose lifetime is used to signal
/// the readiness of the data in the VMO.  This handle can be dropped to
/// indicate the data is ready to be consumed.  Or the handle can safely be
/// leaked; the data will be ready when the process exits.  Note there is no
/// indication of success or failure returned here (though it may be logged).
/// A null handle return value merely indicates there is no way to communicate
/// data readiness before process exit.
pub fn publish_data(sink_name: &CStr, vmo: Vmo) -> NullableHandle {
    // SAFETY: Basic ffi call.
    unsafe {
        let h = __sanitizer_publish_data(sink_name.as_ptr(), vmo.into_raw());
        NullableHandle::from_raw(h)
    }
}

/// This does a fast, best-effort attempt to collect a backtrace.  It writes PC
/// values (return addresses) into the pc_buffer, and returns the subslice of
/// frames collected.  The first frame (pc_buffer[0]) will be fast_backtrace()
/// itself (and that's the only frame guaranteed to be collected), the second
/// will be that frame's caller, and so on.  This is safe even if register and
/// memory state is bogus.  It's best-effort; results will be imprecise in the
/// face of code that doesn't use either shadow-call-stack or frame pointers.
pub fn fast_backtrace(pc_buffer: &mut [usize]) -> &mut [usize] {
    // SAFETY: Basic ffi call.
    unsafe {
        let n = __sanitizer_fast_backtrace(pc_buffer.as_mut_ptr(), pc_buffer.len());
        &mut pc_buffer[0..n]
    }
}

/// This is an ephemeral object that implements the core::fmt::Write trait.
/// It's used as `write!(&zx_libc::sanitizer::Log::new(), "...", ...)` to send
/// a single logging line.  The object holds a fixed buffer that is used to
/// collect the multiple fragments from formatters; it's written using
/// `zx_libc::sanitizer::log()` when the buffer fills or the object is dropped.
#[derive(Debug)]
pub struct Log {
    buffer: [u8; ZX_LOG_RECORD_DATA_MAX],
    used: usize,
}

impl Log {
    pub fn new() -> Log {
        Log { buffer: [0; _], used: 0 }
    }

    fn space(&self) -> usize {
        self.buffer.len() - self.used
    }

    fn flush(&mut self) {
        let buf = &self.buffer[0..self.used];
        self.used = 0;

        // SAFETY: The string was vetted on the way into the buffer.
        let s = unsafe { str::from_utf8_unchecked(buf) };
        if !s.is_empty() {
            log(s)
        }
    }
}

impl Drop for Log {
    fn drop(&mut self) {
        self.flush()
    }
}

impl core::fmt::Write for Log {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let mut left = s;
        while !left.is_empty() {
            // Find a newline in `left` that fits within the remaining space in
            // the buffer. If found, chunk up to the newline. Otherwise, take
            // as many full UTF-8 code points as can fit in the remaining space.
            let (to_copy, has_newline) = match left.find('\n') {
                Some(pos) if pos <= self.space() => (pos, true),
                _ => (left.floor_char_boundary(self.space()), false),
            };

            // If no characters fit and there is no newline to complete a line,
            // flush the buffer to make room.
            if to_copy == 0 && !has_newline {
                if self.used == 0 {
                    // Pathological case with no valid UTF-8 chars at all.
                    return Err(core::fmt::Error);
                }
                self.flush();
                continue;
            }

            // Copy the chunk into the buffer.
            let (chunk, rest) = left.split_at(to_copy);
            let buf = &mut self.buffer[self.used..self.used + to_copy];
            buf.copy_from_slice(chunk.as_bytes());
            self.used += to_copy;

            if has_newline {
                // Skip the newline character and flush the completed log line.
                left = &rest[1..];
                self.flush();
            } else {
                left = rest;
                // If the buffer is now full, flush it to free up space for
                // subsequent writes.
                if self.space() == 0 {
                    self.flush();
                }
            }
        }
        Ok(())
    }
}
