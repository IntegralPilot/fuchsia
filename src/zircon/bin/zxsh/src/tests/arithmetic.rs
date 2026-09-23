// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::testing::evaluate_arithmetic;
use crate::eval::{ExecutionContext, ShellState};
use bstr::BStr;

fn eval(expr: &[u8], state: &mut ShellState) -> i64 {
    let ctx = ExecutionContext::initial().unwrap();
    evaluate_arithmetic(BStr::new(expr), state, &ctx).unwrap()
}

fn eval_res(expr: &[u8], state: &mut ShellState) -> Result<i64, String> {
    let ctx = ExecutionContext::initial().unwrap();
    evaluate_arithmetic(BStr::new(expr), state, &ctx)
}

#[test]
fn test_basic_arithmetic() {
    let mut state = ShellState::new();

    assert_eq!(eval(b"1 + 2", &mut state), 3);
    assert_eq!(eval(b"5 - 3", &mut state), 2);
    assert_eq!(eval(b"2 * 3", &mut state), 6);
    assert_eq!(eval(b"8 / 2", &mut state), 4);
    assert_eq!(eval(b"7 % 3", &mut state), 1);
}

#[test]
fn test_precedence() {
    let mut state = ShellState::new();

    assert_eq!(eval(b"1 + 2 * 3", &mut state), 7);
    assert_eq!(eval(b"(1 + 2) * 3", &mut state), 9);
    assert_eq!(eval(b"10 - 2 * 3 + 4", &mut state), 8);
    assert_eq!(eval(b"10 / 2 * 3", &mut state), 15);
}

#[test]
fn test_unary_operators() {
    let mut state = ShellState::new();

    assert_eq!(eval(b"-5", &mut state), -5);
    assert_eq!(eval(b"+5", &mut state), 5);
    assert_eq!(eval(b"--5", &mut state), 5);
    assert_eq!(eval(b"~0", &mut state), -1);
    assert_eq!(eval(b"!0", &mut state), 1);
    assert_eq!(eval(b"!5", &mut state), 0);
}

#[test]
fn test_comparison_operators() {
    let mut state = ShellState::new();

    assert_eq!(eval(b"5 == 5", &mut state), 1);
    assert_eq!(eval(b"5 == 6", &mut state), 0);
    assert_eq!(eval(b"5 != 6", &mut state), 1);
    assert_eq!(eval(b"5 != 5", &mut state), 0);
    assert_eq!(eval(b"5 < 6", &mut state), 1);
    assert_eq!(eval(b"6 < 5", &mut state), 0);
    assert_eq!(eval(b"5 <= 5", &mut state), 1);
    assert_eq!(eval(b"5 > 4", &mut state), 1);
    assert_eq!(eval(b"5 >= 5", &mut state), 1);
}

#[test]
fn test_bitwise_operators() {
    let mut state = ShellState::new();

    assert_eq!(eval(b"5 & 3", &mut state), 1); // 101 & 011 = 001
    assert_eq!(eval(b"5 | 3", &mut state), 7); // 101 | 011 = 111
    assert_eq!(eval(b"5 ^ 3", &mut state), 6); // 101 ^ 011 = 110
    assert_eq!(eval(b"1 << 3", &mut state), 8);
    assert_eq!(eval(b"8 >> 2", &mut state), 2);
}

#[test]
fn test_logical_operators() {
    let mut state = ShellState::new();

    assert_eq!(eval(b"5 && 3", &mut state), 1);
    assert_eq!(eval(b"5 && 0", &mut state), 0);
    assert_eq!(eval(b"0 && 3", &mut state), 0);
    assert_eq!(eval(b"5 || 3", &mut state), 1);
    assert_eq!(eval(b"5 || 0", &mut state), 1);
    assert_eq!(eval(b"0 || 0", &mut state), 0);
}

#[test]
fn test_conditional_operator() {
    let mut state = ShellState::new();

    assert_eq!(eval(b"1 ? 10 : 20", &mut state), 10);
    assert_eq!(eval(b"0 ? 10 : 20", &mut state), 20);
    assert_eq!(eval(b"1 + 1 ? 5 * 2 : 3 * 3", &mut state), 10);
}

