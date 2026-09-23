// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::ast::{
    ASTBuilder, Command, CommandTag, Fd, Redirect, RedirectTag, RedirectTemplate, ResolvedWordPart,
    WordPart, WordPartTag,
};
use crate::string::parse_int;
use bstr::{BStr, BString, ByteSlice};

use super::error::{IncompleteReason, ParseError};
use super::token::{RawWordPart, Token};
use super::tokenizer::tokenize;
use crate::relative;

fn get_literal_word(
    builder: &ASTBuilder,
    slice: relative::Slice<WordPart>,
) -> Result<BString, String> {
    if slice.len() == 1 {
        let parts = builder.get_slice(slice);
        if parts[0].tag == WordPartTag::LITERAL {
            return Ok(parts[0].text.to_bstring(builder));
        }
    }
    Err("Function name must be a literal word".to_string())
}

fn get_literal_word_from_parts(parts: &[RawWordPart]) -> Result<BString, String> {
    if parts.len() == 1 {
        match &parts[0] {
            RawWordPart::Literal(s) => Ok(s.clone()),
            _ => Err("Function name must be a literal word".to_string()),
        }
    } else {
        Err("Function name must be a literal word".to_string())
    }
}

/// Parses a subshell command body (usually enclosed in parentheses) into the AST.
pub fn parse_subshell_command(
    builder: &mut ASTBuilder,
    inner: &[u8],
) -> Result<relative::Ptr<Command>, ParseError> {
    let tokens = tokenize(inner)?;
    let cmd_ptrs = parse_script(builder, &tokens)?;
    if cmd_ptrs.is_empty() {
        Ok(builder.add_empty_simple_command())
    } else {
        Ok(builder.add_sequence_or_single(&cmd_ptrs))
    }
}

/// Resolves raw word parts (from tokenization) into resolved word parts,
/// recursively parsing nested command substitutions.
pub fn resolve_word_parts(
    builder: &mut ASTBuilder,
    parts: &[RawWordPart],
) -> Result<Vec<ResolvedWordPart>, ParseError> {
    let mut resolved_parts = Vec::new();
    for part in parts {
        match part {
            RawWordPart::Literal(s) => resolved_parts.push(ResolvedWordPart::Literal(s.clone())),
            RawWordPart::Var(s) => resolved_parts.push(ResolvedWordPart::Var(s.clone())),
            RawWordPart::QuotedLiteral(s) => {
                resolved_parts.push(ResolvedWordPart::QuotedLiteral(s.clone()))
            }
            RawWordPart::QuotedVar(s) => {
                resolved_parts.push(ResolvedWordPart::QuotedVar(s.clone()))
            }
            RawWordPart::CommandSubstitution(s) => {
                let cmd_ptr = parse_subshell_command(builder, s.as_bytes())?;
                resolved_parts.push(ResolvedWordPart::CommandSubstitution(cmd_ptr));
            }
            RawWordPart::QuotedCommandSubstitution(s) => {
                let cmd_ptr = parse_subshell_command(builder, s.as_bytes())?;
                resolved_parts.push(ResolvedWordPart::QuotedCommandSubstitution(cmd_ptr));
            }
            RawWordPart::Arithmetic(s) => {
                resolved_parts.push(ResolvedWordPart::Arithmetic(s.clone()))
            }
            RawWordPart::QuotedArithmetic(s) => {
                resolved_parts.push(ResolvedWordPart::QuotedArithmetic(s.clone()))
            }
        }
    }
    Ok(resolved_parts)
}

struct Parser<'a, 'b> {
    tokens: &'a [Token],
    builder: &'b mut ASTBuilder,
    pos: usize,
}

impl<'a, 'b> Parser<'a, 'b> {
    fn new(builder: &'b mut ASTBuilder, tokens: &'a [Token]) -> Self {
        Self { tokens, builder, pos: 0 }
    }

