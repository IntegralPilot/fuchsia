// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parser::ast::{
    ASTBuilder, Command, CommandTag, FlatASTHeader, RedirectTag, ResolvedWordPart, WordPart,
    WordPartTag,
};
use crate::parser::error::{IncompleteReason, ParseError};
use crate::parser::{parse_script, parse_subshell_command, tokenize};
use crate::relative;

fn parse(input: &[u8], builder: &mut ASTBuilder) -> Vec<relative::Ptr<Command>> {
    let tokens = tokenize(input).unwrap();
    parse_script(builder, &tokens).unwrap()
}

fn get_cmd_str(cmd: &Command, buf: &relative::Buffer) -> String {
    if cmd.tag == CommandTag::SIMPLE {
        cmd.simple_args
            .as_slice(buf)
            .iter()
            .map(|w| get_word_str(w, buf))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        "...".to_string()
    }
}

fn get_word_str(word: &relative::Slice<WordPart>, buf: &relative::Buffer) -> String {
    let parts = word.as_slice(buf);
    let mut s = String::new();
    for part in parts {
        match part.tag {
            WordPartTag::LITERAL => s.push_str(&part.text.to_bstring(buf).to_string()),
            WordPartTag::QUOTED_LITERAL => s.push_str(&format!("'{}'", part.text.to_bstring(buf))),
            WordPartTag::VAR => s.push_str(&format!("${}", part.text.to_bstring(buf))),
            WordPartTag::QUOTED_VAR => s.push_str(&format!("\"${}\"", part.text.to_bstring(buf))),
            WordPartTag::COMMAND_SUBSTITUTION => {
                s.push_str(&format!("$({})", get_cmd_str(part.command.as_ref(buf), buf)))
            }
            WordPartTag::QUOTED_COMMAND_SUBSTITUTION => {
                s.push_str(&format!("\"$({})\"", get_cmd_str(part.command.as_ref(buf), buf)))
            }
            WordPartTag::ARITHMETIC => s.push_str(&format!("$(({}))", part.text.to_bstring(buf))),
            WordPartTag::QUOTED_ARITHMETIC => {
                s.push_str(&format!("\"$(({}))\"", part.text.to_bstring(buf)))
            }
            _ => panic!("unknown tag"),
        }
    }
    s
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
enum ExpectRedir {
    ToFile(i32, String, bool, bool),
    FromFile(i32, String),
    DupFd(i32, i32),
    CloseFd(i32),
    HereDoc(i32, String, bool),
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
enum ExpectCmd {
    Simple(Vec<String>),
    Pipeline(Box<ExpectCmd>, Box<ExpectCmd>),
    Redirect(Box<ExpectCmd>, Vec<ExpectRedir>),
    Subshell(Box<ExpectCmd>),
    If(Box<ExpectCmd>, Box<ExpectCmd>, Option<Box<ExpectCmd>>),
    While(Box<ExpectCmd>, Box<ExpectCmd>),
    Until(Box<ExpectCmd>, Box<ExpectCmd>),
    For(String, Vec<String>, Box<ExpectCmd>),
    Case(String, Vec<(Vec<String>, ExpectCmd)>),
    FunctionDef(String, Box<ExpectCmd>),
    And(Box<ExpectCmd>, Box<ExpectCmd>),
    Or(Box<ExpectCmd>, Box<ExpectCmd>),
    Bg(Box<ExpectCmd>),
    Sequence(Vec<ExpectCmd>),
}

fn r_out(file: &str) -> ExpectRedir {
    ExpectRedir::ToFile(1, file.to_string(), false, false)
}
fn r_out_fd(fd: i32, file: &str) -> ExpectRedir {
    ExpectRedir::ToFile(fd, file.to_string(), false, false)
}
fn r_app(file: &str) -> ExpectRedir {
    ExpectRedir::ToFile(1, file.to_string(), true, false)
}
fn r_clob(file: &str) -> ExpectRedir {
    ExpectRedir::ToFile(1, file.to_string(), false, true)
}
fn r_in(file: &str) -> ExpectRedir {
    ExpectRedir::FromFile(0, file.to_string())
}
fn r_in_fd(fd: i32, file: &str) -> ExpectRedir {
    ExpectRedir::FromFile(fd, file.to_string())
}
fn r_dup_in(src: i32, dest: i32) -> ExpectRedir {
    ExpectRedir::DupFd(src, dest)
}
fn r_close(src: i32) -> ExpectRedir {
    ExpectRedir::CloseFd(src)
}
fn r_heredoc(body: &str, expand: bool) -> ExpectRedir {
    ExpectRedir::HereDoc(0, body.to_string(), expand)
}

fn simple(args: &[&str]) -> ExpectCmd {
    ExpectCmd::Simple(args.iter().map(|s| s.to_string()).collect())
}
fn pipe(left: ExpectCmd, right: ExpectCmd) -> ExpectCmd {
    ExpectCmd::Pipeline(Box::new(left), Box::new(right))
}
fn redir(cmd: ExpectCmd, redirs: Vec<ExpectRedir>) -> ExpectCmd {
    ExpectCmd::Redirect(Box::new(cmd), redirs)
}
fn subshell(cmd: ExpectCmd) -> ExpectCmd {
    ExpectCmd::Subshell(Box::new(cmd))
}
fn if_cmd(cond: ExpectCmd, then_b: ExpectCmd, else_b: Option<ExpectCmd>) -> ExpectCmd {
    ExpectCmd::If(Box::new(cond), Box::new(then_b), else_b.map(Box::new))
}
fn while_cmd(cond: ExpectCmd, body: ExpectCmd) -> ExpectCmd {
    ExpectCmd::While(Box::new(cond), Box::new(body))
}
#[allow(dead_code)]
fn until_cmd(cond: ExpectCmd, body: ExpectCmd) -> ExpectCmd {
    ExpectCmd::Until(Box::new(cond), Box::new(body))
}
fn for_cmd(var: &str, items: &[&str], body: ExpectCmd) -> ExpectCmd {
    ExpectCmd::For(var.to_string(), items.iter().map(|s| s.to_string()).collect(), Box::new(body))
}
fn case_cmd(word: &str, items: Vec<(Vec<&str>, ExpectCmd)>) -> ExpectCmd {
    ExpectCmd::Case(
        word.to_string(),
        items
            .into_iter()
            .map(|(pats, cmd)| (pats.into_iter().map(|s| s.to_string()).collect(), cmd))
            .collect(),
    )
}
fn func_def(name: &str, body: ExpectCmd) -> ExpectCmd {
    ExpectCmd::FunctionDef(name.to_string(), Box::new(body))
}
fn and(left: ExpectCmd, right: ExpectCmd) -> ExpectCmd {
    ExpectCmd::And(Box::new(left), Box::new(right))
}
fn or(left: ExpectCmd, right: ExpectCmd) -> ExpectCmd {
    ExpectCmd::Or(Box::new(left), Box::new(right))
}
fn bg(cmd: ExpectCmd) -> ExpectCmd {
    ExpectCmd::Bg(Box::new(cmd))
}
fn seq(cmds: Vec<ExpectCmd>) -> ExpectCmd {
    ExpectCmd::Sequence(cmds)
}

fn verify_redirs(
    actual: &[crate::parser::ast::Redirect],
    expected: &[ExpectRedir],
    buf: &relative::Buffer,
) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "Redirections count mismatch: actual {:?} vs expected {:?}",
        actual,
        expected
    );
    for (i, r) in actual.iter().enumerate() {
        match &expected[i] {
            ExpectRedir::ToFile(src_fd, filename, append, clobber) => {
                assert_eq!(r.tag, RedirectTag::TO_FILE);
                assert_eq!(r.src_fd.raw(), *src_fd);
                assert_eq!(get_word_str(&r.filename, buf), *filename);
                assert_eq!(r.append != 0, *append);
                assert_eq!(r.clobber != 0, *clobber);
            }
            ExpectRedir::FromFile(src_fd, filename) => {
                assert_eq!(r.tag, RedirectTag::FROM_FILE);
                assert_eq!(r.src_fd.raw(), *src_fd);
                assert_eq!(get_word_str(&r.filename, buf), *filename);
            }
            ExpectRedir::DupFd(src_fd, dest_fd) => {
                assert_eq!(r.tag, RedirectTag::DUP_FD);
                assert_eq!(r.src_fd.raw(), *src_fd);
                assert_eq!(r.dest_fd.raw(), *dest_fd);
            }
            ExpectRedir::CloseFd(src_fd) => {
                assert_eq!(r.tag, RedirectTag::CLOSE_FD);
                assert_eq!(r.src_fd.raw(), *src_fd);
            }
            ExpectRedir::HereDoc(src_fd, body, expand) => {
                assert_eq!(r.tag, RedirectTag::HERE_DOC);
                assert_eq!(r.src_fd.raw(), *src_fd);
                assert_eq!(r.body.to_bstring(buf).to_string(), *body);
                assert_eq!(r.expand != 0, *expand);
            }
        }
    }
}

