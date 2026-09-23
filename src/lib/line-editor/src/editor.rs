// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::config::{ColumnWidth, Config, DEFAULT_COLUMN_COUNT, OperatingMode};
use crate::control::{self, *};
use crate::history::{History, HistoryDir};
use crate::io::UnbufferedStdout;
use crate::state::State;
use crate::types::{CompletionHandler, HintHandler, ReadlineError};
use bstr::{BStr, BString, ByteSlice};
use std::io::{IsTerminal, Read, Write};

/// Interactive line editor instance.
pub struct Editor {
    /// Configuration for this editor instance.
    pub config: Config,
    /// History manager.
    pub(crate) history: History,
    pub(crate) completion_handler: Option<Box<dyn CompletionHandler>>,
    pub(crate) hint_handler: Option<Box<dyn HintHandler>>,
    pub(crate) last_cols: std::cell::Cell<usize>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    /// Creates a new editor with default configuration.
    pub fn new() -> Self {
        Self::with_config(Config::default())
    }

    /// Creates a new editor with the specified configuration.
    pub fn with_config(config: Config) -> Self {
        let max_history_len = config.max_history_len;
        Self {
            config,
            history: History::new(max_history_len),
            completion_handler: None,
            hint_handler: None,
            last_cols: std::cell::Cell::new(DEFAULT_COLUMN_COUNT),
        }
    }

    /// Reads a line from stdin/stdout using this editor.
    pub fn readline(&mut self, prompt: impl AsRef<BStr>) -> Result<BString, ReadlineError> {
        let stdin = std::io::stdin();
        let mode = self.config.resolve_operating_mode(|| stdin.is_terminal());
        self.readline_from(stdin.lock(), UnbufferedStdout, mode, prompt)
    }

    /// Reads a line from the provided reader and writer streams using the specified operating mode.
    pub fn readline_from<R: Read, W: Write>(
        &mut self,
        mut reader: R,
        mut writer: W,
        mode: OperatingMode,
        prompt: impl AsRef<BStr>,
    ) -> Result<BString, ReadlineError> {
        let prompt_bstr = prompt.as_ref();
        match mode {
            OperatingMode::Interactive => {
                let res = self.readline_stream(&mut reader, &mut writer, prompt_bstr);
                let _ = writer.write_all(b"\n");
                let _ = writer.flush();
                res
            }
            fallback_mode => {
                self.readline_fallback_mode(reader, writer, fallback_mode, prompt_bstr)
            }
        }
    }

    /// Reads a line directly from raw reader and writer streams in interactive mode.
    pub fn readline_stream<R: Read, W: Write>(
        &mut self,
        reader: &mut R,
        writer: &mut W,
        prompt: &BStr,
    ) -> Result<BString, ReadlineError> {
        let column_count = self.get_columns(reader, writer);
        let mut state = State {
            reader,
            writer,
            buffer: BString::default(),
            prompt,
            prompt_length: prompt.len(),
            cursor_position: 0,
            previous_cursor_position: 0,
            column_count,
            max_rows: 0,
            history_index: 0,
            draft_line: None,
            editor: self,
            render_buf: Vec::with_capacity(512),
        };

        if state.write_all(prompt.as_bytes()).is_err() {
            return Err(ReadlineError::Eof);
        }

        loop {
            let Some(mut c) = control::read_byte(state.reader)? else {
                if state.buffer.is_empty() {
                    return Err(ReadlineError::Eof);
                } else {
                    return Ok(state.buffer);
                }
            };

            let has_completion = state.editor.completion_handler.is_some();
            if c == KEY_TAB && has_completion {
                c = state.complete_line();
                if c == 0 {
                    continue;
                }
            }

            match c {
                KEY_ENTER | KEY_CARRIAGE_RETURN => {
                    if state.editor.config.multiline_mode {
                        state.edit_move_end();
                    }
                    if state.editor.hint_handler.is_some() {
                        let handler = state.editor.hint_handler.take();
                        state.refresh_line();
                        state.editor.hint_handler = handler;
                    }
                    return Ok(state.buffer);
                }
                KEY_CTRL_C => {
                    return Err(ReadlineError::Interrupted);
                }
                KEY_BACKSPACE | KEY_CTRL_H => {
                    state.edit_backspace();
                }
                KEY_CTRL_D => {
                    if !state.buffer.is_empty() {
                        state.delete();
                    } else {
                        return Err(ReadlineError::Eof);
                    }
                }
                KEY_CTRL_T => {
                    if state.cursor_position > 0 && state.cursor_position < state.buffer.len() {
                        state.buffer.swap(state.cursor_position - 1, state.cursor_position);
                        if state.cursor_position != state.buffer.len() - 1 {
                            state.cursor_position += 1;
                        }
                        state.refresh_line();
                    }
                }
                KEY_CTRL_B => {
                    state.edit_move_left();
                }
                KEY_CTRL_F => {
                    state.edit_move_right();
                }
                KEY_CTRL_P => {
                    state.history_next(HistoryDir::Prev);
                }
                KEY_CTRL_N => {
                    state.history_next(HistoryDir::Next);
                }
                KEY_ESC => {
                    if let Some((b1, b2)) = control::read_bytes_2(state.reader)? {
                        state.handle_escape_sequence(b1, b2);
                    }
                }
                KEY_CTRL_U => {
                    state.buffer.clear();
                    state.cursor_position = 0;
                    state.refresh_line();
                }
                KEY_CTRL_K => {
                    state.buffer.truncate(state.cursor_position);
                    state.refresh_line();
                }
                KEY_CTRL_A => {
                    state.edit_move_home();
                }
                KEY_CTRL_E => {
                    state.edit_move_end();
                }
                KEY_CTRL_L => {
                    let _ = control::write_clear_screen(state.writer);
                    state.refresh_line();
                }
                KEY_CTRL_W => {
                    state.edit_delete_prev_word();
                }
                _ => {
                    state.edit_insert(c);
                }
            }
        }
    }