    fn is_tok(&self, tok: &Token, expected: &str) -> bool {
        tok.as_unquoted_bstr() == Some(BStr::new(expected.as_bytes()))
    }

    fn serialize_word(
        &mut self,
        parts: &[RawWordPart],
    ) -> Result<relative::Slice<WordPart>, ParseError> {
        let resolved_parts = resolve_word_parts(self.builder, parts)?;
        Ok(self.builder.add_resolved_word(&resolved_parts))
    }

    fn expect_keyword(
        &mut self,
        expected: &str,
        incomplete: IncompleteReason,
    ) -> Result<(), ParseError> {
        match self.next() {
            Some(tok) if self.is_tok(&tok, expected) => Ok(()),
            None => Err(ParseError::Incomplete(incomplete)),
            Some(t) => Err(ParseError::Syntax(format!("Expected '{}', got {:?}", expected, t))),
        }
    }

    fn parse_loop(&mut self, is_while: bool) -> Result<relative::Ptr<Command>, ParseError> {
        let cond = self.parse_command_list()?;
        self.expect_keyword("do", IncompleteReason::Keyword)?;
        let body = self.parse_command_list()?;
        self.expect_keyword("done", IncompleteReason::Keyword)?;

        let tag = if is_while { CommandTag::WHILE } else { CommandTag::UNTIL };
        Ok(self.builder.add_loop_command(tag, cond, body))
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn is_block_terminator(&self) -> bool {
        match self.peek().and_then(|t| t.as_unquoted_bstr()) {
            Some(s) => matches!(
                s.as_bytes(),
                b"then" | b"else" | b"elif" | b"fi" | b"do" | b"done" | b"}" | b"esac"
            ),
            None => false,
        }
    }

    fn parse_command_list(&mut self) -> Result<relative::Ptr<Command>, ParseError> {
        let cmds = self.parse_commands()?;
        if cmds.is_empty() {
            return Err(ParseError::Syntax("Expected at least one command".to_string()));
        }
        Ok(self.builder.add_sequence_or_single(&cmds))
    }

    fn parse_commands(&mut self) -> Result<Vec<relative::Ptr<Command>>, ParseError> {
        let mut cmds = Vec::new();
        loop {
            while let Some(Token::Semi) | Some(Token::Newline) = self.peek() {
                self.next();
            }
            if self.peek().is_none()
                || self.peek() == Some(&Token::RParen)
                || self.peek() == Some(&Token::DoubleSemi)
                || self.is_block_terminator()
            {
                break;
            }
            let mut cmd = self.parse_logical()?;

            let mut is_bg = false;
            if self.peek() == Some(&Token::Ampersand) {
                self.next();
                is_bg = true;
            }

            if is_bg {
                cmd = self.builder.add_unary_command(CommandTag::BACKGROUND, cmd);
            }
            cmds.push(cmd);

            if is_bg {
                while let Some(Token::Semi) | Some(Token::Newline) = self.peek() {
                    self.next();
                }
                continue;
            }

            match self.peek() {
                Some(Token::Semi) | Some(Token::Newline) => {
                    self.next();
                }
                None | Some(Token::RParen) | Some(Token::DoubleSemi) => {}
                _ if self.is_block_terminator() => {}
                _ => {
                    return Err(ParseError::Syntax(format!("Unexpected token: {:?}", self.peek())));
                }
            }
        }
        Ok(cmds)
    }

    fn parse_logical(&mut self) -> Result<relative::Ptr<Command>, ParseError> {
        let mut left = self.parse_pipeline()?;
        while let Some(t) = self.peek() {
            match t {
                Token::And => {
                    self.next();
                    if self.peek().is_none() {
                        return Err(ParseError::Incomplete(IncompleteReason::LogicalOperator));
                    }
                    let right = self.parse_pipeline()?;
                    left = self.builder.add_binary_command(CommandTag::LOGICAL_AND, left, right);
                }
                Token::Or => {
                    self.next();
                    if self.peek().is_none() {
                        return Err(ParseError::Incomplete(IncompleteReason::LogicalOperator));
                    }
                    let right = self.parse_pipeline()?;
                    left = self.builder.add_binary_command(CommandTag::LOGICAL_OR, left, right);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_pipeline(&mut self) -> Result<relative::Ptr<Command>, ParseError> {
        let mut left = self.parse_redirected()?;
        while let Some(Token::Pipe) = self.peek() {
            self.next();
            if self.peek().is_none() {
                return Err(ParseError::Incomplete(IncompleteReason::Pipeline));
            }
            let right = self.parse_redirected()?;
            left = self.builder.add_binary_command(CommandTag::PIPELINE, left, right);
        }
        Ok(left)
    }

    fn is_redirect_token(tok: &Token) -> bool {
        matches!(
            tok,
            Token::RedirectOut(_)
                | Token::RedirectOutClobber(_)
                | Token::RedirectAppend(_)
                | Token::RedirectIn(_)
                | Token::RedirectDupOut(_)
                | Token::RedirectDupIn(_)
                | Token::RedirectHereDoc { .. }
        )
    }

    fn parse_consecutive_redirects(
        &mut self,
    ) -> Result<Vec<(Token, Option<relative::Slice<WordPart>>)>, ParseError> {
        let mut redirect_toks = Vec::new();
        while let Some(t) = self.peek() {
            if Self::is_redirect_token(t) {
                let op = t.clone();
                self.next();
                let target = if matches!(op, Token::RedirectHereDoc { .. }) {
                    None
                } else {
                    match self.next() {
                        Some(Token::Word(parts)) => Some(self.serialize_word(&parts)?),
                        None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                        Some(other) => {
                            return Err(ParseError::Syntax(format!(
                                "Expected filename/target after redirect operator, got {:?}",
                                other
                            )));
                        }
                    }
                };
                redirect_toks.push((op, target));
            } else {
                break;
            }
        }
        Ok(redirect_toks)
    }

    fn parse_redirects_slice(
        &mut self,
        redirects: &[(Token, Option<relative::Slice<WordPart>>)],
    ) -> Result<relative::Slice<Redirect>, ParseError> {
        let mut templates = Vec::new();
        for (op, target) in redirects {
            match op {
                Token::RedirectOut(src_fd) => {
                    let fd = src_fd.map(Fd).unwrap_or(Fd::STDOUT);
                    templates.push(RedirectTemplate {
                        tag: RedirectTag::TO_FILE,
                        append: 0,
                        clobber: 0,
                        expand: 0,
                        src_fd: fd,
                        dest_fd: Fd::STDIN,
                        filename: *target,
                        body: None,
                    });
                }
                Token::RedirectOutClobber(src_fd) => {
                    let fd = src_fd.map(Fd).unwrap_or(Fd::STDOUT);
                    templates.push(RedirectTemplate {
                        tag: RedirectTag::TO_FILE,
                        append: 0,
                        clobber: 1,
                        expand: 0,
                        src_fd: fd,
                        dest_fd: Fd::STDIN,
                        filename: *target,
                        body: None,
                    });
                }
                Token::RedirectAppend(src_fd) => {
                    let fd = src_fd.map(Fd).unwrap_or(Fd::STDOUT);
                    templates.push(RedirectTemplate {
                        tag: RedirectTag::TO_FILE,
                        append: 1,
                        clobber: 0,
                        expand: 0,
                        src_fd: fd,
                        dest_fd: Fd::STDIN,
                        filename: *target,
                        body: None,
                    });
                }
                Token::RedirectIn(src_fd) => {
                    let fd = src_fd.map(Fd).unwrap_or(Fd::STDIN);
                    templates.push(RedirectTemplate {
                        tag: RedirectTag::FROM_FILE,
                        append: 0,
                        clobber: 0,
                        expand: 0,
                        src_fd: fd,
                        dest_fd: Fd::STDIN,
                        filename: *target,
                        body: None,
                    });
                }
                Token::RedirectDupOut(src_fd) => {
                    let target_slice = target.unwrap();
                    let src = src_fd.map(Fd).unwrap_or(Fd::STDOUT);
                    let slice = self.builder.get_slice(target_slice);
                    let is_close = slice.len() == 1
                        && slice[0].tag == WordPartTag::LITERAL
                        && slice[0].text.as_bstr(&self.builder) == "-";
                    if is_close {
                        templates.push(RedirectTemplate {
                            tag: RedirectTag::CLOSE_FD,
                            append: 0,
                            clobber: 0,
                            expand: 0,
                            src_fd: src,
                            dest_fd: Fd::STDIN,
                            filename: None,
                            body: None,
                        });
                    } else {
                        let dest_bstr = get_literal_word(self.builder, target_slice)
                            .map_err(|e| ParseError::Syntax(e))?;
                        let dest = parse_int::<i32>(&dest_bstr).ok_or_else(|| {
                            ParseError::Syntax(format!(
                                "Expected destination FD after >&, got {}",
                                target_slice
                            ))
                        })?;
                        templates.push(RedirectTemplate {
                            tag: RedirectTag::DUP_FD,
                            append: 0,
                            clobber: 0,
                            expand: 0,
                            src_fd: src,
                            dest_fd: Fd(dest),
                            filename: None,
                            body: None,
                        });
                    }
                }
                Token::RedirectDupIn(src_fd) => {
                    let target_slice = target.unwrap();
                    let src = src_fd.map(Fd).unwrap_or(Fd::STDIN);
                    let slice = self.builder.get_slice(target_slice);
                    let is_close = slice.len() == 1
                        && slice[0].tag == WordPartTag::LITERAL
                        && slice[0].text.as_bstr(&self.builder) == "-";
                    if is_close {
                        templates.push(RedirectTemplate {
                            tag: RedirectTag::CLOSE_FD,
                            append: 0,
                            clobber: 0,
                            expand: 0,
                            src_fd: src,
                            dest_fd: Fd::STDIN,
                            filename: None,
                            body: None,
                        });
                    } else {
                        let dest_bstr = get_literal_word(self.builder, target_slice)
                            .map_err(|e| ParseError::Syntax(e))?;
                        let dest = parse_int::<i32>(&dest_bstr).ok_or_else(|| {
                            ParseError::Syntax(format!(
                                "Expected destination FD after <&, got {}",
                                target_slice
                            ))
                        })?;
                        templates.push(RedirectTemplate {
                            tag: RedirectTag::DUP_FD,
                            append: 0,
                            clobber: 0,
                            expand: 0,
                            src_fd: src,
                            dest_fd: Fd(dest),
                            filename: None,
                            body: None,
                        });
                    }
                }
                Token::RedirectHereDoc { src_fd, delimiter, body, expand } => {
                    let fd = src_fd.map(Fd).unwrap_or(Fd::STDIN);
                    let del_slice = self.serialize_word(delimiter)?;
                    templates.push(RedirectTemplate {
                        tag: RedirectTag::HERE_DOC,
                        append: 0,
                        clobber: 0,
                        expand: if *expand { 1 } else { 0 },
                        src_fd: fd,
                        dest_fd: Fd::STDIN,
                        filename: Some(del_slice),
                        body: Some(body.clone()),
                    });
                }
                _ => unreachable!(),
            }
        }

        Ok(self.builder.add_redirects_from_templates(&templates))
    }

    fn parse_redirected(&mut self) -> Result<relative::Ptr<Command>, ParseError> {
        let cmd = self.parse_primary()?;
        let redirect_toks = self.parse_consecutive_redirects()?;
        if redirect_toks.is_empty() {
            Ok(cmd)
        } else {
            let redirects = self.parse_redirects_slice(&redirect_toks)?;

            Ok(self.builder.add_redirect_command(cmd, redirects))
        }
    }

    fn parse_function_body(&mut self) -> Result<relative::Ptr<Command>, ParseError> {
        if let Some(tok) = self.peek() {
            if self.is_tok(tok, "{") {
                self.next(); // consume "{"
                let body = self.parse_command_list()?;
                match self.next() {
                    Some(tok) if self.is_tok(&tok, "}") => {}
                    None => return Err(ParseError::Incomplete(IncompleteReason::Brace)),
                    t => {
                        return Err(ParseError::Syntax(format!(
                            "Expected '}}' to close function body, got {:?}",
                            t
                        )));
                    }
                }
                return Ok(body);
            }
        }
        self.parse_redirected()
    }

    fn parse_if_remainder(&mut self) -> Result<Option<relative::Ptr<Command>>, ParseError> {
        if let Some(tok) = self.peek() {
            if self.is_tok(tok, "elif") {
                self.next(); // consume "elif"
                let cond = self.parse_command_list()?;

                // expect "then"
                match self.next() {
                    Some(t) if self.is_tok(&t, "then") => {}
                    None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                    t => {
                        return Err(ParseError::Syntax(format!(
                            "Expected 'then' after elif condition, got {:?}",
                            t
                        )));
                    }
                }

                let then_branch = self.parse_command_list()?;
                let else_branch = self.parse_if_remainder()?;

                return Ok(Some(self.builder.add_if_command(cond, then_branch, else_branch)));
            } else if self.is_tok(tok, "else") {
                self.next(); // consume "else"
                let else_body = self.parse_command_list()?;
                return Ok(Some(else_body));
            }
        }
        Ok(None)
    }

    fn parse_primary(&mut self) -> Result<relative::Ptr<Command>, ParseError> {
        match self.peek() {
            Some(Token::LParen) => {
                self.next();
                let cmd = self.parse_command_list()?;
                match self.next() {
                    Some(Token::RParen) => {
                        Ok(self.builder.add_unary_command(CommandTag::SUBSHELL, cmd))
                    }
                    None => Err(ParseError::Incomplete(IncompleteReason::Paren)),
                    Some(t) => {
                        Err(ParseError::Syntax(format!("Expected matching ')', got {:?}", t)))
                    }
                }
            }
            Some(t) if self.is_tok(t, "{") => {
                self.next(); // consume "{"
                let cmd = self.parse_command_list()?;
                match self.next() {
                    Some(tok) if self.is_tok(&tok, "}") => {}
                    None => return Err(ParseError::Incomplete(IncompleteReason::Brace)),
                    t => {
                        return Err(ParseError::Syntax(format!(
                            "Expected '}}' to close block, got {:?}",
                            t
                        )));
                    }
                }
                Ok(cmd)
            }
            Some(t) if self.is_tok(t, "if") => {
                self.next(); // consume "if"
                let cond = self.parse_command_list()?;

                // expect "then"
                self.expect_keyword("then", IncompleteReason::Keyword)?;

                let then_branch = self.parse_command_list()?;
                let else_branch = self.parse_if_remainder()?;

                // expect "fi"
                self.expect_keyword("fi", IncompleteReason::Keyword)?;

                Ok(self.builder.add_if_command(cond, then_branch, else_branch))
            }
            Some(t) if self.is_tok(t, "while") => {
                self.next(); // consume "while"
                self.parse_loop(true)
            }
            Some(t) if self.is_tok(t, "until") => {
                self.next(); // consume "until"
                self.parse_loop(false)
            }
            Some(t) if self.is_tok(t, "case") => {
                self.next(); // consume "case"
                let word_slice = match self.next() {
                    Some(Token::Word(parts)) => self.serialize_word(&parts)?,
                    None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                    t => {
                        return Err(ParseError::Syntax(format!(
                            "Expected word after 'case', got {:?}",
                            t
                        )));
                    }
                };

                // expect "in"
                match self.next() {
                    Some(tok) if self.is_tok(&tok, "in") => {}
                    None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                    t => {
                        return Err(ParseError::Syntax(format!(
                            "Expected 'in' after case word, got {:?}",
                            t
                        )));
                    }
                }

                // consume any semi/newlines after "in"
                while let Some(Token::Semi) | Some(Token::Newline) = self.peek() {
                    self.next();
                }

                let mut case_clauses: Vec<(
                    Vec<relative::Slice<WordPart>>,
                    relative::Ptr<Command>,
                )> = Vec::new();

                while let Some(tok) = self.peek() {
                    if self.is_tok(tok, "esac") {
                        break;
                    }

                    // optional opening parenthesis '('
                    if let Some(Token::LParen) = self.peek() {
                        self.next();
                    }

                    let mut patterns = Vec::new();
                    loop {
                        let pat = match self.next() {
                            Some(Token::Word(parts)) => self.serialize_word(&parts)?,
                            None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                            t => {
                                return Err(ParseError::Syntax(format!(
                                    "Expected pattern in case clause, got {:?}",
                                    t
                                )));
                            }
                        };
                        patterns.push(pat);

                        if let Some(Token::Pipe) = self.peek() {
                            self.next(); // consume '|'
                        } else {
                            break;
                        }
                    }

                    // expect closing parenthesis ')'
                    match self.next() {
                        Some(Token::RParen) => {}
                        None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                        t => {
                            return Err(ParseError::Syntax(format!(
                                "Expected ')' after case pattern(s), got {:?}",
                                t
                            )));
                        }
                    }

                    // consume any semi/newlines before body
                    while let Some(Token::Semi) | Some(Token::Newline) = self.peek() {
                        self.next();
                    }

                    // parse command list for this case (could be empty)
                    let cmds = self.parse_commands()?;
                    let body = self.builder.add_sequence_or_single(&cmds);

                    case_clauses.push((patterns, body));

                    // consume double semi ';;' if present
                    if let Some(Token::DoubleSemi) = self.peek() {
                        self.next();
                    } else {
                        // POSIX says ;; is optional on the very last clause before esac
                        if self.peek().map(|t| self.is_tok(t, "esac")) != Some(true) {
                            return Err(ParseError::Syntax(
                                "Expected ';;' after case clause".to_string(),
                            ));
                        }
                    }

                    // consume any semi/newlines after clause
                    while let Some(Token::Semi) | Some(Token::Newline) = self.peek() {
                        self.next();
                    }
                }

                // expect "esac"
                if self.next().is_none() {
                    return Err(ParseError::Incomplete(IncompleteReason::Keyword));
                }

                let mut pat_refs = Vec::new();
                for (pats, _) in &case_clauses {
                    let pat_slice = self.builder.add_argument_refs(pats);
                    pat_refs.push(pat_slice);
                }

                let items_to_add: Vec<(
                    relative::Slice<relative::Slice<WordPart>>,
                    relative::Ptr<Command>,
                )> = pat_refs
                    .iter()
                    .copied()
                    .zip(case_clauses.into_iter().map(|(_, b)| b))
                    .collect();
                let case_items = self.builder.add_case_items_from_refs(&items_to_add);
                let cmd_ptr = self.builder.add_case_command(word_slice, case_items);
                Ok(cmd_ptr)
            }
            Some(t) if self.is_tok(t, "for") => {
                self.next(); // consume "for"
                let var = match self.next() {
                    Some(tok) => match tok.as_unquoted_bstr() {
                        Some(v) => v.to_owned(),
                        None => {
                            return Err(ParseError::Syntax(
                                "Expected unquoted variable name after 'for'".to_string(),
                            ));
                        }
                    },
                    None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                };

                let mut items = Vec::new();
                if self.peek().map(|tok| self.is_tok(tok, "in")) == Some(true) {
                    self.next(); // consume "in"
                    while let Some(tok) = self.peek() {
                        match tok {
                            Token::Semi | Token::Newline => {
                                self.next();
                                break;
                            }
                            Token::Word(parts) => {
                                let parts = parts.clone();
                                self.next();
                                let word_slice = self.serialize_word(&parts)?;
                                items.push(word_slice);
                            }
                            _ => {
                                let tok = self.next().unwrap();
                                return Err(ParseError::Syntax(format!(
                                    "Unexpected token in for list: {:?}",
                                    tok
                                )));
                            }
                        }
                    }
                } else {
                    match self.peek() {
                        Some(Token::Semi) | Some(Token::Newline) => {
                            self.next();
                        }
                        Some(tok) if self.is_tok(tok, "do") => {}
                        None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                        t => {
                            return Err(ParseError::Syntax(format!(
                                "Expected ';' or newline or 'do' after for variable, got {:?}",
                                t
                            )));
                        }
                    }
                    let at_word_slice =
                        self.builder.add_resolved_word(&[ResolvedWordPart::QuotedVar("@".into())]);
                    items.push(at_word_slice);
                }

                match self.next() {
                    Some(tok) if self.is_tok(&tok, "do") => {}
                    None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                    t => {
                        return Err(ParseError::Syntax(format!(
                            "Expected 'do' in for loop, got {:?}",
                            t
                        )));
                    }
                }

                let body = self.parse_command_list()?;

                match self.next() {
                    Some(tok) if self.is_tok(&tok, "done") => {}
                    None => return Err(ParseError::Incomplete(IncompleteReason::Keyword)),
                    t => {
                        return Err(ParseError::Syntax(format!(
                            "Expected 'done' to close for loop, got {:?}",
                            t
                        )));
                    }
                }

                let var_bstr = self.builder.add_bstr(&var);
                let items_slice = self.builder.add_argument_refs(&items);
                Ok(self.builder.add_for_command(var_bstr, items_slice, body))
            }
            Some(tok) if Self::is_redirect_token(tok) || matches!(tok, Token::Word(_)) => {
                if let Some(Token::Word(parts)) = self.peek() {
                    if self.peek_at(1) == Some(&Token::LParen)
                        && self.peek_at(2) == Some(&Token::RParen)
                    {
                        let name = get_literal_word_from_parts(parts)
                            .map_err(|e| ParseError::Syntax(e))?;
                        self.next(); // consume name
                        self.next(); // consume LParen
                        self.next(); // consume RParen
                        let body = self.parse_function_body()?;

                        let name_bstr = self.builder.add_bstr(&name);
                        return Ok(self.builder.add_function_def_command(name_bstr, body));
                    }
                }

                let mut args = Vec::new();
                let mut redirect_toks = Vec::new();

                while let Some(t) = self.peek() {
                    if Self::is_redirect_token(t) {
                        let sub_reds = self.parse_consecutive_redirects()?;
                        redirect_toks.extend(sub_reds);
                    } else if let Token::Word(parts) = t {
                        let parts = parts.clone();
                        self.next();
                        let word_slice = self.serialize_word(&parts)?;
                        args.push(word_slice);
                    } else {
                        break;
                    }
                }

                let simple_ptr = self.builder.add_simple_command(&args);

                if redirect_toks.is_empty() {
                    Ok(simple_ptr)
                } else {
                    let redirects = self.parse_redirects_slice(&redirect_toks)?;

                    Ok(self.builder.add_redirect_command(simple_ptr, redirects))
                }
            }
            t => Err(ParseError::Syntax(format!("Expected primary command, got {:?}", t))),
        }
    }
}

/// Parses a list of tokens representing a complete shell script into a sequence of AST command
/// offsets.
pub fn parse_script(
    builder: &mut ASTBuilder,
    tokens: &[Token],
) -> Result<Vec<relative::Ptr<Command>>, ParseError> {
    let mut parser = Parser::new(builder, tokens);
    parser.parse_commands()
}
