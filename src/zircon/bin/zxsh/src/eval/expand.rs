// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::ExecutionContext;
use super::arithmetic::evaluate_arithmetic;
use super::glob::{WordChar, expand_glob, match_glob, word_chars_to_bstring};
use super::state::ShellState;
use crate::collections::FlatSet;
use crate::errors::{io_err_str, zx_status_str};
use crate::parser::ast::*;
use crate::parser::{Token, parse_script, parse_subshell_command, resolve_word_parts, tokenize};
use crate::process::{clone_fd_to_action, make_pipe, read_fd_to_end};
use crate::relative;
use crate::subshell::{SubshellScriptArgs, spawn_subshell_process};
use bstr::{BStr, BString, ByteSlice};

pub fn is_assignment_flat(arg: &[WordPart], buf: &relative::Buffer) -> bool {
    if arg.is_empty() {
        return false;
    }
    if arg[0].tag == WordPartTag::LITERAL {
        let s = arg[0].text.as_bstr(buf);
        if let Some(pos) = s.as_bytes().iter().position(|&b| b == b'=') {
            let name = &s.as_bytes()[..pos];
            if name.is_empty() {
                return false;
            }
            let mut bytes = name.iter();
            let &first = bytes.next().unwrap();
            return (first.is_ascii_alphabetic() || first == b'_')
                && bytes.all(|&c| c.is_ascii_alphanumeric() || c == b'_');
        }
    }
    false
}

fn run_command_substitution(
    cmd: &Command,
    state: &ShellState,
    ctx: &ExecutionContext,
    source_buf: &relative::Buffer,
) -> Result<BString, String> {
    let (read_fd, write_fd) = make_pipe()?;

    let stdin = ctx.stdin();
    let stdout = Some(&write_fd);
    let stderr = ctx.stderr();

    let mut actions = Vec::new();
    for (fd_opt, target) in [(stdin, Fd::STDIN), (stdout, Fd::STDOUT), (stderr, Fd::STDERR)] {
        if let Some(fd) = fd_opt {
            if let Some(action) = clone_fd_to_action(fd, target) {
                actions.push(action);
            }
        }
    }

    let proc =
        spawn_subshell_process(cmd, state, &mut actions, SubshellScriptArgs::Pass, source_buf)?;

    drop(write_fd);

    let output_bytes =
        read_fd_to_end(read_fd).map_err(|e| format!("Failed to read pipe: {}", io_err_str(e)))?;

    proc.wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::INFINITE)
        .map_err(|e| format!("Wait for subshell failed: {}", zx_status_str(e)))?;

    let mut output = output_bytes;
    while output.last() == Some(&b'\n') {
        output.pop();
    }
    Ok(BString::from(output))
}

fn is_builtin(name: &BStr) -> bool {
    matches!(
        name.as_bytes(),
        b"." | b"["
            | b":"
            | b"alias"
            | b"break"
            | b"cd"
            | b"chdir"
            | b"command"
            | b"continue"
            | b"cp"
            | b"dm"
            | b"dump"
            | b"echo"
            | b"eval"
            | b"exec"
            | b"exit"
            | b"export"
            | b"false"
            | b"getopts"
            | b"hash"
            | b"k"
            | b"list"
            | b"local"
            | b"ls"
            | b"mkdir"
            | b"msleep"
            | b"mv"
            | b"power"
            | b"printf"
            | b"pwd"
            | b"read"
            | b"readonly"
            | b"return"
            | b"rm"
            | b"set"
            | b"shift"
            | b"test"
            | b"times"
            | b"trap"
            | b"true"
            | b"type"
            | b"ulimit"
            | b"umask"
            | b"unalias"
            | b"unset"
            | b"wait"
    )
}

