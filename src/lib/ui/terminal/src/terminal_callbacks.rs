// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex};
use vt100::Screen;

/// A standard vt100 callback implementation that writes responses
/// back to a provided PTY file descriptor (such as Device Attributes
/// and Device Status Report).
pub struct TerminalCallbacks {
    pty_fd: Arc<Mutex<Option<File>>>,
}

impl TerminalCallbacks {
    pub fn new(pty_fd: Option<File>) -> Self {
        Self { pty_fd: Arc::new(Mutex::new(pty_fd)) }
    }

    pub fn new_with_shared(pty_fd: Arc<Mutex<Option<File>>>) -> Self {
        Self { pty_fd }
    }
}

impl vt100::Callbacks for TerminalCallbacks {
    fn unhandled_csi(
        &mut self,
        screen: &mut Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if let Some(fd) = self.pty_fd.lock().unwrap().as_mut() {
            if i1 == None && c == 'c' {
                // Primary Device Attributes (DA1)
                let _ = fd.write_all(b"\x1b[?6c");
            } else if i1 == Some(b'>') && c == 'c' {
                // Secondary Device Attributes (DA2)
                let _ = fd.write_all(b"\x1b[>0;278;0c");
            } else if i1 == None && c == 'n' {
                // Device Status Report (DSR)
                let param = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(0);
                if param == 5 {
                    let _ = fd.write_all(b"\x1b[0n");
                } else if param == 6 {
                    let (row, col) = screen.cursor_position();
                    let response = format!("\x1b[{};{}R", row + 1, col + 1);
                    let _ = fd.write_all(response.as_bytes());
                }
            }
        }
    }
}