fn verify_cmd(cmd: &Command, expected: &ExpectCmd, buf: &relative::Buffer) {
    match expected {
        ExpectCmd::Simple(args) => {
            assert_eq!(cmd.tag, CommandTag::SIMPLE, "Expected SIMPLE, got {:?}", cmd.tag);
            let actual_args: Vec<String> =
                cmd.simple_args.as_slice(buf).iter().map(|w| get_word_str(w, buf)).collect();
            assert_eq!(&actual_args, args, "Simple args mismatch");
        }
        ExpectCmd::Pipeline(l, r) => {
            assert_eq!(cmd.tag, CommandTag::PIPELINE, "Expected PIPELINE, got {:?}", cmd.tag);
            verify_cmd(cmd.left.as_ref(buf), l, buf);
            verify_cmd(cmd.right.as_ref(buf), r, buf);
        }
        ExpectCmd::Redirect(l, redirs) => {
            assert_eq!(cmd.tag, CommandTag::REDIRECT, "Expected REDIRECT, got {:?}", cmd.tag);
            verify_cmd(cmd.left.as_ref(buf), l, buf);
            verify_redirs(cmd.redirects.as_slice(buf), redirs, buf);
        }
        ExpectCmd::Subshell(l) => {
            assert_eq!(cmd.tag, CommandTag::SUBSHELL, "Expected SUBSHELL, got {:?}", cmd.tag);
            verify_cmd(cmd.left.as_ref(buf), l, buf);
        }
        ExpectCmd::If(cond, then_b, else_b) => {
            assert_eq!(cmd.tag, CommandTag::IF, "Expected IF, got {:?}", cmd.tag);
            verify_cmd(cmd.cond.as_ref(buf), cond, buf);
            verify_cmd(cmd.then_branch.as_ref(buf), then_b, buf);
            match (
                else_b,
                if cmd.else_branch.is_null() { None } else { Some(cmd.else_branch.as_ref(buf)) },
            ) {
                (Some(eb), Some(ab)) => verify_cmd(ab, eb, buf),
                (None, None) => {}
                _ => panic!("If else branch mismatch"),
            }
        }
        ExpectCmd::While(cond, body) => {
            assert_eq!(cmd.tag, CommandTag::WHILE, "Expected WHILE, got {:?}", cmd.tag);
            verify_cmd(cmd.cond.as_ref(buf), cond, buf);
            verify_cmd(cmd.then_branch.as_ref(buf), body, buf);
        }
        ExpectCmd::Until(cond, body) => {
            assert_eq!(cmd.tag, CommandTag::UNTIL, "Expected UNTIL, got {:?}", cmd.tag);
            verify_cmd(cmd.cond.as_ref(buf), cond, buf);
            verify_cmd(cmd.then_branch.as_ref(buf), body, buf);
        }
        ExpectCmd::For(var, items, body) => {
            assert_eq!(cmd.tag, CommandTag::FOR, "Expected FOR, got {:?}", cmd.tag);
            assert_eq!(cmd.for_var.to_bstring(buf).to_string(), *var);
            let actual_items: Vec<String> =
                cmd.for_items.as_slice(buf).iter().map(|w| get_word_str(w, buf)).collect();
            assert_eq!(&actual_items, items);
            verify_cmd(cmd.then_branch.as_ref(buf), body, buf);
        }
        ExpectCmd::Case(word, items) => {
            assert_eq!(cmd.tag, CommandTag::CASE, "Expected CASE, got {:?}", cmd.tag);
            assert_eq!(get_word_str(&cmd.case_word, buf), *word);
            let actual_items = cmd.case_items.as_slice(buf);
            assert_eq!(actual_items.len(), items.len());
            for (i, item) in actual_items.iter().enumerate() {
                let (expected_pats, expected_body) = &items[i];
                let actual_pats: Vec<String> =
                    item.patterns.as_slice(buf).iter().map(|w| get_word_str(w, buf)).collect();
                assert_eq!(&actual_pats, expected_pats);
                verify_cmd(item.body.as_ref(buf), expected_body, buf);
            }
        }
        ExpectCmd::FunctionDef(name, body) => {
            assert_eq!(
                cmd.tag,
                CommandTag::FUNCTION_DEF,
                "Expected FUNCTION_DEF, got {:?}",
                cmd.tag
            );
            assert_eq!(cmd.name.to_bstring(buf).to_string(), *name);
            verify_cmd(cmd.then_branch.as_ref(buf), body, buf);
        }
        ExpectCmd::And(l, r) => {
            assert_eq!(cmd.tag, CommandTag::LOGICAL_AND, "Expected LOGICAL_AND, got {:?}", cmd.tag);
            verify_cmd(cmd.left.as_ref(buf), l, buf);
            verify_cmd(cmd.right.as_ref(buf), r, buf);
        }
        ExpectCmd::Or(l, r) => {
            assert_eq!(cmd.tag, CommandTag::LOGICAL_OR, "Expected LOGICAL_OR, got {:?}", cmd.tag);
            verify_cmd(cmd.left.as_ref(buf), l, buf);
            verify_cmd(cmd.right.as_ref(buf), r, buf);
        }
        ExpectCmd::Bg(l) => {
            assert_eq!(cmd.tag, CommandTag::BACKGROUND, "Expected BACKGROUND, got {:?}", cmd.tag);
            verify_cmd(cmd.left.as_ref(buf), l, buf);
        }
        ExpectCmd::Sequence(cmds) => {
            assert_eq!(cmd.tag, CommandTag::SEQUENCE, "Expected SEQUENCE, got {:?}", cmd.tag);
            let seq = cmd.sequence.as_slice(buf);
            assert_eq!(seq.len(), cmds.len(), "Sequence length mismatch");
            for (i, ptr) in seq.iter().enumerate() {
                verify_cmd(ptr.as_ref(buf), &cmds[i], buf);
            }
        }
    }
}

