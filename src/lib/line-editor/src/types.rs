// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bstr::{BStr, BString};
use std::fmt;

/// Errors that can occur during line reading.
#[derive(Debug)]
pub enum ReadlineError {
    /// End of file reached (e.g., Ctrl-D on an empty line).
    Eof,
    /// Line reading was interrupted (e.g., Ctrl-C).
    Interrupted,
    /// An I/O error occurred while reading or writing.
    Io(std::io::Error),
}

impl fmt::Display for ReadlineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eof => write!(f, "End of file"),
            Self::Interrupted => write!(f, "Interrupted"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
        }
    }
}

impl std::error::Error for ReadlineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ReadlineError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Standard ANSI colors for hint text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Standard Black (ANSI 30)
    Black,
    /// Standard Red (ANSI 31)
    Red,
    /// Standard Green (ANSI 32)
    Green,
    /// Standard Yellow (ANSI 33)
    Yellow,
    /// Standard Blue (ANSI 34)
    Blue,
    /// Standard Magenta (ANSI 35)
    Magenta,
    /// Standard Cyan (ANSI 36)
    Cyan,
    /// Standard White (ANSI 37)
    White,
    /// Custom ANSI color code
    Custom(u8),
}

impl Color {
    /// Converts this color to its ANSI numeric code.
    pub fn to_ansi_code(&self) -> u8 {
        match self {
            Self::Black => 30,
            Self::Red => 31,
            Self::Green => 32,
            Self::Yellow => 33,
            Self::Blue => 34,
            Self::Magenta => 35,
            Self::Cyan => 36,
            Self::White => 37,
            Self::Custom(code) => *code,
        }
    }

    /// Creates a `Color` from an ANSI numeric code.
    pub fn from_ansi_code(code: u8) -> Self {
        match code {
            30 => Self::Black,
            31 => Self::Red,
            32 => Self::Green,
            33 => Self::Yellow,
            34 => Self::Blue,
            35 => Self::Magenta,
            36 => Self::Cyan,
            37 => Self::White,
            c => Self::Custom(c),
        }
    }
}

impl From<u8> for Color {
    fn from(code: u8) -> Self {
        Self::from_ansi_code(code)
    }
}

/// Represents an inline hint displayed alongside the input buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// Text of the hint.
    pub text: BString,
    /// Optional color of the hint text.
    pub color: Option<Color>,
    /// Whether the hint text should be displayed in bold.
    pub bold: bool,
}

impl Hint {
    /// Creates a new hint with default styling (no color, not bold).
    pub fn new(text: impl Into<BString>) -> Self {
        Self { text: text.into(), color: None, bold: false }
    }

    /// Sets the color of the hint.
    pub fn with_color(mut self, color: impl Into<Color>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Sets whether the hint should be displayed in bold.
    pub fn with_bold(mut self, bold: bool) -> Self {
        self.bold = bold;
        self
    }
}

/// Closure type for tab completion handlers.
pub trait CompletionHandler: Fn(&BStr) -> Vec<BString> + Send + Sync {}
impl<F: Fn(&BStr) -> Vec<BString> + Send + Sync> CompletionHandler for F {}

/// Closure type for inline hint handlers.
pub trait HintHandler: Fn(&BStr) -> Option<Hint> + Send + Sync {}
impl<F: Fn(&BStr) -> Option<Hint> + Send + Sync> HintHandler for F {}
