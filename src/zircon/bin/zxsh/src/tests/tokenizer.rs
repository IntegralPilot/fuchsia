// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parser::error::{IncompleteReason, ParseError};
use crate::parser::{RawWordPart, Token, tokenize};
use bstr::BString;

fn l(s: &str) -> RawWordPart {
    RawWordPart::Literal(BString::from(s))
}
fn ql(s: &str) -> RawWordPart {
    RawWordPart::QuotedLiteral(BString::from(s))
}
fn v(s: &str) -> RawWordPart {
    RawWordPart::Var(BString::from(s))
}
fn qv(s: &str) -> RawWordPart {
    RawWordPart::QuotedVar(BString::from(s))
}
fn c(s: &str) -> RawWordPart {
    RawWordPart::CommandSubstitution(BString::from(s))
}
fn qc(s: &str) -> RawWordPart {
    RawWordPart::QuotedCommandSubstitution(BString::from(s))
}
fn a(s: &str) -> RawWordPart {
    RawWordPart::Arithmetic(BString::from(s))
}
fn qa(s: &str) -> RawWordPart {
    RawWordPart::QuotedArithmetic(BString::from(s))
}

fn word(parts: &[RawWordPart]) -> Token {
    Token::Word(parts.to_vec())
}
fn w(s: &str) -> Token {
    word(&[l(s)])
}
fn qw(s: &str) -> Token {
    word(&[ql(s)])
}
fn var(s: &str) -> Token {
    word(&[v(s)])
}
fn cmd(s: &str) -> Token {
    word(&[c(s)])
}
fn arith(s: &str) -> Token {
    word(&[a(s)])
}
fn heredoc(src_fd: Option<i32>, delimiter: &[RawWordPart], body: &str, expand: bool) -> Token {
    Token::RedirectHereDoc {
        src_fd,
        delimiter: delimiter.to_vec(),
        body: BString::from(body),
        expand,
    }
}

#[test]
fn test_tokenizer_words_quotes_escapes() {
    // Unquoted words
    assert_eq!(tokenize(b"hello world"), Ok(vec![w("hello"), w("world")]));

    // Escaped spaces in unquoted word
    assert_eq!(tokenize(b"hello\\ world"), Ok(vec![w("hello world")]));

    // Single quotes (preserve everything)
    assert_eq!(tokenize(b"'hello $world'"), Ok(vec![qw("hello $world")]));

    // Double quotes (variables and substitutions)
    assert_eq!(
        tokenize(b"\"hello $world $(cmd)\""),
        Ok(vec![word(&[ql("hello "), qv("world"), ql(" "), qc("cmd")])])
    );

    // Empty double quotes
    assert_eq!(tokenize(b"\"\""), Ok(vec![word(&[])]));
}

#[test]
fn test_tokenizer_operators() {
    assert_eq!(
        tokenize(b"a; b & c | d && e || f ;; g\nh"),
        Ok(vec![
            w("a"),
            Token::Semi,
            w("b"),
            Token::Ampersand,
            w("c"),
            Token::Pipe,
            w("d"),
            Token::And,
            w("e"),
            Token::Or,
            w("f"),
            Token::DoubleSemi,
            w("g"),
            Token::Newline,
            w("h"),
        ])
    );
}

#[test]
fn test_tokenizer_redirects() {
    assert_eq!(
        tokenize(b"cmd >file <file2 >>append 2>&1 2<&-"),
        Ok(vec![
            w("cmd"),
            Token::RedirectOut(None),
            w("file"),
            Token::RedirectIn(None),
            w("file2"),
            Token::RedirectAppend(None),
            w("append"),
            Token::RedirectDupOut(Some(2)),
            w("1"),
            Token::RedirectDupIn(Some(2)),
            w("-"),
        ])
    );
}

#[test]
fn test_tokenizer_subshells_and_vars() {
    assert_eq!(
        tokenize(b"$(simple) `backtick` $var ${var2} $((1+1))"),
        Ok(vec![cmd("simple"), cmd("backtick"), var("var"), var("var2"), arith("1+1")])
    );
}

#[test]
fn test_tokenizer_heredocs() {
    // Simple heredoc
    assert_eq!(
        tokenize(b"cmd <<EOF\nline1\nline2\nEOF\n"),
        Ok(vec![w("cmd"), heredoc(None, &[l("EOF")], "line1\nline2\n", true), Token::Newline,])
    );

    assert_eq!(
        tokenize(b"cmd <<\"EOF\"\nline1 $var\nEOF\n"),
        Ok(vec![w("cmd"), heredoc(None, &[ql("EOF")], "line1 $var\n", false), Token::Newline,])
    );

    // Strip tabs
    assert_eq!(
        tokenize(b"cmd <<-EOF\n\tline1\n\tEOF\n"),
        Ok(vec![w("cmd"), heredoc(None, &[l("EOF")], "line1\n", true), Token::Newline,])
    );
}

