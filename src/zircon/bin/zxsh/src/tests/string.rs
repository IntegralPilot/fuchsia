// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::string::{
    LineChar, bstr_to_cstring, bstrings_to_cstrings, cstrings_to_c_strs, parse_int,
    parse_mode_mask, parse_non_negative_int, parse_octal_digits, parse_standard_escape,
    process_escape_bytes, split_ifs_read, split_ifs_read_bytes, split_key_value,
};
use bstr::{BStr, BString};

#[test]
fn test_parse_int() {
    assert_eq!(parse_int::<i32>(b"123"), Some(123));
    assert_eq!(parse_int::<i32>(b" -456 "), Some(-456));
    assert_eq!(parse_int::<u64>(b"42"), Some(42));
    assert_eq!(parse_int::<i32>(b"+789"), Some(789));

    // Invalid integer inputs
    assert_eq!(parse_int::<i32>(b"abc"), None);
    assert_eq!(parse_int::<i32>(b"12a3"), None);
    assert_eq!(parse_int::<i32>(b""), None);
    assert_eq!(parse_int::<i32>(b"   "), None);
}

#[test]
fn test_parse_non_negative_int() {
    assert_eq!(parse_non_negative_int(b"0"), Some(0));
    assert_eq!(parse_non_negative_int(b" 123 "), Some(123));
    assert_eq!(parse_non_negative_int(b"2147483647"), Some(i32::MAX));

    // Negative numbers, overflow, or non-numeric inputs return None
    assert_eq!(parse_non_negative_int(b"-1"), None);
    assert_eq!(parse_non_negative_int(b"-123"), None);
    assert_eq!(parse_non_negative_int(b"2147483648"), None);
    assert_eq!(parse_non_negative_int(b"abc"), None);
    assert_eq!(parse_non_negative_int(b""), None);
}

#[test]
fn test_parse_mode_mask_octal() {
    // Valid octal values
    assert_eq!(parse_mode_mask(b"022", 0o000), Some(0o022));
    assert_eq!(parse_mode_mask(b"000", 0o022), Some(0o000));
    assert_eq!(parse_mode_mask(b"777", 0o000), Some(0o777));
    assert_eq!(parse_mode_mask(b"0", 0o022), Some(0o000));
    assert_eq!(parse_mode_mask(b"7", 0o000), Some(0o007));
    assert_eq!(parse_mode_mask(b"27", 0o000), Some(0o027));
    assert_eq!(parse_mode_mask(b"0777", 0o000), Some(0o777));

    // Whitespace trimming
    assert_eq!(parse_mode_mask(b"  022  ", 0o000), Some(0o022));

    // Invalid octal values
    assert_eq!(parse_mode_mask(b"800", 0o022), None);
    assert_eq!(parse_mode_mask(b"999", 0o022), None);
    assert_eq!(parse_mode_mask(b"1000", 0o022), None);
    assert_eq!(parse_mode_mask(b"089", 0o022), None);
    assert_eq!(parse_mode_mask(b"", 0o022), None);
    assert_eq!(parse_mode_mask(b"   ", 0o022), None);
}