#[test]
fn test_variables_and_assignment() {
    let mut state = ShellState::new();

    state.set_var(b"A", b"5");
    assert_eq!(eval(b"A + 1", &mut state), 6);
    assert_eq!(eval(b"B + 1", &mut state), 1); // Unset is 0

    assert_eq!(eval(b"C = 10", &mut state), 10);
    assert_eq!(state.get_var(b"C").unwrap(), "10");

    assert_eq!(eval(b"C += 5", &mut state), 15);
    assert_eq!(state.get_var(b"C").unwrap(), "15");

    assert_eq!(eval(b"C -= 3", &mut state), 12);
    assert_eq!(eval(b"C *= 2", &mut state), 24);
    assert_eq!(eval(b"C /= 4", &mut state), 6);
    assert_eq!(eval(b"C %= 4", &mut state), 2);

    state.set_var(b"D", b"2");
    assert_eq!(eval(b"D <<= 2", &mut state), 8);
    assert_eq!(eval(b"D >>= 1", &mut state), 4);

    state.set_var(b"E", b"5");
    assert_eq!(eval(b"E &= 3", &mut state), 1);
    assert_eq!(eval(b"E |= 6", &mut state), 7);
    assert_eq!(eval(b"E ^= 3", &mut state), 4);
}

#[test]
fn test_nested_variables() {
    let mut state = ShellState::new();

    state.set_var(b"A", b"B + 1");
    state.set_var(b"B", b"5");
    assert_eq!(eval(b"A + 1", &mut state), 7);
}

#[test]
fn test_errors() {
    let mut state = ShellState::new();

    assert!(eval_res(b"5 / 0", &mut state).is_err());
    assert!(eval_res(b"5 % 0", &mut state).is_err());
    assert!(eval_res(b"C /= 0", &mut state).is_err());
    assert!(eval_res(b"C %= 0", &mut state).is_err());

    // Loop detection
    state.set_var(b"A", b"B");
    state.set_var(b"B", b"A");
    assert!(eval_res(b"A", &mut state).is_err());

    // Bad assignment LHS
    assert!(eval_res(b"5 = 10", &mut state).is_err());
    assert!(eval_res(b"1 + 2 = 10", &mut state).is_err());
}

#[test]
fn test_arithmetic_invalid_byte() {
    let mut state = ShellState::new();
    let res = eval_res(b"1 + @", &mut state);
    assert_eq!(res, Err("Invalid byte in arithmetic expression: 0x40".to_string()));
}

#[test]
fn test_arithmetic_unexpected_end_unary() {
    let mut state = ShellState::new();
    let res = eval_res(b"1 + -", &mut state);
    assert_eq!(res, Err("Unexpected end of expression".to_string()));
}

#[test]
fn test_arithmetic_unexpected_end_factor() {
    let mut state = ShellState::new();
    let res = eval_res(b"1 +", &mut state);
    assert_eq!(res, Err("Unexpected end of expression".to_string()));
}

#[test]
fn test_arithmetic_nounset_error() {
    let mut state = ShellState::new();
    state.set_option_by_name(BStr::new(b"nounset"), true).unwrap();
    let res = eval_res(b"UNSET_VAR + 1", &mut state);
    assert_eq!(res, Err("UNSET_VAR: parameter not set".to_string()));
}

#[test]
fn test_arithmetic_unclosed_paren() {
    let mut state = ShellState::new();
    let res = eval_res(b"(1 + 2", &mut state);
    assert_eq!(res, Err("Expected matching ')' in arithmetic expression".to_string()));
}

#[test]
fn test_arithmetic_unexpected_token() {
    let mut state = ShellState::new();
    let res = eval_res(b")", &mut state);
    assert_eq!(res, Err("Unexpected token in factor: RParen".to_string()));
}

#[test]
fn test_arithmetic_recursion_limit() {
    let mut state = ShellState::new();
    for i in 0..33 {
        let name = format!("V{}", i);
        let val = format!("V{}", i + 1);
        state.set_var(BStr::new(name.as_bytes()), BStr::new(val.as_bytes()));
    }
    let res = eval_res(b"V0", &mut state);
    assert_eq!(res, Err("Recursion limit exceeded in variable arithmetic expansion".to_string()));
}

#[test]
fn test_arithmetic_missing_colon_in_ternary() {
    let mut state = ShellState::new();
    let res = eval_res(b"1 ? 2", &mut state);
    assert_eq!(res, Err("Expected ':' in conditional expression".to_string()));
}
#[test]
fn test_arithmetic_overflow() {
    let mut state = ShellState::new();
    assert_eq!(eval(b"9223372036854775807 + 1", &mut state), i64::MIN);
    assert_eq!(eval(b"-9223372036854775808 - 1", &mut state), i64::MAX);
    assert_eq!(eval(b"9223372036854775807 * 2", &mut state), -2);
    assert_eq!(eval(b"-(-9223372036854775808)", &mut state), i64::MIN);
    assert_eq!(eval(b"-9223372036854775808 / -1", &mut state), i64::MIN);
    eval(b"X = 9223372036854775807", &mut state);
    assert_eq!(eval(b"X += 1", &mut state), i64::MIN);
}
