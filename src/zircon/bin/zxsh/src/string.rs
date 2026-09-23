// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bstr::{BStr, BString, ByteSlice, ByteVec};
use std::ffi::{CStr, CString, NulError};
use std::path::{Path, PathBuf};

/// Converts a `PathBuf` into a `BString` without allocating a new buffer if possible.
pub fn path_buf_to_bstring(path: PathBuf) -> Option<BString> {
    Vec::from_path_buf(path).ok().map(BString::from)
}

/// Converts a `&Path` into a `BString`.
#[allow(dead_code)]
pub fn path_to_bstring(path: &Path) -> Option<BString> {
    <[u8]>::from_path(path).map(BString::from)
}

/// Parse an integer of generic type `T` from a byte slice, ignoring leading and trailing ASCII
/// whitespace. Supports optional leading '+' or '-'.
///
/// **When to use**: Use [`parse_int`] when parsing arbitrary signed or unsigned integer types
/// (including negative numbers or types like `usize`/`u64`/`i32`). For validating shell
/// numerical arguments where negative values or overflows above `i32::MAX` are forbidden, use
/// [`parse_non_negative_int`].
pub fn parse_int<T: std::str::FromStr>(bytes: &[u8]) -> Option<T> {
    let trimmed = bytes.trim_ascii();
    if trimmed.is_empty() {
        return None;
    }
    std::str::from_utf8(trimmed).ok()?.parse::<T>().ok()
}

/// Parse a non-negative 32-bit signed integer (in `0..=i32::MAX`) from a byte slice, ignoring
/// leading and trailing ASCII whitespace.
///
/// **When to use**: Use [`parse_non_negative_int`] when parsing and validating non-negative
/// numerical shell builtin arguments (such as for `exit`, `return`, `shift`, or `trap`).
/// Unlike [`parse_int`], this function rejects negative numbers, overflow values greater than
/// `i32::MAX`, empty inputs, or non-numeric strings.
#[allow(dead_code)]
pub fn parse_non_negative_int(bytes: &[u8]) -> Option<i32> {
    let trimmed = bytes.trim_ascii();
    if trimmed.is_empty() {
        return None;
    }
    let str_val = std::str::from_utf8(trimmed).ok()?;
    let val = str_val.parse::<i64>().ok()?;
    if (0..=i32::MAX as i64).contains(&val) { Some(val as i32) } else { None }
}

/// Parse a file mode mask (in octal or symbolic format) from a byte slice.
/// `current_umask` is used as the baseline when evaluating symbolic mode modifications (+, -, =).
/// Returns `Some(new_umask)` if the mode string is valid, or `None` if invalid.
pub fn parse_mode_mask(bytes: &[u8], current_umask: u32) -> Option<u32> {
    let trimmed = bytes.trim_ascii();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed[0].is_ascii_digit() {
        let mut new_mask: u32 = 0;
        for &b in trimmed {
            if !(b'0'..=b'7').contains(&b) {
                return None;
            }
            new_mask = (new_mask << 3) + u32::from(b - b'0');
        }
        if new_mask > 0o777 {
            return None;
        }
        return Some(new_mask);
    }

    let mask = 0o777 & !current_umask;
    let mut new_mask = mask;
    let mut idx = 0;
    let mut positions = 0u32;

    while idx < trimmed.len() {
        while idx < trimmed.len() && matches!(trimmed[idx], b'a' | b'u' | b'g' | b'o') {
            match trimmed[idx] {
                b'a' => positions |= 0o111,
                b'u' => positions |= 0o100,
                b'g' => positions |= 0o010,
                b'o' => positions |= 0o001,
                _ => {}
            }
            idx += 1;
        }
        let pos = if positions == 0 { 0o111 } else { positions };
        if idx >= trimmed.len() {
            break;
        }
        let op = trimmed[idx];
        if op != b'=' && op != b'+' && op != b'-' {
            break;
        }
        idx += 1;

        let mut new_val = 0u32;
        while idx < trimmed.len()
            && matches!(trimmed[idx], b'r' | b'w' | b'x' | b'u' | b'g' | b'o' | b'X' | b's')
        {
            match trimmed[idx] {
                b'r' => new_val |= 0o4,
                b'w' => new_val |= 0o2,
                b'x' => new_val |= 0o1,
                b'u' => new_val |= (mask >> 6) & 7,
                b'g' => new_val |= (mask >> 3) & 7,
                b'o' => new_val |= mask & 7,
                b'X' => {
                    if (mask & 0o111) != 0 {
                        new_val |= 0o1;
                    }
                }
                b's' => {}
                _ => {}
            }
            idx += 1;
        }

        new_val = (new_val & 0o7) * pos;
        match op {
            b'-' => new_mask &= !new_val,
            b'=' => new_mask = new_val | (new_mask & !(pos * 0o7)),
            b'+' => new_mask |= new_val,
            _ => unreachable!(),
        }

        if idx < trimmed.len() {
            if trimmed[idx] == b',' {
                positions = 0;
                idx += 1;
            } else if !matches!(trimmed[idx], b'=' | b'+' | b'-') {
                break;
            }
        }
    }

    if idx != trimmed.len() {
        return None;
    }

    Some(0o777 & !new_mask)
}