    pub(crate) fn readline_fallback_mode<R: Read, W: Write>(
        &mut self,
        mut reader: R,
        mut writer: W,
        mode: OperatingMode,
        prompt: &BStr,
    ) -> Result<BString, ReadlineError> {
        match mode {
            OperatingMode::PromptOnly | OperatingMode::UartEcho => {
                let _ = writer.write_all(prompt.as_bytes());
                let _ = writer.flush();
            }
            OperatingMode::NonTty => {}
            OperatingMode::Interactive => unreachable!(),
        }

        let mut buf = BString::default();
        let mut hit_newline = false;
        let is_nontty = mode == OperatingMode::NonTty;

        while is_nontty || buf.len() < self.config.max_line_len - 1 {
            let Some(mut ch) = control::read_byte(&mut reader)? else {
                break;
            };

            if mode == OperatingMode::UartEcho {
                if ch == KEY_CARRIAGE_RETURN {
                    continue;
                }
                if ch == KEY_DELETE {
                    ch = KEY_BACKSPACE;
                }
                if ch == KEY_BACKSPACE {
                    if !buf.is_empty() {
                        let _ = control::write_erase_previous_char_uart(&mut writer);
                        let _ = writer.flush();
                        buf.pop();
                    }
                    continue;
                } else {
                    let _ = writer.write_all(&[ch]);
                    let _ = writer.flush();
                }
            }

            if ch == KEY_ENTER {
                hit_newline = true;
                break;
            }
            buf.push(ch);
        }

        if buf.is_empty() && !hit_newline {
            return Err(ReadlineError::Eof);
        }

        while buf.ends_with(b"\n") || buf.ends_with(b"\r") {
            buf.pop();
        }

        Ok(buf)
    }

    /// Registers a completion handler for tab autocompletion.
    pub fn set_completion_handler<H: CompletionHandler + 'static>(&mut self, handler: H) {
        self.completion_handler = Some(Box::new(handler));
    }

    /// Removes any registered completion handler.
    pub fn clear_completion_handler(&mut self) {
        self.completion_handler = None;
    }

    /// Registers a hint handler for inline completions/hints.
    pub fn set_hint_handler<H: HintHandler + 'static>(&mut self, handler: H) {
        self.hint_handler = Some(Box::new(handler));
    }

    /// Removes any registered hint handler.
    pub fn clear_hint_handler(&mut self) {
        self.hint_handler = None;
    }

    /// Adds a line to the editor's history buffer.
    ///
    /// Returns `true` if the entry was added (deduplicating adjacent identical entries).
    pub fn add_history(&mut self, line: impl Into<BString>) -> bool {
        self.history.add(line)
    }

    /// Returns a shared reference to the editor's history manager.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Returns a mutable reference to the editor's history manager.
    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    /// Clears the terminal screen using stdout.
    pub fn clear_screen(&self) -> Result<(), std::io::Error> {
        let mut stdout = UnbufferedStdout;
        self.clear_screen_writer(&mut stdout)
    }

    /// Clears the terminal screen using the provided writer.
    pub fn clear_screen_writer<W: Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        control::write_clear_screen(writer)?;
        writer.flush()
    }

    /// Determines the number of columns in the terminal window.
    pub(crate) fn get_columns<R: Read, W: Write>(&self, reader: &mut R, writer: &mut W) -> usize {
        match self.config.column_width {
            ColumnWidth::Fixed(cols) => cols,
            ColumnWidth::AnsiCursor => self.get_columns_ansi(reader, writer),
            ColumnWidth::Auto => {
                let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
                if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0
                    && ws.ws_col > 0
                {
                    ws.ws_col as usize
                } else {
                    self.get_columns_ansi(reader, writer)
                }
            }
        }
    }

    fn get_columns_ansi<R: Read, W: Write>(&self, reader: &mut R, writer: &mut W) -> usize {
        let start_pos = get_cursor_position(reader, writer);
        if start_pos.is_none() {
            return self.last_cols.get();
        }
        let (_start_row, start_col) = start_pos.unwrap();

        if control::write_query_column_width(writer).is_err() {
            return self.last_cols.get();
        }

        let max_pos = get_cursor_position(reader, writer);
        let cols = match max_pos {
            None => self.last_cols.get(),
            Some((_, col)) => {
                if col <= 1 {
                    self.last_cols.get()
                } else {
                    self.last_cols.set(col);
                    col
                }
            }
        };

        if cols > start_col {
            let seq = format!("\x1b[{}D", cols - start_col);
            let _ = writer.write_all(seq.as_bytes());
            let _ = writer.flush();
        }
        cols
    }
}

fn get_cursor_position<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> Option<(usize, usize)> {
    if control::write_query_cursor_position(writer).is_err() {
        return None;
    }

    let mut buf = [0u8; 32];
    let mut i = 0;
    while i < buf.len() - 1 {
        let Some(byte) = control::read_byte(reader).ok()? else {
            return None;
        };
        buf[i] = byte;
        if buf[i] == b'R' {
            i += 1;
            break;
        }
        i += 1;
    }

    if buf[0] != b'\x1b' || buf[1] != b'[' {
        return None;
    }

    let s = std::str::from_utf8(&buf[2..i - 1]).ok()?;
    let mut parts = s.split(';');
    let row: usize = parts.next()?.parse().ok()?;
    let col: usize = parts.next()?.parse().ok()?;
    Some((row, col))
}
