// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::control::{self, *};
use crate::editor::Editor;
use crate::history::HistoryDir;
use crate::types::Color;
use bstr::{BStr, BString, ByteSlice};
use std::io::{Read, Write};

/// Manages active state during an interactive line editing session.
pub(crate) struct State<'a, R: Read, W: Write> {
    pub(crate) reader: &'a mut R,
    pub(crate) writer: &'a mut W,
    pub(crate) buffer: BString,
    pub(crate) prompt: &'a BStr,
    pub(crate) prompt_length: usize,
    pub(crate) cursor_position: usize,
    pub(crate) previous_cursor_position: usize,
    pub(crate) column_count: usize,
    pub(crate) max_rows: usize,
    pub(crate) history_index: usize,
    pub(crate) draft_line: Option<BString>,
    pub(crate) editor: &'a mut Editor,
    pub(crate) render_buf: Vec<u8>,
}

impl<'a, R: Read, W: Write> State<'a, R, W> {
    pub(crate) fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(buf)
    }

    pub(crate) fn refresh_singleline(&mut self) {
        self.render_buf.clear();
        let prompt_len = self.prompt_length;
        let mut buf_str = self.buffer.as_bstr();
        let mut pos = self.cursor_position;

        while pos > 0 && prompt_len + pos >= self.column_count {
            if !buf_str.is_empty() {
                buf_str = &buf_str[1..];
            }
            pos -= 1;
        }
        while !buf_str.is_empty() && prompt_len + buf_str.len() > self.column_count {
            buf_str = &buf_str[..buf_str.len() - 1];
        }

        let _ = control::write_bytes(&mut self.render_buf, b"\r");
        let _ = control::write_bytes(&mut self.render_buf, self.prompt.as_bytes());
        let _ = control::write_bytes(&mut self.render_buf, buf_str.as_bytes());
        self.show_refresh_hints();
        let _ = control::write_clear_to_eol(&mut self.render_buf);
        let _ = control::write_move_cursor_column(&mut self.render_buf, pos + prompt_len);

        let _ = self.writer.write_all(&self.render_buf);
        let _ = self.writer.flush();
    }

    pub(crate) fn refresh_multiline(&mut self) {
        self.render_buf.clear();
        let mut rows = ((self.prompt_length + self.buffer.len() + self.column_count - 1)
            / self.column_count)
            .max(1);
        let rpos = (self.prompt_length + self.previous_cursor_position + self.column_count)
            / self.column_count;
        let rpos2 =
            (self.prompt_length + self.cursor_position + self.column_count) / self.column_count;
        let old_rows = self.max_rows;

        if rows > self.max_rows {
            self.max_rows = rows;
        }

        if old_rows > 0 {
            if rpos < old_rows {
                let _ = control::write_move_cursor_down(&mut self.render_buf, old_rows - rpos);
            }
            for _ in 0..(old_rows - 1) {
                let _ = control::write_clear_line_and_move_up(&mut self.render_buf);
            }
            let _ = control::write_bytes(&mut self.render_buf, b"\r");
            let _ = control::write_clear_to_eol(&mut self.render_buf);
        }

        let _ = control::write_bytes(&mut self.render_buf, self.prompt.as_bytes());
        let _ = control::write_bytes(&mut self.render_buf, self.buffer.as_bytes());
        self.show_refresh_hints();

        if self.cursor_position > 0
            && self.cursor_position == self.buffer.len()
            && (self.prompt_length + self.buffer.len()) % self.column_count == 0
        {
            let _ = control::write_bytes(&mut self.render_buf, b"\n\r");
            let _ = control::write_clear_to_eol(&mut self.render_buf);
            rows += 1;
            if rows > self.max_rows {
                self.max_rows = rows;
            }
        }

        if rows > rpos2 {
            let _ = control::write_move_cursor_up(&mut self.render_buf, rows - rpos2);
        }

        let col = (self.prompt_length + self.cursor_position) % self.column_count;
        let _ = control::write_move_cursor_column(&mut self.render_buf, col);

        self.previous_cursor_position = self.cursor_position;
        let _ = self.writer.write_all(&self.render_buf);
        let _ = self.writer.flush();
    }

    pub(crate) fn refresh_line(&mut self) {
        if self.editor.config.multiline_mode {
            self.refresh_multiline();
        } else {
            self.refresh_singleline();
        }
    }

    fn show_refresh_hints(&mut self) {
        if self.prompt_length + self.buffer.len() < self.column_count {
            if let Some(ref handler) = self.editor.hint_handler {
                if let Some(hint) = handler(self.buffer.as_bstr()) {
                    let max_len = self.column_count - (self.prompt_length + self.buffer.len());
                    let text = if hint.text.len() > max_len {
                        &hint.text[..max_len]
                    } else {
                        &hint.text[..]
                    };

                    let effective_color = match (hint.color, hint.bold) {
                        (Some(color), _) => Some(color),
                        // When bold is set without an explicit color, linenoise defaults to White
                        // (ANSI 37).
                        (None, true) => Some(Color::White),
                        (None, false) => None,
                    };

                    if effective_color.is_some() || hint.bold {
                        let _ = control::write_sgr_formatting(
                            &mut self.render_buf,
                            hint.bold,
                            effective_color,
                        );
                    }
                    let _ = control::write_bytes(&mut self.render_buf, text);
                    if effective_color.is_some() || hint.bold {
                        let _ = control::write_reset_sgr(&mut self.render_buf);
                    }
                }
            }
        }
    }

    pub(crate) fn edit_insert(&mut self, c: u8) {
        if self.buffer.len() < self.editor.config.max_line_len {
            if self.buffer.len() == self.cursor_position {
                self.buffer.push(c);
                self.cursor_position += 1;
                if !self.editor.config.multiline_mode
                    && self.prompt_length + self.buffer.len() < self.column_count
                    && self.editor.hint_handler.is_none()
                {
                    let _ = self.writer.write_all(&[c]);
                    let _ = self.writer.flush();
                } else {
                    self.refresh_line();
                }
            } else {
                self.buffer.insert(self.cursor_position, c);
                self.cursor_position += 1;
                self.refresh_line();
            }
        }
    }

    pub(crate) fn edit_move_left(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            self.refresh_line();
        }
    }

    pub(crate) fn edit_move_right(&mut self) {
        if self.cursor_position < self.buffer.len() {
            self.cursor_position += 1;
            self.refresh_line();
        }
    }

    pub(crate) fn edit_move_home(&mut self) {
        if self.cursor_position != 0 {
            self.cursor_position = 0;
            self.refresh_line();
        }
    }

    pub(crate) fn edit_move_end(&mut self) {
        if self.cursor_position != self.buffer.len() {
            self.cursor_position = self.buffer.len();
            self.refresh_line();
        }
    }

    pub(crate) fn history_next(&mut self, dir: HistoryDir) {
        let history_len = self.editor.history.len();
        if history_len == 0 {
            return;
        }

        match dir {
            HistoryDir::Prev => {
                if self.history_index == 0 {
                    // Save initial draft line before moving into history entries
                    self.draft_line = Some(self.buffer.clone());
                    self.history_index = 1;
                } else if self.history_index < history_len {
                    self.history_index += 1;
                } else {
                    return;
                }
            }
            HistoryDir::Next => {
                if self.history_index == 0 {
                    return;
                }
                self.history_index -= 1;
            }
        }

        if self.history_index == 0 {
            self.buffer = self.draft_line.take().unwrap_or_default();
        } else {
            let entry_idx = history_len - self.history_index;
            if let Some(entry) = self.editor.history.get(entry_idx) {
                self.buffer = entry.clone();
            }
        }

        self.cursor_position = self.buffer.len();
        self.refresh_line();
    }

    pub(crate) fn delete(&mut self) {
        if !self.buffer.is_empty() && self.cursor_position < self.buffer.len() {
            self.buffer.remove(self.cursor_position);
            self.refresh_line();
        }
    }

    pub(crate) fn edit_backspace(&mut self) {
        if self.cursor_position > 0 && !self.buffer.is_empty() {
            self.buffer.remove(self.cursor_position - 1);
            self.cursor_position -= 1;
            self.refresh_line();
        }
    }

    pub(crate) fn edit_delete_prev_word(&mut self) {
        let old_pos = self.cursor_position;
        while self.cursor_position > 0 && self.buffer[self.cursor_position - 1] == b' ' {
            self.cursor_position -= 1;
        }
        while self.cursor_position > 0 && self.buffer[self.cursor_position - 1] != b' ' {
            self.cursor_position -= 1;
        }
        let diff = old_pos - self.cursor_position;
        self.buffer.drain(self.cursor_position..old_pos);
        if diff > 0 {
            self.refresh_line();
        }
    }

    pub(crate) fn complete_line(&mut self) -> u8 {
        let completions = if let Some(ref handler) = self.editor.completion_handler {
            handler(self.buffer.as_bstr())
        } else {
            return 0;
        };

        if completions.is_empty() {
            let _ = self.writer.write_all(b"\x07");
            let _ = self.writer.flush();
            return 0;
        }

        let mut i = 0;

        loop {
            if i < completions.len() {
                let saved_buffer = self.buffer.clone();
                let saved_pos = self.cursor_position;

                self.buffer = completions[i].clone();
                self.cursor_position = self.buffer.len();
                self.refresh_line();

                self.buffer = saved_buffer;
                self.cursor_position = saved_pos;
            } else {
                self.refresh_line();
            }

            let Some(c) = control::read_byte(self.reader).ok().flatten() else {
                return 0;
            };

            match c {
                KEY_TAB => {
                    i = (i + 1) % (completions.len() + 1);
                    if i == completions.len() {
                        let _ = self.writer.write_all(b"\x07");
                        let _ = self.writer.flush();
                    }
                }
                KEY_ESC => {
                    if i < completions.len() {
                        self.refresh_line();
                    }
                    let Some(b1) = control::read_byte(self.reader).ok().flatten() else {
                        return 0;
                    };
                    if b1 == b'[' || b1 == b'O' {
                        let Some(b2) = control::read_byte(self.reader).ok().flatten() else {
                            return 0;
                        };
                        self.handle_escape_sequence(b1, b2);
                        return 0;
                    } else {
                        return b1;
                    }
                }
                _ => {
                    if i < completions.len() {
                        self.buffer = completions[i].clone();
                        self.cursor_position = self.buffer.len();
                    }
                    return c;
                }
            }
        }
    }

    pub(crate) fn handle_escape_sequence(&mut self, b1: u8, b2: u8) {
        if b1 == b'[' {
            if control::CSI_PARAM_OR_INTERMEDIATE_BYTES.contains(&b2) {
                let mut term = 0u8;
                loop {
                    let Some(b) = control::read_byte(self.reader).ok().flatten() else {
                        break;
                    };
                    if control::CSI_FINAL_BYTES.contains(&b) {
                        term = b;
                        break;
                    }
                }
                if term == b'~' && b2 == b'3' {
                    self.delete();
                }
            } else {
                match b2 {
                    control::CMD_CURSOR_UP => self.history_next(HistoryDir::Prev),
                    control::CMD_CURSOR_DOWN => self.history_next(HistoryDir::Next),
                    control::CMD_CURSOR_RIGHT => self.edit_move_right(),
                    control::CMD_CURSOR_LEFT => self.edit_move_left(),
                    control::CMD_CURSOR_HOME => self.edit_move_home(),
                    control::CMD_CURSOR_END => self.edit_move_end(),
                    _ => {}
                }
            }
        } else if b1 == b'O' {
            match b2 {
                control::CMD_CURSOR_HOME => self.edit_move_home(),
                control::CMD_CURSOR_END => self.edit_move_end(),
                _ => {}
            }
        }
    }
}
