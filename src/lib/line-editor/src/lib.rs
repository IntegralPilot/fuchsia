// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A line editing library providing an idiomatic Rust interface (`Editor`, `History`, `Config`,
//! `Color`, `Hint`, autocompletion, and multiline mode) derived from `linenoise`.

mod config;
mod control;
mod editor;
mod history;
mod io;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use config::{
    ColumnWidth, Config, DEFAULT_COLUMN_COUNT, DEFAULT_MAX_HISTORY_LEN, DEFAULT_MAX_LINE_LEN,
    OperatingMode, TerminalCapability, TerminalMode, terminal_capability_from_name,
};
pub use editor::Editor;
pub use history::History;
pub use io::UnbufferedStdout;
pub use types::{Color, CompletionHandler, Hint, HintHandler, ReadlineError};

use bstr::BStr;

/// Reads a line from stdin using default configuration.
pub fn readline(prompt: impl AsRef<BStr>) -> Result<bstr::BString, ReadlineError> {
    let mut editor = Editor::new();
    editor.readline(prompt)
}
