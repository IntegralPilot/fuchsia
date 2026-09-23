// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use bstr::{BStr, BString, ByteSlice};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;

fn create_pipe() -> (File, File) {
    let mut fds = [0; 2];
    unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1]))
    }
}

fn test_editor() -> Editor {
    Editor::with_config(
        Config::default()
            .with_terminal_mode(TerminalMode::Tty)
            .with_column_width(ColumnWidth::AnsiCursor)
            .with_term_name(Some("xterm-256color")),
    )
}

#[test]
fn test_readline_error_traits() {
    let err_int = ReadlineError::Interrupted;
    assert_eq!(format!("{}", err_int), "Interrupted");
    assert!(std::error::Error::source(&err_int).is_none());

    let err_eof = ReadlineError::Eof;
    assert_eq!(format!("{}", err_eof), "End of file");
    assert!(std::error::Error::source(&err_eof).is_none());

    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "test error");
    let err_io = ReadlineError::from(io_err);
    assert!(format!("{}", err_io).contains("I/O error"));
    assert!(std::error::Error::source(&err_io).is_some());
}

#[test]
fn test_color_enum() {
    assert_eq!(Color::Red.to_ansi_code(), 31);
    assert_eq!(Color::Green.to_ansi_code(), 32);
    assert_eq!(Color::Yellow.to_ansi_code(), 33);
    assert_eq!(Color::Blue.to_ansi_code(), 34);
    assert_eq!(Color::White.to_ansi_code(), 37);
    assert_eq!(Color::Custom(123).to_ansi_code(), 123);

    assert_eq!(Color::from(31), Color::Red);
    assert_eq!(Color::from(37), Color::White);
    assert_eq!(Color::from(42), Color::Custom(42));
}

#[test]
fn test_hint_struct() {
    let hint = Hint::new("test").with_color(Color::Red).with_bold(true);
    assert_eq!(hint.text, BString::from("test"));
    assert_eq!(hint.color, Some(Color::Red));
    assert_eq!(hint.bold, true);

    let hint_int = Hint::new("test2").with_color(32);
    assert_eq!(hint_int.color, Some(Color::Green));
}

#[test]
fn test_config_struct() {
    let cfg = Config::default();
    assert_eq!(cfg.max_history_len, DEFAULT_MAX_HISTORY_LEN);
    assert_eq!(cfg.multiline_mode, false);
    assert_eq!(cfg.max_line_len, DEFAULT_MAX_LINE_LEN);
    assert_eq!(cfg.terminal_mode, TerminalMode::Auto);
    assert_eq!(cfg.column_width, ColumnWidth::Auto);
}

#[test]
fn test_config_builder() {
    let cfg = Config::default()
        .with_terminal_mode(TerminalMode::Tty)
        .with_term_name(Some("xterm"))
        .with_column_width(ColumnWidth::Fixed(120));
    assert_eq!(cfg.terminal_mode, TerminalMode::Tty);
    assert_eq!(cfg.term_name.as_deref(), Some("xterm"));
    assert_eq!(cfg.column_width, ColumnWidth::Fixed(120));
}

#[test]
fn test_history_operations() {
    let mut history = History::new(3);
    assert_eq!(history.entries().len(), 0);

    assert!(history.add("first"));
    assert!(history.add("second"));
    assert!(!history.add("second")); // Duplicate ignored
    assert!(history.add("third"));
    assert_eq!(history.entries().len(), 3);

    assert!(history.add("fourth")); // Evicts "first"
    assert_eq!(
        history.entries(),
        &[BString::from("second"), BString::from("third"), BString::from("fourth")]
    );

    let mut zero_history = History::new(0);
    assert!(!zero_history.add("test"));
}

#[test]
fn test_editor_creation_and_handlers() {
    let mut editor = Editor::new();
    editor.set_completion_handler(|line: &BStr| {
        if line == b"f" { vec![BString::from("foo"), BString::from("bar")] } else { vec![] }
    });

    editor.set_hint_handler(|line: &BStr| {
        if line == b"foo" { Some(Hint::new("bar").with_color(32)) } else { None }
    });

    assert!(editor.completion_handler.is_some());
    assert!(editor.hint_handler.is_some());

    editor.clear_completion_handler();
    editor.clear_hint_handler();

    assert!(editor.completion_handler.is_none());
    assert!(editor.hint_handler.is_none());
}