/// Converts a byte slice (e.g., `&BStr` or `&BString`) into a `CString` for FFI calls.
pub fn bstr_to_cstring(s: &[u8]) -> Result<CString, NulError> {
    CString::new(s)
}

/// Converts a slice of `BString`s into a vector of `CString`s for FFI calls.
pub fn bstrings_to_cstrings(strings: &[BString]) -> Result<Vec<CString>, NulError> {
    strings.iter().map(|s| bstr_to_cstring(s.as_slice())).collect()
}

/// Converts a slice of `CString`s into a vector of `&CStr` references for FFI calls.
pub fn cstrings_to_c_strs(cstrings: &[CString]) -> Vec<&CStr> {
    cstrings.iter().map(|s| s.as_c_str()).collect()
}

/// Returns `true` if `name` is a valid shell variable name (a letter or underscore
/// followed by zero or more letters, underscores, and digits).
pub fn is_valid_var_name(name: &BStr) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes[1..].iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Splits a byte slice around the first `=` character into `(key, value)`.
/// Returns `Some((&BStr, &BStr))` if `=` is present, or `None` if there is no `=`.
pub fn split_key_value(bytes: &[u8]) -> Option<(&BStr, &BStr)> {
    bytes.find_byte(b'=').map(|idx| (BStr::new(&bytes[..idx]), BStr::new(&bytes[idx + 1..])))
}

/// Formats a byte string into a single-quoted string suitable for shell input,
/// matching dash's `single_quote` function semantics.
pub fn single_quote(s: &BStr) -> BString {
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut s_slice = bytes;
    loop {
        let len = s_slice.iter().position(|&b| b == b'\'').unwrap_or(s_slice.len());
        result.push(b'\'');
        result.extend_from_slice(&s_slice[..len]);
        result.push(b'\'');
        s_slice = &s_slice[len..];

        let quote_len = s_slice.iter().take_while(|&&b| b == b'\'').count();
        if quote_len == 0 {
            break;
        }
        result.push(b'"');
        result.extend_from_slice(&s_slice[..quote_len]);
        result.push(b'"');
        s_slice = &s_slice[quote_len..];

        if s_slice.is_empty() {
            break;
        }
    }
    BString::from(result)
}

/// Represents a byte in an input line read by `read`, tracking whether it was escaped by a
/// backslash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineChar {
    pub byte: u8,
    pub escaped: bool,
}

impl LineChar {
    #[cfg(test)]
    pub fn unescaped(byte: u8) -> Self {
        Self { byte, escaped: false }
    }
}