fn check_ast(input: &[u8], expected: &[ExpectCmd]) {
    let mut builder = ASTBuilder::new();
    let tokens = tokenize(input).unwrap();
    let cmds = parse_script(&mut builder, &tokens).unwrap();
    assert_eq!(
        cmds.len(),
        expected.len(),
        "Command count mismatch for {:?}",
        bstr::BStr::new(input)
    );
    for (i, cmd_ptr) in cmds.iter().enumerate() {
        let cmd = builder.get_ref(*cmd_ptr);
        verify_cmd(cmd, &expected[i], &builder);
    }
}

#[test]
fn test_parser_simple_and_pipeline() {
    check_ast(
        b"ls -l | grep foo > out",
        &[pipe(simple(&["ls", "-l"]), redir(simple(&["grep", "foo"]), vec![r_out("out")]))],
    );
}

#[test]
fn test_parser_sequences() {
    check_ast(
        b"cmd1 arg1; cmd2\ncmd3",
        &[simple(&["cmd1", "arg1"]), simple(&["cmd2"]), simple(&["cmd3"])],
    );
}

#[test]
fn test_parser_logical() {
    check_ast(b"a && b || c", &[or(and(simple(&["a"]), simple(&["b"])), simple(&["c"]))]);
}

#[test]
fn test_parser_subshell_and_brace() {
    check_ast(b"(cmd1; cmd2)", &[subshell(seq(vec![simple(&["cmd1"]), simple(&["cmd2"])]))]);
    check_ast(b"{ cmd1; cmd2; }", &[seq(vec![simple(&["cmd1"]), simple(&["cmd2"])])]);
}

