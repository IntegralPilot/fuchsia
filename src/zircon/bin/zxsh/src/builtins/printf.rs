// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EXIT_FAILURE, EXIT_SUCCESS, ShellState};
use crate::string::{parse_octal_digits, parse_standard_escape, process_escape_bytes};
use bstr::{BString, ByteSlice};
use std::io::{Read, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PrintfFlags {
    left_align: bool,
    show_sign: bool,
    space_sign: bool,
    alt_form: bool,
    zero_pad: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WidthOrPrec {
    Value(usize),
    FromArg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrintfSpec {
    flags: PrintfFlags,
    width: Option<WidthOrPrec>,
    precision: Option<WidthOrPrec>,
    spec_char: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FormatElement {
    Literal(Vec<u8>),
    Specifier(PrintfSpec),
}

fn parse_format_string(fmt: &[u8]) -> Result<Vec<FormatElement>, String> {
    let mut elements = Vec::new();
    let mut i = 0;
    let len = fmt.len();

    while i < len {
        if fmt[i] == b'\\' {
            i += 1;
            if i >= len {
                elements.push(FormatElement::Literal(vec![b'\\']));
                break;
            }
            let c = fmt[i];
            if let Some(b) = parse_standard_escape(c) {
                elements.push(FormatElement::Literal(vec![b]));
                i += 1;
            } else if (b'0'..=b'7').contains(&c) {
                let (val, count) = parse_octal_digits(&fmt[i..], 3);
                elements.push(FormatElement::Literal(vec![val]));
                i += count;
            } else {
                elements.push(FormatElement::Literal(vec![b'\\', c]));
                i += 1;
            }
            continue;
        }

        if fmt[i] == b'%' {
            i += 1;
            if i >= len {
                return Err("missing format character".to_string());
            }
            if fmt[i] == b'%' {
                elements.push(FormatElement::Literal(vec![b'%']));
                i += 1;
                continue;
            }

            let mut flags = PrintfFlags {
                left_align: false,
                show_sign: false,
                space_sign: false,
                alt_form: false,
                zero_pad: false,
            };

            while i < len {
                match fmt[i] {
                    b'-' => flags.left_align = true,
                    b'+' => flags.show_sign = true,
                    b' ' => flags.space_sign = true,
                    b'#' => flags.alt_form = true,
                    b'0' => flags.zero_pad = true,
                    _ => break,
                }
                i += 1;
            }

            let mut width = None;
            if i < len {
                if fmt[i] == b'*' {
                    width = Some(WidthOrPrec::FromArg);
                    i += 1;
                } else if fmt[i].is_ascii_digit() {
                    let mut w = 0usize;
                    while i < len && fmt[i].is_ascii_digit() {
                        w = w * 10 + (fmt[i] - b'0') as usize;
                        i += 1;
                    }
                    width = Some(WidthOrPrec::Value(w));
                }
            }

            let mut precision = None;
            if i < len && fmt[i] == b'.' {
                i += 1;
                if i < len && fmt[i] == b'*' {
                    precision = Some(WidthOrPrec::FromArg);
                    i += 1;
                } else {
                    let mut p = 0usize;
                    while i < len && fmt[i].is_ascii_digit() {
                        p = p * 10 + (fmt[i] - b'0') as usize;
                        i += 1;
                    }
                    precision = Some(WidthOrPrec::Value(p));
                }
            }

            if i >= len {
                return Err("missing format character".to_string());
            }

            let spec_char = fmt[i];
            i += 1;

            match spec_char {
                b's' | b'c' | b'b' | b'd' | b'i' | b'o' | b'u' | b'x' | b'X' | b'f' | b'F'
                | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                    elements.push(FormatElement::Specifier(PrintfSpec {
                        flags,
                        width,
                        precision,
                        spec_char,
                    }));
                }
                _ => {
                    return Err(format!("{}: invalid directive", spec_char as char));
                }
            }
            continue;
        }

        let start = i;
        while i < len && fmt[i] != b'\\' && fmt[i] != b'%' {
            i += 1;
        }
        elements.push(FormatElement::Literal(fmt[start..i].to_vec()));
    }

    Ok(elements)
}

fn get_num_arg(
    arg_opt: Option<&BString>,
    sign: bool,
    stderr: &mut dyn Write,
    status: &mut i32,
) -> u64 {
    let arg = match arg_opt {
        Some(a) => a,
        None => return 0,
    };
    let bytes = arg.as_bytes();
    if bytes.is_empty() {
        let _ = writeln!(stderr, "printf: : expected numeric value");
        *status = EXIT_FAILURE;
        return 0;
    }
    if bytes[0] == b'\'' || bytes[0] == b'"' {
        return bytes.get(1).copied().unwrap_or(0) as u64;
    }

    let s = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => {
            let _ = writeln!(stderr, "printf: {}: expected numeric value", arg);
            *status = EXIT_FAILURE;
            return 0;
        }
    };

    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        let _ = writeln!(stderr, "printf: {}: expected numeric value", arg);
        *status = EXIT_FAILURE;
        return 0;
    }

    let (is_neg, rest) = if sign && trimmed.starts_with('-') {
        (true, &trimmed[1..])
    } else if sign && trimmed.starts_with('+') {
        (false, &trimmed[1..])
    } else {
        (false, trimmed)
    };

    let (radix, num_str) = if rest.starts_with("0x") || rest.starts_with("0X") {
        (16, &rest[2..])
    } else if rest.starts_with('0') {
        (8, rest)
    } else {
        (10, rest)
    };

    let mut end_idx = 0;
    for (i, c) in num_str.char_indices() {
        if c.is_digit(radix) {
            end_idx = i + c.len_utf8();
        } else {
            break;
        }
    }

    if end_idx == 0 {
        let _ = writeln!(stderr, "printf: {}: expected numeric value", arg);
        *status = EXIT_FAILURE;
        return 0;
    }

    let valid_num_str = &num_str[..end_idx];
    let has_trailing = end_idx < num_str.len();

    let val = if sign {
        match i64::from_str_radix(valid_num_str, radix) {
            Ok(v) => {
                let final_v = if is_neg { v.wrapping_neg() } else { v };
                final_v as u64
            }
            Err(_) => {
                let _ = writeln!(stderr, "printf: {}: Out of range", arg);
                *status = EXIT_FAILURE;
                if is_neg { i64::MIN as u64 } else { i64::MAX as u64 }
            }
        }
    } else {
        match u64::from_str_radix(valid_num_str, radix) {
            Ok(v) => v,
            Err(_) => {
                let _ = writeln!(stderr, "printf: {}: Out of range", arg);
                *status = EXIT_FAILURE;
                u64::MAX
            }
        }
    };

    if has_trailing {
        let _ = writeln!(stderr, "printf: {}: value may be truncated", arg);
        *status = EXIT_FAILURE;
    }

    val
}

fn get_float_arg(arg_opt: Option<&BString>, stderr: &mut dyn Write, status: &mut i32) -> f64 {
    let arg = match arg_opt {
        Some(a) => a,
        None => return 0.0,
    };
    let bytes = arg.as_bytes();
    if bytes.is_empty() {
        let _ = writeln!(stderr, "printf: : expected numeric value");
        *status = EXIT_FAILURE;
        return 0.0;
    }
    if bytes[0] == b'\'' || bytes[0] == b'"' {
        return bytes.get(1).copied().unwrap_or(0) as f64;
    }

    let s = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => {
            let _ = writeln!(stderr, "printf: {}: expected numeric value", arg);
            *status = EXIT_FAILURE;
            return 0.0;
        }
    };

    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        let _ = writeln!(stderr, "printf: {}: expected numeric value", arg);
        *status = EXIT_FAILURE;
        return 0.0;
    }

    match trimmed.parse::<f64>() {
        Ok(v) => v,
        Err(_) => {
            let mut end = 0;
            for (i, c) in trimmed.char_indices() {
                if c.is_ascii_digit() || c == '.' || c == '+' || c == '-' || c == 'e' || c == 'E' {
                    end = i + c.len_utf8();
                } else {
                    break;
                }
            }
            if end == 0 {
                let _ = writeln!(stderr, "printf: {}: expected numeric value", arg);
                *status = EXIT_FAILURE;
                0.0
            } else {
                match trimmed[..end].parse::<f64>() {
                    Ok(v) => {
                        let _ = writeln!(stderr, "printf: {}: value may be truncated", arg);
                        *status = EXIT_FAILURE;
                        v
                    }
                    Err(_) => {
                        let _ = writeln!(stderr, "printf: {}: expected numeric value", arg);
                        *status = EXIT_FAILURE;
                        0.0
                    }
                }
            }
        }
    }
}