#[test]
fn test_editor_history_public_api() {
    let mut editor = Editor::new();
    assert!(editor.history().is_empty());
    assert!(editor.add_history("cmd 1"));
    assert!(editor.add_history("cmd 2"));
    assert_eq!(editor.history().len(), 2);
    assert_eq!(editor.history().entries(), &[BString::from("cmd 1"), BString::from("cmd 2")]);

    editor.history_mut().clear();
    assert!(editor.history().is_empty());
}

#[test]
fn test_readline_long_prompt() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let drain_handle = std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = r_out.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    });

    let handle = std::thread::spawn(move || {
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(TerminalMode::Tty)
                .with_column_width(ColumnWidth::Fixed(80))
                .with_term_name(Some("xterm-256color")),
        );
        let long_prompt = "P".repeat(200);
        let mode = editor.config.resolve_operating_mode(|| true);
        editor.readline_from(&mut r_in, &mut w_out, mode, long_prompt.as_bytes().as_bstr())
    });

    w_in.write_all(b"hello\n").unwrap();

    let res = handle.join().unwrap();
    let _ = drain_handle.join();
    assert_eq!(res.ok(), Some(BString::from("hello")));
}

#[test]
fn test_line_editing_keybindings() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    // Type "wordl", Left (2), Transpose (20) -> "world", End (5), Enter (10)
    let _ = w_in.write_all(&[
        b'w', b'o', b'r', b'd', b'l', 2, 20, 5, 10, // Enter
    ]);

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("world")));
}

#[test]
fn test_escape_sequences_and_history_navigation() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.history.add("first_cmd");
        editor.history.add("second_cmd");
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    // Up Arrow (ESC [ A), Down Arrow (ESC [ B), Up Arrow (ESC [ A), Up Arrow (ESC [ A), Enter
    let _ = w_in.write_all(b"\x1b[A\x1b[B\x1b[A\x1b[A\n");

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("first_cmd")));
}

#[test]
fn test_history_draft_line_preservation() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.history.add("prev_cmd1");
        editor.history.add("prev_cmd2");
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    // Type partial draft "draft_command", navigate up through history, navigate back down to draft,
    // append " _appended", and submit.
    let _ = w_in.write_all(b"draft_command\x1b[A\x1b[A\x1b[B\x1b[B _appended\n");

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("draft_command _appended")));
}

#[test]
fn test_ctrl_c_and_ctrl_d() {
    // Test Ctrl-C
    {
        let (mut r_in, mut w_in) = create_pipe();
        let (mut r_out, mut w_out) = create_pipe();

        let handle = std::thread::spawn(move || {
            let mut editor = test_editor();
            editor.readline_from(
                &mut r_in,
                &mut w_out,
                OperatingMode::Interactive,
                b"prompt> ".as_bstr(),
            )
        });

        let mut buf = [0u8; 32];
        let _ = r_out.read(&mut buf[..4]);
        let _ = w_in.write_all(b"\x1b[10;80R");
        let _ = r_out.read(&mut buf[..6]);
        let _ = r_out.read(&mut buf[..4]);
        let _ = w_in.write_all(b"\x1b[10;80R");

        let _ = w_in.write_all(&[3]); // Ctrl-C

        let res = handle.join().unwrap();
        assert!(matches!(res, Err(ReadlineError::Interrupted)));
    }

    // Test Ctrl-D on empty line
    {
        let (mut r_in, mut w_in) = create_pipe();
        let (mut r_out, mut w_out) = create_pipe();

        let handle = std::thread::spawn(move || {
            let mut editor = test_editor();
            editor.readline_from(
                &mut r_in,
                &mut w_out,
                OperatingMode::Interactive,
                b"prompt> ".as_bstr(),
            )
        });

        let mut buf = [0u8; 32];
        let _ = r_out.read(&mut buf[..4]);
        let _ = w_in.write_all(b"\x1b[10;80R");
        let _ = r_out.read(&mut buf[..6]);
        let _ = r_out.read(&mut buf[..4]);
        let _ = w_in.write_all(b"\x1b[10;80R");

        let _ = w_in.write_all(&[4]); // Ctrl-D

        let res = handle.join().unwrap();
        assert!(matches!(res, Err(ReadlineError::Eof)));
    }

    // Test stream EOF on empty line
    {
        let (mut r_in, w_in) = create_pipe();
        let (mut r_out, mut w_out) = create_pipe();

        let handle = std::thread::spawn(move || {
            let mut editor = test_editor();
            editor.readline_from(
                &mut r_in,
                &mut w_out,
                OperatingMode::Interactive,
                b"prompt> ".as_bstr(),
            )
        });

        let mut buf = [0u8; 32];
        let _ = r_out.read(&mut buf[..4]);
        let mut w_in = w_in;
        let _ = w_in.write_all(b"\x1b[10;80R");
        let _ = r_out.read(&mut buf[..6]);
        let _ = r_out.read(&mut buf[..4]);
        let _ = w_in.write_all(b"\x1b[10;80R");

        // Close write end of pipe without writing any keys
        drop(w_in);

        let res = handle.join().unwrap();
        assert!(matches!(res, Err(ReadlineError::Eof)));
    }
}