#[test]
fn test_parser_if() {
    check_ast(
        b"if cond_cmd; then then_cmd; else else_cmd; fi",
        &[if_cmd(simple(&["cond_cmd"]), simple(&["then_cmd"]), Some(simple(&["else_cmd"])))],
    );
}

#[test]
fn test_parser_loops() {
    check_ast(b"while cond; do body; done", &[while_cmd(simple(&["cond"]), simple(&["body"]))]);
    check_ast(
        b"for x in a b c; do body; done",
        &[for_cmd("x", &["a", "b", "c"], simple(&["body"]))],
    );
}

#[test]
fn test_parser_function() {
    check_ast(b"my_func() { body_cmd; }", &[func_def("my_func", simple(&["body_cmd"]))]);
}

#[test]
fn test_parse_error_display() {
    assert_eq!(
        ParseError::Incomplete(IncompleteReason::Quote).to_string(),
        "Incomplete input (Quote)"
    );
    assert_eq!(ParseError::Syntax("unexpected".into()).to_string(), "unexpected");
}

#[test]
fn test_ast_tags_display() {
    assert_eq!(WordPartTag(0).to_string(), "0");
    assert_eq!(RedirectTag(0).to_string(), "0");
    assert_eq!(CommandTag(0).to_string(), "0");
}