fn output_padded_bytes(
    bytes: &[u8],
    flags: PrintfFlags,
    width: Option<usize>,
    stdout: &mut dyn Write,
) {
    if let Some(w) = width {
        if w > bytes.len() {
            let pad_len = w - bytes.len();
            let pad_byte = if flags.zero_pad && !flags.left_align { b'0' } else { b' ' };
            let pad = vec![pad_byte; pad_len];

            if flags.left_align {
                let _ = stdout.write_all(bytes);
                let _ = stdout.write_all(&pad);
            } else {
                let _ = stdout.write_all(&pad);
                let _ = stdout.write_all(bytes);
            }
            return;
        }
    }
    let _ = stdout.write_all(bytes);
}

fn format_integer(
    val: u64,
    is_signed: bool,
    is_negative: bool,
    radix: u32,
    uppercase: bool,
    flags: PrintfFlags,
    width: Option<usize>,
    precision: Option<usize>,
    stdout: &mut dyn Write,
) {
    let digits_str = if val == 0 && precision == Some(0) {
        String::new()
    } else if radix == 8 {
        format!("{:o}", val)
    } else if radix == 16 {
        if uppercase { format!("{:X}", val) } else { format!("{:x}", val) }
    } else {
        format!("{}", val)
    };

    let zero_padded_digits = if let Some(p) = precision {
        if p > digits_str.len() {
            format!("{}{}", "0".repeat(p - digits_str.len()), digits_str)
        } else {
            digits_str
        }
    } else {
        digits_str
    };

    let prefix = if is_signed {
        if is_negative {
            "-"
        } else if flags.show_sign {
            "+"
        } else if flags.space_sign {
            " "
        } else {
            ""
        }
    } else {
        if radix == 8 && flags.alt_form {
            if !zero_padded_digits.starts_with('0') { "0" } else { "" }
        } else if radix == 16 && flags.alt_form && val != 0 {
            if uppercase { "0X" } else { "0x" }
        } else {
            ""
        }
    };

    write_formatted(prefix, &zero_padded_digits, flags, width, precision.is_none(), stdout);
}