#[test]
fn test_multiline_mode() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = Editor::with_config(
            Config { multiline_mode: true, ..Config::default() }
                .with_terminal_mode(TerminalMode::Tty)
                .with_term_name(Some("xterm-256color"))
                .with_column_width(ColumnWidth::AnsiCursor),
        );
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    let _ = w_in.write_all(b"multiline_input\n");

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("multiline_input")));
}

#[test]
fn test_completion_and_hints() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.set_completion_handler(|line: &BStr| {
            if line.starts_with(b"h") {
                vec![BString::from("hello"), BString::from("help")]
            } else {
                vec![]
            }
        });
        editor.set_hint_handler(|line: &BStr| {
            if line == b"hello" {
                Some(Hint::new(" world").with_color(33).with_bold(true))
            } else {
                None
            }
        });
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    // Type 'h', TAB (9), ENTER (10)
    let _ = w_in.write_all(&[b'h', 9, 10]);

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("hello")));
}

#[test]
fn test_binary_non_utf8() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    let _ = w_in.write_all(&[0xFF, 0xFE, 0xFD, 10]);

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from(&[0xFF, 0xFE, 0xFD][..])));
}

#[test]
fn test_completion_empty_and_wrap_beep() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        // Completion returning empty list
        editor.set_completion_handler(|_line: &BStr| vec![]);
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    // Press TAB on empty completions, then enter
    let _ = w_in.write_all(&[9, b'a', 10]);

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("a")));
}

#[test]
fn test_completion_cycling_wrap() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.set_completion_handler(|_line: &BStr| vec![BString::from("one")]);
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    // Press TAB once ("one"), TAB twice (restores ""), then Enter
    let _ = w_in.write_all(&[9, 9, 10]);

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("")));
}

#[test]
fn test_completion_esc_cancel() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.set_completion_handler(|_line: &BStr| vec![BString::from("one")]);
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    // Press TAB (shows "one"), then ESC (27, cancels back to ""), then Enter
    let _ = w_in.write_all(&[9, 27, 10]);

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("")));
}

#[test]
fn test_clear_screen_method() {
    let editor = Editor::new();
    assert!(editor.clear_screen().is_ok());
}

#[test]
fn test_terminal_support_checks() {
    assert_eq!(terminal_capability_from_name(Some("dumb")), TerminalCapability::PromptOnly);
    assert_eq!(terminal_capability_from_name(Some("cons25")), TerminalCapability::PromptOnly);
    assert_eq!(terminal_capability_from_name(Some("emacs")), TerminalCapability::PromptOnly);
    assert_eq!(terminal_capability_from_name(Some("uart")), TerminalCapability::UartEcho);
    assert_eq!(
        terminal_capability_from_name(Some("xterm-256color")),
        TerminalCapability::Supported
    );
    assert_eq!(terminal_capability_from_name(None), TerminalCapability::Supported);
}

