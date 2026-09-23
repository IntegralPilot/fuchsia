// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::execution_context::ExecutionContext;
use super::expand::parse_and_expand_modifier;
use super::state::ShellState;
use crate::collections::FlatSet;
use bstr::{BStr, BString, ByteSlice};

#[derive(Clone, Debug, PartialEq, Eq)]
enum ArithToken {
    Num(i64),
    Var(BString),
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    LParen,
    RParen,
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    AndAssign,
    OrAssign,
    XorAssign,
    ShlAssign,
    ShrAssign,
    LogAnd,
    LogOr,
    LogNot,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BitAnd,
    BitOr,
    BitXor,
    BitNot,
    Shl,
    Shr,
    Question,
    Colon,
}

fn parse_arith_int(bytes: &[u8]) -> Option<i64> {
    let trimmed = bytes.trim_ascii();
    if trimmed.is_empty() {
        return None;
    }
    let (negative, digits) = match trimmed[0] {
        b'-' => (true, &trimmed[1..]),
        b'+' => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() || !digits.iter().all(|&b| b.is_ascii_digit()) {
        return None;
    }
    let mut val: i64 = 0;
    for &digit in digits {
        val = val.wrapping_mul(10).wrapping_add((digit - b'0') as i64);
    }
    Some(if negative { val.wrapping_neg() } else { val })
}

fn tokenize_arith(input: &BStr) -> Result<Vec<ArithToken>, String> {
    let mut tokens = Vec::new();
    let mut bytes = input.as_bytes().iter().copied().peekable();
    while let Some(&ch) = bytes.peek() {
        match ch {
            b' ' | b'\t' | b'\n' | b'\r' => {
                bytes.next();
            }
            b'(' => {
                tokens.push(ArithToken::LParen);
                bytes.next();
            }
            b')' => {
                tokens.push(ArithToken::RParen);
                bytes.next();
            }
            b'?' => {
                tokens.push(ArithToken::Question);
                bytes.next();
            }
            b':' => {
                tokens.push(ArithToken::Colon);
                bytes.next();
            }
            b'~' => {
                tokens.push(ArithToken::BitNot);
                bytes.next();
            }
            b'+' => {
                bytes.next();
                if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::AddAssign);
                } else {
                    tokens.push(ArithToken::Plus);
                }
            }
            b'-' => {
                bytes.next();
                if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::SubAssign);
                } else {
                    tokens.push(ArithToken::Minus);
                }
            }
            b'*' => {
                bytes.next();
                if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::MulAssign);
                } else {
                    tokens.push(ArithToken::Mul);
                }
            }
            b'/' => {
                bytes.next();
                if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::DivAssign);
                } else {
                    tokens.push(ArithToken::Div);
                }
            }
            b'%' => {
                bytes.next();
                if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::ModAssign);
                } else {
                    tokens.push(ArithToken::Mod);
                }
            }
            b'=' => {
                bytes.next();
                if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::Eq);
                } else {
                    tokens.push(ArithToken::Assign);
                }
            }
            b'!' => {
                bytes.next();
                if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::Ne);
                } else {
                    tokens.push(ArithToken::LogNot);
                }
            }
            b'<' => {
                bytes.next();
                if bytes.peek() == Some(&b'<') {
                    bytes.next();
                    if bytes.peek() == Some(&b'=') {
                        bytes.next();
                        tokens.push(ArithToken::ShlAssign);
                    } else {
                        tokens.push(ArithToken::Shl);
                    }
                } else if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::Le);
                } else {
                    tokens.push(ArithToken::Lt);
                }
            }
            b'>' => {
                bytes.next();
                if bytes.peek() == Some(&b'>') {
                    bytes.next();
                    if bytes.peek() == Some(&b'=') {
                        bytes.next();
                        tokens.push(ArithToken::ShrAssign);
                    } else {
                        tokens.push(ArithToken::Shr);
                    }
                } else if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::Ge);
                } else {
                    tokens.push(ArithToken::Gt);
                }
            }
            b'&' => {
                bytes.next();
                if bytes.peek() == Some(&b'&') {
                    bytes.next();
                    tokens.push(ArithToken::LogAnd);
                } else if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::AndAssign);
                } else {
                    tokens.push(ArithToken::BitAnd);
                }
            }
            b'|' => {
                bytes.next();
                if bytes.peek() == Some(&b'|') {
                    bytes.next();
                    tokens.push(ArithToken::LogOr);
                } else if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::OrAssign);
                } else {
                    tokens.push(ArithToken::BitOr);
                }
            }
            b'^' => {
                bytes.next();
                if bytes.peek() == Some(&b'=') {
                    bytes.next();
                    tokens.push(ArithToken::XorAssign);
                } else {
                    tokens.push(ArithToken::BitXor);
                }
            }
            b if b.is_ascii_digit() => {
                let mut num_bytes = Vec::new();
                while let Some(&digit) = bytes.peek() {
                    if digit.is_ascii_digit() {
                        num_bytes.push(digit);
                        bytes.next();
                    } else {
                        break;
                    }
                }
                let val = parse_arith_int(&num_bytes)
                    .ok_or_else(|| "invalid integer constant".to_string())?;
                tokens.push(ArithToken::Num(val));
            }
            b if b.is_ascii_alphabetic() || b == b'_' => {
                let mut var_bytes = Vec::new();
                while let Some(&alphanumeric) = bytes.peek() {
                    if alphanumeric.is_ascii_alphanumeric() || alphanumeric == b'_' {
                        var_bytes.push(alphanumeric);
                        bytes.next();
                    } else {
                        break;
                    }
                }
                tokens.push(ArithToken::Var(BString::from(var_bytes)));
            }
            _ => return Err(format!("Invalid byte in arithmetic expression: {:#x}", ch)),
        }
    }
    Ok(tokens)
}

