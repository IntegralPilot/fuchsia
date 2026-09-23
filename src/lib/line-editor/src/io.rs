// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::Write;

/// An unbuffered writer to standard output using the `write(2)` system call directly on
/// `libc::STDOUT_FILENO`.
///
/// Bypasses any userspace buffering (such as `std::io::stdout`'s internal `LineWriter`),
/// matching the unbuffered I/O behavior of `linenoise`.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnbufferedStdout;

impl Write for UnbufferedStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let ret = unsafe {
            libc::write(libc::STDOUT_FILENO, buf.as_ptr() as *const libc::c_void, buf.len())
        };
        if ret < 0 { Err(std::io::Error::last_os_error()) } else { Ok(ret as usize) }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Write for &UnbufferedStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let ret = unsafe {
            libc::write(libc::STDOUT_FILENO, buf.as_ptr() as *const libc::c_void, buf.len())
        };
        if ret < 0 { Err(std::io::Error::last_os_error()) } else { Ok(ret as usize) }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
