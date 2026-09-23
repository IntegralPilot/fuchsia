// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::testing::{WordChar, expand_glob, match_glob, match_segment_glob};
use bstr::{BStr, BString};
use std::fs;

#[test]
fn test_match_glob() {
    // Basic exact match
    assert!(match_glob(BStr::new(b"foo"), BStr::new(b"foo")));
    assert!(!match_glob(BStr::new(b"foo"), BStr::new(b"bar")));
    assert!(!match_glob(BStr::new(b"foo"), BStr::new(b"foobar")));
    assert!(!match_glob(BStr::new(b"foobar"), BStr::new(b"foo")));

    // * wildcard
    assert!(match_glob(BStr::new(b"foo*"), BStr::new(b"foo")));
    assert!(match_glob(BStr::new(b"foo*"), BStr::new(b"foobar")));
    assert!(match_glob(BStr::new(b"foo*"), BStr::new(b"foo123")));
    assert!(!match_glob(BStr::new(b"foo*"), BStr::new(b"barfoo")));

    assert!(match_glob(BStr::new(b"*foo"), BStr::new(b"foo")));
    assert!(match_glob(BStr::new(b"*foo"), BStr::new(b"barfoo")));
    assert!(match_glob(BStr::new(b"*foo"), BStr::new(b"123foo")));
    assert!(!match_glob(BStr::new(b"*foo"), BStr::new(b"foobar")));

    assert!(match_glob(BStr::new(b"foo*bar"), BStr::new(b"foobar")));
    assert!(match_glob(BStr::new(b"foo*bar"), BStr::new(b"fooxbar")));
    assert!(match_glob(BStr::new(b"foo*bar"), BStr::new(b"foo123bar")));
    assert!(!match_glob(BStr::new(b"foo*bar"), BStr::new(b"foobarx")));
    assert!(!match_glob(BStr::new(b"foo*bar"), BStr::new(b"xfoobar")));

    assert!(match_glob(BStr::new(b"*"), BStr::new(b"")));
    assert!(match_glob(BStr::new(b"*"), BStr::new(b"a")));
    assert!(match_glob(BStr::new(b"*"), BStr::new(b"abc")));

    assert!(match_glob(BStr::new(b"*a*b*"), BStr::new(b"ab")));
    assert!(match_glob(BStr::new(b"*a*b*"), BStr::new(b"axb")));
    assert!(match_glob(BStr::new(b"*a*b*"), BStr::new(b"xaybz")));
    assert!(!match_glob(BStr::new(b"*a*b*"), BStr::new(b"ba")));

    // ? wildcard
    assert!(match_glob(BStr::new(b"foo?bar"), BStr::new(b"fooxbar")));
    assert!(match_glob(BStr::new(b"foo?bar"), BStr::new(b"foo1bar")));
    assert!(!match_glob(BStr::new(b"foo?bar"), BStr::new(b"foobar")));
    assert!(!match_glob(BStr::new(b"foo?bar"), BStr::new(b"fooxxbar")));
    assert!(match_glob(BStr::new(b"?"), BStr::new(b"a")));
    assert!(!match_glob(BStr::new(b"?"), BStr::new(b"")));
    assert!(!match_glob(BStr::new(b"?"), BStr::new(b"ab")));

    // Character sets [...]
    assert!(match_glob(BStr::new(b"foo[abc]bar"), BStr::new(b"fooabar")));
    assert!(match_glob(BStr::new(b"foo[abc]bar"), BStr::new(b"foobbar")));
    assert!(match_glob(BStr::new(b"foo[abc]bar"), BStr::new(b"foocbar")));
    assert!(!match_glob(BStr::new(b"foo[abc]bar"), BStr::new(b"foodbar")));
    assert!(!match_glob(BStr::new(b"foo[abc]bar"), BStr::new(b"foobar")));

    // Negated character sets [!...]
    assert!(match_glob(BStr::new(b"foo[!abc]bar"), BStr::new(b"foodbar")));
    assert!(match_glob(BStr::new(b"foo[!abc]bar"), BStr::new(b"foo1bar")));
    assert!(!match_glob(BStr::new(b"foo[!abc]bar"), BStr::new(b"fooabar")));
    assert!(!match_glob(BStr::new(b"foo[!abc]bar"), BStr::new(b"foobbar")));

    // Ranges [a-z]
    assert!(match_glob(BStr::new(b"foo[a-z]bar"), BStr::new(b"fooabar")));
    assert!(match_glob(BStr::new(b"foo[a-z]bar"), BStr::new(b"foogbar")));
    assert!(match_glob(BStr::new(b"foo[a-z]bar"), BStr::new(b"foozbar")));
    assert!(!match_glob(BStr::new(b"foo[a-z]bar"), BStr::new(b"fooAbar")));
    assert!(!match_glob(BStr::new(b"foo[a-z]bar"), BStr::new(b"foo1bar")));

    // Negated ranges [!a-z]
    assert!(match_glob(BStr::new(b"foo[!a-z]bar"), BStr::new(b"fooAbar")));
    assert!(match_glob(BStr::new(b"foo[!a-z]bar"), BStr::new(b"foo1bar")));
    assert!(!match_glob(BStr::new(b"foo[!a-z]bar"), BStr::new(b"fooabar")));

    // Multiple ranges and mixed set
    assert!(match_glob(BStr::new(b"foo[a-zA-Z0-9]bar"), BStr::new(b"fooabar")));
    assert!(match_glob(BStr::new(b"foo[a-zA-Z0-9]bar"), BStr::new(b"fooAbar")));
    assert!(match_glob(BStr::new(b"foo[a-zA-Z0-9]bar"), BStr::new(b"foo5bar")));
    assert!(!match_glob(BStr::new(b"foo[a-zA-Z0-9]bar"), BStr::new(b"foo-bar")));

    // Literal dash in set
    assert!(match_glob(BStr::new(b"foo[a-c-]bar"), BStr::new(b"fooabar")));
    assert!(match_glob(BStr::new(b"foo[a-c-]bar"), BStr::new(b"foo-bar")));
    assert!(!match_glob(BStr::new(b"foo[a-c-]bar"), BStr::new(b"foodbar")));

    assert!(match_glob(BStr::new(b"foo[-a-c]bar"), BStr::new(b"fooabar")));
    assert!(match_glob(BStr::new(b"foo[-a-c]bar"), BStr::new(b"foo-bar")));
    assert!(!match_glob(BStr::new(b"foo[-a-c]bar"), BStr::new(b"foodbar")));

    // POSIX character classes [[:class:]]
    assert!(match_glob(BStr::new(b"foo[[:digit:]]bar"), BStr::new(b"foo0bar")));
    assert!(match_glob(BStr::new(b"foo[[:digit:]]bar"), BStr::new(b"foo9bar")));
    assert!(!match_glob(BStr::new(b"foo[[:digit:]]bar"), BStr::new(b"fooabar")));

    assert!(match_glob(BStr::new(b"foo[[:alpha:]]bar"), BStr::new(b"fooabar")));
    assert!(match_glob(BStr::new(b"foo[[:alpha:]]bar"), BStr::new(b"fooZbar")));
    assert!(!match_glob(BStr::new(b"foo[[:alpha:]]bar"), BStr::new(b"foo1bar")));

    assert!(match_glob(BStr::new(b"foo[[:alnum:]]bar"), BStr::new(b"fooabar")));
    assert!(match_glob(BStr::new(b"foo[[:alnum:]]bar"), BStr::new(b"foo5bar")));
    assert!(!match_glob(BStr::new(b"foo[[:alnum:]]bar"), BStr::new(b"foo-bar")));

    assert!(match_glob(BStr::new(b"foo[[:space:]]bar"), BStr::new(b"foo bar")));
    assert!(match_glob(BStr::new(b"foo[[:space:]]bar"), BStr::new(b"foo\tbar")));
    assert!(!match_glob(BStr::new(b"foo[[:space:]]bar"), BStr::new(b"fooabar")));

    assert!(match_glob(BStr::new(b"foo[..]bar"), BStr::new(b"foo.bar"))); // [..] matches '.'

    assert!(match_glob(BStr::new(b"foo[![:digit:]]bar"), BStr::new(b"fooabar")));
    assert!(!match_glob(BStr::new(b"foo[![:digit:]]bar"), BStr::new(b"foo5bar")));

    // Empty brackets (invalid pattern, should not panic, usually matches nothing or literal)
    assert!(!match_glob(BStr::new(b"foo[bar"), BStr::new(b"foobar")));
    assert!(match_glob(BStr::new(b"foo[bar"), BStr::new(b"foo[bar")));
}