#[test]
fn test_resolve_operating_mode() {
    let cfg = Config::default();
    assert_eq!(cfg.resolve_operating_mode(|| false), OperatingMode::NonTty);
    assert_eq!(cfg.resolve_operating_mode(|| true), OperatingMode::Interactive);

    let cfg_dumb =
        Config::default().with_terminal_mode(TerminalMode::Tty).with_term_name(Some("dumb"));
    assert_eq!(cfg_dumb.resolve_operating_mode(|| false), OperatingMode::PromptOnly);

    let cfg_uart =
        Config::default().with_terminal_mode(TerminalMode::Tty).with_term_name(Some("uart"));
    assert_eq!(cfg_uart.resolve_operating_mode(|| false), OperatingMode::UartEcho);

    let cfg_nontty = Config::default().with_terminal_mode(TerminalMode::NonTty);
    assert_eq!(cfg_nontty.resolve_operating_mode(|| true), OperatingMode::NonTty);
}

#[test]
fn test_readline_fallback_stream() {
    let (r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = Editor::new();
        editor.readline_fallback_mode(
            r_in,
            &mut w_out,
            OperatingMode::PromptOnly,
            b"prompt> ".as_bstr(),
        )
    });

    w_in.write_all(b"fallback input\n").unwrap();

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("fallback input")));

    let mut buf = [0u8; 32];
    let n = r_out.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"prompt> ");
}

#[test]
fn test_custom_stream_isolation_from_stdin_tty() {
    // Validate that readline_from performs interactive line editing on custom streams
    // regardless of whether process std::io::stdin().is_terminal() is true or false.
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"custom_prompt> ".as_bstr(),
        )
    });

    // Interactive readline_stream writes cursor position query \x1b[6n to the custom output stream
    let mut buf = [0u8; 32];
    r_out.read_exact(&mut buf[..4]).unwrap();
    assert_eq!(&buf[..4], b"\x1b[6n");

    // Send cursor position response back to custom input stream
    w_in.write_all(b"\x1b[10;80R").unwrap();

    // Read column width query \x1b[999C and position query \x1b[6n
    r_out.read_exact(&mut buf[..6]).unwrap();
    assert_eq!(&buf[..6], b"\x1b[999C");
    r_out.read_exact(&mut buf[..4]).unwrap();
    assert_eq!(&buf[..4], b"\x1b[6n");
    w_in.write_all(b"\x1b[10;80R").unwrap();

    // Send typed input and newline over custom stream
    w_in.write_all(b"test_input\n").unwrap();

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("test_input")));
}

#[test]
fn test_term_is_tty_override_false() {
    let (r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor =
            Editor::with_config(Config::default().with_terminal_mode(TerminalMode::NonTty));
        let mode = editor.config.resolve_operating_mode(|| true);
        assert_eq!(mode, OperatingMode::NonTty);
        editor.readline_from(r_in, &mut w_out, mode, b"prompt> ".as_bstr())
    });

    w_in.write_all(b"non_tty_input\n").unwrap();

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("non_tty_input")));

    let mut buf = [0u8; 32];
    let n = r_out.read(&mut buf).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn test_term_columns_override() {
    let (mut r_in, mut w_in) = create_pipe();
    let (_r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(TerminalMode::Tty)
                .with_term_name(Some("xterm-256color"))
                .with_column_width(ColumnWidth::Fixed(100)),
        );
        let mode = editor.config.resolve_operating_mode(|| true);
        editor.readline_from(&mut r_in, &mut w_out, mode, b"prompt> ".as_bstr())
    });

    // Since column_width is overridden to Fixed(100), no cursor position query (\x1b[6n) is sent!
    w_in.write_all(b"fixed_cols_input\n").unwrap();

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("fixed_cols_input")));
}

#[test]
fn test_auto_column_width_fallback() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(TerminalMode::Tty)
                .with_term_name(Some("xterm-256color")),
        );
        let mode = editor.config.resolve_operating_mode(|| true);
        editor.readline_from(&mut r_in, &mut w_out, mode, b"prompt> ".as_bstr())
    });

    let mut buf = [0u8; 32];
    r_out.read_exact(&mut buf[..4]).unwrap();
    w_in.write_all(b"\x1b[10;80R").unwrap();
    r_out.read_exact(&mut buf[..6]).unwrap();
    r_out.read_exact(&mut buf[..4]).unwrap();
    w_in.write_all(b"\x1b[10;80R").unwrap();

    let mut prompt_buf = [0u8; 8];
    r_out.read_exact(&mut prompt_buf).unwrap();
    assert_eq!(&prompt_buf, b"prompt> ");

    w_in.write_all(b"hello\n").unwrap();
    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("hello")));
}

