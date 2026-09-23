// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::run_builtin;
use crate::builtins::times::builtin_times;
use crate::eval::{EXIT_SUCCESS, EvalOutcome, ExecutionContext, ShellState};

#[test]
fn test_builtin_times() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = builtin_times(&[], &mut state, &mut std::io::empty(), &mut stdout, &mut stderr);

    assert_eq!(code, EXIT_SUCCESS);
    let output = String::from_utf8(stdout).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "output: {:?}", output);
    for line in &lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 2, "line: {:?}", line);
        for part in parts {
            assert!(part.contains('m'), "part: {:?}", part);
            assert!(part.ends_with('s'), "part: {:?}", part);
        }
    }
}

#[test]
fn test_run_builtin_times() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let res = run_builtin("times", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
}