enum Modifier<'a> {
    Length,
    Default(&'a BStr, bool),       // (word, null_too)
    Assign(&'a BStr, bool),        // (word, null_too)
    Error(Option<&'a BStr>, bool), // (msg, null_too)
    Alternative(&'a BStr, bool),   // (word, null_too)
    RemovePrefix(&'a BStr, bool),  // (pattern, longest)
    RemoveSuffix(&'a BStr, bool),  // (pattern, longest)
}

fn parse_modifier<'a>(name: &'a BStr) -> (&'a BStr, Option<Modifier<'a>>) {
    if name == "#"
        || name == "?"
        || name == "@"
        || name == "*"
        || name == "$"
        || name == "!"
        || name == "-"
    {
        return (name, None);
    }
    if name.starts_with(b"#") && name.len() > 1 {
        let var_name = BStr::new(&name.as_bytes()[1..]);
        return (var_name, Some(Modifier::Length));
    }

    if let Some(idx) = name.find(b":-") {
        return (
            BStr::new(&name.as_bytes()[..idx]),
            Some(Modifier::Default(BStr::new(&name.as_bytes()[idx + 2..]), true)),
        );
    }
    if let Some(idx) = name.find(b":=") {
        return (
            BStr::new(&name.as_bytes()[..idx]),
            Some(Modifier::Assign(BStr::new(&name.as_bytes()[idx + 2..]), true)),
        );
    }
    if let Some(idx) = name.find(b":?") {
        let msg = BStr::new(&name.as_bytes()[idx + 2..]);
        let opt_msg = if msg.is_empty() { None } else { Some(msg) };
        return (BStr::new(&name.as_bytes()[..idx]), Some(Modifier::Error(opt_msg, true)));
    }
    if let Some(idx) = name.find(b":+") {
        return (
            BStr::new(&name.as_bytes()[..idx]),
            Some(Modifier::Alternative(BStr::new(&name.as_bytes()[idx + 2..]), true)),
        );
    }

    let mut min_idx = None;
    let mut matched_char = None;
    for &c in &[b'-', b'=', b'?', b'+', b'#', b'%'] {
        if let Some(idx) = name.find(&[c]) {
            if min_idx.map_or(true, |m| idx < m) {
                min_idx = Some(idx);
                matched_char = Some(c);
            }
        }
    }

    if let Some(idx) = min_idx {
        let var_name = BStr::new(&name.as_bytes()[..idx]);
        let rest = BStr::new(&name.as_bytes()[idx..]);
        match matched_char.unwrap() {
            b'-' => {
                return (
                    var_name,
                    Some(Modifier::Default(BStr::new(&rest.as_bytes()[1..]), false)),
                );
            }
            b'=' => {
                return (var_name, Some(Modifier::Assign(BStr::new(&rest.as_bytes()[1..]), false)));
            }
            b'?' => {
                let msg = BStr::new(&rest.as_bytes()[1..]);
                let opt_msg = if msg.is_empty() { None } else { Some(msg) };
                return (var_name, Some(Modifier::Error(opt_msg, false)));
            }
            b'+' => {
                return (
                    var_name,
                    Some(Modifier::Alternative(BStr::new(&rest.as_bytes()[1..]), false)),
                );
            }
            b'#' => {
                if rest.starts_with(b"##") {
                    return (
                        var_name,
                        Some(Modifier::RemovePrefix(BStr::new(&rest.as_bytes()[2..]), true)),
                    );
                } else {
                    return (
                        var_name,
                        Some(Modifier::RemovePrefix(BStr::new(&rest.as_bytes()[1..]), false)),
                    );
                }
            }
            b'%' => {
                if rest.starts_with(b"%%") {
                    return (
                        var_name,
                        Some(Modifier::RemoveSuffix(BStr::new(&rest.as_bytes()[2..]), true)),
                    );
                } else {
                    return (
                        var_name,
                        Some(Modifier::RemoveSuffix(BStr::new(&rest.as_bytes()[1..]), false)),
                    );
                }
            }
            _ => unreachable!(),
        }
    }

    (name, None)
}

/// Helper to parse and expand parameter modifier words.
/// Note: this function will become more elaborate in a later CL.
pub fn parse_and_expand_modifier(
    modifier_str: &BStr,
    state: &mut ShellState,
    ctx: &ExecutionContext,
) -> Result<BString, String> {
    let mut escaped = Vec::new();
    for &b in modifier_str.as_bytes() {
        if b == b'\\' || b == b'"' {
            escaped.push(b'\\');
        }
        escaped.push(b);
    }
    let mut quoted = Vec::new();
    quoted.push(b'"');
    quoted.extend_from_slice(&escaped);
    quoted.push(b'"');

    let mut builder = ASTBuilder::new();
    let tokens = tokenize(&quoted).map_err(|e| e.to_string())?;
    if tokens.len() == 1 {
        if let Token::Word(parts) = &tokens[0] {
            let temp_parts = resolve_word_parts(&mut builder, parts).map_err(|e| e.to_string())?;
            let word_slice = builder.add_resolved_word(&temp_parts);
            let slice = builder.get_slice(word_slice);
            return expand_argument_no_split(slice, state, ctx, &builder);
        }
    }
    Err(format!("Invalid modifier/expression: {}", modifier_str))
}

/// Expands a shell parameter expression including modifiers (e.g. `${var:-default}`, `${#var}`,
/// `${var%pattern}`).
///
/// Evaluates defaults, alternate values, string slicing, length expansion, and prefix/suffix
/// stripping.
pub fn expand_var_with_modifiers(
    name: &BStr,
    state: &mut ShellState,
    ctx: &ExecutionContext,
) -> Result<BString, String> {
    let (var_name, modifier) = parse_modifier(name);

    if state.opt_nounset {
        let is_unbound = state.get_var(var_name).is_none();
        if is_unbound {
            let needs_fail = match &modifier {
                None => true,
                Some(Modifier::Length)
                | Some(Modifier::RemovePrefix(_, _))
                | Some(Modifier::RemoveSuffix(_, _)) => true,
                _ => false,
            };
            if needs_fail {
                let msg = format!("{}: parameter not set", var_name);
                ctx.print_err(&msg)?;
                return Err(msg);
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StripKind {
        Prefix,
        Suffix,
    }

    fn strip_pattern<'a>(
        val: &'a BStr,
        pattern: &BStr,
        kind: StripKind,
        longest: bool,
    ) -> &'a BStr {
        let len = val.len();
        let val_bytes = val.as_bytes();

        let find_match_index = || {
            let indices: Box<dyn Iterator<Item = usize>> = match (kind, longest) {
                (StripKind::Prefix, false) => Box::new(0..=len),
                (StripKind::Prefix, true) => Box::new((0..=len).rev()),
                (StripKind::Suffix, false) => Box::new((0..=len).rev()),
                (StripKind::Suffix, true) => Box::new(0..=len),
            };

            for i in indices {
                let candidate = match kind {
                    StripKind::Prefix => &val_bytes[..i],
                    StripKind::Suffix => &val_bytes[i..],
                };
                if match_glob(pattern, BStr::new(candidate)) {
                    return Some(i);
                }
            }
            None
        };

        if let Some(i) = find_match_index() {
            match kind {
                StripKind::Prefix => BStr::new(&val_bytes[i..]),
                StripKind::Suffix => BStr::new(&val_bytes[..i]),
            }
        } else {
            val
        }
    }

    if let Some(mod_type) = modifier {
        match mod_type {
            Modifier::Length => {
                let val = state.get_var(var_name).unwrap_or_default();
                Ok(BString::from(val.len().to_string()))
            }
            Modifier::Default(_, null_too)
            | Modifier::Assign(_, null_too)
            | Modifier::Error(_, null_too)
            | Modifier::Alternative(_, null_too) => {
                let val = state.get_var(var_name);
                let null_or_unset = val.as_ref().map_or(true, |v| null_too && v.is_empty());
                match mod_type {
                    Modifier::Alternative(word, _) => {
                        if null_or_unset {
                            Ok(BString::default())
                        } else {
                            parse_and_expand_modifier(word, state, ctx)
                        }
                    }
                    _ if !null_or_unset => Ok(val.unwrap()),
                    Modifier::Default(word, _) => parse_and_expand_modifier(word, state, ctx),
                    Modifier::Assign(word, _) => {
                        let expanded_word = parse_and_expand_modifier(word, state, ctx)?;
                        if state.is_readonly(var_name) {
                            return Err(format!(
                                "{}: is read only",
                                String::from_utf8_lossy(var_name.as_bytes())
                            ));
                        }
                        state.set_var(var_name, &expanded_word);
                        Ok(expanded_word)
                    }
                    Modifier::Error(opt_msg, _) => {
                        let msg = match opt_msg {
                            Some(w) => {
                                let expanded = parse_and_expand_modifier(w, state, ctx)?;
                                String::from_utf8_lossy(expanded.as_bytes()).into_owned()
                            }
                            None => format!(
                                "{}: parameter null or unset",
                                String::from_utf8_lossy(var_name.as_bytes())
                            ),
                        };
                        ctx.print_err(&msg)?;
                        Err(msg)
                    }
                    _ => unreachable!(),
                }
            }
            Modifier::RemovePrefix(pattern_word, longest) => {
                let val = state.get_var(var_name).unwrap_or_default();
                let pattern = parse_and_expand_modifier(pattern_word, state, ctx)?;
                Ok(strip_pattern(val.as_bstr(), pattern.as_bstr(), StripKind::Prefix, longest)
                    .to_owned())
            }
            Modifier::RemoveSuffix(pattern_word, longest) => {
                let val = state.get_var(var_name).unwrap_or_default();
                let pattern = parse_and_expand_modifier(pattern_word, state, ctx)?;
                Ok(strip_pattern(val.as_bstr(), pattern.as_bstr(), StripKind::Suffix, longest)
                    .to_owned())
            }
        }
    } else {
        Ok(state.get_var(var_name).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TildeExpansionMode {
    Assignment,
    LeadingWordPart,
    SubsequentWordPart,
}

fn expand_tilde(word_part_string: &BStr, state: &ShellState, mode: TildeExpansionMode) -> BString {
    let home_directory = state.get_var(b"HOME").unwrap_or_default();

    match mode {
        TildeExpansionMode::Assignment => {
            let mut result_bytes =
                Vec::with_capacity(word_part_string.len() + home_directory.len());
            let mut is_first_colon_part = true;
            for colon_part in word_part_string.as_bytes().split(|&byte| byte == b':') {
                if !is_first_colon_part {
                    result_bytes.push(b':');
                }
                is_first_colon_part = false;

                let colon_part_string = BStr::new(colon_part);
                if colon_part_string == "~" {
                    result_bytes.extend_from_slice(home_directory.as_bytes());
                } else if colon_part_string.starts_with(b"~/") {
                    result_bytes.extend_from_slice(home_directory.as_bytes());
                    result_bytes.extend_from_slice(&colon_part[1..]);
                } else {
                    result_bytes.extend_from_slice(colon_part);
                }
            }
            BString::from(result_bytes)
        }
        TildeExpansionMode::LeadingWordPart => {
            if word_part_string == "~" {
                home_directory
            } else if word_part_string.starts_with(b"~/") {
                let mut result_bytes = Vec::from(home_directory);
                result_bytes.extend_from_slice(&word_part_string.as_bytes()[1..]);
                BString::from(result_bytes)
            } else {
                word_part_string.to_owned()
            }
        }
        TildeExpansionMode::SubsequentWordPart => word_part_string.to_owned(),
    }
}

fn split_word_chars_by_ifs(word: &[WordChar], ifs: &BStr) -> Vec<Vec<WordChar>> {
    let mut results = Vec::new();
    let mut start = 0;

    // Skip leading IFS whitespace
    while start < word.len() && word[start].is_ifs_whitespace(ifs) {
        start += 1;
    }

    if start >= word.len() {
        return results;
    }

    let mut current_field = Vec::new();
    let mut has_fields = false;
    let mut i = start;

    while i < word.len() {
        let w = &word[i];
        if w.is_ifs_whitespace(ifs) {
            let mut has_adjacent_non_whitespace = false;
            let mut next_i = i + 1;
            while next_i < word.len() {
                if word[next_i].is_ifs_whitespace(ifs) {
                    next_i += 1;
                } else if word[next_i].is_ifs_non_whitespace(ifs) {
                    has_adjacent_non_whitespace = true;
                    next_i += 1;
                    break;
                } else {
                    break;
                }
            }

            if has_adjacent_non_whitespace {
                while next_i < word.len() && word[next_i].is_ifs_whitespace(ifs) {
                    next_i += 1;
                }
            }

            results.push(std::mem::take(&mut current_field));
            has_fields = has_adjacent_non_whitespace;
            i = next_i;
        } else if w.is_ifs_non_whitespace(ifs) {
            let mut next_i = i + 1;
            while next_i < word.len() && word[next_i].is_ifs_whitespace(ifs) {
                next_i += 1;
            }

            results.push(std::mem::take(&mut current_field));
            has_fields = true;
            i = next_i;
        } else {
            current_field.push(w.clone());
            has_fields = true;
            i += 1;
        }
    }

    if has_fields || !current_field.is_empty() {
        results.push(current_field);
    }

    results
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TildeColonMode {
    ExpandAfterColons,
    DoNotExpandAfterColons,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldSplitMode {
    Split,
    DoNotSplit,
}

/// Expands an AST argument slice into sequences of `WordChar`s preserving quote metadata.
///
/// Handles tilde expansion, parameter expansion, command substitution, arithmetic expansion,
/// and optional IFS field splitting while distinguishing quoted literals from unquoted wildcards.
pub fn expand_argument_to_word_chars(
    word_parts: &[WordPart],
    state: &mut ShellState,
    context: &ExecutionContext,
    tilde_colon_mode: TildeColonMode,
    field_split_mode: FieldSplitMode,
    buffer: &relative::Buffer,
) -> Result<Vec<Vec<WordChar>>, String> {
    if word_parts.is_empty() {
        return Ok(vec![Vec::new()]);
    }

    let internal_field_separator = state.get_var(b"IFS").unwrap_or_else(|| BString::from(" \t\n"));

    let mut fields: Vec<Vec<WordChar>> = Vec::new();
    let mut current_field: Vec<WordChar> = Vec::new();
    let mut current_has_quoted = false;
    let mut has_fields = false;

    for (part_index, part) in word_parts.iter().enumerate() {
        match part.tag {
            WordPartTag::LITERAL => {
                let literal_string = part.text.as_bstr(buffer);
                let tilde_mode = if tilde_colon_mode == TildeColonMode::ExpandAfterColons {
                    TildeExpansionMode::Assignment
                } else if part_index == 0 {
                    TildeExpansionMode::LeadingWordPart
                } else {
                    TildeExpansionMode::SubsequentWordPart
                };
                let expanded_string = expand_tilde(literal_string, state, tilde_mode);
                for &byte in expanded_string.as_bytes() {
                    current_field.push(WordChar::Unquoted(byte));
                }
                has_fields = true;
            }
            WordPartTag::QUOTED_LITERAL => {
                let quoted_string = part.text.as_bstr(buffer);
                for &byte in quoted_string.as_bytes() {
                    current_field.push(WordChar::Quoted(byte));
                }
                current_has_quoted = true;
                has_fields = true;
            }
            WordPartTag::QUOTED_VAR => {
                let variable_name = part.text.as_bstr(buffer);
                if variable_name == "@" {
                    let arguments = state.get_args();
                    if !arguments.is_empty() {
                        current_has_quoted = true;
                        has_fields = true;
                        for &byte in arguments[0].as_bytes() {
                            current_field.push(WordChar::Quoted(byte));
                        }
                        if arguments.len() > 1 {
                            fields.push(std::mem::take(&mut current_field));
                            for i in 1..arguments.len() - 1 {
                                let mut argument_word = Vec::new();
                                for &byte in arguments[i].as_bytes() {
                                    argument_word.push(WordChar::Quoted(byte));
                                }
                                fields.push(argument_word);
                            }
                            current_field = Vec::new();
                            for &byte in arguments.last().unwrap().as_bytes() {
                                current_field.push(WordChar::Quoted(byte));
                            }
                        }
                    }
                } else {
                    current_has_quoted = true;
                    has_fields = true;
                    let value = expand_var_with_modifiers(variable_name, state, context)?;
                    for &byte in value.as_bytes() {
                        current_field.push(WordChar::Quoted(byte));
                    }
                }
            }
            WordPartTag::QUOTED_COMMAND_SUBSTITUTION => {
                current_has_quoted = true;
                has_fields = true;
                let command = part.command.as_ref(buffer);
                let value = run_command_substitution(command, state, context, buffer)?;
                for &byte in value.as_bytes() {
                    current_field.push(WordChar::Quoted(byte));
                }
            }
            WordPartTag::VAR => {
                let variable_name = part.text.as_bstr(buffer);
                let value = expand_var_with_modifiers(variable_name, state, context)?;
                for &byte in value.as_bytes() {
                    current_field.push(WordChar::Expansion(byte));
                }
                has_fields = true;
            }
            WordPartTag::COMMAND_SUBSTITUTION => {
                let command = part.command.as_ref(buffer);
                let value = run_command_substitution(command, state, context, buffer)?;
                for &byte in value.as_bytes() {
                    current_field.push(WordChar::Expansion(byte));
                }
                has_fields = true;
            }
            WordPartTag::ARITHMETIC => {
                let expression = part.text.as_bstr(buffer);
                let value = evaluate_arithmetic(expression, state, context)?.to_string();
                for &byte in value.as_bytes() {
                    current_field.push(WordChar::Expansion(byte));
                }
                has_fields = true;
            }
            WordPartTag::QUOTED_ARITHMETIC => {
                current_has_quoted = true;
                has_fields = true;
                let expression = part.text.as_bstr(buffer);
                let value = evaluate_arithmetic(expression, state, context)?.to_string();
                for &byte in value.as_bytes() {
                    current_field.push(WordChar::Quoted(byte));
                }
            }
            _ => unreachable!(),
        }
    }

    if has_fields || current_has_quoted || !current_field.is_empty() {
        fields.push(current_field);
    }

    if field_split_mode == FieldSplitMode::Split && !internal_field_separator.is_empty() {
        let mut split_fields = Vec::new();
        for field in fields {
            split_fields
                .extend(split_word_chars_by_ifs(&field, internal_field_separator.as_bstr()));
        }
        Ok(split_fields)
    } else {
        Ok(fields)
    }
}

/// Expands an AST argument word into a list of resulting byte strings.
///
/// Performs full POSIX word expansion including field splitting and glob pattern expansion
/// (unless `noglob` is set).
pub fn expand_argument(
    arg: &[WordPart],
    state: &mut ShellState,
    ctx: &ExecutionContext,
    buf: &relative::Buffer,
) -> Result<Vec<BString>, String> {
    let word_chars_list = expand_argument_to_word_chars(
        arg,
        state,
        ctx,
        TildeColonMode::DoNotExpandAfterColons,
        FieldSplitMode::Split,
        buf,
    )?;
    let mut final_results = Vec::new();
    for word in word_chars_list {
        if state.opt_noglob {
            final_results.push(word_chars_to_bstring(&word));
        } else {
            let matches = expand_glob(&word);
            final_results.extend(matches);
        }
    }
    Ok(final_results)
}

/// Expands an AST argument word into a single byte string without IFS field splitting or globbing.
///
/// Used for word expansions in contexts like double quotes or case statements.
pub fn expand_argument_no_split(
    arg: &[WordPart],
    state: &mut ShellState,
    ctx: &ExecutionContext,
    buf: &relative::Buffer,
) -> Result<BString, String> {
    let word_chars_list = expand_argument_to_word_chars(
        arg,
        state,
        ctx,
        TildeColonMode::DoNotExpandAfterColons,
        FieldSplitMode::DoNotSplit,
        buf,
    )?;
    if word_chars_list.is_empty() {
        return Ok(BString::default());
    }
    let word = &word_chars_list[0];
    Ok(word_chars_to_bstring(word))
}

/// Expands the value side of a variable assignment statement (e.g. `VAR=value`).
///
/// Supports tilde expansion after colons inside the assignment value (`PATH=~/bin:~/usr/bin`).
pub fn expand_assignment_value(
    val_start: &BStr,
    remaining: &[WordPart],
    state: &mut ShellState,
    ctx: &ExecutionContext,
    buf: &relative::Buffer,
) -> Result<BString, String> {
    let mut parts = Vec::new();
    let mut builder = ASTBuilder::new();
    if !val_start.is_empty() {
        parts.push(ResolvedWordPart::Literal(val_start.to_owned()));
    }
    for p in remaining {
        match p.tag {
            WordPartTag::LITERAL => parts.push(ResolvedWordPart::Literal(p.text.to_bstring(buf))),
            WordPartTag::VAR => parts.push(ResolvedWordPart::Var(p.text.to_bstring(buf))),
            WordPartTag::QUOTED_LITERAL => {
                parts.push(ResolvedWordPart::QuotedLiteral(p.text.to_bstring(buf)))
            }
            WordPartTag::QUOTED_VAR => {
                parts.push(ResolvedWordPart::QuotedVar(p.text.to_bstring(buf)))
            }
            WordPartTag::COMMAND_SUBSTITUTION => {
                let cmd = p.command.as_ref(buf);
                let bytes = cmd.serialize(buf);
                let off = builder.import_serialized_ast(&bytes);
                parts.push(ResolvedWordPart::CommandSubstitution(off));
            }
            WordPartTag::QUOTED_COMMAND_SUBSTITUTION => {
                let cmd = p.command.as_ref(buf);
                let bytes = cmd.serialize(buf);
                let off = builder.import_serialized_ast(&bytes);
                parts.push(ResolvedWordPart::QuotedCommandSubstitution(off));
            }
            WordPartTag::ARITHMETIC => {
                parts.push(ResolvedWordPart::Arithmetic(p.text.to_bstring(buf)))
            }
            WordPartTag::QUOTED_ARITHMETIC => {
                parts.push(ResolvedWordPart::QuotedArithmetic(p.text.to_bstring(buf)))
            }
            _ => unreachable!(),
        }
    }
    let word_slice = builder.add_resolved_word(&parts);
    let slice = builder.get_slice(word_slice);
    let word_chars_list = expand_argument_to_word_chars(
        slice,
        state,
        ctx,
        TildeColonMode::ExpandAfterColons,
        FieldSplitMode::DoNotSplit,
        &builder,
    )?;
    if word_chars_list.is_empty() {
        return Ok(BString::default());
    }
    let word = &word_chars_list[0];
    Ok(word_chars_to_bstring(word))
}

pub fn needs_subshell_process<'a>(
    mut command: &'a Command,
    state: &ShellState,
    buffer: &'a relative::Buffer,
) -> bool {
    loop {
        match command.tag {
            CommandTag::SIMPLE => {
                let arguments = command.simple_args.as_slice(buffer);
                let mut command_arguments = Vec::new();
                let mut parsing_assignments = true;
                for argument in arguments {
                    let parts = argument.as_slice(buffer);
                    if parsing_assignments && is_assignment_flat(parts, buffer) {
                        // skip leading assignments
                    } else {
                        parsing_assignments = false;
                        command_arguments.push(parts);
                    }
                }
                if command_arguments.is_empty() {
                    return false;
                } else {
                    let first_argument = command_arguments[0];
                    if first_argument.len() == 1 {
                        let part = &first_argument[0];
                        match part.tag {
                            WordPartTag::LITERAL | WordPartTag::QUOTED_LITERAL => {
                                let literal_string = part.text.as_bstr(buffer);
                                return is_builtin(literal_string)
                                    || state.get_function(literal_string).is_some()
                                    || state.aliases.contains_key(literal_string);
                            }
                            _ => return false,
                        }
                    } else {
                        return false;
                    }
                }
            }
            CommandTag::REDIRECT => {
                command = command.left.as_ref(buffer);
            }
            _ => return true,
        }
    }
}

fn extract_parenthesized_bytes<'a>(
    bytes: &'a [u8],
    index: &mut usize,
    is_double: bool,
) -> &'a BStr {
    let start_index = *index;
    let mut depth = if is_double { 2 } else { 1 };
    let mut end_index = start_index;

    while *index < bytes.len() {
        let current_byte = bytes[*index];
        match current_byte {
            b'\\' => {
                *index += 1;
                if *index < bytes.len() {
                    *index += 1;
                }
            }
            b'(' => {
                depth += 1;
                *index += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    end_index = *index;
                    *index += 1;
                    break;
                }
                if is_double && depth == 1 {
                    if *index + 1 < bytes.len() && bytes[*index + 1] == b')' {
                        end_index = *index;
                        *index += 2;
                        break;
                    }
                }
                *index += 1;
            }
            _ => {
                *index += 1;
            }
        }
    }
    if end_index < start_index {
        end_index = *index;
    }
    BStr::new(&bytes[start_index..end_index])
}

fn expand_dollar(
    bytes: &[u8],
    index: &mut usize,
    state: &mut ShellState,
    context: &ExecutionContext,
    result_bytes: &mut Vec<u8>,
) -> Result<(), String> {
    if *index + 1 < bytes.len() && bytes[*index + 1] == b'(' {
        *index += 2; // consume '$' and '('
        let is_double = if *index < bytes.len() && bytes[*index] == b'(' {
            *index += 1; // consume second '('
            true
        } else {
            false
        };

        let inner_bytes = extract_parenthesized_bytes(bytes, index, is_double);

        if is_double {
            let expanded_inner = expand_string(inner_bytes, state, context)?;
            let value = evaluate_arithmetic(expanded_inner.as_bstr(), state, context)?;
            result_bytes.extend_from_slice(value.to_string().as_bytes());
        } else {
            let mut sub_builder = ASTBuilder::new();
            let command_pointer = parse_subshell_command(&mut sub_builder, inner_bytes.as_bytes())
                .map_err(|e| e.to_string())?;
            let command = sub_builder.get_ref(command_pointer);
            let value = run_command_substitution(command, state, context, &sub_builder)?;
            result_bytes.extend_from_slice(value.as_bytes());
        }
    } else if *index + 1 < bytes.len() && bytes[*index + 1] == b'{' {
        *index += 2; // consume '$' and '{'
        let mut variable_name_bytes = Vec::new();
        while *index < bytes.len() {
            let current_byte = bytes[*index];
            if current_byte == b'}' {
                *index += 1;
                break;
            }
            variable_name_bytes.push(current_byte);
            *index += 1;
        }
        let value = expand_var_with_modifiers(BStr::new(&variable_name_bytes), state, context)?;
        result_bytes.extend_from_slice(value.as_bytes());
    } else {
        *index += 1; // consume '$'
        let mut variable_name_bytes = Vec::new();
        if *index < bytes.len() {
            let current_byte = bytes[*index];
            if matches!(current_byte, b'?' | b'#' | b'@' | b'*' | b'$' | b'!' | b'-')
                || current_byte.is_ascii_digit()
            {
                variable_name_bytes.push(current_byte);
                *index += 1;
            } else {
                while *index < bytes.len() {
                    let c = bytes[*index];
                    if c.is_ascii_alphanumeric() || c == b'_' {
                        variable_name_bytes.push(c);
                        *index += 1;
                    } else {
                        break;
                    }
                }
            }
        }
        if variable_name_bytes.is_empty() {
            result_bytes.push(b'$');
        } else {
            let value = expand_var_with_modifiers(BStr::new(&variable_name_bytes), state, context)?;
            result_bytes.extend_from_slice(value.as_bytes());
        }
    }
    Ok(())
}

/// Expands variable and arithmetic expressions inside an unquoted string or heredoc body.
pub fn expand_string(
    string: &BStr,
    state: &mut ShellState,
    context: &ExecutionContext,
) -> Result<BString, String> {
    let mut result_bytes: Vec<u8> = Vec::new();
    let bytes = string.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'\\' => {
                if index + 1 < bytes.len() {
                    let next_byte = bytes[index + 1];
                    if next_byte == b'$' || next_byte == b'\\' || next_byte == b'`' {
                        result_bytes.push(next_byte);
                        index += 2;
                    } else {
                        result_bytes.push(b'\\');
                        index += 1;
                    }
                } else {
                    result_bytes.push(b'\\');
                    index += 1;
                }
            }
            b'$' => {
                expand_dollar(bytes, &mut index, state, context, &mut result_bytes)?;
            }
            _ => {
                result_bytes.push(byte);
                index += 1;
            }
        }
    }
    Ok(BString::from(result_bytes))
}

/// Reads a prompt variable by name, falling back to `default_prompt` if unset,
/// and expands variable and arithmetic expressions inside it.
pub fn expand_prompt(
    var_name: &BStr,
    default_prompt: &BStr,
    state: &mut ShellState,
    context: &ExecutionContext,
) -> BString {
    let prompt_owned = state.get_var(var_name);
    let prompt_raw = match &prompt_owned {
        Some(s) => s.as_ref(),
        None => default_prompt,
    };
    match expand_string(prompt_raw, state, context) {
        Ok(expanded) => expanded,
        Err(_) => BString::from(default_prompt),
    }
}

/// Extracts the literal command string if the argument word consists solely of a single unquoted
/// literal.
pub fn get_literal_command_name(arg: &[WordPart], buf: &relative::Buffer) -> Option<BString> {
    if arg.len() == 1 {
        match arg[0].tag {
            WordPartTag::LITERAL => Some(arg[0].text.to_bstring(buf)),
            _ => None,
        }
    } else {
        None
    }
}

/// Appends additional argument words to an existing AST `Command` node in the buffer.
///
/// Recursively traverses pipelines, sequences, logical lists, and control flow branches to append
/// to the trailing command.
pub fn append_args_to_command(
    builder: &mut ASTBuilder,
    cmd_ptr: relative::Ptr<Command>,
    extra_args: &[relative::Slice<WordPart>],
) -> relative::Ptr<Command> {
    if extra_args.is_empty() {
        return cmd_ptr;
    }
    let tag = builder.get_ref(cmd_ptr).tag;
    match tag {
        CommandTag::SIMPLE => {
            let mut all_refs = Vec::new();
            {
                let cmd = builder.get_ref(cmd_ptr);
                let old_args_slice = cmd.simple_args.as_slice(builder);
                for &old_arg in old_args_slice {
                    all_refs.push(old_arg);
                }
            }
            for &new_arg in extra_args {
                all_refs.push(new_arg);
            }
            let new_simple_args = builder.add_argument_refs(&all_refs);

            let cmd_mut = builder.get_mut(cmd_ptr);
            cmd_mut.simple_args = new_simple_args;

            cmd_ptr
        }
        CommandTag::PIPELINE => {
            let right_ptr = {
                let cmd = builder.get_ref(cmd_ptr);
                cmd.right
            };
            append_args_to_command(builder, right_ptr, extra_args);
            cmd_ptr
        }
        CommandTag::SEQUENCE => {
            let last_ptr = {
                let cmd = builder.get_ref(cmd_ptr);
                let seq = cmd.sequence.as_slice(builder);
                seq.last().copied()
            };
            if let Some(last_ptr) = last_ptr {
                append_args_to_command(builder, last_ptr, extra_args);
            }
            cmd_ptr
        }
        CommandTag::LOGICAL_AND | CommandTag::LOGICAL_OR => {
            let right_ptr = {
                let cmd = builder.get_ref(cmd_ptr);
                cmd.right
            };
            append_args_to_command(builder, right_ptr, extra_args);
            cmd_ptr
        }
        CommandTag::IF => {
            let then_ptr = {
                let cmd = builder.get_ref(cmd_ptr);
                cmd.then_branch
            };
            append_args_to_command(builder, then_ptr, extra_args);
            cmd_ptr
        }
        CommandTag::WHILE | CommandTag::UNTIL | CommandTag::FOR => {
            let body_ptr = {
                let cmd = builder.get_ref(cmd_ptr);
                cmd.then_branch
            };
            append_args_to_command(builder, body_ptr, extra_args);
            cmd_ptr
        }
        _ => cmd_ptr,
    }
}

/// Represents the result of expanding an alias definition.
pub enum ExpandedCommand {
    /// A list of simple argument words resulting from alias expansion.
    Words(Vec<relative::Slice<WordPart>>),
    /// A complex compound AST command resulting from alias expansion.
    Command(relative::Ptr<Command>),
}

/// Evaluates and expands alias definitions for the initial command word, detecting recursive
/// expansion cycles.
pub fn expand_alias(
    builder: &mut ASTBuilder,
    args: &[relative::Slice<WordPart>],
    state: &ShellState,
    ctx: &mut ExecutionContext,
    active: &mut FlatSet<BString>,
) -> Result<Option<ExpandedCommand>, String> {
    if args.is_empty() {
        return Ok(None);
    }
    let name_opt = {
        let arg0 = builder.get_slice(args[0]);
        get_literal_command_name(arg0, builder)
    };
    if let Some(name) = name_opt {
        if let Some(val) = state.aliases.get(&name).cloned() {
            if !ctx.active_aliases.contains(&name) && !active.contains(&name) {
                let val_tokens = tokenize(val.as_bytes()).map_err(|e| e.to_string())?;
                let is_simple = val_tokens.iter().all(|t| matches!(t, Token::Word(_)));
                if is_simple {
                    let mut new_words = Vec::new();
                    for t in &val_tokens {
                        if let Token::Word(parts) = t {
                            let temp_parts =
                                resolve_word_parts(builder, parts).map_err(|e| e.to_string())?;
                            let word_slice = builder.add_resolved_word(&temp_parts);
                            new_words.push(word_slice);
                        }
                    }
                    active.insert(name.clone());

                    let mut resolved_replacement = new_words.clone();
                    if let Some(ExpandedCommand::Words(nested)) =
                        expand_alias(builder, &new_words, state, ctx, active)?
                    {
                        resolved_replacement = nested;
                    }
                    active.remove(&name);

                    let mut final_args = resolved_replacement;
                    if val.ends_with(b" ") && args.len() > 1 {
                        let mut remaining_refs = args[1..].to_vec();
                        if let Some(ExpandedCommand::Words(nested)) =
                            expand_alias(builder, &remaining_refs, state, ctx, active)?
                        {
                            remaining_refs = nested;
                        }
                        final_args.extend(remaining_refs);
                    } else {
                        for &arg in &args[1..] {
                            final_args.push(arg);
                        }
                    }
                    return Ok(Some(ExpandedCommand::Words(final_args)));
                } else {
                    let val_cmds = parse_script(builder, &val_tokens).map_err(|e| e.to_string())?;
                    let val_cmd_ptr = builder.add_sequence_or_single(&val_cmds);
                    let merged = append_args_to_command(builder, val_cmd_ptr, &args[1..]);
                    return Ok(Some(ExpandedCommand::Command(merged)));
                }
            }
        }
    }
    Ok(None)
}