/// Splits input `LineChar`s according to IFS rules for the shell `read` command into
/// `num_vars` fields.
pub fn split_ifs_read(input: &[LineChar], ifs: &BStr, num_vars: usize) -> Vec<BString> {
    if num_vars == 0 {
        return Vec::new();
    }

    let is_ifs_whitespace = |ch: &LineChar| {
        !ch.escaped
            && (ch.byte == b' ' || ch.byte == b'\t' || ch.byte == b'\n')
            && ifs.as_bytes().contains(&ch.byte)
    };
    let is_ifs_non_whitespace =
        |ch: &LineChar| !ch.escaped && !is_ifs_whitespace(ch) && ifs.as_bytes().contains(&ch.byte);

    let mut fields = Vec::new();
    let mut start = 0;

    // Skip leading IFS whitespace
    while start < input.len() && is_ifs_whitespace(&input[start]) {
        start += 1;
    }

    for _ in 0..num_vars - 1 {
        if start >= input.len() {
            break;
        }

        let mut i = start;
        let mut delim_start = None;
        let mut delim_end = None;

        while i < input.len() {
            let ch = &input[i];
            if is_ifs_whitespace(ch) {
                if delim_start.is_none() {
                    delim_start = Some(i);
                }
                i += 1;
                while i < input.len() && is_ifs_whitespace(&input[i]) {
                    i += 1;
                }
                if i < input.len() && is_ifs_non_whitespace(&input[i]) {
                    i += 1;
                    while i < input.len() && is_ifs_whitespace(&input[i]) {
                        i += 1;
                    }
                }
                delim_end = Some(i);
                break;
            } else if is_ifs_non_whitespace(ch) {
                delim_start = Some(i);
                i += 1;
                while i < input.len() && is_ifs_whitespace(&input[i]) {
                    i += 1;
                }
                delim_end = Some(i);
                break;
            } else {
                i += 1;
            }
        }

        if let (Some(ds), Some(de)) = (delim_start, delim_end) {
            let field_bytes: Vec<u8> = input[start..ds].iter().map(|c| c.byte).collect();
            fields.push(BString::from(field_bytes));
            start = de;
        } else {
            let field_bytes: Vec<u8> = input[start..].iter().map(|c| c.byte).collect();
            fields.push(BString::from(field_bytes));
            start = input.len();
        }
    }

    if start < input.len() {
        let mut end = input.len();
        while end > start && is_ifs_whitespace(&input[end - 1]) {
            end -= 1;
        }
        let last_slice = &input[start..end];
        let ifs_non_whitespace_count =
            last_slice.iter().filter(|c| is_ifs_non_whitespace(c)).count();
        let mut last_bytes: Vec<u8> = last_slice.iter().map(|c| c.byte).collect();
        if ifs_non_whitespace_count == 1
            && last_slice.last().map_or(false, |c| is_ifs_non_whitespace(c))
        {
            last_bytes.pop();
        }
        fields.push(BString::from(last_bytes));
    }

    while fields.len() < num_vars {
        fields.push(BString::default());
    }

    fields
}

/// Splits input bytes according to IFS rules for the shell `read` command into `num_vars` fields.
#[cfg(test)]
pub fn split_ifs_read_bytes(input: &[u8], ifs: &BStr, num_vars: usize) -> Vec<BString> {
    let line_chars: Vec<LineChar> = input.iter().map(|&b| LineChar::unescaped(b)).collect();
    split_ifs_read(&line_chars, ifs, num_vars)
}

/// Returns the byte value for a standard escape character if valid.
pub fn parse_standard_escape(c: u8) -> Option<u8> {
    match c {
        b'a' => Some(b'\x07'),
        b'b' => Some(b'\x08'),
        b'f' => Some(b'\x0c'),
        b'n' => Some(b'\n'),
        b'r' => Some(b'\r'),
        b't' => Some(b'\t'),
        b'v' => Some(b'\x0b'),
        b'\\' => Some(b'\\'),
        _ => None,
    }
}

/// Parses up to `max_digits` octal digits (`0`..=`7`) from `slice` and returns `(val, num_digits)`.
pub fn parse_octal_digits(slice: &[u8], max_digits: usize) -> (u8, usize) {
    let mut val: u16 = 0;
    let mut count = 0;
    while count < max_digits && count < slice.len() && (b'0'..=b'7').contains(&slice[count]) {
        val = (val << 3) + (slice[count] - b'0') as u16;
        count += 1;
    }
    (val as u8, count)
}

/// Processes escape sequences in `arg` (e.g. for `echo -e` and `printf %b`).
/// Returns the processed bytes and a boolean indicating whether execution should halt (`\c`).
pub fn process_escape_bytes(arg: &[u8]) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(arg.len());
    let mut i = 0;
    while i < arg.len() {
        if arg[i] == b'\\' {
            if i + 1 < arg.len() {
                let next = arg[i + 1];
                if next == b'c' {
                    return (out, true);
                } else if let Some(b) = parse_standard_escape(next) {
                    out.push(b);
                    i += 2;
                } else if next == b'0' && i + 2 < arg.len() && (b'0'..=b'7').contains(&arg[i + 2]) {
                    let (val, count) = parse_octal_digits(&arg[i + 2..], 3);
                    out.push(val);
                    i += 2 + count;
                } else if (b'0'..=b'7').contains(&next) {
                    let (val, count) = parse_octal_digits(&arg[i + 1..], 3);
                    out.push(val);
                    i += 1 + count;
                } else {
                    out.push(b'\\');
                    i += 1;
                }
            } else {
                out.push(b'\\');
                i += 1;
            }
        } else {
            out.push(arg[i]);
            i += 1;
        }
    }
    (out, false)
}