#[test]
fn test_parse_mode_mask_symbolic() {
    // Initial umask 0o022 -> Initial perm = 0o755 (u=rwx, g=rx, o=rx)

    // '=' operator
    assert_eq!(parse_mode_mask(b"u=rwx,g=rx,o=rx", 0o000), Some(0o022));
    assert_eq!(parse_mode_mask(b"u=rwx,g=rwx,o=rwx", 0o022), Some(0o000));
    assert_eq!(parse_mode_mask(b"a=rwx", 0o022), Some(0o000));
    assert_eq!(parse_mode_mask(b"a=", 0o000), Some(0o777));
    assert_eq!(parse_mode_mask(b"u=r", 0o000), Some(0o300)); // u=r (0400) -> perm=0477 -> mask=0300

    // '+' operator
    // perm 0o755 + a+w (0o222) = 0o777 -> mask 0o000
    assert_eq!(parse_mode_mask(b"a+w", 0o022), Some(0o000));
    // perm 0o755 + u+x (0o100) = 0o755 -> mask 0o022
    assert_eq!(parse_mode_mask(b"u+x", 0o022), Some(0o022));
    // perm 0o755 + g+w (0o020) = 0o775 -> mask 0o002
    assert_eq!(parse_mode_mask(b"g+w", 0o022), Some(0o002));

    // '-' operator
    // perm 0o777 - u-w (0o200) = 0o577 -> mask 0o200
    assert_eq!(parse_mode_mask(b"u-w", 0o000), Some(0o200));
    // perm 0o777 - 0o777 = 0o000 -> mask 0o777
    assert_eq!(parse_mode_mask(b"a-rwx", 0o000), Some(0o777));
    // perm 0o777 - go-rx (0o055) = 0o722 -> mask 0o055
    assert_eq!(parse_mode_mask(b"go-rx", 0o000), Some(0o055));

    // Implicit 'all' who
    assert_eq!(parse_mode_mask(b"+w", 0o022), Some(0o000));
    assert_eq!(parse_mode_mask(b"-rwx", 0o000), Some(0o777));
    assert_eq!(parse_mode_mask(b"=rwx", 0o022), Some(0o000));

    // Multiple combined clauses
    assert_eq!(parse_mode_mask(b"u=rwx,g=rx,o=", 0o000), Some(0o027));
    assert_eq!(parse_mode_mask(b"u+w,g-r", 0o022), Some(0o062));

    // Invalid symbolic strings
    assert_eq!(parse_mode_mask(b"z+r", 0o022), None);
    assert_eq!(parse_mode_mask(b"u+z", 0o022), None);
    assert_eq!(parse_mode_mask(b"urwx", 0o022), None);
    assert_eq!(parse_mode_mask(b"invalid", 0o022), None);
    assert_eq!(parse_mode_mask(&[0xff, 0xfe], 0o022), None);
}

#[test]
fn test_bstr_to_cstring() {
    let cs = bstr_to_cstring(b"hello world").expect("valid CString");
    assert_eq!(cs.to_bytes_with_nul(), b"hello world\0");

    // Invalid CString with embedded NUL byte
    assert!(bstr_to_cstring(b"hello\0world").is_err());
}

#[test]
fn test_bstrings_to_cstrings() {
    let bstrings = vec![BString::from("foo"), BString::from("bar")];
    let cstrings = bstrings_to_cstrings(&bstrings).expect("valid CStrings");
    assert_eq!(cstrings.len(), 2);
    assert_eq!(cstrings[0].to_bytes_with_nul(), b"foo\0");
    assert_eq!(cstrings[1].to_bytes_with_nul(), b"bar\0");

    // Array containing embedded NUL should error
    let invalid_bstrings = vec![BString::from("foo"), BString::from("b\0ar")];
    assert!(bstrings_to_cstrings(&invalid_bstrings).is_err());
}

#[test]
fn test_cstrings_to_c_strs() {
    let cs1 = std::ffi::CString::new("alpha").unwrap();
    let cs2 = std::ffi::CString::new("beta").unwrap();
    let cstrings = vec![cs1, cs2];
    let c_strs = cstrings_to_c_strs(&cstrings);
    assert_eq!(c_strs.len(), 2);
    assert_eq!(c_strs[0].to_bytes_with_nul(), b"alpha\0");
    assert_eq!(c_strs[1].to_bytes_with_nul(), b"beta\0");
}

#[test]
fn test_split_key_value() {
    assert_eq!(split_key_value(b"FOO=bar"), Some((BStr::new(b"FOO"), BStr::new(b"bar"))));
    assert_eq!(split_key_value(b"KEY="), Some((BStr::new(b"KEY"), BStr::new(b""))));
    assert_eq!(split_key_value(b"=VAL"), Some((BStr::new(b""), BStr::new(b"VAL"))));
    assert_eq!(
        split_key_value(b"A=B=C"),
        Some((BStr::new(b"A"), BStr::new(b"B=C"))) // Splits at first '='
    );
    assert_eq!(split_key_value(b"NOEQUALS"), None);
    assert_eq!(split_key_value(b""), None);
}

