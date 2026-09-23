// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_SYNTAX_ERROR, ShellState};
use bstr::{BStr, BString, ByteSlice};
use std::io::{Read, Write};

use std::os::fuchsia::fs::MetadataExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpType {
    Unop,
    Binop,
    Bunop,
    Bbinop,
    Paren,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenNum {
    Filrd,
    Filwr,
    Filex,
    Filexist,
    Filreg,
    Fildir,
    Filcdev,
    Filbdev,
    Filfifo,
    Filsuid,
    Filsgid,
    Filstck,
    Filgz,
    Filtt,
    Strez,
    Strnz,
    Filsym,
    Filuid,
    Filgid,
    Filsock,
    Streq,
    Strne,
    Strlt,
    Strgt,
    Inteq,
    Intne,
    Intge,
    Intgt,
    Intle,
    Intlt,
    Filnt,
    Filot,
    Fileq,
    Unot,
    Band,
    Bor,
    Lparen,
    Rparen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpDef {
    text: &'static [u8],
    num: TokenNum,
    op_type: OpType,
}

static OPS: &[OpDef] = &[
    OpDef { text: b"-r", num: TokenNum::Filrd, op_type: OpType::Unop },
    OpDef { text: b"-w", num: TokenNum::Filwr, op_type: OpType::Unop },
    OpDef { text: b"-x", num: TokenNum::Filex, op_type: OpType::Unop },
    OpDef { text: b"-e", num: TokenNum::Filexist, op_type: OpType::Unop },
    OpDef { text: b"-f", num: TokenNum::Filreg, op_type: OpType::Unop },
    OpDef { text: b"-d", num: TokenNum::Fildir, op_type: OpType::Unop },
    OpDef { text: b"-c", num: TokenNum::Filcdev, op_type: OpType::Unop },
    OpDef { text: b"-b", num: TokenNum::Filbdev, op_type: OpType::Unop },
    OpDef { text: b"-p", num: TokenNum::Filfifo, op_type: OpType::Unop },
    OpDef { text: b"-u", num: TokenNum::Filsuid, op_type: OpType::Unop },
    OpDef { text: b"-g", num: TokenNum::Filsgid, op_type: OpType::Unop },
    OpDef { text: b"-k", num: TokenNum::Filstck, op_type: OpType::Unop },
    OpDef { text: b"-s", num: TokenNum::Filgz, op_type: OpType::Unop },
    OpDef { text: b"-t", num: TokenNum::Filtt, op_type: OpType::Unop },
    OpDef { text: b"-z", num: TokenNum::Strez, op_type: OpType::Unop },
    OpDef { text: b"-n", num: TokenNum::Strnz, op_type: OpType::Unop },
    OpDef { text: b"-h", num: TokenNum::Filsym, op_type: OpType::Unop },
    OpDef { text: b"-O", num: TokenNum::Filuid, op_type: OpType::Unop },
    OpDef { text: b"-G", num: TokenNum::Filgid, op_type: OpType::Unop },
    OpDef { text: b"-L", num: TokenNum::Filsym, op_type: OpType::Unop },
    OpDef { text: b"-S", num: TokenNum::Filsock, op_type: OpType::Unop },
    OpDef { text: b"=", num: TokenNum::Streq, op_type: OpType::Binop },
    OpDef { text: b"==", num: TokenNum::Streq, op_type: OpType::Binop },
    OpDef { text: b"!=", num: TokenNum::Strne, op_type: OpType::Binop },
    OpDef { text: b"<", num: TokenNum::Strlt, op_type: OpType::Binop },
    OpDef { text: b">", num: TokenNum::Strgt, op_type: OpType::Binop },
    OpDef { text: b"-eq", num: TokenNum::Inteq, op_type: OpType::Binop },
    OpDef { text: b"-ne", num: TokenNum::Intne, op_type: OpType::Binop },
    OpDef { text: b"-ge", num: TokenNum::Intge, op_type: OpType::Binop },
    OpDef { text: b"-gt", num: TokenNum::Intgt, op_type: OpType::Binop },
    OpDef { text: b"-le", num: TokenNum::Intle, op_type: OpType::Binop },
    OpDef { text: b"-lt", num: TokenNum::Intlt, op_type: OpType::Binop },
    OpDef { text: b"-nt", num: TokenNum::Filnt, op_type: OpType::Binop },
    OpDef { text: b"-ot", num: TokenNum::Filot, op_type: OpType::Binop },
    OpDef { text: b"-ef", num: TokenNum::Fileq, op_type: OpType::Binop },
    OpDef { text: b"!", num: TokenNum::Unot, op_type: OpType::Bunop },
    OpDef { text: b"-a", num: TokenNum::Band, op_type: OpType::Bbinop },
    OpDef { text: b"-o", num: TokenNum::Bor, op_type: OpType::Bbinop },
    OpDef { text: b"(", num: TokenNum::Lparen, op_type: OpType::Paren },
    OpDef { text: b")", num: TokenNum::Rparen, op_type: OpType::Paren },
];

fn getop(s: &BStr) -> Option<&'static OpDef> {
    OPS.iter().find(|op| op.text == s.as_bytes())
}

fn is_operand(args: &[BString], idx: usize) -> bool {
    if idx + 1 >= args.len() {
        return true;
    }
    if idx + 2 >= args.len() {
        return false;
    }
    if let Some(op) = getop(args[idx + 1].as_bstr()) { op.op_type == OpType::Binop } else { false }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexToken {
    Eoi,
    Op(&'static OpDef),
    Operand,
}

fn t_lex(args: &[BString], idx: usize) -> LexToken {
    if idx >= args.len() {
        return LexToken::Eoi;
    }
    let s = args[idx].as_bstr();
    if let Some(op) = getop(s) {
        if op.op_type == OpType::Unop && is_operand(args, idx) {
            return LexToken::Operand;
        }
        if op.num == TokenNum::Lparen && idx + 1 >= args.len() {
            return LexToken::Operand;
        }
        return LexToken::Op(op);
    }
    LexToken::Operand
}

fn parse_test_int(cmd_name: &str, s: &BStr) -> Result<i64, String> {
    let s_str = std::str::from_utf8(s.as_bytes())
        .map_err(|_| format!("{}: {}: bad number", cmd_name, s))?;
    let trimmed = s_str.trim_start();
    if trimmed.is_empty() {
        return Err(format!("{}: {}: bad number", cmd_name, s));
    }
    let rest =
        if trimmed.starts_with('+') || trimmed.starts_with('-') { &trimmed[1..] } else { trimmed };
    if rest.is_empty() {
        return Err(format!("{}: {}: bad number", cmd_name, s));
    }
    let digit_len = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_len == 0 {
        return Err(format!("{}: {}: bad number", cmd_name, s));
    }
    let after_digits = &rest[digit_len..];
    if !after_digits.trim().is_empty() {
        return Err(format!("{}: {}: bad number", cmd_name, s));
    }
    let num_str = &trimmed[..trimmed.len() - after_digits.len()];
    num_str.parse::<i64>().map_err(|_| format!("{}: {}: bad number", cmd_name, s))
}

fn test_file_access(path_bytes: &BStr, mode: libc::c_int) -> bool {
    let path = match path_bytes.to_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    use std::os::unix::ffi::OsStrExt as _;
    let c_path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    unsafe { libc::faccessat(libc::AT_FDCWD, c_path.as_ptr(), mode, libc::AT_EACCESS) == 0 }
}

fn test_file_stat(path_bytes: &BStr, mode: TokenNum) -> bool {
    let path = match path_bytes.to_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    if mode == TokenNum::Filsym {
        match std::fs::symlink_metadata(path) {
            Ok(m) => m.file_type().is_symlink(),
            Err(_) => false,
        }
    } else {
        let m = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        let st_mode = m.st_mode() as u32;
        match mode {
            TokenNum::Filexist => true,
            TokenNum::Filreg => m.is_file(),
            TokenNum::Fildir => m.is_dir(),
            TokenNum::Filcdev => (st_mode & 0o170000) == 0o020000,
            TokenNum::Filbdev => (st_mode & 0o170000) == 0o060000,
            TokenNum::Filfifo => (st_mode & 0o170000) == 0o010000,
            TokenNum::Filsock => (st_mode & 0o170000) == 0o140000,
            TokenNum::Filsuid => (st_mode & 0o004000) != 0,
            TokenNum::Filsgid => (st_mode & 0o002000) != 0,
            TokenNum::Filstck => (st_mode & 0o001000) != 0,
            TokenNum::Filgz => m.len() > 0,
            TokenNum::Filuid => m.st_uid() as u32 == unsafe { libc::geteuid() as u32 },
            TokenNum::Filgid => m.st_gid() as u32 == unsafe { libc::getegid() as u32 },
            _ => false,
        }
    }
}

fn newerf(f1: &BStr, f2: &BStr) -> bool {
    let p1 = match f1.to_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let p2 = match f2.to_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let m1 = match std::fs::metadata(p1) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let m2 = match std::fs::metadata(p2) {
        Ok(m) => m,
        Err(_) => return false,
    };
    match (m1.modified(), m2.modified()) {
        (Ok(t1), Ok(t2)) => t1 > t2,
        _ => false,
    }
}

fn olderf(f1: &BStr, f2: &BStr) -> bool {
    let p1 = match f1.to_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let p2 = match f2.to_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let m1 = match std::fs::metadata(p1) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let m2 = match std::fs::metadata(p2) {
        Ok(m) => m,
        Err(_) => return false,
    };
    match (m1.modified(), m2.modified()) {
        (Ok(t1), Ok(t2)) => t1 < t2,
        _ => false,
    }
}

fn equalf(f1: &BStr, f2: &BStr) -> bool {
    let p1 = match f1.to_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let p2 = match f2.to_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let m1 = match std::fs::metadata(p1) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let m2 = match std::fs::metadata(p2) {
        Ok(m) => m,
        Err(_) => return false,
    };
    m1.st_dev() == m2.st_dev() && m1.st_ino() == m2.st_ino()
}

fn eval_unary_op(cmd_name: &str, num: TokenNum, arg: &BStr) -> Result<bool, String> {
    match num {
        TokenNum::Strez => Ok(arg.is_empty()),
        TokenNum::Strnz => Ok(!arg.is_empty()),
        TokenNum::Filtt => {
            let fd = parse_test_int(cmd_name, arg)?;
            if !(0..=i32::MAX as i64).contains(&fd) {
                Ok(false)
            } else {
                Ok(unsafe { libc::isatty(fd as i32) == 1 })
            }
        }
        TokenNum::Filrd => Ok(test_file_access(arg, libc::R_OK)),
        TokenNum::Filwr => Ok(test_file_access(arg, libc::W_OK)),
        TokenNum::Filex => Ok(test_file_access(arg, libc::X_OK)),
        TokenNum::Filexist => Ok(test_file_stat(arg, TokenNum::Filexist)),
        TokenNum::Filreg
        | TokenNum::Fildir
        | TokenNum::Filcdev
        | TokenNum::Filbdev
        | TokenNum::Filfifo
        | TokenNum::Filsock
        | TokenNum::Filsym
        | TokenNum::Filsuid
        | TokenNum::Filsgid
        | TokenNum::Filstck
        | TokenNum::Filgz
        | TokenNum::Filuid
        | TokenNum::Filgid => Ok(test_file_stat(arg, num)),
        _ => Err(format!("{}: unknown unary operator", cmd_name)),
    }
}

fn eval_binary_op(
    cmd_name: &str,
    opnd1: &BStr,
    num: TokenNum,
    opnd2: &BStr,
) -> Result<bool, String> {
    match num {
        TokenNum::Streq => Ok(opnd1 == opnd2),
        TokenNum::Strne => Ok(opnd1 != opnd2),
        TokenNum::Strlt => Ok(opnd1 < opnd2),
        TokenNum::Strgt => Ok(opnd1 > opnd2),
        TokenNum::Inteq => {
            let n1 = parse_test_int(cmd_name, opnd1)?;
            let n2 = parse_test_int(cmd_name, opnd2)?;
            Ok(n1 == n2)
        }
        TokenNum::Intne => {
            let n1 = parse_test_int(cmd_name, opnd1)?;
            let n2 = parse_test_int(cmd_name, opnd2)?;
            Ok(n1 != n2)
        }
        TokenNum::Intge => {
            let n1 = parse_test_int(cmd_name, opnd1)?;
            let n2 = parse_test_int(cmd_name, opnd2)?;
            Ok(n1 >= n2)
        }
        TokenNum::Intgt => {
            let n1 = parse_test_int(cmd_name, opnd1)?;
            let n2 = parse_test_int(cmd_name, opnd2)?;
            Ok(n1 > n2)
        }
        TokenNum::Intle => {
            let n1 = parse_test_int(cmd_name, opnd1)?;
            let n2 = parse_test_int(cmd_name, opnd2)?;
            Ok(n1 <= n2)
        }
        TokenNum::Intlt => {
            let n1 = parse_test_int(cmd_name, opnd1)?;
            let n2 = parse_test_int(cmd_name, opnd2)?;
            Ok(n1 < n2)
        }
        TokenNum::Filnt => Ok(newerf(opnd1, opnd2)),
        TokenNum::Filot => Ok(olderf(opnd1, opnd2)),
        TokenNum::Fileq => Ok(equalf(opnd1, opnd2)),
        _ => Err(format!("{}: unknown binary operator", cmd_name)),
    }
}

struct TestParser<'a> {
    cmd_name: &'a str,
    args: &'a [BString],
    pos: usize,
}

impl<'a> TestParser<'a> {
    fn new(cmd_name: &'a str, args: &'a [BString]) -> Self {
        Self { cmd_name, args, pos: 0 }
    }

    fn curr_lex(&self) -> LexToken {
        t_lex(self.args, self.pos)
    }

    fn lex_at(&self, idx: usize) -> LexToken {
        t_lex(self.args, idx)
    }

    fn parse_oexpr(&mut self, tok: LexToken) -> Result<bool, String> {
        let mut res = self.parse_aexpr(tok)?;
        loop {
            let next_tok = self.lex_at(self.pos + 1);
            if matches!(next_tok, LexToken::Op(op) if op.num == TokenNum::Bor) {
                self.pos += 2;
                let n = self.curr_lex();
                let right = self.parse_aexpr(n)?;
                res = res || right;
            } else {
                break;
            }
        }
        Ok(res)
    }

    fn parse_aexpr(&mut self, tok: LexToken) -> Result<bool, String> {
        let mut res = true;
        let mut current_tok = tok;
        loop {
            let nexpr_res = self.parse_nexpr(current_tok)?;
            if !nexpr_res {
                res = false;
            }
            let next_tok = self.lex_at(self.pos + 1);
            if matches!(next_tok, LexToken::Op(op) if op.num == TokenNum::Band) {
                self.pos += 2;
                current_tok = self.curr_lex();
            } else {
                break;
            }
        }
        Ok(res)
    }

    fn parse_nexpr(&mut self, tok: LexToken) -> Result<bool, String> {
        if matches!(tok, LexToken::Op(op) if op.num == TokenNum::Unot) {
            let next_tok = self.lex_at(self.pos + 1);
            if next_tok != LexToken::Eoi {
                self.pos += 1;
            }
            let n = self.curr_lex();
            let sub = self.parse_nexpr(n)?;
            Ok(!sub)
        } else {
            self.parse_primary(tok)
        }
    }

    fn parse_primary(&mut self, tok: LexToken) -> Result<bool, String> {
        match tok {
            LexToken::Eoi => Ok(false),
            LexToken::Op(op) if op.num == TokenNum::Lparen => {
                self.pos += 1;
                let nn = self.curr_lex();
                if matches!(nn, LexToken::Op(op2) if op2.num == TokenNum::Rparen) {
                    return Ok(false);
                }
                let res = self.parse_oexpr(nn)?;
                self.pos += 1;
                let closing = self.curr_lex();
                if !matches!(closing, LexToken::Op(op2) if op2.num == TokenNum::Rparen) {
                    return Err(format!("{}: closing paren expected", self.cmd_name));
                }
                Ok(res)
            }
            LexToken::Op(op) if op.op_type == OpType::Unop => {
                self.pos += 1;
                if self.pos >= self.args.len() {
                    return Err(format!(
                        "{}: {}: argument expected",
                        self.cmd_name,
                        std::str::from_utf8(op.text).unwrap_or("")
                    ));
                }
                let arg = self.args[self.pos].as_bstr();
                eval_unary_op(self.cmd_name, op.num, arg)
            }
            _ => {
                let next_tok = self.lex_at(self.pos + 1);
                if matches!(next_tok, LexToken::Op(op) if op.op_type == OpType::Binop) {
                    self.parse_binop(next_tok)
                } else {
                    if self.pos < self.args.len() {
                        Ok(!self.args[self.pos].is_empty())
                    } else {
                        Ok(false)
                    }
                }
            }
        }
    }

    fn parse_binop(&mut self, binop_tok: LexToken) -> Result<bool, String> {
        let opnd1 = self.args[self.pos].as_bstr();
        self.pos += 1;
        let op_def = match binop_tok {
            LexToken::Op(op) => op,
            _ => unreachable!(),
        };
        self.pos += 1;
        if self.pos >= self.args.len() {
            return Err(format!(
                "{}: {}: argument expected",
                self.cmd_name,
                std::str::from_utf8(op_def.text).unwrap_or("")
            ));
        }
        let opnd2 = self.args[self.pos].as_bstr();
        eval_binary_op(self.cmd_name, opnd1, op_def.num, opnd2)
    }
}

fn eval_test_cmd(cmd_name: &str, args: &[BString], stderr: &mut dyn Write) -> i32 {
    let mut argv = args;
    let mut res_invert = false;

    loop {
        match argv.len() {
            3 => {
                let second_lex = t_lex(argv, 1);
                if matches!(second_lex, LexToken::Op(op) if op.op_type == OpType::Binop) {
                    let binop_def = match second_lex {
                        LexToken::Op(op) => op,
                        _ => unreachable!(),
                    };
                    let eval_res = eval_binary_op(
                        cmd_name,
                        argv[0].as_bstr(),
                        binop_def.num,
                        argv[2].as_bstr(),
                    );
                    return match eval_res {
                        Ok(b) => {
                            let final_b = if res_invert { !b } else { b };
                            if final_b { EXIT_SUCCESS } else { EXIT_FAILURE }
                        }
                        Err(msg) => {
                            let _ = writeln!(stderr, "{}", msg);
                            EXIT_SYNTAX_ERROR
                        }
                    };
                }
                if argv[0] == "(" && argv[2] == ")" {
                    argv = &argv[1..2];
                    continue;
                } else if argv[0] == "!" {
                    res_invert = !res_invert;
                    argv = &argv[1..];
                    continue;
                }
            }
            4 => {
                if argv[0] == "(" && argv[3] == ")" {
                    argv = &argv[1..3];
                    continue;
                } else if argv[0] == "!" {
                    res_invert = !res_invert;
                    argv = &argv[1..];
                    continue;
                }
            }
            _ => {}
        }
        break;
    }

    if argv.is_empty() {
        return if res_invert { EXIT_SUCCESS } else { EXIT_FAILURE };
    }

    let n = t_lex(argv, 0);
    let mut parser = TestParser::new(cmd_name, argv);
    match parser.parse_oexpr(n) {
        Ok(res) => {
            if parser.pos + 1 < argv.len() {
                let extra_op = argv[parser.pos].as_bstr();
                let _ = writeln!(stderr, "{}: {}: unexpected operator", cmd_name, extra_op);
                return EXIT_SYNTAX_ERROR;
            }
            let final_bool = if res_invert { !res } else { res };
            if final_bool { EXIT_SUCCESS } else { EXIT_FAILURE }
        }
        Err(msg) => {
            let _ = writeln!(stderr, "{}", msg);
            EXIT_SYNTAX_ERROR
        }
    }
}

pub fn builtin_test(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    eval_test_cmd("test", args, stderr)
}

pub fn builtin_left_bracket(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.last().map(|s| s.as_bstr()) != Some(BStr::new(b"]")) {
        let _ = writeln!(stderr, "[: missing ']'");
        return EXIT_SYNTAX_ERROR;
    }
    eval_test_cmd("[", &args[..args.len() - 1], stderr)
}
