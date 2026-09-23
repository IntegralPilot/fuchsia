// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{
    EvalOutcome, ExecutionContext, ShellState, eval_command, expand_prompt, run_exit_trap,
};
use crate::parser::ast::ASTBuilder;
use crate::parser::{ParseError, parse_script, tokenize};
use crate::tty::ShellSignals;
use bstr::{BStr, BString, ByteSlice, ByteVec};
use line_editor::{Config, Editor, ReadlineError};
use std::io::{BufRead, IsTerminal, Write};

mod completion;

const DEFAULT_PS1: &str = "$ ";
const DEFAULT_PS2: &str = "> ";

fn get_prompt(input_buffer: &BStr, state: &mut ShellState, ctx: &ExecutionContext) -> BString {
    let prompt_var =
        if input_buffer.is_empty() { bstr::BStr::new(b"PS1") } else { bstr::BStr::new(b"PS2") };
    let default_prompt =
        if input_buffer.is_empty() { BStr::new(DEFAULT_PS1) } else { BStr::new(DEFAULT_PS2) };
    expand_prompt(prompt_var, default_prompt, state, ctx)
}

/// Creates an interactive line editor configured for zxsh.
fn create_editor() -> Editor {
    Editor::with_config(Config { max_history_len: 100, ..Default::default() })
}

/// Starts the interactive read-eval-print loop (REPL) using `line-editor` for command history and
/// autocompletion.
pub fn run_repl(state: ShellState) {
    let stdin = std::io::stdin();
    let is_tty = stdin.is_terminal();
    if let Some(code) = run_repl_reader(stdin.lock(), state, is_tty) {
        std::process::exit(code);
    }
}

fn run_repl_reader<R: BufRead>(reader: R, state: ShellState, is_tty: bool) -> Option<i32> {
    run_repl_stream(reader, line_editor::UnbufferedStdout, state, is_tty)
}

fn run_repl_stream<R: BufRead, W: Write>(
    reader: R,
    writer: W,
    state: ShellState,
    is_tty: bool,
) -> Option<i32> {
    let mut editor = if is_tty { Some(create_editor()) } else { None };
    run_repl_loop(reader, writer, state, editor.as_mut())
}

