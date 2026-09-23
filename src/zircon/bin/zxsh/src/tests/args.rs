// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::parse_args;
use bstr::BString;

fn to_bstrings(args: &[&str]) -> Vec<BString> {
    args.iter().map(|&s| BString::from(s)).collect()
}

#[test]
fn test_parse_empty() {
    let args = to_bstrings(&["zxsh"]);
    let parsed = parse_args(&args).unwrap();
    assert!(parsed.command.is_none());
    assert!(!parsed.stdin);
    assert!(!parsed.opt_interactive);
    assert!(parsed.script_name.is_none());
    assert!(parsed.positional_args.is_empty());
}

#[test]
fn test_parse_c_option() {
    // -c command
    let args = to_bstrings(&["zxsh", "-c", "echo foo"]);
    let parsed = parse_args(&args).unwrap();
    assert_eq!(parsed.command.as_ref().unwrap(), "echo foo");
    assert!(parsed.script_name.is_none());
    assert!(parsed.positional_args.is_empty());

    // -c command script_name args...
    let args = to_bstrings(&["zxsh", "-c", "echo foo", "my_script", "arg1", "arg2"]);
    let parsed = parse_args(&args).unwrap();
    assert_eq!(parsed.command.as_ref().unwrap(), "echo foo");
    assert_eq!(parsed.script_name.as_ref().unwrap(), "my_script");
    assert_eq!(parsed.positional_args, to_bstrings(&["arg1", "arg2"]));

    // Attached -ccommand
    let args = to_bstrings(&["zxsh", "-cecho foo", "my_script", "arg1"]);
    let parsed = parse_args(&args).unwrap();
    assert_eq!(parsed.command.as_ref().unwrap(), "echo foo");
    assert_eq!(parsed.script_name.as_ref().unwrap(), "my_script");
    assert_eq!(parsed.positional_args, to_bstrings(&["arg1"]));
}

#[test]
fn test_parse_flags() {
    let args = to_bstrings(&["zxsh", "-xive", "script.sh"]);
    let parsed = parse_args(&args).unwrap();
    assert_eq!(parsed.opt_xtrace, Some(true));
    assert_eq!(parsed.opt_interactive, true);
    assert_eq!(parsed.opt_verbose, Some(true));
    assert_eq!(parsed.opt_errexit, Some(true));
    assert_eq!(parsed.script_name.as_ref().unwrap(), "script.sh");
}

#[test]
fn test_parse_plus_flags() {
    let args = to_bstrings(&["zxsh", "+x", "-e", "script.sh"]);
    let parsed = parse_args(&args).unwrap();
    assert_eq!(parsed.opt_xtrace, Some(false));
    assert_eq!(parsed.opt_errexit, Some(true));
    assert_eq!(parsed.script_name.as_ref().unwrap(), "script.sh");
}

#[test]
fn test_parse_o_option() {
    let args = to_bstrings(&["zxsh", "-o", "xtrace", "+o", "noglob", "script.sh"]);
    let parsed = parse_args(&args).unwrap();
    assert_eq!(parsed.options_to_set, to_bstrings(&["xtrace"]));
    assert_eq!(parsed.options_to_clear, to_bstrings(&["noglob"]));
    assert_eq!(parsed.script_name.as_ref().unwrap(), "script.sh");

    // Attached -ooption
    let args = to_bstrings(&["zxsh", "-oxtrace", "script.sh"]);
    let parsed = parse_args(&args).unwrap();
    assert_eq!(parsed.options_to_set, to_bstrings(&["xtrace"]));
    assert_eq!(parsed.script_name.as_ref().unwrap(), "script.sh");
}

#[test]
fn test_parse_stdin() {
    // -s
    let args = to_bstrings(&["zxsh", "-s", "arg1", "arg2"]);
    let parsed = parse_args(&args).unwrap();
    assert!(parsed.stdin);
    assert!(parsed.script_name.is_none());
    assert_eq!(parsed.positional_args, to_bstrings(&["arg1", "arg2"]));

    // -
    let args = to_bstrings(&["zxsh", "-", "arg1"]);
    let parsed = parse_args(&args).unwrap();
    assert!(parsed.stdin);
    assert!(parsed.script_name.is_none());
    assert_eq!(parsed.positional_args, to_bstrings(&["arg1"]));
}

#[test]
fn test_parse_double_dash() {
    let args = to_bstrings(&["zxsh", "-x", "--", "-c", "script.sh"]);
    let parsed = parse_args(&args).unwrap();
    assert_eq!(parsed.opt_xtrace, Some(true));
    assert!(parsed.command.is_none());
    assert_eq!(parsed.script_name.as_ref().unwrap(), "-c");
    assert_eq!(parsed.positional_args, to_bstrings(&["script.sh"]));
}

#[test]
fn test_parse_errors() {
    // Missing argument for -c
    let args = to_bstrings(&["zxsh", "-c"]);
    assert!(parse_args(&args).is_err());

    // Cannot unset -c (+c)
    let args = to_bstrings(&["zxsh", "+c", "foo"]);
    assert!(parse_args(&args).is_err());

    // Missing argument for -o
    let args = to_bstrings(&["zxsh", "-o"]);
    assert!(parse_args(&args).is_err());

    // Unknown option
    let args = to_bstrings(&["zxsh", "-Z"]);
    assert!(parse_args(&args).is_err());
}

#[test]
fn test_option_parser_direct_usage() {
    use crate::args::{OptionItem, OptionParser};
    use bstr::BStr;

    let args = to_bstrings(&["-xve", "-c", "echo foo", "--", "-arg"]);
    let mut parser = OptionParser::new(&args);

    assert_eq!(
        parser.next_option(|f| f == b'c'),
        Some(Ok(OptionItem::Flag { enable: true, flag: b'x' }))
    );
    assert_eq!(
        parser.next_option(|f| f == b'c'),
        Some(Ok(OptionItem::Flag { enable: true, flag: b'v' }))
    );
    assert_eq!(
        parser.next_option(|f| f == b'c'),
        Some(Ok(OptionItem::Flag { enable: true, flag: b'e' }))
    );
    assert_eq!(
        parser.next_option(|f| f == b'c'),
        Some(Ok(OptionItem::OptArg { enable: true, flag: b'c', value: BStr::new(b"echo foo") }))
    );
    assert_eq!(parser.next_option(|_| false), None);
    assert_eq!(parser.rest(), &to_bstrings(&["-arg"])[..]);
}

#[test]
fn test_option_parser_plus_and_attached_args() {
    use crate::args::{OptionItem, OptionParser};
    use bstr::BStr;

    let args = to_bstrings(&["+x", "-cecho", "-"]);
    let mut parser = OptionParser::new(&args).allow_plus_options(true);

    assert_eq!(
        parser.next_option(|_| false),
        Some(Ok(OptionItem::Flag { enable: false, flag: b'x' }))
    );
    assert_eq!(
        parser.next_option(|f| f == b'c'),
        Some(Ok(OptionItem::OptArg { enable: true, flag: b'c', value: BStr::new(b"echo") }))
    );
    assert_eq!(parser.next_option(|_| false), None);
    assert_eq!(parser.rest(), &to_bstrings(&["-"])[..]);
}