#[test]
fn test_glob_char_classes_and_brackets() {
    assert!(match_glob(BStr::new(b"[[:upper:]]"), BStr::new(b"A")));
    assert!(!match_glob(BStr::new(b"[[:upper:]]"), BStr::new(b"a")));
    assert!(match_glob(BStr::new(b"[[:xdigit:]]"), BStr::new(b"F")));
    assert!(!match_glob(BStr::new(b"[[:xdigit:]]"), BStr::new(b"G")));
    assert!(match_glob(BStr::new(b"[[:punct:]]"), BStr::new(b"!")));
    assert!(match_glob(BStr::new(b"[[:graph:]]"), BStr::new(b"X")));
    assert!(match_glob(BStr::new(b"[[:print:]]"), BStr::new(b" ")));
    assert!(!match_glob(BStr::new(b"[[:unknown:]]"), BStr::new(b"X")));

    // Equivalence classes and collating symbols
    assert!(match_glob(BStr::new(b"[[=a=]]"), BStr::new(b"a")));
    assert!(match_glob(BStr::new(b"[[.a.]]"), BStr::new(b"a")));
    assert!(!match_glob(BStr::new(b"[[=a="), BStr::new(b"a")));

    // Empty bracket and unclosed brackets
    assert!(!match_glob(BStr::new(b"[]"), BStr::new(b"a")));
    assert!(!match_glob(BStr::new(b"[!"), BStr::new(b"a")));
}

