// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::iter::Peekable;
use std::slice::Iter;

use crate::string::parse_int;

use super::{IncompleteReason, ParseError, RawWordPart, Token};
use bstr::{BString, ByteSlice};

const WHITESPACE: u8 = 1 << 0; // ' ', '\t', '\r'
const NEWLINE: u8 = 1 << 1; // '\n'
const META_CHAR: u8 = 1 << 2; // ';', '|', '&', '>', '<', '(', ')'
const QUOTE_CHAR: u8 = 1 << 3; // '\'', '"', '\\', '`'
const IDENT_START: u8 = 1 << 4; // 'a'..='z', 'A'..='Z', '_'
const IDENT_CHAR: u8 = 1 << 5; // 'a'..='z', 'A'..='Z', '0'..='9', '_'
const DIGIT: u8 = 1 << 6; // '0'..='9'
const VAR_SPECIAL: u8 = 1 << 7; // '#', '?', '@', '*', '$', '!', '-'

const CHAR_CLASS_TABLE_SIZE: usize = 256;

pub const fn make_char_class_table() -> [u8; CHAR_CLASS_TABLE_SIZE] {
    let mut table = [0u8; CHAR_CLASS_TABLE_SIZE];
    let mut i = 0;
    while i < CHAR_CLASS_TABLE_SIZE {
        let ch = i as u8;
        let mut class = 0;
        if ch == b' ' || ch == b'\t' || ch == b'\r' {
            class |= WHITESPACE;
        }
        if ch == b'\n' {
            class |= NEWLINE;
        }
        if ch == b';'
            || ch == b'|'
            || ch == b'&'
            || ch == b'>'
            || ch == b'<'
            || ch == b'('
            || ch == b')'
        {
            class |= META_CHAR;
        }
        if ch == b'\'' || ch == b'"' || ch == b'\\' || ch == b'`' {
            class |= QUOTE_CHAR;
        }
        if (ch >= b'a' && ch <= b'z') || (ch >= b'A' && ch <= b'Z') || ch == b'_' {
            class |= IDENT_START;
        }
        if (ch >= b'a' && ch <= b'z')
            || (ch >= b'A' && ch <= b'Z')
            || (ch >= b'0' && ch <= b'9')
            || ch == b'_'
        {
            class |= IDENT_CHAR;
        }
        if ch >= b'0' && ch <= b'9' {
            class |= DIGIT;
        }
        if ch == b'#'
            || ch == b'?'
            || ch == b'@'
            || ch == b'*'
            || ch == b'$'
            || ch == b'!'
            || ch == b'-'
        {
            class |= VAR_SPECIAL;
        }
        table[i] = class;
        i += 1;
    }
    table
}

static CHAR_CLASSES: [u8; CHAR_CLASS_TABLE_SIZE] = make_char_class_table();

fn trim_start_tabs(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < bytes.len() && bytes[start] == b'\t' {
        start += 1;
    }
    &bytes[start..]
}

struct Tokenizer<'a> {
    chars: Peekable<Iter<'a, u8>>,
    tokens: Vec<Token>,
    parsing_heredoc_delimiter: bool,
    pending_heredoc_src_fd: Option<i32>,
    pending_heredoc_strip_tabs: bool,
    pending_indices: Vec<usize>,
}