#[test]
fn test_ast_builder_empty_slices() {
    let mut builder = ASTBuilder::new();
    assert_eq!(builder.add_argument_refs(&[]), relative::Slice::empty());
    assert_eq!(builder.add_case_items_from_refs(&[]), relative::Slice::empty());
    assert_eq!(builder.add_redirects_from_templates(&[]), relative::Slice::empty());
    let _ = builder.add_empty_simple_command();
}

#[test]
fn test_ast_serialize_all_types() {
    let expected = &[
        or(
            and(pipe(simple(&["echo", "a"]), simple(&["echo", "b"])), simple(&["echo", "c"])),
            simple(&["echo", "d"]),
        ),
        redir(simple(&["echo", "e"]), vec![r_out("file")]),
        bg(subshell(simple(&["echo", "f"]))),
        if_cmd(simple(&["true"]), simple(&["echo", "g"]), Some(simple(&["echo", "h"]))),
        while_cmd(simple(&["true"]), simple(&["echo", "i"])),
        until_cmd(simple(&["true"]), simple(&["echo", "j"])),
        for_cmd("x", &["1", "2"], simple(&["echo", "$x"])),
        case_cmd("foo", vec![(vec!["*"], simple(&["echo", "k"]))]),
        func_def("fn", simple(&["echo", "l"])),
        simple(&["echo", "m"]),
        simple(&["echo", "n"]),
    ];
    let input = b"echo a | echo b && echo c || echo d; echo e > file; (echo f) & if true; then echo g; else echo h; fi; while true; do echo i; done; until true; do echo j; done; for x in 1 2; do echo $x; done; case foo in * ) echo k ;; esac; fn() { echo l; }; echo m; echo n";
    check_ast(input, expected);

    let mut b1 = ASTBuilder::new();
    let cmds = parse(input, &mut b1);
    for (i, ptr) in cmds.into_iter().enumerate() {
        let cmd_ref = b1.get_ref(ptr);
        let bytes = cmd_ref.serialize(&b1);
        let b2 = relative::Buffer::from_bytes(&bytes);
        let header = b2.get_ref(relative::Ptr::<FlatASTHeader>::new(0));
        verify_cmd(b2.get_ref(header.root_cmd()), &expected[i], b2);
    }
}

#[test]
fn test_ast_resolved_word_quoted_arithmetic() {
    let mut builder = ASTBuilder::new();
    let _ = builder.add_resolved_word(&[ResolvedWordPart::QuotedArithmetic("1+2".into())]);
}