#[test]
fn test_split_ifs_read() {
    let default_ifs = BStr::new(b" \t\n");

    // Default IFS, single variable (gets entire line, stripped leading/trailing IFS whitespace)
    assert_eq!(
        split_ifs_read_bytes(b"hello world", default_ifs, 1),
        vec![BString::from("hello world")]
    );
    assert_eq!(
        split_ifs_read_bytes(b"   hello   world   ", default_ifs, 1),
        vec![BString::from("hello   world")]
    );

    // Default IFS, multiple variables
    assert_eq!(
        split_ifs_read_bytes(b"foo bar baz", default_ifs, 3),
        vec![BString::from("foo"), BString::from("bar"), BString::from("baz")]
    );
    assert_eq!(
        split_ifs_read_bytes(b"foo   bar   baz", default_ifs, 3),
        vec![BString::from("foo"), BString::from("bar"), BString::from("baz")]
    );

    // More variables than input fields
    assert_eq!(
        split_ifs_read_bytes(b"foo bar", default_ifs, 3),
        vec![BString::from("foo"), BString::from("bar"), BString::from("")]
    );

    // Fewer variables than input fields (last variable receives remaining line)
    assert_eq!(
        split_ifs_read_bytes(b"foo bar baz qux", default_ifs, 2),
        vec![BString::from("foo"), BString::from("bar baz qux")]
    );

    // Non-whitespace IFS (e.g. colon delimiter)
    let colon_ifs = BStr::new(b":");
    assert_eq!(
        split_ifs_read_bytes(b"foo:bar:baz", colon_ifs, 3),
        vec![BString::from("foo"), BString::from("bar"), BString::from("baz")]
    );
    assert_eq!(
        split_ifs_read_bytes(b"foo::baz", colon_ifs, 3),
        vec![BString::from("foo"), BString::from(""), BString::from("baz")]
    );

    // Empty input
    assert_eq!(
        split_ifs_read_bytes(b"", default_ifs, 2),
        vec![BString::from(""), BString::from("")]
    );
}

#[test]
fn test_split_ifs_read_zero_vars() {
    let default_ifs = BStr::new(b" \t\n");
    assert_eq!(split_ifs_read_bytes(b"foo bar", default_ifs, 0), Vec::<BString>::new());
}

#[test]
fn test_split_ifs_read_mixed_delimiters() {
    let space_colon_ifs = BStr::new(b" :");
    // Space before colon and space after colon
    assert_eq!(
        split_ifs_read_bytes(b"foo : bar", space_colon_ifs, 2),
        vec![BString::from("foo"), BString::from("bar")]
    );
    // Space before colon, no space after
    assert_eq!(
        split_ifs_read_bytes(b"foo :bar", space_colon_ifs, 2),
        vec![BString::from("foo"), BString::from("bar")]
    );
    // Colon before space
    assert_eq!(
        split_ifs_read_bytes(b"foo: bar", space_colon_ifs, 2),
        vec![BString::from("foo"), BString::from("bar")]
    );
}

#[test]
fn test_split_ifs_read_no_delimiters_remaining() {
    let default_ifs = BStr::new(b" \t\n");
    assert_eq!(
        split_ifs_read_bytes(b"foo", default_ifs, 3),
        vec![BString::from("foo"), BString::from(""), BString::from("")]
    );
}

#[test]
fn test_split_ifs_read_trailing_non_whitespace_delimiter() {
    let colon_ifs = BStr::new(b":");
    // Single trailing non-whitespace delimiter (should pop trailing delimiter)
    assert_eq!(
        split_ifs_read_bytes(b"foo:bar:", colon_ifs, 2),
        vec![BString::from("foo"), BString::from("bar")]
    );
    // Multiple non-whitespace delimiters in last field (should retain trailing delimiter)
    assert_eq!(
        split_ifs_read_bytes(b"foo:bar:baz:", colon_ifs, 2),
        vec![BString::from("foo"), BString::from("bar:baz:")]
    );
    // Single variable with trailing delimiter
    assert_eq!(split_ifs_read_bytes(b"foo:", colon_ifs, 1), vec![BString::from("foo")]);
}