fn write_formatted(
    prefix: &str,
    body: &str,
    flags: PrintfFlags,
    width: Option<usize>,
    allow_zero_pad: bool,
    stdout: &mut dyn Write,
) {
    let total_len = prefix.len() + body.len();
    if let Some(w) = width {
        if w > total_len {
            let pad_len = w - total_len;
            if allow_zero_pad && flags.zero_pad && !flags.left_align {
                let _ = stdout.write_all(prefix.as_bytes());
                let _ = stdout.write_all("0".repeat(pad_len).as_bytes());
                let _ = stdout.write_all(body.as_bytes());
            } else if flags.left_align {
                let _ = stdout.write_all(prefix.as_bytes());
                let _ = stdout.write_all(body.as_bytes());
                let _ = stdout.write_all(" ".repeat(pad_len).as_bytes());
            } else {
                let _ = stdout.write_all(" ".repeat(pad_len).as_bytes());
                let _ = stdout.write_all(prefix.as_bytes());
                let _ = stdout.write_all(body.as_bytes());
            }
            return;
        }
    }

    let _ = stdout.write_all(prefix.as_bytes());
    let _ = stdout.write_all(body.as_bytes());
}

fn format_float(
    val: f64,
    spec_char: u8,
    flags: PrintfFlags,
    width: Option<usize>,
    precision: Option<usize>,
    stdout: &mut dyn Write,
) {
    let prec = precision.unwrap_or(6);
    let abs_val = val.abs();
    let is_neg = val.is_sign_negative();

    let num_str = match spec_char {
        b'e' => format!("{:.1$e}", abs_val, prec),
        b'E' => format!("{:.1$E}", abs_val, prec),
        b'g' | b'G' => {
            let formatted = format!("{:.1$}", abs_val, prec);
            if !flags.alt_form && formatted.contains('.') {
                formatted.trim_end_matches('0').trim_end_matches('.').to_string()
            } else {
                formatted
            }
        }
        _ => format!("{:.1$}", abs_val, prec),
    };

    let prefix = if is_neg {
        "-"
    } else if flags.show_sign {
        "+"
    } else if flags.space_sign {
        " "
    } else {
        ""
    };

    write_formatted(prefix, &num_str, flags, width, true, stdout);
}

