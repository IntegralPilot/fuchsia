// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EvalOutcome, ExecutionContext, ShellState, eval_command, run_exit_trap};
use crate::parser::ast::ASTBuilder;
use crate::parser::{parse_script, tokenize};
use crate::string::parse_int;
use bstr::{BStr, ByteSlice};
use std::io::Write;

/// Run a script from a byte slice.
pub fn run_string(input: &BStr, state: ShellState) -> i32 {
    let mut ctx = match ExecutionContext::initial() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Context error: {}", e);
            return 1;
        }
    };
    run_string_with_context(input, state, &mut ctx)
}

/// Run a script from a byte slice with a pre-created execution context.
fn run_string_with_context(input: &BStr, mut state: ShellState, ctx: &mut ExecutionContext) -> i32 {
    let mut builder = ASTBuilder::new();
    let tokens = match tokenize(input.as_bytes()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Tokenize error: {}", e);
            return 1;
        }
    };
    let cmds = match parse_script(&mut builder, &tokens) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            return 1;
        }
    };
    let mut last_code = 0;
    for &cmd_offset in &cmds {
        match eval_command(&mut builder, cmd_offset, &mut state, ctx) {
            Ok(EvalOutcome::Code(c)) => {
                last_code = c;
            }
            Ok(EvalOutcome::Exit(c)) => {
                run_exit_trap(&mut state, ctx);
                return c;
            }
            Ok(EvalOutcome::Return(c)) => {
                run_exit_trap(&mut state, ctx);
                return c;
            }
            Ok(EvalOutcome::Break(_)) => {
                eprintln!("zxsh: break: can only break from a loop");
                run_exit_trap(&mut state, ctx);
                return 1;
            }
            Ok(EvalOutcome::Continue(_)) => {
                eprintln!("zxsh: continue: can only continue from a loop");
                run_exit_trap(&mut state, ctx);
                return 1;
            }
            Err(e) => {
                if !e.is_empty() {
                    eprintln!("Eval error: {}", e);
                }
                let code_bstr = state.get_var(b"?");
                let code =
                    code_bstr.as_ref().and_then(|b| parse_int::<i32>(b.as_bytes())).unwrap_or(1);
                let code = if code == 0 { 1 } else { code };
                state.set_last_status(code);
                run_exit_trap(&mut state, ctx);
                return code;
            }
        }
    }
    run_exit_trap(&mut state, ctx);
    last_code
}

/// Run a script from a file path.
pub fn run_script(path: &BStr, state: ShellState) -> i32 {
    let path_ref = match path.to_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Invalid path {}: {}", path, e);
            return 1;
        }
    };
    let content = match std::fs::read(path_ref) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read script {}: {}", path, e);
            return 1;
        }
    };
    let mut ctx = match ExecutionContext::initial() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Context error: {}", e);
            return 1;
        }
    };
    if state.opt_verbose {
        if let Some(mut err) = ctx.stderr() {
            let _ = err.write_all(&content);
            let _ = err.flush();
        }
    }
    run_string_with_context(BStr::new(&content), state, &mut ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_string_outcomes() {
        assert_eq!(run_string(BStr::new(""), ShellState::new()), 0);
        assert_eq!(run_string(BStr::new("x=100"), ShellState::new()), 0);
        assert_eq!(run_string(BStr::new("x=1; y=2; z=3"), ShellState::new()), 0);
        assert_eq!(run_string(BStr::new("exit 42"), ShellState::new()), 42);
        assert_eq!(run_string(BStr::new("return 7"), ShellState::new()), 7);
    }

    #[test]
    fn test_run_string_errors() {
        // Tokenize error
        assert_eq!(run_string(BStr::new("'unterminated quote"), ShellState::new()), 1);

        // Parse error
        assert_eq!(run_string(BStr::new("if; then"), ShellState::new()), 1);

        // Break outside loop
        assert_eq!(run_string(BStr::new("break"), ShellState::new()), 0);

        // Continue outside loop
        assert_eq!(run_string(BStr::new("continue"), ShellState::new()), 0);

        // Non-zero exit status
        assert_eq!(run_string(BStr::new("false"), ShellState::new()), 1);

        // Eval error with non-empty error message
        assert_eq!(run_string(BStr::new("${invalid?error_msg}"), ShellState::new()), 1);

        // Eval error with custom $? status
        let mut state = ShellState::new();
        state.set_last_status(5);
        assert_eq!(run_string(BStr::new("${unset_var?}"), state), 5);
    }

    #[test]
    fn test_run_script() {
        // Invalid path
        assert_eq!(run_script(BStr::new(b"\xFF\xFE"), ShellState::new()), 1);

        // Non-existent script
        assert_eq!(run_script(BStr::new("/nonexistent_zxsh_script_9999.sh"), ShellState::new()), 1);

        // Valid script using tempfile/tempdir
        let temp_dir = std::env::temp_dir();
        let script_file = temp_dir.join("test_script_zxsh_runner.sh");
        std::fs::write(&script_file, "a=10\nb=20\n").unwrap();

        let path_bstr = BStr::new(script_file.to_str().unwrap());
        assert_eq!(run_script(path_bstr, ShellState::new()), 0);

        let mut state_verbose = ShellState::new();
        state_verbose.opt_verbose = true;
        assert_eq!(run_script(path_bstr, state_verbose), 0);

        let _ = std::fs::remove_file(script_file);
    }

    #[test]
    fn test_verbose_and_xtrace() {
        let mut state_v = ShellState::new();
        state_v.opt_verbose = true;
        assert_eq!(run_string(BStr::new("a=10"), state_v), 0);

        let mut state_x = ShellState::new();
        state_x.opt_xtrace = true;
        assert_eq!(run_string(BStr::new("a=10"), state_x), 0);
    }
}