fn evaluate_assignment(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let start_pos = *pos;
    let val = evaluate_conditional(tokens, pos, state, ctx, visited)?;
    if *pos < tokens.len() {
        let op = &tokens[*pos];
        if is_assignment_operator(op) {
            *pos += 1;
            let var_name = match tokens.get(start_pos) {
                Some(ArithToken::Var(name)) if start_pos + 1 == *pos - 1 => name.clone(),
                _ => return Err("Left-hand side of assignment must be a variable".to_string()),
            };
            let right_val = evaluate_assignment(tokens, pos, state, ctx, visited)?;
            let current_val = {
                let val_str = state.get_var(&var_name).unwrap_or_default();
                parse_arith_int(val_str.as_bytes()).unwrap_or(0)
            };
            let new_val = match op {
                ArithToken::Assign => right_val,
                ArithToken::AddAssign => current_val.wrapping_add(right_val),
                ArithToken::SubAssign => current_val.wrapping_sub(right_val),
                ArithToken::MulAssign => current_val.wrapping_mul(right_val),
                ArithToken::DivAssign => {
                    if right_val == 0 {
                        return Err("Division by zero (/=)".to_string());
                    }
                    current_val.wrapping_div(right_val)
                }
                ArithToken::ModAssign => {
                    if right_val == 0 {
                        return Err("Modulo by zero (%=)".to_string());
                    }
                    current_val.wrapping_rem(right_val)
                }
                ArithToken::AndAssign => current_val & right_val,
                ArithToken::OrAssign => current_val | right_val,
                ArithToken::XorAssign => current_val ^ right_val,
                ArithToken::ShlAssign => current_val.wrapping_shl(right_val as u32),
                ArithToken::ShrAssign => current_val.wrapping_shr(right_val as u32),
                _ => unreachable!(),
            };
            let new_val_str = new_val.to_string();
            state.set_var(&var_name, new_val_str.as_bytes());
            return Ok(new_val);
        }
    }
    Ok(val)
}

fn is_assignment_operator(op: &ArithToken) -> bool {
    matches!(
        op,
        ArithToken::Assign
            | ArithToken::AddAssign
            | ArithToken::SubAssign
            | ArithToken::MulAssign
            | ArithToken::DivAssign
            | ArithToken::ModAssign
            | ArithToken::AndAssign
            | ArithToken::OrAssign
            | ArithToken::XorAssign
            | ArithToken::ShlAssign
            | ArithToken::ShrAssign
    )
}

fn evaluate_conditional(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let cond = evaluate_logical_or(tokens, pos, state, ctx, visited)?;
    if *pos < tokens.len() && tokens[*pos] == ArithToken::Question {
        *pos += 1;
        let true_expr = evaluate_assignment(tokens, pos, state, ctx, visited)?;
        if *pos >= tokens.len() || tokens[*pos] != ArithToken::Colon {
            return Err("Expected ':' in conditional expression".to_string());
        }
        *pos += 1;
        let false_expr = evaluate_assignment(tokens, pos, state, ctx, visited)?;
        if cond != 0 { Ok(true_expr) } else { Ok(false_expr) }
    } else {
        Ok(cond)
    }
}

