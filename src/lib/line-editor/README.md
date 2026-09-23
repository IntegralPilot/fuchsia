# line-editor

`line-editor` is a lightweight line-editing library for Fuchsia. It provides
single-line and multi-line terminal line editing, history management, tab
completion, inline hints with ANSI colors, and VT100 terminal escape sequence
keybindings.

## Features

- **Binary String API**: Works natively with `bstr::BString` and `bstr::BStr`
  without requiring UTF-8 validation.
- **Generic Stream Support**: Operates on any `std::io::Read` and
  `std::io::Write` streams, standard I/O (`stdin`/`stdout`), or pipe handles.
- **History Management**: In-memory history with deduplication and maximum entry
  bounds. Callers can manually record history entries using `editor.add_history(...)`.
- **Tab Completion**: Register custom completion handlers (`CompletionHandler`
  or closures) for tab autocompletion.
- **Inline Hints**: Register custom hint callbacks (`HintHandler`) with
  optional ANSI foreground colors (`Color`) and bold formatting.
- **Single-Line & Multi-Line Modes**: Supports horizontal scrolling single-line
  mode as well as automatic multi-line wrapping.
- **VT100 & Emacs Keybindings**:
  - `Ctrl-A` / `Home`: Move to start of line
  - `Ctrl-E` / `End`: Move to end of line
  - `Ctrl-B` / `Left Arrow`: Move cursor left
  - `Ctrl-F` / `Right Arrow`: Move cursor right
  - `Ctrl-P` / `Up Arrow`: Navigate back in history
  - `Ctrl-N` / `Down Arrow`: Navigate forward in history
  - `Ctrl-K`: Kill text to end of line
  - `Ctrl-U`: Clear entire line
  - `Ctrl-W`: Delete previous word
  - `Ctrl-T`: Transpose characters
  - `Ctrl-L`: Clear screen
  - `Ctrl-C`: Interrupt (`ReadlineError::Interrupted`)
  - `Ctrl-D`: EOF on empty line (`ReadlineError::Eof`) or delete character

## Usage

### Simple Standalone Readline

```rust
use line_editor::readline;

fn main() -> Result<(), line_editor::ReadlineError> {
    let line = readline("prompt> ")?;
    println!("Entered: {}", line);
    Ok(())
}
```

### Advanced Usage with `Editor`

```rust
use bstr::{BStr, BString};
use line_editor::{Color, Config, Editor, Hint, ReadlineError};

fn main() -> Result<(), ReadlineError> {
    let mut editor = Editor::with_config(Config {
        max_history_len: 100,
        multiline_mode: true,
        max_line_len: 4096,
        ..Default::default()
    });

    // Register a completion handler
    editor.set_completion_handler(|line: &BStr| {
        if line.starts_with(b"h") {
            vec![BString::from("hello"), BString::from("help")]
        } else {
            vec![]
        }
    });

    // Register a hint handler
    editor.set_hint_handler(|line: &BStr| {
        if line == b"hello" {
            Some(Hint::new(" world").with_color(Color::Green).with_bold(true))
        } else {
            None
        }
    });

    // Read lines from stdin/stdout in an interactive loop
    while let Ok(line) = editor.readline("app> ") {
        if !line.is_empty() {
            // Record non-empty lines in history
            editor.add_history(&line);
        }
        println!("Read line: {}", line);
    }

    Ok(())
}
```

### Managing History

You can access and modify the editor's history directly via `editor.history()`
and `editor.history_mut()`:

```rust
use line_editor::Editor;

let mut editor = Editor::new();
editor.add_history("first command");
editor.add_history("second command");

println!("History count: {}", editor.history().len());
for entry in editor.history().entries() {
    println!("  {}", entry);
}

// Clear all history entries
editor.history_mut().clear();
```

### Reading from Custom Streams

```rust
use line_editor::{Editor, OperatingMode};

fn read_from_stream(mut reader: impl std::io::Read, mut writer: impl std::io::Write) {
    let mut editor = Editor::new();
    let line = editor.readline_from(
        &mut reader,
        &mut writer,
        OperatingMode::Interactive,
        "stream> ",
    );
}
```

## Testing

Run unit tests with `fx`:

```bash
fx test line-editor-tests
```

## Future Work

- **ANSI Escape Codes in Prompt Strings**: Prompts currently compute column width
  using raw byte length (`prompt.len()`). Escape sequences for terminal colors or
  formatting (e.g., `\x1b[32m$ \x1b[0m`) occupy bytes but not visual columns, which
  can misalign cursor positioning calculations. Stripping non-printable escape
  sequences when computing visual prompt length will support styled prompts.
- **UTF-8 Character Navigation**: Line editing currently operates on raw byte
  slices. Multi-byte Unicode code points and grapheme clusters should be handled
  cohesively during cursor movement (Left/Right), backspace/delete operations, and
  display column width calculations (e.g., double-width characters).

