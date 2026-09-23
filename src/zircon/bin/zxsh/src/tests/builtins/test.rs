// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::run_builtin;
use crate::builtins::test::{builtin_left_bracket, builtin_test};
use crate::eval::{EXIT_SYNTAX_ERROR, EvalOutcome, ExecutionContext, ShellState};
use bstr::BString;
use std::fs::File;
use std::io::Write as _;

#[test]
fn test_builtin_test_brackets() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // Basic equality
    let test_args = vec![BString::from("abc"), BString::from("="), BString::from("abc")];
    let res = run_builtin("test", &test_args, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // [ missing ] returns EXIT_SYNTAX_ERROR (2)
    let missing_bracket = vec![BString::from("1"), BString::from("-eq"), BString::from("1")];
    let mut stderr = Vec::new();
    let mut stdout = Vec::new();
    let mut stdin = std::io::empty();
    let res =
        builtin_left_bracket(&missing_bracket, &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(res, EXIT_SYNTAX_ERROR);
    assert_eq!(String::from_utf8_lossy(&stderr), "[: missing ']'\n");

    // [ ] returns exit status 1
    let empty_bracket = vec![BString::from("]")];
    let res = run_builtin("[", &empty_bracket, &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // 0-arg test returns 1
    let res = run_builtin("test", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // 1-arg test
    let res = run_builtin("test", &[BString::from("-f")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    let res = run_builtin("test", &[BString::from("")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // 2-arg test
    let res = run_builtin("test", &[BString::from("!"), BString::from("")], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    let res =
        run_builtin("test", &[BString::from("!"), BString::from("abc")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // POSIX 3-arg rules: binary op takes precedence over operator-like operand
    let res = run_builtin(
        "test",
        &[BString::from("!"), BString::from("="), BString::from("!")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("("), BString::from("="), BString::from("(")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // POSIX 4-arg rules
    let res = run_builtin(
        "test",
        &[BString::from("("), BString::from("-z"), BString::from(""), BString::from(")")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // String comparisons < and >
    let res = run_builtin(
        "test",
        &[BString::from("abc"), BString::from("<"), BString::from("def")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Invalid integer handling returns status 2 and error to stderr
    stderr.clear();
    let res = builtin_test(
        &[BString::from("abc"), BString::from("-eq"), BString::from("123")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(res, EXIT_SYNTAX_ERROR);
    assert_eq!(String::from_utf8_lossy(&stderr), "test: abc: bad number\n");

    // Syntax error on unexpected extra arguments
    stderr.clear();
    let res = builtin_test(
        &[
            BString::from("a"),
            BString::from("b"),
            BString::from("c"),
            BString::from("d"),
            BString::from("e"),
        ],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(res, EXIT_SYNTAX_ERROR);
    assert!(String::from_utf8_lossy(&stderr).contains("unexpected operator"));
}

#[test]
fn test_string_and_integer_comparisons() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // String operators: -z, -n, ==, !=, >
    let res = run_builtin("test", &[BString::from("-z"), BString::from("")], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res =
        run_builtin("test", &[BString::from("-n"), BString::from("foo")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("hello"), BString::from("=="), BString::from("hello")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("hello"), BString::from("!="), BString::from("world")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("z"), BString::from(">"), BString::from("a")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Integer operators: -eq, -ne, -ge, -gt, -le, -lt with signs and whitespace
    let res = run_builtin(
        "test",
        &[BString::from(" 10"), BString::from("-eq"), BString::from("+10")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("-5"), BString::from("-ne"), BString::from("5")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("10"), BString::from("-ge"), BString::from("10")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("20"), BString::from("-gt"), BString::from("10")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("5"), BString::from("-le"), BString::from("5")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[BString::from("3"), BString::from("-lt"), BString::from("7")],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}

#[test]
fn test_bad_number_parsing() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    let bad_inputs = vec!["", " ", "+", "-", "12abc", "12 34", "not_a_num"];
    for bad in bad_inputs {
        stderr.clear();
        let code = builtin_test(
            &[BString::from(bad), BString::from("-eq"), BString::from("0")],
            &mut state,
            &mut stdin,
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(code, EXIT_SYNTAX_ERROR, "failed on bad input {:?}", bad);
        assert!(String::from_utf8_lossy(&stderr).contains("bad number"));
    }
}

#[test]
fn test_file_operators() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let dir = std::env::temp_dir().join("zxsh_test_file_ops");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let file_path = dir.join("test_file.txt");
    let mut f = File::create(&file_path).unwrap();
    f.write_all(b"content").unwrap();
    drop(f);

    let file_str = BString::from(file_path.to_str().unwrap());
    let dir_str = BString::from(dir.to_str().unwrap());
    let nonexist_str = BString::from(dir.join("nonexistent.txt").to_str().unwrap());

    // Existence and types
    let res = run_builtin("test", &[BString::from("-e"), file_str.clone()], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("test", &[BString::from("-f"), file_str.clone()], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res =
        run_builtin("test", &[BString::from("-d"), dir_str.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("test", &[BString::from("-s"), file_str.clone()], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res =
        run_builtin("test", &[BString::from("-e"), nonexist_str.clone()], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let res =
        run_builtin("test", &[BString::from("-f"), nonexist_str.clone()], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let res =
        run_builtin("test", &[BString::from("-d"), nonexist_str.clone()], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let res = run_builtin("test", &[BString::from("-r"), file_str.clone()], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin("test", &[BString::from("-w"), file_str.clone()], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Unop flags covering mode matchers: -c, -b, -p, -u, -g, -k, -O, -G, -S, -h, -L
    let flags = vec!["-c", "-b", "-p", "-u", "-g", "-k", "-O", "-G", "-S", "-h", "-L"];
    for flag in flags {
        let _ = run_builtin("test", &[BString::from(flag), file_str.clone()], &mut state, &mut ctx);
        let _ =
            run_builtin("test", &[BString::from(flag), nonexist_str.clone()], &mut state, &mut ctx);
    }

    // Binary file comparisons: -ef, -nt, -ot
    let file2_path = dir.join("test_file2.txt");
    File::create(&file2_path).unwrap();
    let file2_str = BString::from(file2_path.to_str().unwrap());

    let res = run_builtin(
        "test",
        &[file_str.clone(), BString::from("-ef"), file_str.clone()],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[file_str.clone(), BString::from("-ef"), file2_str.clone()],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let _ = run_builtin(
        "test",
        &[file_str.clone(), BString::from("-nt"), file2_str.clone()],
        &mut state,
        &mut ctx,
    );
    let _ = run_builtin(
        "test",
        &[file_str.clone(), BString::from("-ot"), file2_str.clone()],
        &mut state,
        &mut ctx,
    );
    let _ = run_builtin(
        "test",
        &[nonexist_str.clone(), BString::from("-ef"), file_str.clone()],
        &mut state,
        &mut ctx,
    );

    // TTY test -t
    let _ = run_builtin("test", &[BString::from("-t"), BString::from("0")], &mut state, &mut ctx);
    let _ =
        run_builtin("test", &[BString::from("-t"), BString::from("9999")], &mut state, &mut ctx);
    let _ = run_builtin("test", &[BString::from("-t"), BString::from("-1")], &mut state, &mut ctx);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_boolean_logic_and_grouping() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    // -a (AND) and -o (OR)
    let res = run_builtin(
        "test",
        &[
            BString::from("1"),
            BString::from("-eq"),
            BString::from("1"),
            BString::from("-a"),
            BString::from("2"),
            BString::from("-eq"),
            BString::from("2"),
        ],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let res = run_builtin(
        "test",
        &[
            BString::from("1"),
            BString::from("-eq"),
            BString::from("2"),
            BString::from("-o"),
            BString::from("2"),
            BString::from("-eq"),
            BString::from("2"),
        ],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Parentheses grouping: ( expr )
    let res = run_builtin(
        "test",
        &[
            BString::from("("),
            BString::from("1"),
            BString::from("-eq"),
            BString::from("2"),
            BString::from("-o"),
            BString::from("3"),
            BString::from("-eq"),
            BString::from("3"),
            BString::from(")"),
        ],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Empty parentheses
    let res = run_builtin("test", &[BString::from("("), BString::from(")")], &mut state, &mut ctx)
        .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));
}

#[test]
fn test_syntax_errors_and_missing_arguments() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    // Missing closing paren
    stderr.clear();
    let code = builtin_test(
        &[BString::from("("), BString::from("1"), BString::from("-eq"), BString::from("1")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, EXIT_SYNTAX_ERROR);
    assert!(String::from_utf8_lossy(&stderr).contains("closing paren expected"));

    // Missing argument for unary operator
    stderr.clear();
    let code =
        builtin_test(&[BString::from("-f")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(code, 0); // Note: 1 argument "-f" evaluates as non-empty string per POSIX rules

    // Unary operator with missing operand in complex parse
    stderr.clear();
    let code = builtin_test(
        &[BString::from("!"), BString::from("-f")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);

    // Missing argument for binary operator
    stderr.clear();
    let code = builtin_test(
        &[BString::from("("), BString::from("1"), BString::from("-eq"), BString::from(")")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, EXIT_SYNTAX_ERROR);
    assert!(String::from_utf8_lossy(&stderr).contains("argument expected"));

    // Left bracket ] matching
    let code = builtin_left_bracket(
        &[BString::from("1"), BString::from("-eq"), BString::from("1"), BString::from("]")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
}