impl<'a> Tokenizer<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            chars: input.iter().peekable(),
            tokens: Vec::new(),
            parsing_heredoc_delimiter: false,
            pending_heredoc_src_fd: None,
            pending_heredoc_strip_tabs: false,
            pending_indices: Vec::new(),
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.chars.peek().map(|&&c| c)
    }

    fn next(&mut self) -> Option<u8> {
        self.chars.next().map(|&c| c)
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.next();
            true
        } else {
            false
        }
    }

    fn parse_var_name(&mut self) -> Option<BString> {
        let mut name = Vec::new();
        if self.consume(b'{') {
            while let Some(ch) = self.peek() {
                if ch == b'}' {
                    self.next(); // consume '}'
                    return Some(name.into());
                }
                name.push(ch);
                self.next();
            }
            return None; // Unclosed brace
        }

        if let Some(first_ch) = self.peek() {
            let first_class = CHAR_CLASSES[first_ch as usize];
            if (first_class & DIGIT) != 0 {
                name.push(first_ch);
                self.next();
            } else if (first_class & VAR_SPECIAL) != 0 {
                name.push(first_ch);
                self.next();
            } else if (first_class & IDENT_START) != 0 {
                while let Some(ch) = self.peek() {
                    let class = CHAR_CLASSES[ch as usize];
                    if (class & IDENT_CHAR) != 0 {
                        name.push(ch);
                        self.next();
                    } else {
                        break;
                    }
                }
            } else {
                return None;
            }
            Some(name.into())
        } else {
            None
        }
    }

    fn scan_command_substitution(&mut self) -> Result<BString, ParseError> {
        let mut inner = Vec::new();
        let mut depth = 1;
        while let Some(ch) = self.next() {
            if ch == b'(' {
                depth += 1;
            } else if ch == b')' {
                depth -= 1;
                if depth == 0 {
                    return Ok(inner.into());
                }
            }
            inner.push(ch);
        }
        Err(ParseError::Incomplete(IncompleteReason::Paren))
    }

    fn scan_arithmetic_expansion(&mut self) -> Result<BString, ParseError> {
        let mut expr = Vec::new();
        let mut paren_depth = 1;
        while let Some(ch) = self.next() {
            if ch == b'(' {
                paren_depth += 1;
                expr.push(ch);
            } else if ch == b')' {
                if paren_depth == 1 && self.consume(b')') {
                    return Ok(expr.into());
                }
                paren_depth -= 1;
                expr.push(ch);
            } else {
                expr.push(ch);
            }
        }
        Err(ParseError::Incomplete(IncompleteReason::Arithmetic))
    }

    fn scan_backtick_command_substitution(&mut self) -> Result<BString, ParseError> {
        let mut inner = Vec::new();
        while let Some(ch) = self.next() {
            if ch == b'`' {
                return Ok(inner.into());
            }
            if ch == b'\\' {
                if let Some(next_ch) = self.peek() {
                    if next_ch == b'`' || next_ch == b'\\' || next_ch == b'$' {
                        inner.push(next_ch);
                        self.next();
                    } else if next_ch == b'\n' {
                        self.next();
                    } else {
                        inner.push(b'\\');
                    }
                } else {
                    inner.push(b'\\');
                }
            } else {
                inner.push(ch);
            }
        }
        Err(ParseError::Incomplete(IncompleteReason::Quote))
    }

    fn process_heredocs(&mut self) -> Result<(), ParseError> {
        let indices: Vec<usize> = std::mem::take(&mut self.pending_indices);
        for idx in indices {
            let Token::RedirectHereDocPlaceholder { src_fd, delimiter, strip_tabs } =
                &self.tokens[idx]
            else {
                unreachable!();
            };
            let (src_fd, delimiter, strip_tabs) = (*src_fd, delimiter.clone(), *strip_tabs);

            let mut delimiter_string = Vec::new();
            let mut expand = true;
            for part in delimiter.iter() {
                match part {
                    RawWordPart::Literal(s) => {
                        delimiter_string.extend_from_slice(s.as_bytes());
                    }
                    RawWordPart::QuotedLiteral(s) => {
                        delimiter_string.extend_from_slice(s.as_bytes());
                        expand = false;
                    }
                    _ => unreachable!(),
                }
            }

            let mut body = Vec::new();
            let mut current_line = Vec::new();
            let mut found = false;
            while let Some(ch) = self.next() {
                if ch == b'\n' {
                    let check_line = current_line.clone();
                    if strip_tabs {
                        let trimmed = trim_start_tabs(&check_line);
                        if trimmed == delimiter_string {
                            found = true;
                            break;
                        }
                    } else if current_line == delimiter_string {
                        found = true;
                        break;
                    }

                    if strip_tabs {
                        let trimmed = trim_start_tabs(&current_line);
                        body.extend_from_slice(trimmed);
                    } else {
                        body.extend_from_slice(&current_line);
                    }
                    body.push(b'\n');
                    current_line.clear();
                } else {
                    current_line.push(ch);
                }
            }
            if !found {
                if strip_tabs {
                    let trimmed = trim_start_tabs(&current_line);
                    if trimmed == delimiter_string {
                        found = true;
                    }
                } else if current_line == delimiter_string {
                    found = true;
                }
                if !found {
                    if strip_tabs {
                        let trimmed = trim_start_tabs(&current_line);
                        body.extend_from_slice(trimmed);
                    } else {
                        body.extend_from_slice(&current_line);
                    }
                }
            }
            if !found {
                return Err(ParseError::Incomplete(IncompleteReason::Heredoc));
            }

            self.tokens[idx] = Token::RedirectHereDoc {
                src_fd,
                delimiter: delimiter.clone(),
                body: body.into(),
                expand,
            };
        }
        Ok(())
    }

    fn tokenize(mut self) -> Result<Vec<Token>, ParseError> {
        while let Some(c) = self.peek() {
            // Redirect with fd logic
            let mut lookahead = self.chars.clone();
            let mut digits = Vec::new();
            while let Some(&&lc) = lookahead.peek() {
                if (CHAR_CLASSES[lc as usize] & DIGIT) != 0 {
                    digits.push(lc);
                    lookahead.next();
                } else {
                    break;
                }
            }

            let has_redirect = if !digits.is_empty() {
                if let Some(&&next_c) = lookahead.peek() {
                    next_c == b'>' || next_c == b'<'
                } else {
                    false
                }
            } else {
                false
            };

            if has_redirect {
                for _ in 0..digits.len() {
                    self.next();
                }
                let src_fd = parse_int::<i32>(&digits).unwrap();
                let next_c = self.next().unwrap();
                if next_c == b'>' {
                    if self.consume(b'>') {
                        self.tokens.push(Token::RedirectAppend(Some(src_fd)));
                    } else if self.consume(b'&') {
                        self.tokens.push(Token::RedirectDupOut(Some(src_fd)));
                    } else if self.consume(b'|') {
                        self.tokens.push(Token::RedirectOutClobber(Some(src_fd)));
                    } else {
                        self.tokens.push(Token::RedirectOut(Some(src_fd)));
                    }
                } else {
                    if self.consume(b'<') {
                        if self.consume(b'-') {
                            self.pending_heredoc_strip_tabs = true;
                        }
                        self.parsing_heredoc_delimiter = true;
                        self.pending_heredoc_src_fd = Some(src_fd);
                    } else if self.consume(b'&') {
                        self.tokens.push(Token::RedirectDupIn(Some(src_fd)));
                    } else {
                        self.tokens.push(Token::RedirectIn(Some(src_fd)));
                    }
                }
                continue;
            }

            match c {
                b'#' => {
                    self.next();
                    while let Some(nc) = self.peek() {
                        if nc == b'\n' {
                            break;
                        }
                        self.next();
                    }
                }
                b' ' | b'\t' | b'\r' => {
                    self.next();
                }
                b'\n' => {
                    self.next();
                    self.tokens.push(Token::Newline);
                    self.process_heredocs()?;
                }
                b';' => {
                    self.next();
                    if self.consume(b';') {
                        self.tokens.push(Token::DoubleSemi);
                    } else {
                        self.tokens.push(Token::Semi);
                    }
                }
                b'|' => {
                    self.next();
                    if self.consume(b'|') {
                        self.tokens.push(Token::Or);
                    } else {
                        self.tokens.push(Token::Pipe);
                    }
                }
                b'&' => {
                    self.next();
                    if self.consume(b'&') {
                        self.tokens.push(Token::And);
                    } else {
                        self.tokens.push(Token::Ampersand);
                    }
                }
                b'>' => {
                    self.next();
                    if self.consume(b'>') {
                        self.tokens.push(Token::RedirectAppend(None));
                    } else if self.consume(b'&') {
                        self.tokens.push(Token::RedirectDupOut(None));
                    } else if self.consume(b'|') {
                        self.tokens.push(Token::RedirectOutClobber(None));
                    } else {
                        self.tokens.push(Token::RedirectOut(None));
                    }
                }
                b'<' => {
                    self.next();
                    if self.consume(b'<') {
                        if self.consume(b'-') {
                            self.pending_heredoc_strip_tabs = true;
                        }
                        self.parsing_heredoc_delimiter = true;
                        self.pending_heredoc_src_fd = None;
                    } else if self.consume(b'&') {
                        self.tokens.push(Token::RedirectDupIn(None));
                    } else {
                        self.tokens.push(Token::RedirectIn(None));
                    }
                }
                b'(' => {
                    self.next();
                    self.tokens.push(Token::LParen);
                }
                b')' => {
                    self.next();
                    self.tokens.push(Token::RParen);
                }
                _ => {
                    let mut parts = Vec::new();
                    let mut current_bytes = Vec::new();
                    let mut state = TokenizeState::Unquoted;

                    #[derive(Clone, Copy, PartialEq, Eq)]
                    enum TokenizeState {
                        Unquoted,
                        SingleQuoted,
                        DoubleQuoted,
                    }

                    while let Some(ch) = self.peek() {
                        match state {
                            TokenizeState::Unquoted => match ch {
                                _ if (CHAR_CLASSES[ch as usize]
                                    & (WHITESPACE | NEWLINE | META_CHAR))
                                    != 0 =>
                                {
                                    break;
                                }
                                b'\'' => {
                                    self.next();
                                    if !current_bytes.is_empty() {
                                        parts.push(RawWordPart::Literal(
                                            current_bytes.clone().into(),
                                        ));
                                        current_bytes.clear();
                                    }
                                    state = TokenizeState::SingleQuoted;
                                }
                                b'"' => {
                                    self.next();
                                    if !current_bytes.is_empty() {
                                        parts.push(RawWordPart::Literal(
                                            current_bytes.clone().into(),
                                        ));
                                        current_bytes.clear();
                                    }
                                    state = TokenizeState::DoubleQuoted;
                                }
                                b'\\' => {
                                    self.next();
                                    if let Some(next_ch) = self.peek() {
                                        if next_ch == b'\n' {
                                            self.next();
                                            if self.peek().is_none() {
                                                return Err(ParseError::Incomplete(
                                                    IncompleteReason::LineContinuation,
                                                ));
                                            }
                                        } else {
                                            let next_ch = self.next().unwrap();
                                            current_bytes.push(next_ch);
                                        }
                                    } else {
                                        current_bytes.push(b'\\');
                                    }
                                }
                                b'`' => {
                                    self.next();
                                    let inner = self.scan_backtick_command_substitution()?;
                                    if !current_bytes.is_empty() {
                                        parts.push(RawWordPart::Literal(
                                            current_bytes.clone().into(),
                                        ));
                                        current_bytes.clear();
                                    }
                                    parts.push(RawWordPart::CommandSubstitution(inner));
                                }
                                b'$' if !self.parsing_heredoc_delimiter => {
                                    self.next();
                                    if self.consume(b'(') {
                                        if self.consume(b'(') {
                                            let expr = self.scan_arithmetic_expansion()?;
                                            if !current_bytes.is_empty() {
                                                parts.push(RawWordPart::Literal(
                                                    current_bytes.clone().into(),
                                                ));
                                                current_bytes.clear();
                                            }
                                            parts.push(RawWordPart::Arithmetic(expr));
                                        } else {
                                            let inner = self.scan_command_substitution()?;
                                            if !current_bytes.is_empty() {
                                                parts.push(RawWordPart::Literal(
                                                    current_bytes.clone().into(),
                                                ));
                                                current_bytes.clear();
                                            }
                                            parts.push(RawWordPart::CommandSubstitution(inner));
                                        }
                                    } else if self.peek() == Some(b'{') {
                                        if let Some(var_name) = self.parse_var_name() {
                                            if !current_bytes.is_empty() {
                                                parts.push(RawWordPart::Literal(
                                                    current_bytes.clone().into(),
                                                ));
                                                current_bytes.clear();
                                            }
                                            parts.push(RawWordPart::Var(var_name));
                                        } else {
                                            return Err(ParseError::Incomplete(
                                                IncompleteReason::Brace,
                                            ));
                                        }
                                    } else if let Some(var_name) = self.parse_var_name() {
                                        if !current_bytes.is_empty() {
                                            parts.push(RawWordPart::Literal(
                                                current_bytes.clone().into(),
                                            ));
                                            current_bytes.clear();
                                        }
                                        parts.push(RawWordPart::Var(var_name));
                                    } else {
                                        current_bytes.push(b'$');
                                    }
                                }
                                _ => {
                                    self.next();
                                    current_bytes.push(ch);
                                }
                            },
                            TokenizeState::SingleQuoted => {
                                self.next();
                                if ch == b'\'' {
                                    if !current_bytes.is_empty() {
                                        parts.push(RawWordPart::QuotedLiteral(
                                            current_bytes.clone().into(),
                                        ));
                                        current_bytes.clear();
                                    }
                                    state = TokenizeState::Unquoted;
                                } else {
                                    current_bytes.push(ch);
                                }
                            }
                            TokenizeState::DoubleQuoted => match ch {
                                b'"' => {
                                    self.next();
                                    if !current_bytes.is_empty() {
                                        parts.push(RawWordPart::QuotedLiteral(
                                            current_bytes.clone().into(),
                                        ));
                                        current_bytes.clear();
                                    }
                                    state = TokenizeState::Unquoted;
                                }
                                b'\\' => {
                                    self.next();
                                    if let Some(next_ch) = self.peek() {
                                        if next_ch == b'\n' {
                                            self.next();
                                            if self.peek().is_none() {
                                                return Err(ParseError::Incomplete(
                                                    IncompleteReason::LineContinuation,
                                                ));
                                            }
                                        } else if next_ch == b'"'
                                            || next_ch == b'\\'
                                            || next_ch == b'$'
                                        {
                                            current_bytes.push(next_ch);
                                            self.next();
                                        } else {
                                            current_bytes.push(b'\\');
                                        }
                                    } else {
                                        current_bytes.push(b'\\');
                                    }
                                }
                                b'`' => {
                                    self.next();
                                    let inner = self.scan_backtick_command_substitution()?;
                                    if !current_bytes.is_empty() {
                                        parts.push(RawWordPart::QuotedLiteral(
                                            current_bytes.clone().into(),
                                        ));
                                        current_bytes.clear();
                                    }
                                    parts.push(RawWordPart::QuotedCommandSubstitution(inner));
                                }
                                b'$' if !self.parsing_heredoc_delimiter => {
                                    self.next();
                                    if self.consume(b'(') {
                                        if self.consume(b'(') {
                                            let expr = self.scan_arithmetic_expansion()?;
                                            if !current_bytes.is_empty() {
                                                parts.push(RawWordPart::QuotedLiteral(
                                                    current_bytes.clone().into(),
                                                ));
                                                current_bytes.clear();
                                            }
                                            parts.push(RawWordPart::QuotedArithmetic(expr));
                                        } else {
                                            let inner = self.scan_command_substitution()?;
                                            if !current_bytes.is_empty() {
                                                parts.push(RawWordPart::QuotedLiteral(
                                                    current_bytes.clone().into(),
                                                ));
                                                current_bytes.clear();
                                            }
                                            parts.push(RawWordPart::QuotedCommandSubstitution(
                                                inner,
                                            ));
                                        }
                                    } else if self.peek() == Some(b'{') {
                                        if let Some(var_name) = self.parse_var_name() {
                                            if !current_bytes.is_empty() {
                                                parts.push(RawWordPart::QuotedLiteral(
                                                    current_bytes.clone().into(),
                                                ));
                                                current_bytes.clear();
                                            }
                                            parts.push(RawWordPart::QuotedVar(var_name));
                                        } else {
                                            return Err(ParseError::Incomplete(
                                                IncompleteReason::Brace,
                                            ));
                                        }
                                    } else if let Some(var_name) = self.parse_var_name() {
                                        if !current_bytes.is_empty() {
                                            parts.push(RawWordPart::QuotedLiteral(
                                                current_bytes.clone().into(),
                                            ));
                                            current_bytes.clear();
                                        }
                                        parts.push(RawWordPart::QuotedVar(var_name));
                                    } else {
                                        current_bytes.push(b'$');
                                    }
                                }
                                _ => {
                                    self.next();
                                    current_bytes.push(ch);
                                }
                            },
                        }
                    }

                    if state != TokenizeState::Unquoted {
                        return Err(ParseError::Incomplete(IncompleteReason::Quote));
                    }

                    if !current_bytes.is_empty() {
                        parts.push(RawWordPart::Literal(current_bytes.into()));
                    }

                    if self.parsing_heredoc_delimiter {
                        self.pending_indices.push(self.tokens.len());
                        self.tokens.push(Token::RedirectHereDocPlaceholder {
                            src_fd: self.pending_heredoc_src_fd,
                            delimiter: parts,
                            strip_tabs: self.pending_heredoc_strip_tabs,
                        });
                        self.parsing_heredoc_delimiter = false;
                        self.pending_heredoc_src_fd = None;
                        self.pending_heredoc_strip_tabs = false;
                    } else {
                        self.tokens.push(Token::Word(parts));
                    }
                }
            }
        }
        self.process_heredocs()?;
        Ok(self.tokens)
    }
}

pub fn tokenize(input: &[u8]) -> Result<Vec<Token>, ParseError> {
    Tokenizer::new(input).tokenize()
}