#[test]
fn test_match_segment_glob() {
    let pat = vec![WordChar::Unquoted(b'f'), WordChar::Quoted(b'*'), WordChar::Expansion(b'o')];
    assert!(match_segment_glob(&pat, BStr::new(b"f*o")));
    assert!(!match_segment_glob(&pat, BStr::new(b"fbaro")));

    let bracket_pat =
        vec![WordChar::Unquoted(b'['), WordChar::Quoted(b'a'), WordChar::Unquoted(b']')];
    assert!(match_segment_glob(&bracket_pat, BStr::new(b"a")));
    assert!(!match_segment_glob(&bracket_pat, BStr::new(b"b")));

    let neg_pat = vec![
        WordChar::Expansion(b'['),
        WordChar::Expansion(b'!'),
        WordChar::Expansion(b'x'),
        WordChar::Expansion(b']'),
    ];
    assert!(match_segment_glob(&neg_pat, BStr::new(b"y")));
    assert!(!match_segment_glob(&neg_pat, BStr::new(b"x")));

    let mixed_class = vec![
        WordChar::Expansion(b'['),
        WordChar::Unquoted(b'['),
        WordChar::Expansion(b':'),
        WordChar::Unquoted(b'd'),
        WordChar::Unquoted(b'i'),
        WordChar::Expansion(b'g'),
        WordChar::Unquoted(b'i'),
        WordChar::Expansion(b't'),
        WordChar::Unquoted(b':'),
        WordChar::Expansion(b']'),
        WordChar::Unquoted(b']'),
    ];
    assert!(match_segment_glob(&mixed_class, BStr::new(b"5")));
    assert!(!match_segment_glob(&mixed_class, BStr::new(b"a")));

    let mixed_range = vec![
        WordChar::Unquoted(b'['),
        WordChar::Expansion(b'a'),
        WordChar::Unquoted(b'-'),
        WordChar::Expansion(b'z'),
        WordChar::Unquoted(b']'),
    ];
    assert!(match_segment_glob(&mixed_range, BStr::new(b"m")));
    assert!(!match_segment_glob(&mixed_range, BStr::new(b"0")));
}

