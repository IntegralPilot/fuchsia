// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Default maximum number of history entries retained in memory.
pub const DEFAULT_MAX_HISTORY_LEN: usize = 100;

/// Default maximum length of an input line in bytes.
pub const DEFAULT_MAX_LINE_LEN: usize = 4096;

/// Default terminal column width when auto-detection fails.
pub const DEFAULT_COLUMN_COUNT: usize = 80;

/// Controls how TTY terminal capability detection is performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalMode {
    /// Automatically check `std::io::stdin().is_terminal()` at runtime.
    #[default]
    Auto,
    /// Force TTY mode regardless of ambient `is_terminal()` check.
    Tty,
    /// Force non-TTY mode regardless of ambient `is_terminal()` check.
    NonTty,
}

/// Controls how the terminal window column width is determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnWidth {
    /// Automatically determine width by querying `TIOCGWINSZ` via ioctl, falling back to ANSI
    /// cursor position queries.
    #[default]
    Auto,
    /// Determine width using only ANSI cursor position queries (`\x1b[6n`), bypassing `TIOCGWINSZ`
    /// ioctl.
    AnsiCursor,
    /// Use an explicitly specified fixed column width without querying the terminal.
    Fixed(usize),
}

/// Represents the capability level of a terminal type name (e.g., `TERM` variable or explicit
/// configuration).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalCapability {
    /// Fully supported terminal with ANSI/VT100 capabilities.
    #[default]
    Supported,
    /// Limited UART terminal that requires manual character echoing.
    UartEcho,
    /// Unsupported terminal (e.g., `dumb`, `cons25`, `emacs`) that only supports line reading with
    /// a prompt.
    PromptOnly,
}

/// Represents the resolved concrete operating mode of the line editor.
///
/// `OperatingMode` determines how `line-editor` handles input streams, character echoing, escape
/// sequences, line editing keybindings, and prompt rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingMode {
    /// Non-TTY mode (e.g., input redirected from a file, pipe, or non-interactive script).
    ///
    /// - **When used**: Resolved when standard input is not a terminal stream, or when
    ///   `TerminalMode::NonTty` is explicitly configured.
    /// - **Consequences**: No prompt is printed to the output stream. Escape sequences, raw mode,
    ///   completion, hints, and line-editing keybindings are completely disabled. Input bytes are
    ///   read sequentially until a newline or EOF is reached.
    NonTty,

    /// Prompt-only mode for unsupported or dumb terminals (e.g., `TERM=dumb`, `cons25`, `emacs`).
    ///
    /// - **When used**: Resolved when the terminal capability check determines the terminal does
    ///   not support ANSI/VT100 escape sequences.
    /// - **Consequences**: The prompt is written to the output stream before reading input, but
    ///   ANSI/VT100 control sequences (cursor positioning, line clearing, colorized hints) and raw
    ///   mode editing are disabled. Standard stream line buffering applies.
    PromptOnly,

    /// UART character echo mode (e.g., `TERM=uart` or serial console).
    ///
    /// - **When used**: Resolved when connected to a serial UART console that lacks full ANSI/VT100
    ///   screen manipulation but requires character-by-character echoing.
    /// - **Consequences**: The prompt is printed, and typed characters are echoed back to the
    ///   output stream byte-by-byte. Basic backspace erasing (`\x08 \x08`) is handled, but complex
    ///   VT100 cursor movement and multi-line editing are disabled.
    UartEcho,

    /// Full interactive VT100/ANSI terminal mode.
    ///
    /// - **When used**: Resolved when connected to an interactive TTY terminal supporting
    ///   ANSI/VT100 control sequences (e.g., `xterm`, `vt100`, `screen`, `tmux`).
    /// - **Consequences**: Full raw-mode line editing with VT100 escape sequence handling, cursor
    ///   movement, history navigation, tab autocompletion, inline hints, and multi-line refresh.
    ///   Writes a trailing newline upon line submission.
    Interactive,
}

/// Configuration settings for an [`Editor`](crate::Editor) instance.
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum number of history entries to keep in memory.
    pub max_history_len: usize,
    /// Maximum line length in bytes.
    pub max_line_len: usize,
    /// Enable multi-line editing mode.
    pub multiline_mode: bool,
    /// Mode for TTY terminal detection.
    pub terminal_mode: TerminalMode,
    /// Mode for determining column width.
    pub column_width: ColumnWidth,
    /// Explicit terminal name override (or `None` to check `TERM` env var).
    pub term_name: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_history_len: DEFAULT_MAX_HISTORY_LEN,
            max_line_len: DEFAULT_MAX_LINE_LEN,
            multiline_mode: false,
            terminal_mode: TerminalMode::default(),
            column_width: ColumnWidth::default(),
            term_name: None,
        }
    }
}

impl Config {
    /// Sets the terminal detection mode.
    pub fn with_terminal_mode(mut self, mode: TerminalMode) -> Self {
        self.terminal_mode = mode;
        self
    }

    /// Sets the column width determination mode.
    pub fn with_column_width(mut self, column_width: ColumnWidth) -> Self {
        self.column_width = column_width;
        self
    }

    /// Sets an explicit terminal name override.
    pub fn with_term_name(mut self, name: Option<impl Into<String>>) -> Self {
        self.term_name = name.map(Into::into);
        self
    }

    /// Resolves this configuration specification into a `TerminalCapability` level.
    pub fn resolve_terminal_capability(&self) -> TerminalCapability {
        terminal_capability_from_name(self.term_name.as_deref())
    }

    /// Resolves this configuration specification into a concrete `OperatingMode` for the editor.
    ///
    /// `is_terminal` is invoked only when `self.terminal_mode == TerminalMode::Auto` to resolve
    /// whether the input stream is a TTY.
    pub fn resolve_operating_mode(&self, is_terminal: impl FnOnce() -> bool) -> OperatingMode {
        let is_tty = match self.terminal_mode {
            TerminalMode::Auto => is_terminal(),
            TerminalMode::Tty => true,
            TerminalMode::NonTty => false,
        };

        if !is_tty {
            return OperatingMode::NonTty;
        }

        match self.resolve_terminal_capability() {
            TerminalCapability::PromptOnly => OperatingMode::PromptOnly,
            TerminalCapability::UartEcho => OperatingMode::UartEcho,
            TerminalCapability::Supported => OperatingMode::Interactive,
        }
    }
}

/// Classifies a terminal type name into a `TerminalCapability`.
pub fn terminal_capability_from_name(term: Option<&str>) -> TerminalCapability {
    let term_val = match term {
        Some(t) => t.to_string(),
        None => std::env::var("TERM").unwrap_or_default(),
    };

    if term_val.is_empty() {
        return TerminalCapability::Supported;
    }

    let unsupported = ["dumb", "cons25", "emacs"];
    for name in &unsupported {
        if term_val.eq_ignore_ascii_case(name) {
            return TerminalCapability::PromptOnly;
        }
    }

    if term_val.eq_ignore_ascii_case("uart") {
        return TerminalCapability::UartEcho;
    }

    TerminalCapability::Supported
}