#[test]
fn test_make_char_class_table_runtime() {
    let table = crate::parser::tokenizer::make_char_class_table();
    assert_eq!(table[b' ' as usize] & 1, 1);
    assert_eq!(table[b'\n' as usize] & 2, 2);
    assert_eq!(table[b';' as usize] & 4, 4);
    assert_eq!(table[b'\'' as usize] & 8, 8);
    assert_eq!(table[b'a' as usize] & 16, 16);
    assert_eq!(table[b'0' as usize] & 32, 32);
    assert_eq!(table[b'0' as usize] & 64, 64);
    assert_eq!(table[b'#' as usize] & 128, 128);
}

#[test]
fn test_tokenizer_unclosed_brace_in_var() {
    assert_eq!(tokenize(b"${var"), Err(ParseError::Incomplete(IncompleteReason::Brace)));
    assert_eq!(tokenize(b"${"), Err(ParseError::Incomplete(IncompleteReason::Brace)));
}

#[test]
fn test_tokenizer_var_special_and_invalid() {
    let tokens = tokenize(b"$@ $% $ prefix$%").unwrap();
    assert_eq!(tokens.len(), 4);
}

#[test]
fn test_tokenizer_unclosed_substitutions() {
    assert_eq!(tokenize(b"$(echo"), Err(ParseError::Incomplete(IncompleteReason::Paren)));
    assert_eq!(tokenize(b"$((1+"), Err(ParseError::Incomplete(IncompleteReason::Arithmetic)));
    assert_eq!(tokenize(b"`echo"), Err(ParseError::Incomplete(IncompleteReason::Quote)));
}

#[test]
fn test_tokenizer_backtick_escapes() {
    assert_eq!(
        tokenize(b"`echo \\\n` `echo \\\\` `echo \\a`"),
        Ok(vec![cmd("echo "), cmd("echo \\"), cmd("echo \\a")])
    );
    assert!(tokenize(b"`echo \\").is_err());
}

#[test]
fn test_tokenizer_redirect_with_fd() {
    assert_eq!(
        tokenize(b"2>>file 2>|file 2<<-EOF\n\tbody\nEOF\n 0<file <&2"),
        Ok(vec![
            Token::RedirectAppend(Some(2)),
            w("file"),
            Token::RedirectOutClobber(Some(2)),
            w("file"),
            heredoc(Some(2), &[l("EOF")], "body\n", true),
            Token::Newline,
            Token::RedirectIn(Some(0)),
            w("file"),
            Token::RedirectDupIn(None),
            w("2"),
        ])
    );
}

#[test]
fn test_tokenizer_unclosed_heredoc() {
    assert_eq!(
        tokenize(b"cat <<EOF\nbody\n"),
        Err(ParseError::Incomplete(IncompleteReason::Heredoc))
    );
}

#[test]
fn test_heredoc_delim_var() {
    assert_eq!(
        tokenize(b"cat <<$VAR\n1\n$VAR\n"),
        Ok(vec![w("cat"), heredoc(None, &[l("$VAR")], "1\n", true), Token::Newline,])
    );
}
#[test]
fn test_heredoc_delim_quoted_var() {
    assert_eq!(
        tokenize(b"cat <<\"$VAR\"\n2\n$VAR\n"),
        Ok(vec![w("cat"), heredoc(None, &[ql("$VAR")], "2\n", false), Token::Newline,])
    );
}
#[test]
fn test_heredoc_delim_quoted_cmd() {
    assert_eq!(
        tokenize(b"cat <<\"$(cmd)\"\n4\n$(cmd)\n"),
        Ok(vec![w("cat"), heredoc(None, &[ql("$(cmd)")], "4\n", false), Token::Newline,])
    );
}
#[test]
fn test_heredoc_delim_quoted_arith() {
    assert_eq!(
        tokenize(b"cat <<\"$((1+1))\"\n6\n$((1+1))\n"),
        Ok(vec![w("cat"), heredoc(None, &[ql("$((1+1))")], "6\n", false), Token::Newline,])
    );
}

