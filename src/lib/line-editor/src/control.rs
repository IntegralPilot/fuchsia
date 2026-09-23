// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::Color;
use std::io::{Read, Write};

pub(crate) const KEY_CTRL_A: u8 = 1;
pub(crate) const KEY_CTRL_B: u8 = 2;
pub(crate) const KEY_CTRL_C: u8 = 3;
pub(crate) const KEY_CTRL_D: u8 = 4;
pub(crate) const KEY_CTRL_E: u8 = 5;
pub(crate) const KEY_CTRL_F: u8 = 6;
pub(crate) const KEY_CTRL_H: u8 = 8;
pub(crate) const KEY_TAB: u8 = 9;
pub(crate) const KEY_ENTER: u8 = 10;
pub(crate) const KEY_CTRL_K: u8 = 11;
pub(crate) const KEY_CTRL_L: u8 = 12;
pub(crate) const KEY_CARRIAGE_RETURN: u8 = 13;
pub(crate) const KEY_CTRL_N: u8 = 14;
pub(crate) const KEY_CTRL_P: u8 = 16;
pub(crate) const KEY_CTRL_T: u8 = 20;
pub(crate) const KEY_CTRL_U: u8 = 21;
pub(crate) const KEY_CTRL_W: u8 = 23;
pub(crate) const KEY_ESC: u8 = b'\x1b';
pub(crate) const KEY_BACKSPACE: u8 = 127;
pub(crate) const KEY_DELETE: u8 = 126;

// ANSI Control Sequence Introducer command bytes (`ESC [ ... <cmd>`).
pub(crate) const CMD_CURSOR_UP: u8 = b'A';
pub(crate) const CMD_CURSOR_DOWN: u8 = b'B';
pub(crate) const CMD_CURSOR_RIGHT: u8 = b'C';
pub(crate) const CMD_CURSOR_LEFT: u8 = b'D';
pub(crate) const CMD_CURSOR_END: u8 = b'F';
pub(crate) const CMD_CURSOR_HOME: u8 = b'H';
pub(crate) const CMD_ERASE_DISPLAY: u8 = b'J';
pub(crate) const CMD_ERASE_LINE: u8 = b'K';
pub(crate) const CMD_SELECT_GRAPHIC_RENDITION: u8 = b'm';
pub(crate) const CMD_DEVICE_STATUS_REPORT: u8 = b'n';

// Semantic argument constants for control sequences.
pub(crate) const CSI_PARAM_OR_INTERMEDIATE_BYTES: std::ops::RangeInclusive<u8> = b' '..=b'?';
pub(crate) const CSI_FINAL_BYTES: std::ops::RangeInclusive<u8> = b'@'..=b'~';

const STATUS_REPORT_CURSOR_POSITION: usize = 6;
const ERASE_ENTIRE_DISPLAY: usize = 2;
const ERASE_TO_END_OF_LINE: usize = 0;
const SGR_RESET: usize = 0;
const SGR_DEFAULT_FOREGROUND: usize = 37;
const SGR_DEFAULT_BACKGROUND: usize = 49;

/// A stack-allocated builder for ANSI/VT100 control sequences (`ESC [ ...`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ControlSequenceBuilder {
    buf: [u8; 32],
    len: usize,
}

impl ControlSequenceBuilder {
    /// Creates a new control sequence builder starting with `KEY_ESC` and `'['`.
    pub(crate) const fn new() -> Self {
        Self::empty().control_sequence_introducer()
    }

    /// Creates an empty builder with no initial bytes.
    pub(crate) const fn empty() -> Self {
        Self { buf: [0u8; 32], len: 0 }
    }

    /// Appends the Control Sequence Introducer bytes (`KEY_ESC` and `'['`).
    pub(crate) const fn control_sequence_introducer(mut self) -> Self {
        self.buf[self.len] = KEY_ESC;
        self.buf[self.len + 1] = b'[';
        self.len += 2;
        self
    }

    /// Appends a single raw byte to the builder.
    pub(crate) const fn push_byte(mut self, byte: u8) -> Self {
        self.buf[self.len] = byte;
        self.len += 1;
        self
    }