#[test]
fn test_split_ifs_read_tab_and_newline() {
    let tab_newline_ifs = BStr::new(b" \t\n");
    assert_eq!(
        split_ifs_read_bytes(b"\t\n foo \t bar \n\t", tab_newline_ifs, 2),
        vec![BString::from("foo"), BString::from("bar")]
    );
}

#[test]
fn test_split_ifs_read_escaped() {
    let default_ifs = BStr::new(b" \t\n");

    // Escaped space in "foo bar" should not be treated as IFS delimiter
    let chars = vec![
        LineChar::unescaped(b'f'),
        LineChar::unescaped(b'o'),
        LineChar::unescaped(b'o'),
        LineChar { byte: b' ', escaped: true },
        LineChar::unescaped(b'b'),
        LineChar::unescaped(b'a'),
        LineChar::unescaped(b'r'),
        LineChar::unescaped(b' '),
        LineChar::unescaped(b'b'),
        LineChar::unescaped(b'a'),
        LineChar::unescaped(b'z'),
    ];
    assert_eq!(
        split_ifs_read(&chars, default_ifs, 2),
        vec![BString::from("foo bar"), BString::from("baz")]
    );
}

#[test]
fn test_parse_standard_escape() {
    assert_eq!(parse_standard_escape(b'a'), Some(b'\x07'));
    assert_eq!(parse_standard_escape(b'b'), Some(b'\x08'));
    assert_eq!(parse_standard_escape(b'f'), Some(b'\x0c'));
    assert_eq!(parse_standard_escape(b'n'), Some(b'\n'));
    assert_eq!(parse_standard_escape(b'r'), Some(b'\r'));
    assert_eq!(parse_standard_escape(b't'), Some(b'\t'));
    assert_eq!(parse_standard_escape(b'v'), Some(b'\x0b'));
    assert_eq!(parse_standard_escape(b'\\'), Some(b'\\'));

    assert_eq!(parse_standard_escape(b'c'), None);
    assert_eq!(parse_standard_escape(b'0'), None);
    assert_eq!(parse_standard_escape(b'x'), None);
    assert_eq!(parse_standard_escape(b'z'), None);
}

#[test]
fn test_parse_octal_digits() {
    assert_eq!(parse_octal_digits(b"0", 3), (0, 1));
    assert_eq!(parse_octal_digits(b"7", 3), (7, 1));
    assert_eq!(parse_octal_digits(b"12", 3), (10, 2));
    assert_eq!(parse_octal_digits(b"101", 3), (b'A', 3));
    assert_eq!(parse_octal_digits(b"777", 3), (255, 3));

    // Limit to max_digits
    assert_eq!(parse_octal_digits(b"1015", 3), (b'A', 3));
    assert_eq!(parse_octal_digits(b"1234", 2), (10, 2));

    // Non-octal character terminates parsing
    assert_eq!(parse_octal_digits(b"128", 3), (10, 2));
    assert_eq!(parse_octal_digits(b"89", 3), (0, 0));

    // Empty input
    assert_eq!(parse_octal_digits(b"", 3), (0, 0));
}

#[test]
fn test_process_escape_bytes() {
    // Plain text without escapes
    assert_eq!(process_escape_bytes(b"hello world"), (b"hello world".to_vec(), false));

    // Standard escapes
    assert_eq!(
        process_escape_bytes(r"\a\b\f\n\r\t\v\\".as_bytes()),
        (vec![7, 8, 12, 10, 13, 9, 11, b'\\'], false)
    );

    // Halt on \c
    assert_eq!(process_escape_bytes(r"hello\cworld".as_bytes()), (b"hello".to_vec(), true));

    // Octal with leading 0
    assert_eq!(process_escape_bytes(r"\0101".as_bytes()), (vec![b'A'], false));

    // Octal without leading 0
    assert_eq!(process_escape_bytes(r"\102".as_bytes()), (vec![b'B'], false));

    // Trailing backslash and unrecognized escape
    assert_eq!(process_escape_bytes(r"foo\zbar\".as_bytes()), (b"foo\\zbar\\".to_vec(), false));
}