fn format_specifier(
    spec: &PrintfSpec,
    args: &[BString],
    arg_idx: &mut usize,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    status: &mut i32,
) -> bool {
    let mut flags = spec.flags;

    let width = match spec.width {
        Some(WidthOrPrec::FromArg) => {
            let arg_opt = args.get(*arg_idx);
            if arg_opt.is_some() {
                *arg_idx += 1;
            }
            let w = get_num_arg(arg_opt, true, stderr, status) as i64;
            if w < 0 {
                flags.left_align = true;
                Some((-w) as usize)
            } else {
                Some(w as usize)
            }
        }
        Some(WidthOrPrec::Value(v)) => Some(v),
        None => None,
    };

    let precision = match spec.precision {
        Some(WidthOrPrec::FromArg) => {
            let arg_opt = args.get(*arg_idx);
            if arg_opt.is_some() {
                *arg_idx += 1;
            }
            let p = get_num_arg(arg_opt, true, stderr, status) as i64;
            if p < 0 { None } else { Some(p as usize) }
        }
        Some(WidthOrPrec::Value(v)) => Some(v),
        None => None,
    };

    let spec_arg_opt = args.get(*arg_idx);
    if spec_arg_opt.is_some() {
        *arg_idx += 1;
    }

    match spec.spec_char {
        b's' => {
            let s_bytes = spec_arg_opt.map(|s| s.as_bytes()).unwrap_or(b"");
            let truncated = if let Some(p) = precision {
                if p < s_bytes.len() { &s_bytes[..p] } else { s_bytes }
            } else {
                s_bytes
            };

            output_padded_bytes(truncated, flags, width, stdout);
        }
        b'b' => {
            let s_bytes = spec_arg_opt.map(|s| s.as_bytes()).unwrap_or(b"");
            let (processed, halt) = process_escape_bytes(s_bytes);
            let truncated = if let Some(p) = precision {
                if p < processed.len() { &processed[..p] } else { &processed[..] }
            } else {
                &processed[..]
            };

            output_padded_bytes(truncated, flags, width, stdout);
            if halt {
                return true;
            }
        }
        b'c' => {
            let c_byte = spec_arg_opt.and_then(|s| s.first().copied()).unwrap_or(0);
            let c_slice = &[c_byte];
            output_padded_bytes(c_slice, flags, width, stdout);
        }
        b'd' | b'i' => {
            let num_u64 = get_num_arg(spec_arg_opt, true, stderr, status);
            let num = num_u64 as i64;
            let (abs_val, is_neg) =
                if num < 0 { (num.wrapping_neg() as u64, true) } else { (num as u64, false) };

            format_integer(abs_val, true, is_neg, 10, false, flags, width, precision, stdout);
        }
        b'u' => {
            let num = get_num_arg(spec_arg_opt, false, stderr, status);
            format_integer(num, false, false, 10, false, flags, width, precision, stdout);
        }
        b'o' => {
            let num = get_num_arg(spec_arg_opt, false, stderr, status);
            format_integer(num, false, false, 8, false, flags, width, precision, stdout);
        }
        b'x' => {
            let num = get_num_arg(spec_arg_opt, false, stderr, status);
            format_integer(num, false, false, 16, false, flags, width, precision, stdout);
        }
        b'X' => {
            let num = get_num_arg(spec_arg_opt, false, stderr, status);
            format_integer(num, false, false, 16, true, flags, width, precision, stdout);
        }
        b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
            let num = get_float_arg(spec_arg_opt, stderr, status);
            format_float(num, spec.spec_char, flags, width, precision, stdout);
        }
        _ => unreachable!(),
    }

    false
}

pub fn builtin_printf(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.is_empty() {
        let _ = writeln!(stderr, "printf: usage: printf format [arg ...]");
        return EXIT_FAILURE;
    }

    let format_bytes = args[0].as_bytes();
    let elements = match parse_format_string(format_bytes) {
        Ok(elems) => elems,
        Err(e) => {
            let _ = writeln!(stderr, "printf: {}", e);
            return EXIT_FAILURE;
        }
    };

    let positional_args = &args[1..];
    let mut arg_idx = 0;
    let mut exit_status = EXIT_SUCCESS;

    let mut first_run = true;
    while first_run || (arg_idx < positional_args.len() && arg_idx > 0) {
        let start_idx = arg_idx;
        first_run = false;

        for elem in &elements {
            match elem {
                FormatElement::Literal(lit) => {
                    let _ = stdout.write_all(lit);
                }
                FormatElement::Specifier(spec) => {
                    let halt = format_specifier(
                        spec,
                        positional_args,
                        &mut arg_idx,
                        stdout,
                        stderr,
                        &mut exit_status,
                    );
                    if halt {
                        return exit_status;
                    }
                }
            }
        }

        if arg_idx == start_idx {
            break;
        }
    }

    exit_status
}