#[test]
fn test_tokenizer_double_quote_patterns() {
    assert_eq!(
        tokenize(b"\"\\\n\" \"\\a\" \"\\\"\" \"prefix`echo`\" \"prefix$((1+1))\" \"prefix${var}\" \"$%\""),
        Ok(vec![
            word(&[]),
            qw("\\a"),
            qw("\""),
            word(&[ql("prefix"), qc("echo")]),
            word(&[ql("prefix"), qa("1+1")]),
            word(&[ql("prefix"), qv("var")]),
            qw("$%"),
        ])
    );
    assert_eq!(
        tokenize(b"\"\\\n"),
        Err(ParseError::Incomplete(IncompleteReason::LineContinuation))
    );
    assert_eq!(tokenize(b"\"prefix${\""), Err(ParseError::Incomplete(IncompleteReason::Brace)));
    assert_eq!(tokenize(b"\"hello"), Err(ParseError::Incomplete(IncompleteReason::Quote)));
}

#[test]
fn test_tokenizer_trailing_backslash() {
    assert_eq!(tokenize(b"word\\"), Ok(vec![w("word\\")]));
    assert_eq!(tokenize(b"\\\n"), Err(ParseError::Incomplete(IncompleteReason::LineContinuation)));
}

#[test]
fn test_tokenizer_exhaustive_coverage() {
    assert_eq!(
        tokenize(
            b"$0 $1 $9 $? $# $* $@ $$ $- $! ${0} ${1} ${?} ${#} ${*} ${@} ${$} ${-} ${!} ${var}"
        ),
        Ok(vec![
            var("0"),
            var("1"),
            var("9"),
            var("?"),
            var("#"),
            var("*"),
            var("@"),
            var("$"),
            var("-"),
            var("!"),
            var("0"),
            var("1"),
            var("?"),
            var("#"),
            var("*"),
            var("@"),
            var("$"),
            var("-"),
            var("!"),
            var("var"),
        ])
    );
    assert_eq!(
        tokenize(b"$(echo $(echo hi)) $(((1+2)*3))"),
        Ok(vec![cmd("echo $(echo hi)"), arith("(1+2)*3")])
    );
    assert_eq!(
        tokenize(b"prefix'single' prefix\"double\" prefix`cmd` prefix$((1+1)) prefix$(cmd) prefix${var} prefix$var"),
        Ok(vec![
            word(&[l("prefix"), ql("single")]),
            word(&[l("prefix"), ql("double")]),
            word(&[l("prefix"), c("cmd")]),
            word(&[l("prefix"), a("1+1")]),
            word(&[l("prefix"), c("cmd")]),
            word(&[l("prefix"), v("var")]),
            word(&[l("prefix"), v("var")]),
        ])
    );
    assert_eq!(
        tokenize(b"# comment\necho 2>file >& >|"),
        Ok(vec![
            Token::Newline,
            w("echo"),
            Token::RedirectOut(Some(2)),
            w("file"),
            Token::RedirectDupOut(None),
            Token::RedirectOutClobber(None),
        ])
    );
    assert_eq!(
        tokenize(b"( echo hi )"),
        Ok(vec![Token::LParen, w("echo"), w("hi"), Token::RParen])
    );

    assert_eq!(
        tokenize(b"word\\\n"),
        Err(ParseError::Incomplete(IncompleteReason::LineContinuation))
    );
    assert_eq!(tokenize(b"\"hello\\"), Err(ParseError::Incomplete(IncompleteReason::Quote)));

    assert_eq!(
        tokenize(b"cmd <<EOF\nEOF"),
        Ok(vec![w("cmd"), heredoc(None, &[l("EOF")], "", true), Token::Newline])
    );
    assert_eq!(
        tokenize(b"cmd <<-EOF\n\tEOF"),
        Ok(vec![w("cmd"), heredoc(None, &[l("EOF")], "", true), Token::Newline])
    );
    assert_eq!(
        tokenize(b"cmd <<EOF\nline1"),
        Err(ParseError::Incomplete(IncompleteReason::Heredoc))
    );
    assert_eq!(
        tokenize(b"cmd <<-EOF\n\tline1"),
        Err(ParseError::Incomplete(IncompleteReason::Heredoc))
    );
    assert_eq!(
        tokenize(b"cmd <<EOF\nline1\n"),
        Err(ParseError::Incomplete(IncompleteReason::Heredoc))
    );
    assert_eq!(
        tokenize(b"cmd <<-EOF\n\tline1\n"),
        Err(ParseError::Incomplete(IncompleteReason::Heredoc))
    );
    assert_eq!(tokenize(b"lone $"), Ok(vec![w("lone"), w("$")]));
    assert_eq!(tokenize(b"word\\\nmore"), Ok(vec![w("wordmore")]));
}