fn run_repl_loop<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    mut state: ShellState,
    mut editor: Option<&mut Editor>,
) -> Option<i32> {
    state.opt_interactive = true;
    let mut ctx = match ExecutionContext::initial() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Context error: {}", e);
            return Some(1);
        }
    };

    let has_editor = editor.is_some();
    let mut input_buffer = BString::default();
    let mut numeof = 0;

    'repl_loop: loop {
        let prompt = get_prompt(input_buffer.as_ref(), &mut state, &ctx);

        ctx.signal_state.clear(ShellSignals::INT);

        let line = if let Some(ref mut ed) = editor {
            let path = state.path();
            ed.set_completion_handler(move |line| completion::tab_complete(line, &path));
            let mode = ed.config.resolve_operating_mode(|| true);
            let read_result = ed.readline_from(&mut reader, &mut writer, mode, prompt.as_bstr());
            let mut line = match read_result {
                Ok(l) => {
                    numeof = 0;
                    l
                }
                Err(ReadlineError::Interrupted) => {
                    ctx.signal_state.clear(ShellSignals::INT);
                    state.set_last_status(130);
                    input_buffer.clear();
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    if ctx.signal_state.is_pending(ShellSignals::INT) {
                        ctx.signal_state.clear(ShellSignals::INT);
                        state.set_last_status(130);
                        input_buffer.clear();
                        continue;
                    }
                    if state.opt_ignoreeof && numeof < 10 {
                        numeof += 1;
                        let _ = writeln!(writer, "Use \"exit\" to leave shell.");
                        continue;
                    }
                    break 'repl_loop;
                }
                Err(ReadlineError::Io(e)) => {
                    eprintln!("Read line error: {}", e);
                    break 'repl_loop;
                }
            };

            ed.add_history(line.as_bstr());

            line.push_byte(b'\n');
            line
        } else {
            let mut l = Vec::new();
            match reader.read_until(b'\n', &mut l) {
                Ok(0) => break 'repl_loop,
                Ok(_) => BString::from(l),
                Err(e) => {
                    eprintln!("Read line error: {}", e);
                    break 'repl_loop;
                }
            }
        };

        if state.opt_verbose && !has_editor {
            if let Some(mut err) = ctx.stderr() {
                let _ = err.write_all(line.as_bytes());
                let _ = err.flush();
            }
        }

        let sigint = ctx.signal_state.is_pending(ShellSignals::INT);
        ctx.signal_state.clear(ShellSignals::INT);
        if sigint || line.as_bytes().contains(&b'\x03') {
            println!();
            state.set_last_status(130);
            input_buffer.clear();
            continue;
        }

        input_buffer.extend_from_slice(&line);
        let trimmed = input_buffer.trim_ascii();
        if trimmed.is_empty() {
            input_buffer.clear();
            continue;
        }
        let mut builder = ASTBuilder::new();
        let tokens = match tokenize(input_buffer.as_bytes()) {
            Ok(t) => t,
            Err(ParseError::Incomplete(_)) => {
                continue;
            }
            Err(ParseError::Syntax(e)) => {
                eprintln!("Tokenize error: {}", e);
                state.set_last_status(2);
                input_buffer.clear();
                continue;
            }
        };
        let cmds = match parse_script(&mut builder, &tokens) {
            Ok(c) => c,
            Err(ParseError::Incomplete(_)) => {
                continue;
            }
            Err(ParseError::Syntax(e)) => {
                eprintln!("Parse error: {}", e);
                state.set_last_status(2);
                input_buffer.clear();
                continue;
            }
        };
        input_buffer.clear();
        for &cmd_offset in &cmds {
            match eval_command(&mut builder, cmd_offset, &mut state, &mut ctx) {
                Ok(EvalOutcome::Code(c)) => {
                    state.set_last_status(c);
                }
                Ok(EvalOutcome::Exit(c)) => {
                    run_exit_trap(&mut state, &mut ctx);
                    return Some(c);
                }
                Ok(EvalOutcome::Return(c)) => {
                    run_exit_trap(&mut state, &mut ctx);
                    return Some(c);
                }
                Ok(EvalOutcome::Break(_)) => {
                    eprintln!("zxsh: break: can only break from a loop");
                    state.set_last_status(2);
                }
                Ok(EvalOutcome::Continue(_)) => {
                    eprintln!("zxsh: continue: can only continue from a loop");
                    state.set_last_status(2);
                }
                Err(e) => {
                    if !e.is_empty() {
                        eprintln!("Eval error: {}", e);
                    }
                    state.set_last_status(1);
                }
            }
        }
    }
    run_exit_trap(&mut state, &mut ctx);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_get_prompt_defaults() {
        let mut state = ShellState::new();
        let ctx = ExecutionContext::initial().unwrap();

        assert_eq!(get_prompt(BStr::new(""), &mut state, &ctx), BStr::new(DEFAULT_PS1));
        assert_eq!(get_prompt(BStr::new("input"), &mut state, &ctx), BStr::new(DEFAULT_PS2));
    }

    #[test]
    fn test_get_prompt_custom_vars() {
        let mut state = ShellState::new();
        state.set_var("PS1", "MYPROMPT> ");
        state.set_var("PS2", "CONTINUE> ");
        let ctx = ExecutionContext::initial().unwrap();

        assert_eq!(get_prompt(BStr::new(""), &mut state, &ctx), BStr::new("MYPROMPT> "));
        assert_eq!(get_prompt(BStr::new("line1"), &mut state, &ctx), BStr::new("CONTINUE> "));
    }

    #[test]
    fn test_get_prompt_expansion_error() {
        let mut state = ShellState::new();
        state.set_var("PS1", "$(( 1 / 0 ))");
        let ctx = ExecutionContext::initial().unwrap();

        // Expands to error -> falls back to default prompt DEFAULT_PS1
        assert_eq!(get_prompt(BStr::new(""), &mut state, &ctx), BStr::new(DEFAULT_PS1));
    }

    #[test]
    fn test_run_repl_reader_execution() {
        let input = "x=10\nexport Y=$x\n\n  \nbreak\ncontinue\nexit 5\n";
        let cursor = Cursor::new(input);
        let res = run_repl_reader(cursor, ShellState::new(), false);
        assert_eq!(res, Some(5));
    }

    #[test]
    fn test_run_repl_reader_incomplete_and_errors() {
        let input = "export Y='multi\nline'\nif; then\n'unterminated quote\n";
        let cursor = Cursor::new(input);
        let res = run_repl_reader(cursor, ShellState::new(), false);
        assert_eq!(res, None);
    }

    #[test]
    fn test_run_repl_reader_verbose() {
        let input = "x=10\nexit 0\n";
        let cursor = Cursor::new(input);
        let mut state = ShellState::new();
        state.opt_verbose = true;
        let res = run_repl_reader(cursor, state, false);
        assert_eq!(res, Some(0));
    }

    #[test]
    fn test_repl_interactive_execution_and_history() {
        let input = "x=100\nexport VAL=$x\nexit 42\n";
        let cursor = Cursor::new(input.as_bytes());
        let mut output = Vec::new();
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(line_editor::TerminalMode::Tty)
                .with_column_width(line_editor::ColumnWidth::Fixed(80)),
        );

        let res = run_repl_loop(cursor, &mut output, ShellState::new(), Some(&mut editor));
        assert_eq!(res, Some(42));

        let history_entries: Vec<&str> =
            editor.history().entries().iter().map(|e| e.to_str().unwrap()).collect();
        assert_eq!(history_entries, vec!["x=100", "export VAL=$x", "exit 42"]);
    }

    #[test]
    fn test_repl_interactive_ctrl_d_eof() {
        let cursor = Cursor::new(b"");
        let mut output = Vec::new();
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(line_editor::TerminalMode::Tty)
                .with_column_width(line_editor::ColumnWidth::Fixed(80)),
        );

        let res = run_repl_loop(cursor, &mut output, ShellState::new(), Some(&mut editor));
        assert_eq!(res, None);
        assert!(editor.history().entries().is_empty());
    }

    #[test]
    fn test_repl_interactive_ignoreeof() {
        let cursor = Cursor::new(b"");
        let mut output = Vec::new();
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(line_editor::TerminalMode::Tty)
                .with_column_width(line_editor::ColumnWidth::Fixed(80)),
        );
        let mut state = ShellState::new();
        state.opt_ignoreeof = true;

        let res = run_repl_loop(cursor, &mut output, state, Some(&mut editor));
        assert_eq!(res, None);
        let out_str = String::from_utf8_lossy(&output);
        assert!(out_str.contains("Use \"exit\" to leave shell."));
    }

    #[test]
    fn test_repl_interactive_multiline_ps2() {
        let input = "if true; then\nexport FOO=BAR\nfi\nexit 7\n";
        let cursor = Cursor::new(input.as_bytes());
        let mut output = Vec::new();
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(line_editor::TerminalMode::Tty)
                .with_column_width(line_editor::ColumnWidth::Fixed(80)),
        );

        let res = run_repl_loop(cursor, &mut output, ShellState::new(), Some(&mut editor));
        assert_eq!(res, Some(7));

        let history_entries: Vec<&str> =
            editor.history().entries().iter().map(|e| e.to_str().unwrap()).collect();
        assert_eq!(history_entries, vec!["if true; then", "export FOO=BAR", "fi", "exit 7"]);
    }

    #[test]
    fn test_repl_interactive_status_codes() {
        let input = "false\nexit $?\n";
        let cursor = Cursor::new(input.as_bytes());
        let mut output = Vec::new();
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(line_editor::TerminalMode::Tty)
                .with_column_width(line_editor::ColumnWidth::Fixed(80)),
        );

        let res = run_repl_loop(cursor, &mut output, ShellState::new(), Some(&mut editor));
        assert_eq!(res, Some(1));
    }

    #[test]
    fn test_repl_interactive_completion_integration() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join("zxsh_repl_complete_test");
        let _ = std::fs::create_dir_all(&test_dir);
        let f1 = test_dir.join("cmd_alpha");
        let f2 = test_dir.join("cmd_beta");
        let _ = std::fs::write(&f1, "1");
        let _ = std::fs::write(&f2, "2");

        let mut state = ShellState::new();
        state.set_var("PATH", test_dir.to_str().unwrap());

        let path = state.path();
        let comps = completion::tab_complete(BStr::new("cmd_"), &path);
        assert_eq!(comps.len(), 2);
        assert!(comps.contains(&BString::from("cmd_alpha")));
        assert!(comps.contains(&BString::from("cmd_beta")));

        let _ = std::fs::remove_file(f1);
        let _ = std::fs::remove_file(f2);
        let _ = std::fs::remove_dir(test_dir);
    }

    #[test]
    fn test_repl_uart_execution() {
        let input = "x=42\nexit $x\n";
        let cursor = Cursor::new(input.as_bytes());
        let mut output = Vec::new();
        let mut editor = Editor::with_config(Config::default().with_term_name(Some("uart")));

        let res = run_repl_loop(cursor, &mut output, ShellState::new(), Some(&mut editor));
        assert_eq!(res, Some(42));

        let out_str = String::from_utf8_lossy(&output);
        // In UartEcho mode, prompt is written and characters are echoed back.
        assert!(out_str.contains("$ "));
        assert!(out_str.contains("x=42"));
        assert!(out_str.contains("exit $x"));

        let history_entries: Vec<&str> =
            editor.history().entries().iter().map(|e| e.to_str().unwrap()).collect();
        assert_eq!(history_entries, vec!["x=42", "exit $x"]);
    }

    #[test]
    fn test_repl_dumb_terminal() {
        let input = "echo hello\nexit 0\n";
        let cursor = Cursor::new(input.as_bytes());
        let mut output = Vec::new();
        let mut editor = Editor::with_config(Config::default().with_term_name(Some("dumb")));

        let res = run_repl_loop(cursor, &mut output, ShellState::new(), Some(&mut editor));
        assert_eq!(res, Some(0));

        let out_str = String::from_utf8_lossy(&output);
        // Prompt is rendered, but PromptOnly mode does not echo typed characters.
        assert!(out_str.contains("$ "));
        assert!(!out_str.contains("echo hello"));
    }

    #[test]
    fn test_repl_interactive_ctrl_c_interrupted() {
        let input = b"\x03exit 42\n";
        let cursor = Cursor::new(input);
        let mut output = Vec::new();
        let mut editor = Editor::with_config(
            Config::default()
                .with_terminal_mode(line_editor::TerminalMode::Tty)
                .with_column_width(line_editor::ColumnWidth::Fixed(80)),
        );

        let state = ShellState::new();
        let res = run_repl_loop(cursor, &mut output, state, Some(&mut editor));
        assert_eq!(res, Some(42));
    }
}