#[test]
fn test_get_columns_auto_default() {
    let (mut r_in, w_in) = create_pipe();
    let (_r_out, mut w_out) = create_pipe();
    drop(w_in);
    let editor = Editor::with_config(Config::default());
    let cols = editor.get_columns(&mut r_in, &mut w_out);
    assert_eq!(cols, DEFAULT_COLUMN_COUNT);
}

#[test]
fn test_term_name_override_dumb() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = Editor::with_config(
            Config::default().with_terminal_mode(TerminalMode::Tty).with_term_name(Some("dumb")),
        );
        let mode = editor.config.resolve_operating_mode(|| true);
        assert_eq!(mode, OperatingMode::PromptOnly);
        editor.readline_from(&mut r_in, &mut w_out, mode, b"prompt> ".as_bstr())
    });

    w_in.write_all(b"dumb_term_input\n").unwrap();

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("dumb_term_input")));

    let mut buf = [0u8; 32];
    let n = r_out.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"prompt> ");
}

#[test]
fn test_term_name_override_uart() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = Editor::with_config(
            Config::default().with_terminal_mode(TerminalMode::Tty).with_term_name(Some("uart")),
        );
        let mode = editor.config.resolve_operating_mode(|| true);
        assert_eq!(mode, OperatingMode::UartEcho);
        editor.readline_from(&mut r_in, &mut w_out, mode, b"prompt> ".as_bstr())
    });

    w_in.write_all(b"ab\n").unwrap();

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("ab")));

    let mut buf = [0u8; 32];
    let n = r_out.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"prompt> ab\n");
}

#[test]
fn test_control_sequence_builder() {
    let mut buf = Vec::new();
    control::ControlSequenceBuilder::new()
        .arg(6)
        .cmd(control::CMD_DEVICE_STATUS_REPORT)
        .write(&mut buf)
        .unwrap();
    assert_eq!(buf, b"\x1b[6n");

    buf.clear();
    control::ControlSequenceBuilder::new()
        .arg(1)
        .arg(31)
        .arg(49)
        .cmd(control::CMD_SELECT_GRAPHIC_RENDITION)
        .write(&mut buf)
        .unwrap();
    assert_eq!(buf, b"\x1b[1;31;49m");
}

#[test]
fn test_control_write_helpers() {
    let mut buf = Vec::new();
    control::write_query_cursor_position(&mut buf).unwrap();
    assert_eq!(buf, b"\x1b[6n");

    buf.clear();
    control::write_query_column_width(&mut buf).unwrap();
    assert_eq!(buf, b"\x1b[999C");

    buf.clear();
    control::write_clear_screen(&mut buf).unwrap();
    assert_eq!(buf, b"\x1b[H\x1b[2J");

    buf.clear();
    control::write_clear_to_eol(&mut buf).unwrap();
    assert_eq!(buf, b"\x1b[0K");

    buf.clear();
    control::write_reset_sgr(&mut buf).unwrap();
    assert_eq!(buf, b"\x1b[0m");

    buf.clear();
    control::write_sgr_formatting(&mut buf, true, Some(Color::Red)).unwrap();
    assert_eq!(buf, b"\x1b[1;31;49m");

    buf.clear();
    control::write_move_cursor_column(&mut buf, 10).unwrap();
    assert_eq!(buf, b"\r\x1b[10C");

    buf.clear();
    control::write_move_cursor_up(&mut buf, 3).unwrap();
    assert_eq!(buf, b"\x1b[3A");

    buf.clear();
    control::write_move_cursor_down(&mut buf, 2).unwrap();
    assert_eq!(buf, b"\x1b[2B");

    buf.clear();
    control::write_clear_line_and_move_up(&mut buf).unwrap();
    assert_eq!(buf, b"\r\x1b[0K\x1b[1A");

    buf.clear();
    control::write_erase_previous_char_uart(&mut buf).unwrap();
    assert_eq!(buf, b"\x08 \x08");
}