    /// Appends a numeric parameter to the sequence, automatically inserting a `;` separator if
    /// needed.
    pub(crate) const fn arg(mut self, mut val: usize) -> Self {
        if self.len >= 2 && self.buf[self.len - 1] != b'[' && self.buf[self.len - 1] != KEY_ESC {
            self.buf[self.len] = b';';
            self.len += 1;
        }
        if val == 0 {
            self.buf[self.len] = b'0';
            self.len += 1;
            return self;
        }
        let mut digits = [0u8; 20];
        let mut n = 0;
        while val > 0 {
            digits[n] = b'0' + ((val % 10) as u8);
            val /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.buf[self.len] = digits[n];
            self.len += 1;
        }
        self
    }

    /// Finishes building the control sequence by appending the command byte.
    pub(crate) const fn cmd(mut self, command: u8) -> Self {
        self.buf[self.len] = command;
        self.len += 1;
        self
    }

    /// Returns the constructed control sequence as a byte slice.
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Writes the constructed control sequence to the given writer.
    pub(crate) fn write<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(self.as_slice())
    }
}

/// Reads a single byte from a reader stream.
///
/// Returns `Ok(Some(byte))` if a byte was read, `Ok(None)` if EOF was encountered,
/// or an `Err` on I/O failure.
pub(crate) fn read_byte<R: Read>(reader: &mut R) -> std::io::Result<Option<u8>> {
    let mut byte = [0u8; 1];
    match reader.read(&mut byte) {
        Ok(1) => Ok(Some(byte[0])),
        Ok(_) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Reads two consecutive bytes from a reader stream if available.
///
/// Returns `Ok(Some((b1, b2)))` if two bytes were successfully read,
/// `Ok(None)` if EOF occurred before two bytes could be read, or an `Err` on I/O error.
pub(crate) fn read_bytes_2<R: Read>(reader: &mut R) -> std::io::Result<Option<(u8, u8)>> {
    let Some(b1) = read_byte(reader)? else {
        return Ok(None);
    };
    let Some(b2) = read_byte(reader)? else {
        return Ok(None);
    };
    Ok(Some((b1, b2)))
}

/// Helper to write raw bytes to a writer stream.
pub(crate) fn write_bytes<W: Write>(writer: &mut W, bytes: &[u8]) -> std::io::Result<()> {
    writer.write_all(bytes)
}

const QUERY_CURSOR_POSITION: ControlSequenceBuilder =
    ControlSequenceBuilder::new().arg(STATUS_REPORT_CURSOR_POSITION).cmd(CMD_DEVICE_STATUS_REPORT);

/// Queries the cursor position from an ANSI/VT100 terminal (`\x1b[6n`).
pub(crate) fn write_query_cursor_position<W: Write>(writer: &mut W) -> std::io::Result<()> {
    QUERY_CURSOR_POSITION.write(writer)?;
    writer.flush()
}

const QUERY_COLUMN_WIDTH: ControlSequenceBuilder =
    ControlSequenceBuilder::new().arg(999).cmd(CMD_CURSOR_RIGHT);

/// Queries terminal column width by positioning the cursor far right (`\x1b[999C`).
pub(crate) fn write_query_column_width<W: Write>(writer: &mut W) -> std::io::Result<()> {
    QUERY_COLUMN_WIDTH.write(writer)?;
    writer.flush()
}

const CLEAR_SCREEN: ControlSequenceBuilder = ControlSequenceBuilder::new()
    .cmd(CMD_CURSOR_HOME)
    .control_sequence_introducer()
    .arg(ERASE_ENTIRE_DISPLAY)
    .cmd(CMD_ERASE_DISPLAY);

/// Clears the entire terminal screen and moves the cursor to home position (`\x1b[H\x1b[2J`).
pub(crate) fn write_clear_screen<W: Write>(writer: &mut W) -> std::io::Result<()> {
    CLEAR_SCREEN.write(writer)
}

const CLEAR_TO_EOL: ControlSequenceBuilder =
    ControlSequenceBuilder::new().arg(ERASE_TO_END_OF_LINE).cmd(CMD_ERASE_LINE);

/// Clears from the current cursor position to the end of the line (`\x1b[0K`).
pub(crate) fn write_clear_to_eol<W: Write>(writer: &mut W) -> std::io::Result<()> {
    CLEAR_TO_EOL.write(writer)
}

const RESET_SGR: ControlSequenceBuilder =
    ControlSequenceBuilder::new().arg(SGR_RESET).cmd(CMD_SELECT_GRAPHIC_RENDITION);

/// Resets Graphic Rendition (SGR) formatting attributes (`\x1b[0m`).
pub(crate) fn write_reset_sgr<W: Write>(writer: &mut W) -> std::io::Result<()> {
    RESET_SGR.write(writer)
}

/// Sets Graphic Rendition (SGR) color and bold attributes (`\x1b[<bold>;<color>;49m`).
pub(crate) fn write_sgr_formatting<W: Write>(
    writer: &mut W,
    bold: bool,
    color: Option<Color>,
) -> std::io::Result<()> {
    let bold_flag = if bold { 1 } else { 0 };
    let color_code = color.map(|c| c.to_ansi_code() as usize).unwrap_or(SGR_DEFAULT_FOREGROUND);
    ControlSequenceBuilder::new()
        .arg(bold_flag)
        .arg(color_code)
        .arg(SGR_DEFAULT_BACKGROUND)
        .cmd(CMD_SELECT_GRAPHIC_RENDITION)
        .write(writer)
}

/// Moves the cursor horizontally to a specific column (`\r\x1b[<col>C`).
pub(crate) fn write_move_cursor_column<W: Write>(
    writer: &mut W,
    col: usize,
) -> std::io::Result<()> {
    let mut builder = ControlSequenceBuilder::empty().push_byte(KEY_CARRIAGE_RETURN);
    if col > 0 {
        builder = builder.control_sequence_introducer().arg(col).cmd(CMD_CURSOR_RIGHT);
    }
    builder.write(writer)
}

/// Moves the cursor up by a specified number of rows (`\x1b[<rows>A`).
pub(crate) fn write_move_cursor_up<W: Write>(writer: &mut W, rows: usize) -> std::io::Result<()> {
    if rows == 0 {
        Ok(())
    } else {
        ControlSequenceBuilder::new().arg(rows).cmd(CMD_CURSOR_UP).write(writer)
    }
}

/// Moves the cursor down by a specified number of rows (`\x1b[<rows>B`).
pub(crate) fn write_move_cursor_down<W: Write>(writer: &mut W, rows: usize) -> std::io::Result<()> {
    if rows == 0 {
        Ok(())
    } else {
        ControlSequenceBuilder::new().arg(rows).cmd(CMD_CURSOR_DOWN).write(writer)
    }
}

const CLEAR_LINE_AND_MOVE_UP: ControlSequenceBuilder = ControlSequenceBuilder::empty()
    .push_byte(KEY_CARRIAGE_RETURN)
    .control_sequence_introducer()
    .arg(ERASE_TO_END_OF_LINE)
    .cmd(CMD_ERASE_LINE)
    .control_sequence_introducer()
    .arg(1)
    .cmd(CMD_CURSOR_UP);

/// Erases the current line and moves up one row (`\r\x1b[0K\x1b[1A`).
pub(crate) fn write_clear_line_and_move_up<W: Write>(writer: &mut W) -> std::io::Result<()> {
    CLEAR_LINE_AND_MOVE_UP.write(writer)
}

const ERASE_PREVIOUS_CHAR_UART: ControlSequenceBuilder =
    ControlSequenceBuilder::empty().push_byte(KEY_CTRL_H).push_byte(b' ').push_byte(KEY_CTRL_H);

/// Erases the previous character in UART non-VT100 mode (`\x08 \x08`).
pub(crate) fn write_erase_previous_char_uart<W: Write>(writer: &mut W) -> std::io::Result<()> {
    ERASE_PREVIOUS_CHAR_UART.write(writer)
}