fn evaluate_logical_or(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_logical_and(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() && tokens[*pos] == ArithToken::LogOr {
        *pos += 1;
        let right = evaluate_logical_and(tokens, pos, state, ctx, visited)?;
        val = if val != 0 || right != 0 { 1 } else { 0 };
    }
    Ok(val)
}

fn evaluate_logical_and(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_bitwise_or(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() && tokens[*pos] == ArithToken::LogAnd {
        *pos += 1;
        let right = evaluate_bitwise_or(tokens, pos, state, ctx, visited)?;
        val = if val != 0 && right != 0 { 1 } else { 0 };
    }
    Ok(val)
}

fn evaluate_bitwise_or(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_bitwise_xor(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() && tokens[*pos] == ArithToken::BitOr {
        *pos += 1;
        let right = evaluate_bitwise_xor(tokens, pos, state, ctx, visited)?;
        val |= right;
    }
    Ok(val)
}

fn evaluate_bitwise_xor(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_bitwise_and(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() && tokens[*pos] == ArithToken::BitXor {
        *pos += 1;
        let right = evaluate_bitwise_and(tokens, pos, state, ctx, visited)?;
        val ^= right;
    }
    Ok(val)
}

fn evaluate_bitwise_and(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_equality(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() && tokens[*pos] == ArithToken::BitAnd {
        *pos += 1;
        let right = evaluate_equality(tokens, pos, state, ctx, visited)?;
        val &= right;
    }
    Ok(val)
}

fn evaluate_equality(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_relational(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() {
        let op = &tokens[*pos];
        if op == &ArithToken::Eq {
            *pos += 1;
            let right = evaluate_relational(tokens, pos, state, ctx, visited)?;
            val = if val == right { 1 } else { 0 };
        } else if op == &ArithToken::Ne {
            *pos += 1;
            let right = evaluate_relational(tokens, pos, state, ctx, visited)?;
            val = if val != right { 1 } else { 0 };
        } else {
            break;
        }
    }
    Ok(val)
}

fn evaluate_relational(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_shift(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() {
        let op = &tokens[*pos];
        match op {
            ArithToken::Lt => {
                *pos += 1;
                let right = evaluate_shift(tokens, pos, state, ctx, visited)?;
                val = if val < right { 1 } else { 0 };
            }
            ArithToken::Le => {
                *pos += 1;
                let right = evaluate_shift(tokens, pos, state, ctx, visited)?;
                val = if val <= right { 1 } else { 0 };
            }
            ArithToken::Gt => {
                *pos += 1;
                let right = evaluate_shift(tokens, pos, state, ctx, visited)?;
                val = if val > right { 1 } else { 0 };
            }
            ArithToken::Ge => {
                *pos += 1;
                let right = evaluate_shift(tokens, pos, state, ctx, visited)?;
                val = if val >= right { 1 } else { 0 };
            }
            _ => break,
        }
    }
    Ok(val)
}

fn evaluate_shift(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_additive(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() {
        let op = &tokens[*pos];
        match op {
            ArithToken::Shl => {
                *pos += 1;
                let right = evaluate_additive(tokens, pos, state, ctx, visited)?;
                val = val.wrapping_shl(right as u32);
            }
            ArithToken::Shr => {
                *pos += 1;
                let right = evaluate_additive(tokens, pos, state, ctx, visited)?;
                val = val.wrapping_shr(right as u32);
            }
            _ => break,
        }
    }
    Ok(val)
}

fn evaluate_additive(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_multiplicative(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            ArithToken::Plus => {
                *pos += 1;
                let right = evaluate_multiplicative(tokens, pos, state, ctx, visited)?;
                val = val.wrapping_add(right);
            }
            ArithToken::Minus => {
                *pos += 1;
                let right = evaluate_multiplicative(tokens, pos, state, ctx, visited)?;
                val = val.wrapping_sub(right);
            }
            _ => break,
        }
    }
    Ok(val)
}

fn evaluate_multiplicative(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let mut val = evaluate_unary(tokens, pos, state, ctx, visited)?;
    while *pos < tokens.len() {
        match tokens[*pos] {
            ArithToken::Mul => {
                *pos += 1;
                let right = evaluate_unary(tokens, pos, state, ctx, visited)?;
                val = val.wrapping_mul(right);
            }
            ArithToken::Div => {
                *pos += 1;
                let right = evaluate_unary(tokens, pos, state, ctx, visited)?;
                if right == 0 {
                    return Err("Division by zero".to_string());
                }
                val = val.wrapping_div(right);
            }
            ArithToken::Mod => {
                *pos += 1;
                let right = evaluate_unary(tokens, pos, state, ctx, visited)?;
                if right == 0 {
                    return Err("Modulo by zero".to_string());
                }
                val = val.wrapping_rem(right);
            }
            _ => break,
        }
    }
    Ok(val)
}

fn evaluate_unary(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    if *pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    match &tokens[*pos] {
        ArithToken::Plus => {
            *pos += 1;
            evaluate_unary(tokens, pos, state, ctx, visited)
        }
        ArithToken::Minus => {
            *pos += 1;
            let val = evaluate_unary(tokens, pos, state, ctx, visited)?;
            Ok(val.wrapping_neg())
        }
        ArithToken::BitNot => {
            *pos += 1;
            let val = evaluate_unary(tokens, pos, state, ctx, visited)?;
            Ok(!val)
        }
        ArithToken::LogNot => {
            *pos += 1;
            let val = evaluate_unary(tokens, pos, state, ctx, visited)?;
            Ok(if val == 0 { 1 } else { 0 })
        }
        _ => evaluate_factor(tokens, pos, state, ctx, visited),
    }
}

fn evaluate_factor(
    tokens: &[ArithToken],
    pos: &mut usize,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    if *pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    match &tokens[*pos] {
        ArithToken::Num(val) => {
            *pos += 1;
            Ok(*val)
        }
        ArithToken::Var(name) => {
            *pos += 1;
            let var_name = name.as_bstr();
            if state.opt_nounset && state.get_var(var_name).is_none() {
                let msg = format!("{}: parameter not set", name);
                ctx.print_err(&msg)?;
                return Err(msg);
            }
            evaluate_variable_arithmetic_nested(var_name, state, ctx, visited)
        }
        ArithToken::LParen => {
            *pos += 1;
            let val = evaluate_assignment(tokens, pos, state, ctx, visited)?;
            if *pos >= tokens.len() || tokens[*pos] != ArithToken::RParen {
                return Err("Expected matching ')' in arithmetic expression".to_string());
            }
            *pos += 1;
            Ok(val)
        }
        t => Err(format!("Unexpected token in factor: {:?}", t)),
    }
}

fn evaluate_variable_arithmetic_nested(
    name: &BStr,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let var_name = name.to_owned();
    if visited.contains(&var_name) {
        return Err(format!("Loop detected in variable arithmetic expansion: {}", name));
    }
    if visited.len() >= 32 {
        return Err("Recursion limit exceeded in variable arithmetic expansion".to_string());
    }
    visited.insert(var_name.clone());

    let val_str = state.get_var(var_name.as_bstr()).unwrap_or_default();
    let val_str_trimmed = val_str.trim_ascii();
    if val_str_trimmed.is_empty() {
        visited.remove(&var_name);
        return Ok(0);
    }

    let res = if let Some(val) = parse_arith_int(val_str_trimmed) {
        Ok(val)
    } else {
        evaluate_arithmetic_recursive(val_str_trimmed.as_bstr(), state, ctx, visited)
    };

    visited.remove(&var_name);
    res
}

/// Evaluates a shell arithmetic expression string (e.g., `"1 + 2 * x"`).
///
/// Modifiers inside variable expansions are expanded first. Variable references
/// within the expression are looked up in `state`; if uninitialized, they evaluate to `0`.
/// Supports standard C-style arithmetic, bitwise operators, logical comparisons,
/// assignments (`=`, `+=`, etc.), and ternary operator (`? :`).
///
/// Returns the integer result of the evaluation, or an error string if syntax or calculation
/// errors occur.
pub fn evaluate_arithmetic(
    expr: &BStr,
    state: &mut ShellState,
    ctx: &ExecutionContext,
) -> Result<i64, String> {
    let mut visited = FlatSet::new();
    evaluate_arithmetic_recursive(expr, state, ctx, &mut visited)
}

fn evaluate_arithmetic_recursive(
    expr: &BStr,
    state: &mut ShellState,
    ctx: &ExecutionContext,
    visited: &mut FlatSet<BString>,
) -> Result<i64, String> {
    let expanded = parse_and_expand_modifier(expr, state, ctx)?;
    let trimmed = expanded.trim_ascii();
    let tokens = tokenize_arith(trimmed.as_bstr())?;
    let mut pos = 0;
    evaluate_assignment(&tokens, &mut pos, state, ctx, visited)
}