#[test]
fn test_completion_escape_sequence_navigation() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let handle = std::thread::spawn(move || {
        let mut editor = test_editor();
        editor.history.add("previous_cmd");
        editor.set_completion_handler(|line: &BStr| {
            if line.starts_with(b"h") {
                vec![BString::from("hello"), BString::from("help")]
            } else {
                vec![]
            }
        });
        editor.readline_from(
            &mut r_in,
            &mut w_out,
            OperatingMode::Interactive,
            b"prompt> ".as_bstr(),
        )
    });

    let mut buf = [0u8; 32];
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");
    let _ = r_out.read(&mut buf[..6]);
    let _ = r_out.read(&mut buf[..4]);
    let _ = w_in.write_all(b"\x1b[10;80R");

    // Type 'h', TAB (9) to trigger completion, then Up Arrow (\x1b[A), then Enter (10)
    let _ = w_in.write_all(&[b'h', 9]);
    let _ = w_in.write_all(b"\x1b[A\n");

    let res = handle.join().unwrap();
    assert_eq!(res.ok(), Some(BString::from("previous_cmd")));
}

#[test]
fn test_multiline_boundary_row_calculation() {
    let (mut r_in, _w_in) = create_pipe();
    let (_r_out, mut w_out) = create_pipe();

    let mut editor = Editor::with_config(
        Config { multiline_mode: true, ..Config::default() }
            .with_terminal_mode(TerminalMode::Tty)
            .with_column_width(ColumnWidth::Fixed(80)),
    );

    let prompt = b"1234567890".as_bstr(); // len 10
    let mut state = state::State {
        reader: &mut r_in,
        writer: &mut w_out,
        buffer: BString::from("A".repeat(70)), // total 80 chars
        prompt,
        prompt_length: prompt.len(),
        cursor_position: 70,
        previous_cursor_position: 0,
        column_count: 80,
        max_rows: 0,
        history_index: 0,
        draft_line: None,
        editor: &mut editor,
        render_buf: Vec::with_capacity(512),
    };

    // Total 79 chars with 80 cols is 1 row, cursor is not at margin
    state.buffer = BString::from("A".repeat(69));
    state.cursor_position = 69;
    state.refresh_multiline();
    assert_eq!(state.max_rows, 1);

    // Total 80 chars with cursor at column 80 wraps to row 2 at the margin
    state.buffer.push(b'A');
    state.cursor_position = 70;
    state.refresh_multiline();
    assert_eq!(state.max_rows, 2);

    // Total 80 chars with cursor at start (column 10) does not wrap at margin
    state.cursor_position = 0;
    state.max_rows = 0;
    state.refresh_multiline();
    assert_eq!(state.max_rows, 1);

    // At 81 chars, it transitions to 2 rows regardless of cursor position
    state.buffer.push(b'A');
    state.cursor_position = 0;
    state.refresh_multiline();
    assert_eq!(state.max_rows, 2);

    // Empty prompt and empty buffer should not emit newline at margin
    state.buffer.clear();
    state.cursor_position = 0;
    state.prompt = b"".as_bstr();
    state.prompt_length = 0;
    state.render_buf.clear();
    state.refresh_multiline();
    assert!(!state.render_buf.windows(2).any(|w| w == b"\n\r"));
}

#[test]
fn test_nontty_unbounded_line_length() {
    let (mut r_in, mut w_in) = create_pipe();
    let (_r_out, mut w_out) = create_pipe();

    let mut long_line = vec![b'x'; 6000];
    long_line.push(b'\n');

    let writer_handle = std::thread::spawn(move || {
        w_in.write_all(&long_line).unwrap();
    });

    let mut editor =
        Editor::with_config(Config::default().with_terminal_mode(TerminalMode::NonTty));

    let res = editor.readline_from(&mut r_in, &mut w_out, OperatingMode::NonTty, b"".as_bstr());

    writer_handle.join().unwrap();
    let line = res.expect("readline should succeed");
    assert_eq!(line.len(), 6000);
    assert_eq!(line, BString::from("x".repeat(6000)));
}

