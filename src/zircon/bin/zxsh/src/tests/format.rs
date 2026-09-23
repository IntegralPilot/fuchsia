// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::command_to_bstring;
use crate::parser::ast::{ASTBuilder, CommandTag, ResolvedWordPart, WordPartTag};
use crate::parser::{parse_script, tokenize};
use bstr::{BStr, BString};

fn parse_cmd(s: &str) -> String {
    let mut builder = ASTBuilder::new();
    let tokens = tokenize(BStr::new(s)).unwrap();
    let cmds = parse_script(&mut builder, &tokens).unwrap();
    let cmd_ptr = builder.add_sequence_or_single(&cmds);
    let bstr = command_to_bstring(builder.get_ref(cmd_ptr), &builder);
    bstr.to_string()
}

#[test]
fn test_fmt_simple() {
    assert_eq!(parse_cmd("echo foo bar"), "echo foo bar");
}

#[test]
fn test_fmt_pipe() {
    assert_eq!(parse_cmd("echo foo | grep bar"), "echo foo | grep bar");
}

#[test]
fn test_fmt_redir_simple() {
    assert_eq!(parse_cmd("echo foo > bar"), "echo foo <redirection>");
}

#[test]
fn test_fmt_redir_non_simple() {
    assert_eq!(parse_cmd("while true; do true; done > /tmp/junk"), "while true; do true; done");
}

#[test]
fn test_fmt_subshell() {
    assert_eq!(parse_cmd("( echo sub )"), "( echo sub )");
}

#[test]
fn test_fmt_bg() {
    assert_eq!(parse_cmd("echo bg &"), "echo bg");
}

#[test]
fn test_fmt_if() {
    assert_eq!(parse_cmd("if true; then echo yes; fi"), "if true; then echo yes; fi");
}

#[test]
fn test_fmt_if_with_else() {
    assert_eq!(
        parse_cmd("if true; then echo yes; else echo no; fi"),
        "if true; then echo yes; else echo no; fi"
    );
}

#[test]
fn test_fmt_while() {
    assert_eq!(parse_cmd("while true; do true; done"), "while true; do true; done");
}

#[test]
fn test_fmt_until() {
    assert_eq!(parse_cmd("until false; do echo hi; done"), "until false; do echo hi; done");
}

#[test]
fn test_fmt_for() {
    assert_eq!(parse_cmd("for x in 1 2; do echo $x; done"), "for x in 1 2; do echo $x; done");
}

#[test]
fn test_fmt_case_single() {
    assert_eq!(parse_cmd("case foo in a) echo a ;; esac"), "case foo in a) echo a;; esac");
}

#[test]
fn test_fmt_case_multiple() {
    assert_eq!(
        parse_cmd("case foo in a | b) echo ab ;; *) echo def ;; esac"),
        "case foo in a | b) echo ab;; *) echo def;; esac"
    );
}

#[test]
fn test_fmt_func() {
    assert_eq!(parse_cmd("foo() { echo bar; }"), "foo() { ... }");
}

#[test]
fn test_fmt_logical_and() {
    assert_eq!(parse_cmd("true && false"), "true && false");
}

#[test]
fn test_fmt_logical_or() {
    assert_eq!(parse_cmd("true || false"), "true || false");
}

#[test]
fn test_fmt_sequence() {
    assert_eq!(parse_cmd("echo 1; echo 2; echo 3"), "echo 1; echo 2; echo 3");
}

#[test]
fn test_fmt_vars() {
    assert_eq!(parse_cmd("echo $VAR \"$QUOTED\""), "echo $VAR $QUOTED");
}

#[test]
fn test_fmt_cmdsub() {
    assert_eq!(parse_cmd("echo $(ls) \"$(pwd)\""), "echo $(ls) $(pwd)");
}

#[test]
fn test_fmt_arith() {
    assert_eq!(parse_cmd("echo $((1 + 2)) \"$((3 + 4))\""), "echo $((1 + 2)) $((3 + 4))");
}

#[test]
#[should_panic(expected = "invalid CommandTag: 99")]
fn test_fallback_command_tag() {
    let mut builder = ASTBuilder::new();
    let child_ptr = builder.add_empty_simple_command();
    let cmd_ptr = builder.add_unary_command(CommandTag(99), child_ptr);
    let _ = command_to_bstring(builder.get_ref(cmd_ptr), &builder);
}

#[test]
#[should_panic(expected = "invalid WordPartTag: 99")]
fn test_fallback_word_part_tag() {
    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Literal(BString::from("test"))];
    let w_slice = builder.add_resolved_word(&parts);
    let parts_mut = builder.get_mut(w_slice.cast());
    parts_mut.tag = WordPartTag(99);
    let simple_ptr = builder.add_simple_command(&[w_slice]);
    let _ = command_to_bstring(builder.get_ref(simple_ptr), &builder);
}
