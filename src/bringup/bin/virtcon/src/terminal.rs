// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::colors::ColorScheme;
use anyhow::Error;
use carnelian::Size;
use pty::ServerPty;
use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::rc::Rc;
use terminal::{Scroll, SizeInfo, TerminalCallbacks};
use vt100::Parser;

/// Wrapper around a term model instance and its associated PTY fd.
pub struct Terminal {
    term: Rc<RefCell<Parser<TerminalCallbacks>>>,
    title: String,
    pty: Option<ServerPty>,
    /// Lazily initialized if `pty` is set.
    pty_fd: Option<File>,
}

impl Terminal {
    pub fn new(
        title: String,
        _color_scheme: ColorScheme,
        scrollback_rows: u32,
        pty: Option<ServerPty>,
    ) -> Self {
        let cell_size = Size::new(8.0, 16.0);
        let size_info = SizeInfo {
            width: cell_size.width * 80.0,
            height: cell_size.height * 24.0,
            cell_width: cell_size.width,
            cell_height: cell_size.height,
            padding_x: 0.0,
            padding_y: 0.0,
            dpr: 1.0,
        };
        let columns = (size_info.width / size_info.cell_width) as u16;
        let rows = (size_info.height / size_info.cell_height) as u16;
        let pty_fd = pty.as_ref().and_then(|p| p.try_clone_fd().ok());
        let term_inner = Parser::new_with_callbacks(
            rows,
            columns,
            scrollback_rows as usize,
            TerminalCallbacks::new(pty_fd),
        );
        let term = Rc::new(RefCell::new(term_inner));

        Self { term: Rc::clone(&term), title, pty, pty_fd: None }
    }

    #[cfg(test)]
    fn new_for_test(pty: ServerPty) -> Self {
        Self::new(String::new(), ColorScheme::default(), 1024, Some(pty))
    }

    pub fn clone_term(&self) -> Rc<RefCell<Parser<TerminalCallbacks>>> {
        Rc::clone(&self.term)
    }

    pub fn try_clone(&self) -> Result<Self, Error> {
        let term = self.clone_term();
        let title = self.title.clone();
        let pty = self.pty.clone();
        let pty_fd = None;
        Ok(Self { term, title, pty, pty_fd })
    }

    pub fn resize(&mut self, size_info: &SizeInfo) {
        let mut term = self.term.borrow_mut();
        let columns = (size_info.width / size_info.cell_width) as u16;
        let rows = (size_info.height / size_info.cell_height) as u16;
        term.screen_mut().set_size(rows, columns);
    }

    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    pub fn pty(&self) -> Option<&ServerPty> {
        self.pty.as_ref()
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        let mut term = self.term.borrow_mut();
        scroll.scroll_screen(term.screen_mut());
    }

    pub fn history_size(&self) -> usize {
        let term = self.term.borrow();
        term.screen().scrollback_len()
    }

    pub fn display_offset(&self) -> usize {
        let term = self.term.borrow();
        term.screen().scrollback()
    }

    pub fn mode(&self) -> bool {
        let term = self.term.borrow();
        term.screen().application_cursor()
    }

    fn file(&mut self) -> Result<Option<&mut File>, std::io::Error> {
        if self.pty_fd.is_none() {
            if let Some(pty) = &self.pty {
                let pty_fd = pty.try_clone_fd().map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, format!("{:?}", e))
                })?;
                self.pty_fd = Some(pty_fd);
            }
        }
        Ok(self.pty_fd.as_mut())
    }
}

impl Write for Terminal {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        let fd = self.file()?;
        if let Some(fd) = fd { fd.write(buf) } else { Ok(buf.len()) }
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        let fd = self.file()?;
        if let Some(fd) = fd { fd.flush() } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;

    #[fasync::run_singlethreaded(test)]
    async fn can_create_terminal() -> Result<(), Error> {
        let pty = ServerPty::new()?;
        let _ = Terminal::new_for_test(pty);
        Ok(())
    }
}
