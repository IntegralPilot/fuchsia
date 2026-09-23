// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::Args;
use crate::collections::{FlatMap, FlatSet};
use crate::eval::testing::{
    ExpandedCommand, append_args_to_command, expand_alias, expand_argument,
    expand_assignment_value, expand_var_with_modifiers, get_literal_command_name,
    needs_subshell_process,
};
use crate::eval::{ExecutionContext, ShellState, expand_prompt, expand_string};
use crate::parser::ast::{ASTBuilder, CommandTag, ResolvedWordPart, WordPart, WordPartTag};
use crate::parser::{parse_script, tokenize};
use crate::relative;
use bstr::{BStr, BString, ByteSlice};

fn is_assignment(arg: &[WordPart], buf: &relative::Buffer) -> bool {
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

fn make_slice(
    builder: &mut ASTBuilder,
    parts: &[ResolvedWordPart],
) -> relative::Slice<crate::parser::ast::WordPart> {
    builder.add_resolved_word(parts)
}

fn check_expand(
    builder: &mut ASTBuilder,
    parts: &[ResolvedWordPart],
    state: &mut ShellState,
    ctx: &ExecutionContext,
) -> Vec<BString> {
    let slice_ptr = make_slice(builder, parts);
    let arg = builder.get_slice(slice_ptr);
    expand_argument(arg, state, ctx, builder).unwrap()
}

fn check_assignment(builder: &mut ASTBuilder, parts: &[ResolvedWordPart]) -> bool {
    let slice_ptr = make_slice(builder, parts);
    let arg = builder.get_slice(slice_ptr);
    is_assignment(arg, builder)
}

#[test]
fn test_expand_argument() {
    let mut state = ShellState::new();
    state.set_var(BStr::new(b"FOO"), BStr::new(b"one two"));
    state.set_var(BStr::new(b"BAR"), BStr::new(b"three"));
    let ctx = ExecutionContext::initial().unwrap();

    // Literal
    let mut enc = ASTBuilder::new();
    assert_eq!(
        check_expand(
            &mut enc,
            &[ResolvedWordPart::Literal(BString::from("hello"))],
            &mut state,
            &ctx
        ),
        vec![BString::from("hello")]
    );

    // Var (unquoted) -> splits
    let mut enc = ASTBuilder::new();
    assert_eq!(
        check_expand(&mut enc, &[ResolvedWordPart::Var(BString::from("FOO"))], &mut state, &ctx),
        vec![BString::from("one"), BString::from("two")]
    );

    // QuotedVar -> no split
    let mut enc = ASTBuilder::new();
    assert_eq!(
        check_expand(
            &mut enc,
            &[ResolvedWordPart::QuotedVar(BString::from("FOO"))],
            &mut state,
            &ctx
        ),
        vec![BString::from("one two")]
    );

    // Combination: hello$FOO"world"
    let mut enc = ASTBuilder::new();
    assert_eq!(
        check_expand(
            &mut enc,
            &[
                ResolvedWordPart::Literal(BString::from("hello")),
                ResolvedWordPart::Var(BString::from("FOO")),
                ResolvedWordPart::QuotedLiteral(BString::from("world")),
            ],
            &mut state,
            &ctx
        ),
        vec![BString::from("helloone"), BString::from("twoworld")]
    );

    // Positional parameters
    let mut env_args = ShellState::with_args(
        Args::with_positionals(
            BString::from("script.sh"),
            vec![BString::from("a b"), BString::from("c")],
        ),
        FlatMap::new(),
    )
    .unwrap();

    // "$@" -> separate args, preserving spaces in args
    let mut enc = ASTBuilder::new();
    assert_eq!(
        check_expand(
            &mut enc,
            &[ResolvedWordPart::QuotedVar(BString::from("@"))],
            &mut env_args,
            &ctx
        ),
        vec![BString::from("a b"), BString::from("c")]
    );

    // "$*" -> single arg joined by space
    let mut enc = ASTBuilder::new();
    assert_eq!(
        check_expand(
            &mut enc,
            &[ResolvedWordPart::QuotedVar(BString::from("*"))],
            &mut env_args,
            &ctx
        ),
        vec![BString::from("a b c")]
    );

    // Empty args "$@" -> 0 args
    let mut env_empty = ShellState::with_args(
        Args::with_positionals(BString::from("script.sh"), Vec::new()),
        FlatMap::new(),
    )
    .unwrap();
    let mut enc = ASTBuilder::new();
    assert_eq!(
        check_expand(
            &mut enc,
            &[ResolvedWordPart::QuotedVar(BString::from("@"))],
            &mut env_empty,
            &ctx
        ),
        Vec::<BString>::new()
    );

    // Empty args "$@" with prefix -> 1 arg
    let mut enc = ASTBuilder::new();
    assert_eq!(
        check_expand(
            &mut enc,
            &[
                ResolvedWordPart::Literal(BString::from("prefix")),
                ResolvedWordPart::QuotedVar(BString::from("@")),
            ],
            &mut env_empty,
            &ctx
        ),
        vec![BString::from("prefix")]
    );
}

#[test]
fn test_is_assignment() {
    let mut enc = ASTBuilder::new();
    assert!(check_assignment(&mut enc, &[ResolvedWordPart::Literal(BString::from("FOO=bar"))]));
    let mut enc = ASTBuilder::new();
    assert!(check_assignment(&mut enc, &[ResolvedWordPart::Literal(BString::from("A_B_C=123"))]));
    let mut enc = ASTBuilder::new();
    assert!(!check_assignment(&mut enc, &[ResolvedWordPart::Literal(BString::from("foo"))]));
    let mut enc = ASTBuilder::new();
    assert!(!check_assignment(&mut enc, &[ResolvedWordPart::Literal(BString::from("=bar"))]));
    // Quoted start is not assignment
    let mut enc = ASTBuilder::new();
    assert!(!check_assignment(
        &mut enc,
        &[ResolvedWordPart::QuotedLiteral(BString::from("FOO=bar"))]
    ));
}

#[test]
fn test_parameter_modifiers() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();

    // 1. Length
    state.set_var(b"VAR", b"hello");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"#VAR"), &mut state, &ctx).unwrap(), "5");

    // 2. Default value (unset / null)
    state.unset_var(b"VAR");
    assert_eq!(
        expand_var_with_modifiers(BStr::new(b"VAR:-default"), &mut state, &ctx).unwrap(),
        "default"
    );
    state.set_var(b"VAR", b"");
    assert_eq!(
        expand_var_with_modifiers(BStr::new(b"VAR:-default"), &mut state, &ctx).unwrap(),
        "default"
    );
    // non-null only
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR-default"), &mut state, &ctx).unwrap(), "");

    // 3. Assign default
    state.unset_var(b"VAR");
    assert_eq!(
        expand_var_with_modifiers(BStr::new(b"VAR:=assigned"), &mut state, &ctx).unwrap(),
        "assigned"
    );
    assert_eq!(state.get_var(b"VAR").unwrap(), "assigned");

    // 4. Alternative value
    state.set_var(b"VAR", b"hello");
    assert_eq!(
        expand_var_with_modifiers(BStr::new(b"VAR:+alternative"), &mut state, &ctx).unwrap(),
        "alternative"
    );
    state.unset_var(b"VAR");
    assert_eq!(
        expand_var_with_modifiers(BStr::new(b"VAR:+alternative"), &mut state, &ctx).unwrap(),
        ""
    );

    // 5. Remove prefix
    state.set_var(b"VAR", b"foobar");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR#foo"), &mut state, &ctx).unwrap(), "bar");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR#f*o"), &mut state, &ctx).unwrap(), "obar");
    // longest vs shortest prefix
    state.set_var(b"VAR", b"a/b/c");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR#*/"), &mut state, &ctx).unwrap(), "b/c");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR##*/"), &mut state, &ctx).unwrap(), "c");

    // 6. Remove suffix
    state.set_var(b"VAR", b"foobar");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR%bar"), &mut state, &ctx).unwrap(), "foo");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR%b*r"), &mut state, &ctx).unwrap(), "foo");
    // longest vs shortest suffix
    state.set_var(b"VAR", b"a/b/c");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR%/*"), &mut state, &ctx).unwrap(), "a/b");
    assert_eq!(expand_var_with_modifiers(BStr::new(b"VAR%%/*"), &mut state, &ctx).unwrap(), "a");
}

#[test]
fn test_expand_string_and_heredoc() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.set_var("FOO", "bar");

    let res = expand_string(BStr::new("hello $FOO $((1+2))"), &mut state, &ctx).unwrap();
    assert_eq!(res, "hello bar 3");

    let hd = expand_string(BStr::new("line 1: $FOO\nline 2: $((4+5))"), &mut state, &ctx).unwrap();
    assert_eq!(hd, "line 1: bar\nline 2: 9");
}

#[test]
fn test_needs_subshell_process() {
    let state = ShellState::new();
    let check_cmd = |s: &str, state: &ShellState| -> bool {
        let mut builder = ASTBuilder::new();
        let tokens = tokenize(BStr::new(s)).unwrap();
        let cmds = parse_script(&mut builder, &tokens).unwrap();
        let cmd = builder.get_ref(cmds[0]);
        needs_subshell_process(cmd, state, &builder)
    };

    assert!(check_cmd("echo hello", &state));
    assert!(!check_cmd("grep pattern", &state));
    assert!(!check_cmd("FOO=bar", &state));

    let mut builder = ASTBuilder::new();
    let sub_ptr = builder.add_unary_command(CommandTag::SUBSHELL, relative::Ptr::null());
    let sub_cmd = builder.get_ref(sub_ptr);
    assert!(needs_subshell_process(sub_cmd, &state, &builder));
}

#[test]
fn test_expand_assignment_value() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.set_var(BStr::new("HOME"), BStr::new("/home/test"));

    let mut builder = ASTBuilder::new();
    let parts = vec![
        ResolvedWordPart::Literal(BString::from("PATH=")),
        ResolvedWordPart::Literal(BString::from("~/bin:~/usr")),
    ];
    let w_slice = builder.add_resolved_word(&parts);
    let slice = builder.get_slice(w_slice);

    let res = expand_assignment_value(BStr::new("PATH="), &slice[1..], &mut state, &ctx, &builder)
        .unwrap();
    assert_eq!(res, "PATH=/home/test/bin:/home/test/usr");
}

#[test]
fn test_append_args_to_command() {
    let mut builder = ASTBuilder::new();
    let tokens = tokenize(BStr::new("echo foo | grep bar")).unwrap();
    let cmds = parse_script(&mut builder, &tokens).unwrap();
    let pipe_ptr = cmds[0];

    let parts = vec![ResolvedWordPart::Literal(BString::from("baz"))];
    let w_slice = builder.add_resolved_word(&parts);
    let extra = vec![w_slice];

    append_args_to_command(&mut builder, pipe_ptr, &extra);
}

#[test]
fn test_expand_alias() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.aliases.insert(BString::from("ll"), BString::from("ls -l"));
    state.aliases.insert(BString::from("rec"), BString::from("rec"));

    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Literal(BString::from("ll"))];
    let w_slice = builder.add_resolved_word(&parts);
    let args = vec![w_slice];

    let mut active = FlatSet::new();
    let res = expand_alias(&mut builder, &args, &state, &mut ctx, &mut active).unwrap();
    assert!(matches!(res, Some(ExpandedCommand::Words(_))));

    let rec_parts = vec![ResolvedWordPart::Literal(BString::from("rec"))];
    let rec_slice = builder.add_resolved_word(&rec_parts);
    let rec_args = vec![rec_slice];
    active.insert(BString::from("rec"));
    let res_rec = expand_alias(&mut builder, &rec_args, &state, &mut ctx, &mut active).unwrap();
    assert!(res_rec.is_none());
}

#[test]
fn test_parameter_modifiers_error() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();

    // Custom message
    state.unset_var(BStr::new("VAR"));
    assert!(expand_var_with_modifiers(BStr::new("VAR:?custom error"), &mut state, &ctx).is_err());
    assert!(expand_var_with_modifiers(BStr::new("VAR?custom error"), &mut state, &ctx).is_err());

    // Default message
    assert!(expand_var_with_modifiers(BStr::new("VAR:?"), &mut state, &ctx).is_err());
    assert!(expand_var_with_modifiers(BStr::new("VAR?"), &mut state, &ctx).is_err());

    // Set variable shouldn't error
    state.set_var(BStr::new("VAR"), BStr::new("set_val"));
    assert_eq!(
        expand_var_with_modifiers(BStr::new("VAR:?custom error"), &mut state, &ctx).unwrap(),
        "set_val"
    );
    assert_eq!(
        expand_var_with_modifiers(BStr::new("VAR?custom error"), &mut state, &ctx).unwrap(),
        "set_val"
    );
}

#[test]
fn test_parameter_modifiers_assign() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();

    // Assign non-null
    state.unset_var(BStr::new("VAR"));
    assert_eq!(
        expand_var_with_modifiers(BStr::new("VAR=new_val"), &mut state, &ctx).unwrap(),
        "new_val"
    );

    // Readonly assignment fails
    state.make_readonly(BStr::new("RO"));
    assert!(expand_var_with_modifiers(BStr::new("RO:=val"), &mut state, &ctx).is_err());
}

#[test]
fn test_parameter_modifiers_alternative() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();

    state.unset_var(BStr::new("VAR"));
    assert_eq!(expand_var_with_modifiers(BStr::new("VAR+alt"), &mut state, &ctx).unwrap(), "");

    state.set_var(BStr::new("VAR"), BStr::new("orig"));
    assert_eq!(expand_var_with_modifiers(BStr::new("VAR+alt"), &mut state, &ctx).unwrap(), "alt");
}

#[test]
fn test_parameter_modifiers_nounset() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.opt_nounset = true;

    state.unset_var(BStr::new("UNBOUND"));
    assert!(expand_var_with_modifiers(BStr::new("UNBOUND"), &mut state, &ctx).is_err());
    assert!(expand_var_with_modifiers(BStr::new("#UNBOUND"), &mut state, &ctx).is_err());
}

#[test]
fn test_positional_args_at_expansion() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.set_args(vec![BString::from("arg1"), BString::from("arg2"), BString::from("arg3")]);

    let mut enc = ASTBuilder::new();
    let res = check_expand(
        &mut enc,
        &[ResolvedWordPart::QuotedVar(BString::from("@"))],
        &mut state,
        &ctx,
    );
    assert_eq!(res, vec![BString::from("arg1"), BString::from("arg2"), BString::from("arg3")]);
}

#[test]
fn test_opt_noglob() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.opt_noglob = true;

    let mut enc = ASTBuilder::new();
    let res = check_expand(
        &mut enc,
        &[ResolvedWordPart::Literal(BString::from("*.rs"))],
        &mut state,
        &ctx,
    );
    assert_eq!(res, vec![BString::from("*.rs")]);
}

#[test]
fn test_expand_string_escapes_and_special_vars() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.set_args(vec![BString::from("first")]);
    state.set_var(BStr::new("?"), BStr::new("0"));

    let res =
        expand_string(BStr::new("escaped \\$FOO and \\\\ and \\` and $1 and $?"), &mut state, &ctx)
            .unwrap();
    assert_eq!(res, "escaped $FOO and \\ and ` and first and 0");
}

#[test]
fn test_expand_alias_trailing_space() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.aliases.insert(BString::from("echo_sp"), BString::from("echo "));
    state.aliases.insert(BString::from("foo"), BString::from("bar"));

    let mut builder = ASTBuilder::new();
    let parts1 = vec![ResolvedWordPart::Literal(BString::from("echo_sp"))];
    let parts2 = vec![ResolvedWordPart::Literal(BString::from("foo"))];
    let slice1 = builder.add_resolved_word(&parts1);
    let slice2 = builder.add_resolved_word(&parts2);
    let args = vec![slice1, slice2];

    let mut active = FlatSet::new();
    let res = expand_alias(&mut builder, &args, &state, &mut ctx, &mut active).unwrap();
    assert!(matches!(res, Some(ExpandedCommand::Words(_))));
}

#[test]
fn test_expand_alias_compound() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.aliases.insert(BString::from("myif"), BString::from("if true; then echo hi; fi"));

    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Literal(BString::from("myif"))];
    let slice = builder.add_resolved_word(&parts);
    let args = vec![slice];

    let mut active = FlatSet::new();
    let res = expand_alias(&mut builder, &args, &state, &mut ctx, &mut active).unwrap();
    assert!(matches!(res, Some(ExpandedCommand::Command(_))));
}

#[test]
fn test_expand_argument_arithmetic() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();

    let mut builder = ASTBuilder::new();
    let parts = vec![
        ResolvedWordPart::Arithmetic(BString::from("2 + 3")),
        ResolvedWordPart::Literal(BString::from("-")),
        ResolvedWordPart::QuotedArithmetic(BString::from("10 * 2")),
    ];
    let slice_ptr = make_slice(&mut builder, &parts);
    let arg = builder.get_slice(slice_ptr);
    let expanded = expand_argument(arg, &mut state, &ctx, &builder).unwrap();
    assert_eq!(expanded, vec![BString::from("5-20")]);
}

#[test]
fn test_tilde_expansion_assignment_and_leading() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.set_var(BStr::new("HOME"), BStr::new("/home/user"));

    let mut builder = ASTBuilder::new();

    // Leading tilde: ~ and ~/sub
    let parts1 = vec![ResolvedWordPart::Literal(BString::from("~"))];
    let parts2 = vec![ResolvedWordPart::Literal(BString::from("~/sub"))];
    assert_eq!(
        check_expand(&mut builder, &parts1, &mut state, &ctx),
        vec![BString::from("/home/user")]
    );
    assert_eq!(
        check_expand(&mut builder, &parts2, &mut state, &ctx),
        vec![BString::from("/home/user/sub")]
    );
}

#[test]
fn test_ifs_non_whitespace_splitting() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.set_var(BStr::new("IFS"), BStr::new(":"));
    state.set_var(BStr::new("VAR"), BStr::new("one:two::three"));

    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Var(BString::from("VAR"))];
    let res = check_expand(&mut builder, &parts, &mut state, &ctx);
    assert_eq!(
        res,
        vec![BString::from("one"), BString::from("two"), BString::from(""), BString::from("three")]
    );
}

#[test]
fn test_expand_string_braced_var_and_trailing_dollar() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    state.set_var(BStr::new("FOO"), BStr::new("bar"));

    let res =
        expand_string(BStr::new("val: ${FOO} and trailing $ or $5"), &mut state, &ctx).unwrap();
    assert_eq!(res, "val: bar and trailing $ or ");
}

#[test]
fn test_get_literal_command_name_edge_cases() {
    let mut builder = ASTBuilder::new();
    let parts = vec![
        ResolvedWordPart::Literal(BString::from("echo")),
        ResolvedWordPart::Literal(BString::from("extra")),
    ];
    let slice_ptr = make_slice(&mut builder, &parts);
    let arg = builder.get_slice(slice_ptr);
    assert!(get_literal_command_name(arg, &builder).is_none());

    let var_parts = vec![ResolvedWordPart::Var(BString::from("VAR"))];
    let var_slice = make_slice(&mut builder, &var_parts);
    let var_arg = builder.get_slice(var_slice);
    assert!(get_literal_command_name(var_arg, &builder).is_none());
}

#[test]
fn test_expand_alias_nested() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();
    state.aliases.insert(BString::from("a"), BString::from("b"));
    state.aliases.insert(BString::from("b"), BString::from("ls"));

    let mut builder = ASTBuilder::new();
    let parts = vec![ResolvedWordPart::Literal(BString::from("a"))];
    let slice = builder.add_resolved_word(&parts);
    let args = vec![slice];

    let mut active = FlatSet::new();
    let res = expand_alias(&mut builder, &args, &state, &mut ctx, &mut active).unwrap();
    assert!(matches!(res, Some(ExpandedCommand::Words(_))));
}

#[test]
fn test_expand_prompt() {
    let mut state = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();

    // Fall back to default when unset
    assert_eq!(
        expand_prompt(BStr::new("MY_PS"), BStr::new("default> "), &mut state, &ctx),
        "default> "
    );

    // Expand variable when set
    state.set_var(BStr::new("MY_PS"), BStr::new("${USER}@host$ "));
    state.set_var(BStr::new("USER"), BStr::new("testuser"));
    assert_eq!(
        expand_prompt(BStr::new("MY_PS"), BStr::new("default> "), &mut state, &ctx),
        "testuser@host$ "
    );

    // Fall back to default on invalid expansion syntax
    state.set_var(BStr::new("MY_PS"), BStr::new("$(( 1 / 0 ))"));
    assert_eq!(
        expand_prompt(BStr::new("MY_PS"), BStr::new("default> "), &mut state, &ctx),
        "default> "
    );
}