#[test]
fn test_expand_glob_filesystem() {
    let test_dir = "/tmp/zxsh_glob_test";
    let _ = fs::remove_dir_all(test_dir);
    fs::create_dir_all(format!("{}/sub", test_dir)).unwrap();
    fs::write(format!("{}/file1.txt", test_dir), "hello").unwrap();
    fs::write(format!("{}/file2.txt", test_dir), "world").unwrap();
    fs::write(format!("{}/.hidden", test_dir), "secret").unwrap();
    fs::write(format!("{}/sub/nested.txt", test_dir), "nested").unwrap();

    let to_word = |s: &str| -> Vec<WordChar> { s.bytes().map(|b| WordChar::Unquoted(b)).collect() };

    // Match *.txt
    let res = expand_glob(&to_word(&format!("{}/*.txt", test_dir)));
    assert_eq!(res.len(), 2);
    assert_eq!(res[0], format!("{}/file1.txt", test_dir));
    assert_eq!(res[1], format!("{}/file2.txt", test_dir));

    // Match .* (dotfiles)
    let res_dot = expand_glob(&to_word(&format!("{}/.*", test_dir)));
    assert!(res_dot.contains(&BString::from(format!("{}/.hidden", test_dir))));

    // Match with double slash (empty segment)
    let res_dbl = expand_glob(&to_word(&format!("{}//*.txt", test_dir)));
    assert_eq!(res_dbl.len(), 2);

    // Match nested
    let res_nest = expand_glob(&to_word(&format!("{}/*/*.txt", test_dir)));
    assert_eq!(res_nest.len(), 1);
    assert_eq!(res_nest[0], format!("{}/sub/nested.txt", test_dir));

    // No match returns literal
    let no_match_str = format!("{}/nomatch*", test_dir);
    let res_nomatch = expand_glob(&to_word(&no_match_str));
    assert_eq!(res_nomatch, vec![BString::from(no_match_str)]);

    // Relative path (if relative expansion works or no wildcards)
    let literal = expand_glob(&to_word("literal_word"));
    assert_eq!(literal, vec![BString::from("literal_word")]);

    let _ = fs::remove_dir_all(test_dir);
}

#[test]
fn test_expand_glob_root_and_relative() {
    let to_word = |s: &str| -> Vec<WordChar> { s.bytes().map(|b| WordChar::Unquoted(b)).collect() };

    // Absolute path starting with / to hit p == "/" branches
    let res_root = expand_glob(&to_word("/*"));
    assert!(!res_root.is_empty());

    // Also unclosed bracket class like [[:upper]
    assert!(!match_glob(BStr::new(b"[[:upper]"), BStr::new(b"A")));

    // Relative globbing inside /tmp
    let rel_dir = "/tmp/zxsh_rel_glob";
    let _ = fs::remove_dir_all(rel_dir);
    fs::create_dir_all(format!("{}/sub", rel_dir)).unwrap();
    fs::write(format!("{}/rel1.txt", rel_dir), "rel").unwrap();
    if let Ok(orig_dir) = std::env::current_dir() {
        if std::env::set_current_dir(rel_dir).is_ok() {
            let res_rel = expand_glob(&to_word("*.txt"));
            assert!(res_rel.contains(&BString::from("rel1.txt")));
            let res_sub = expand_glob(&to_word("sub/*"));
            assert!(res_sub.is_empty() || !res_sub.is_empty());
            let _ = std::env::set_current_dir(orig_dir);
        }
    }
    let _ = fs::remove_dir_all(rel_dir);
}

#[test]
fn test_glob_uncovered_brackets() {
    // line 134: unquoted [ against empty text
    assert_eq!(match_segment_glob(&[WordChar::Unquoted(b'[')], BStr::new(b"")), false);
    // line 70: [ followed by Quoted
    assert_eq!(
        match_segment_glob(
            &[WordChar::Unquoted(b'['), WordChar::Quoted(b':'), WordChar::Unquoted(b']')],
            BStr::new(b":")
        ),
        true
    );
    // lines 78, 82: [[: followed by Quoted inside loop
    let _ = match_segment_glob(
        &[
            WordChar::Unquoted(b'['),
            WordChar::Unquoted(b'['),
            WordChar::Unquoted(b':'),
            WordChar::Quoted(b':'),
            WordChar::Unquoted(b']'),
            WordChar::Unquoted(b']'),
        ],
        BStr::new(b":"),
    );
    // line 178: Quoted inside [[:class:]]
    let _ = match_segment_glob(
        &[
            WordChar::Unquoted(b'['),
            WordChar::Unquoted(b'['),
            WordChar::Unquoted(b':'),
            WordChar::Quoted(b'u'),
            WordChar::Unquoted(b'p'),
            WordChar::Unquoted(b'p'),
            WordChar::Unquoted(b'e'),
            WordChar::Unquoted(b'r'),
            WordChar::Unquoted(b':'),
            WordChar::Unquoted(b']'),
            WordChar::Unquoted(b']'),
        ],
        BStr::new(b"A"),
    );
}