#[test]
fn test_render_buf_capacity_retention() {
    let (mut r_in, _w_in) = create_pipe();
    let (_r_out, mut w_out) = create_pipe();

    let mut editor = Editor::default();
    let prompt = b"> ".as_bstr();
    let mut state = state::State {
        reader: &mut r_in,
        writer: &mut w_out,
        buffer: BString::from("test command"),
        prompt,
        prompt_length: prompt.len(),
        cursor_position: 12,
        previous_cursor_position: 0,
        column_count: 80,
        max_rows: 0,
        history_index: 0,
        draft_line: None,
        editor: &mut editor,
        render_buf: Vec::with_capacity(512),
    };

    state.refresh_singleline();
    let cap1 = state.render_buf.capacity();
    assert!(cap1 >= 512);

    state.edit_insert(b'!');
    let cap2 = state.render_buf.capacity();
    assert_eq!(cap1, cap2, "render_buf capacity should be retained without reallocation");
}

#[test]
fn test_unbuffered_stdout_trait() {
    let mut out = UnbufferedStdout;
    assert_eq!(out.write(&[]).unwrap(), 0);
    assert!(out.flush().is_ok());

    let mut ref_out = &UnbufferedStdout;
    assert_eq!(ref_out.write(&[]).unwrap(), 0);
    assert!(ref_out.flush().is_ok());
}

#[test]
fn test_handle_escape_sequence_csi_draining() {
    let (mut r_in, mut w_in) = create_pipe();
    let (_r_out, mut w_out) = create_pipe();

    // Write a cursor position response: ESC [ 2 4 ; 8 0 R
    // followed by 'a' '\n'
    w_in.write_all(b"4;80Ra\n").unwrap();

    let mut editor = test_editor();
    let prompt = b"".as_bstr();
    let mut state = state::State {
        reader: &mut r_in,
        writer: &mut w_out,
        buffer: BString::default(),
        prompt,
        prompt_length: 0,
        cursor_position: 0,
        previous_cursor_position: 0,
        column_count: 80,
        max_rows: 0,
        history_index: 0,
        draft_line: None,
        editor: &mut editor,
        render_buf: Vec::with_capacity(512),
    };

    // ESC [ 2 has been read, passing b'[' and b'2'
    state.handle_escape_sequence(b'[', b'2');

    // The sequence 4;80R should have been completely consumed.
    // The next byte available in the reader should be 'a'.
    let next_byte = control::read_byte(state.reader).unwrap();
    assert_eq!(next_byte, Some(b'a'));
}

#[test]
fn test_get_columns_ansi_cursor_query() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let editor = Editor::with_config(Config::default().with_column_width(ColumnWidth::AnsiCursor));
    let handle = std::thread::spawn(move || editor.get_columns(&mut r_in, &mut w_out));

    let mut buf = [0u8; 32];
    // Read \x1b[6n
    r_out.read_exact(&mut buf[..4]).unwrap();
    // Respond start pos (row 1, col 10)
    w_in.write_all(b"\x1b[1;10R").unwrap();
    // Read \x1b[999C
    r_out.read_exact(&mut buf[..6]).unwrap();
    // Read \x1b[6n
    r_out.read_exact(&mut buf[..4]).unwrap();
    // Respond max pos (row 1, col 120)
    w_in.write_all(b"\x1b[1;120R").unwrap();

    let cols = handle.join().unwrap();
    assert_eq!(cols, 120);
}

#[test]
fn test_get_columns_ansi_cursor_col_1_uses_last_cols() {
    let (mut r_in, mut w_in) = create_pipe();
    let (mut r_out, mut w_out) = create_pipe();

    let editor = Editor::with_config(Config::default().with_column_width(ColumnWidth::AnsiCursor));
    let handle = std::thread::spawn(move || editor.get_columns(&mut r_in, &mut w_out));

    let mut buf = [0u8; 32];
    r_out.read_exact(&mut buf[..4]).unwrap();
    w_in.write_all(b"\x1b[1;10R").unwrap();
    r_out.read_exact(&mut buf[..6]).unwrap();
    r_out.read_exact(&mut buf[..4]).unwrap();
    // Terminal processing output failure: cols == 1
    w_in.write_all(b"\x1b[1;1R").unwrap();

    let cols = handle.join().unwrap();
    assert_eq!(cols, DEFAULT_COLUMN_COUNT);
}