#[test]
fn test_parser_subshell_command() {
    let mut builder = ASTBuilder::new();
    let _ = parse_subshell_command(&mut builder, b"").unwrap();
    let off2 = parse_subshell_command(&mut builder, b"echo hi; echo bye").unwrap();
    verify_cmd(
        builder.get_ref(off2),
        &seq(vec![simple(&["echo", "hi"]), simple(&["echo", "bye"])]),
        &builder,
    );
}

#[test]
fn test_parser_various_redirects() {
    check_ast(
        b"echo hi >> file1 >| file2 <&0 >&1",
        &[redir(
            simple(&["echo", "hi"]),
            vec![r_app("file1"), r_clob("file2"), r_dup_in(0, 0), r_dup_in(1, 1)],
        )],
    );
}

#[test]
fn test_parser_errors_functions_and_loops() {
    let mut builder = ASTBuilder::new();
    let check_err = |input: &[u8], mut builder: &mut ASTBuilder| -> Result<(), ParseError> {
        let tokens = tokenize(input).expect("tokenize failed");
        match parse_script(&mut builder, &tokens) {
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    };

    assert!(check_err(b"$var() { echo hi; }", &mut builder).is_err());
    assert!(check_err(b"\"func\"() { echo hi; }", &mut builder).is_err());
    assert!(check_err(b"a$b() { echo hi; }", &mut builder).is_err());

    assert!(check_err(b"while true", &mut builder).is_err());
    assert!(check_err(b"while true; foo echo hi; done", &mut builder).is_err());
    assert!(check_err(b"while true; do echo hi", &mut builder).is_err());
    assert!(check_err(b"while true; do echo hi; foo", &mut builder).is_err());
}

#[test]
fn test_parser_errors_bg_and_logical() {
    let mut builder = ASTBuilder::new();
    let check_err = |input: &[u8], mut builder: &mut ASTBuilder| -> Result<(), ParseError> {
        let tokens = tokenize(input).expect("tokenize failed");
        match parse_script(&mut builder, &tokens) {
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    };

    check_ast(b"sleep 1 &\n echo hi", &[bg(simple(&["sleep", "1"])), simple(&["echo", "hi"])]);

    assert!(check_err(b"echo hi ( echo bye )", &mut builder).is_err());
    assert!(check_err(b"echo hi &&", &mut builder).is_err());
    assert!(check_err(b"echo hi ||", &mut builder).is_err());
    assert!(check_err(b"echo hi |", &mut builder).is_err());
}

#[test]
fn test_parser_errors_redirects_and_if() {
    let mut builder = ASTBuilder::new();
    let check_err = |input: &[u8], mut builder: &mut ASTBuilder| -> Result<(), ParseError> {
        let tokens = tokenize(input).expect("tokenize failed");
        match parse_script(&mut builder, &tokens) {
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    };

    assert!(check_err(b"echo >", &mut builder).is_err());
    assert!(check_err(b"echo > ;", &mut builder).is_err());
    assert!(check_err(b"echo >& abc", &mut builder).is_err());
    assert!(check_err(b"echo <& abc", &mut builder).is_err());

    assert!(check_err(b"func() { echo hi", &mut builder).is_err());
    assert!(check_err(b"func() { echo hi ; )", &mut builder).is_err());

    assert!(check_err(b"if true; then echo hi; elif false", &mut builder).is_err());
    assert!(
        check_err(b"if true; then echo hi; elif false; foo echo bye; fi", &mut builder).is_err()
    );
    check_ast(
        b"if true; then echo hi; elif false; then echo bye; fi",
        &[if_cmd(
            simple(&["true"]),
            simple(&["echo", "hi"]),
            Some(if_cmd(simple(&["false"]), simple(&["echo", "bye"]), None)),
        )],
    );
}

#[test]
fn test_parser_errors_subshell_and_case() {
    let mut builder = ASTBuilder::new();
    let check_err = |input: &[u8], mut builder: &mut ASTBuilder| -> Result<(), ParseError> {
        let tokens = tokenize(input).expect("tokenize failed");
        match parse_script(&mut builder, &tokens) {
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    };

    assert!(check_err(b"( echo hi", &mut builder).is_err());
    assert!(check_err(b"( echo hi ; }", &mut builder).is_err());
    assert!(check_err(b"{ echo hi", &mut builder).is_err());
    assert!(check_err(b"{ echo hi ; )", &mut builder).is_err());

    assert!(check_err(b"case", &mut builder).is_err());
    assert!(check_err(b"case ;", &mut builder).is_err());
    assert!(check_err(b"case foo", &mut builder).is_err());
    assert!(check_err(b"case foo ;", &mut builder).is_err());
    check_ast(b"case foo in \n esac", &[case_cmd("foo", vec![])]);
    check_ast(
        b"case foo in ( bar ) echo hi ;; esac",
        &[case_cmd("foo", vec![(vec!["bar"], simple(&["echo", "hi"]))])],
    );
    assert!(check_err(b"case foo in |", &mut builder).is_err());
    assert!(check_err(b"case foo in ;", &mut builder).is_err());
    check_ast(
        b"case foo in bar) \n echo hi ;; esac",
        &[case_cmd("foo", vec![(vec!["bar"], simple(&["echo", "hi"]))])],
    );
    assert!(check_err(b"case foo in a) echo hi b) echo bye ;; esac", &mut builder).is_err());
    check_ast(
        b"case foo in a) echo hi ;; \n b) echo bye ;; esac",
        &[case_cmd(
            "foo",
            vec![(vec!["a"], simple(&["echo", "hi"])), (vec!["b"], simple(&["echo", "bye"]))],
        )],
    );
    assert!(check_err(b"case foo in a) echo hi ;;", &mut builder).is_err());
    assert!(check_err(b"case foo in a) echo hi ;; fi", &mut builder).is_err());
}

#[test]
fn test_parser_errors_for_and_primary() {
    let mut builder = ASTBuilder::new();
    let check_err = |input: &[u8], mut builder: &mut ASTBuilder| -> Result<(), ParseError> {
        let tokens = tokenize(input).expect("tokenize failed");
        match parse_script(&mut builder, &tokens) {
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    };

    assert!(check_err(b"for \"var\" in a; do echo hi; done", &mut builder).is_err());
    assert!(check_err(b"for", &mut builder).is_err());
    assert!(check_err(b"for x in a ( b; do echo hi; done", &mut builder).is_err());
    assert!(check_err(b"for x", &mut builder).is_err());
    assert!(check_err(b"for x ( ; do echo hi; done", &mut builder).is_err());
    assert!(check_err(b"for x in a", &mut builder).is_err());
    assert!(check_err(b"for x in a ; fi", &mut builder).is_err());
    assert!(check_err(b"for x in a ; do echo hi", &mut builder).is_err());
    assert!(check_err(b"for x in a ; do echo hi ; fi", &mut builder).is_err());
    assert!(check_err(b"echo hi | )", &mut builder).is_err());
}

#[test]
fn test_parser_exhaustive_coverage() {
    let mut builder = ASTBuilder::new();
    let check_err = |input: &[u8], mut builder: &mut ASTBuilder| -> Result<(), ParseError> {
        let tokens = tokenize(input).expect("tokenize failed");
        match parse_script(&mut builder, &tokens) {
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        }
    };

    // 1. resolve_word_parts
    check_ast(
        b"echo 'literal' $var \"quoted $var $(echo hi)\" $(cmd) $((1+2)) \"$((3+4))\"",
        &[simple(&[
            "echo",
            "'literal'",
            "$var",
            "'quoted '\"$var\"' '\"$(echo hi)\"",
            "$(cmd)",
            "$((1+2))",
            "\"$((3+4))\"",
        ])],
    );

    // 2. Redirect duplicate targets with quotes and invalid fds, plus various redirects
    assert!(check_err(b"echo hi 2>&\"1\"", &mut builder).is_err());
    assert!(check_err(b"echo hi 2>&'1'", &mut builder).is_err());
    assert!(check_err(b"echo hi 2>&a$b", &mut builder).is_err());
    assert!(check_err(b"echo hi 2>&abc", &mut builder).is_err());
    assert!(check_err(b"echo hi <&abc", &mut builder).is_err());
    check_ast(
        b"echo hi <file 0<file 2>file >&- 2>&- <&- 0<&-",
        &[redir(
            simple(&["echo", "hi"]),
            vec![
                r_in("file"),
                r_in_fd(0, "file"),
                r_out_fd(2, "file"),
                r_close(1),
                r_close(2),
                r_close(0),
                r_close(0),
            ],
        )],
    );

    // 3. Heredoc attached to command
    check_ast(
        b"cat <<EOF\nbody\nEOF\n",
        &[redir(simple(&["cat"]), vec![r_heredoc("body\n", true)])],
    );
    check_ast(
        b"cat <<'EOF'\nbody\nEOF\n",
        &[redir(simple(&["cat"]), vec![r_heredoc("body\n", false)])],
    );

    // 4. is_block_terminator with non-literal keyword
    assert!(check_err(b"if true; then echo hi; \"fi\"", &mut builder).is_err());

    // 5. Empty command lists
    assert!(check_err(b"if true; then ; fi", &mut builder).is_err());
    assert!(check_err(b"while true; do ; done", &mut builder).is_err());
    assert!(check_err(b"{ ; }", &mut builder).is_err());
    assert!(check_err(b"( ; )", &mut builder).is_err());

    // 6. Leading semicolons/newlines in command list
    check_ast(b";\n echo hi", &[simple(&["echo", "hi"])]);

    // 7. Non-simple commands with redirects attached
    check_ast(
        b"(echo hi) > file",
        &[redir(subshell(simple(&["echo", "hi"])), vec![r_out("file")])],
    );
    check_ast(b"{ echo hi; } > file", &[redir(simple(&["echo", "hi"]), vec![r_out("file")])]);
    check_ast(
        b"if true; then echo hi; fi > file",
        &[redir(if_cmd(simple(&["true"]), simple(&["echo", "hi"]), None), vec![r_out("file")])],
    );
    check_ast(
        b"while true; do echo hi; done > file",
        &[redir(while_cmd(simple(&["true"]), simple(&["echo", "hi"])), vec![r_out("file")])],
    );
    check_ast(
        b"for i in a; do echo $i; done > file",
        &[redir(for_cmd("i", &["a"], simple(&["echo", "$i"])), vec![r_out("file")])],
    );

    // 8. Function body without {
    check_ast(b"f() echo hi", &[func_def("f", simple(&["echo", "hi"]))]);
    check_ast(b"f() (echo hi)", &[func_def("f", subshell(simple(&["echo", "hi"])))]);

    // 9. parse_if_remainder hitting EOF after then
    assert!(check_err(b"if true; then echo hi; ", &mut builder).is_err());

    // 10. Case patterns and errors
    check_ast(
        b"case x in ( a | b ) echo hi ;; esac",
        &[case_cmd("x", vec![(vec!["a", "b"], simple(&["echo", "hi"]))])],
    );
    assert!(check_err(b"case x in ( ", &mut builder).is_err());
    assert!(check_err(b"case x in a b echo hi ;; esac", &mut builder).is_err());
    assert!(check_err(b"case x in a) echo hi echo bye esac", &mut builder).is_err());
    assert!(check_err(b"case x in a) echo hi ;; fi", &mut builder).is_err());

    // 11. For loop variants
    check_ast(b"for i; do echo $i; done", &[for_cmd("i", &["\"$@\""], simple(&["echo", "$i"]))]);
    check_ast(b"for i do echo hi; done", &[for_cmd("i", &["\"$@\""], simple(&["echo", "hi"]))]);

    // 12. Remaining coverage edges
    assert!(check_err(b"f()", &mut builder).is_err());
    assert!(check_err(b"func( arg )", &mut builder).is_err());
    check_ast(
        b"case x in a) echo hi ; esac",
        &[case_cmd("x", vec![(vec!["a"], simple(&["echo", "hi"]))])],
    );
    check_ast(b"> file echo hi", &[redir(simple(&["echo", "hi"]), vec![r_out("file")])]);
    check_ast(b"< file cat", &[redir(simple(&["cat"]), vec![r_in("file")])]);
}
