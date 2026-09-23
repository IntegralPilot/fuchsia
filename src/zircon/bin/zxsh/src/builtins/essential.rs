// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{OptionItem, OptionParser};
use crate::errors::zx_status_str;
use crate::eval::{
    ClosedWriter, EXIT_CANNOT_EXEC, EXIT_FAILURE, EXIT_NOT_FOUND, EXIT_SUCCESS, EXIT_SYNTAX_ERROR,
    EvalOutcome, ExecutionContext, RLIM_INFINITY, RLIMIT_AS, RLIMIT_CORE, RLIMIT_CPU, RLIMIT_DATA,
    RLIMIT_FSIZE, RLIMIT_LOCKS, RLIMIT_MEMLOCK, RLIMIT_NOFILE, RLIMIT_NPROC, RLIMIT_RSS,
    RLIMIT_RTPRIO, RLIMIT_STACK, Rlimit, ShellPath, ShellState, clone_fd_to_action, eval_string,
    wait_for_process_to_exit,
};
use crate::fd::Fd;
use crate::path::canonicalize_logical_path;
use crate::process::{spawn_command, spawn_command_with_path};
use crate::string::{
    LineChar, is_valid_var_name, parse_int, parse_mode_mask, parse_non_negative_int,
    path_buf_to_bstring, single_quote, split_ifs_read, split_key_value,
};
use bstr::{BStr, BString, ByteSlice};
use std::io::{Read, Write};

use super::{is_builtin, run_builtin};

macro_rules! write_err {
    ($ctx:expr, $($arg:tt)*) => {{
        if let Some(mut file) = $ctx.stderr() {
            let _ = writeln!(file, $($arg)*);
        }
    }};
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CdPwdMode {
    Logical,
    Physical,
}

fn parse_cd_pwd_opts(args: &[BString]) -> Result<(CdPwdMode, &[BString]), String> {
    let mut parser = OptionParser::new(args);
    let mut mode = CdPwdMode::Logical;

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'L', enable: true }) => mode = CdPwdMode::Logical,
            Ok(OptionItem::Flag { flag: b'P', enable: true }) => mode = CdPwdMode::Physical,
            Ok(OptionItem::Flag { flag, .. }) => {
                return Err(format!("invalid option -- '{}'", flag as char));
            }
            _ => return Err("invalid option".to_string()),
        }
    }

    Ok((mode, parser.rest()))
}

fn is_cur_or_parent_dir(dest: &BStr) -> bool {
    dest == b"." || dest.starts_with(b"./") || dest == b".." || dest.starts_with(b"../")
}

pub fn builtin_pwd(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let (mode, _operand_args) = match parse_cd_pwd_opts(args) {
        Ok(res) => res,
        Err(err) => {
            let _ = writeln!(stderr, "pwd: {}", err);
            return EXIT_FAILURE;
        }
    };

    let pwd = match mode {
        CdPwdMode::Logical => state.cwd().to_owned(),
        CdPwdMode::Physical => std::env::current_dir()
            .ok()
            .and_then(path_buf_to_bstring)
            .unwrap_or_else(|| state.cwd().to_owned()),
    };

    let _ = writeln!(stdout, "{}", pwd);
    EXIT_SUCCESS
}

pub fn builtin_cd(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let (mode, operand_args) = match parse_cd_pwd_opts(args) {
        Ok(res) => res,
        Err(err) => {
            let _ = writeln!(stderr, "cd: {}", err);
            return EXIT_FAILURE;
        }
    };

    let mut print_pwd = false;

    let dest = if operand_args.is_empty() {
        match state.get_var(b"HOME") {
            Some(home) if !home.is_empty() => home,
            _ => BString::from("/"),
        }
    } else if operand_args[0] == "-" {
        let oldpwd = state.get_var(b"OLDPWD").unwrap_or_default();
        if oldpwd.is_empty() {
            let _ = writeln!(stderr, "cd: OLDPWD not set");
            return EXIT_FAILURE;
        }
        print_pwd = true;
        oldpwd
    } else {
        operand_args[0].clone()
    };

    let mut chosen_dest = dest.clone();
    if !dest.starts_with(b"/") && !is_cur_or_parent_dir(dest.as_bstr()) {
        if let Some(cdpath) = state.cdpath() {
            for entry in cdpath.entries() {
                let candidate = if entry.is_empty() {
                    dest.clone()
                } else {
                    let mut p = BString::from(entry);
                    if !p.ends_with(b"/") {
                        p.push(b'/');
                    }
                    p.extend_from_slice(dest.as_bytes());
                    p
                };
                if let Ok(path_buf) = candidate.to_path() {
                    if path_buf.is_dir() {
                        chosen_dest = candidate;
                        if !entry.is_empty() {
                            print_pwd = true;
                        }
                        break;
                    }
                }
            }
        }
    }

    let target_logical_pwd = match mode {
        CdPwdMode::Logical => canonicalize_logical_path(state.cwd(), chosen_dest.as_bstr()),
        CdPwdMode::Physical => chosen_dest.clone(),
    };

    let target_fs_path = match target_logical_pwd.to_path() {
        Ok(p) => p.to_path_buf(),
        Err(err) => {
            let _ = writeln!(stderr, "cd: invalid path {}: {}", chosen_dest, err);
            return EXIT_FAILURE;
        }
    };

    if let Err(err) = std::env::set_current_dir(&target_fs_path) {
        let _ = writeln!(stderr, "cd: {}: {}", chosen_dest, err);
        return EXIT_FAILURE;
    }

    let new_pwd = match mode {
        CdPwdMode::Logical => target_logical_pwd,
        CdPwdMode::Physical => {
            std::env::current_dir().ok().and_then(path_buf_to_bstring).unwrap_or(chosen_dest)
        }
    };

    let prev_cwd = state.cwd().to_owned();
    if !prev_cwd.is_empty() {
        state.set_and_export_var(b"OLDPWD", &prev_cwd);
    }

    state.set_cwd(new_pwd.clone());
    state.export_var(b"PWD");

    if print_pwd {
        let _ = writeln!(stdout, "{}", new_pwd);
    }

    EXIT_SUCCESS
}

fn parse_status_code(
    builtin_name: &str,
    args: &[BString],
    state: &ShellState,
    ctx: &mut ExecutionContext,
) -> Result<i32, EvalOutcome> {
    if args.is_empty() {
        let code = state
            .get_var(b"?")
            .as_ref()
            .and_then(|v| parse_int::<i32>(v.as_bytes()))
            .unwrap_or(EXIT_SUCCESS);
        Ok(code)
    } else {
        match parse_non_negative_int(args[0].as_bytes()) {
            Some(code) => Ok(code),
            None => {
                write_err!(ctx, "{}: Illegal number: {}", builtin_name, args[0]);
                Err(EvalOutcome::Code(EXIT_SYNTAX_ERROR))
            }
        }
    }
}

pub fn builtin_exit(
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    match parse_status_code("exit", args, state, ctx) {
        Ok(code) => Ok(EvalOutcome::Exit(code)),
        Err(outcome) => Ok(outcome),
    }
}

pub fn builtin_return(
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    match parse_status_code("return", args, state, ctx) {
        Ok(code) => Ok(EvalOutcome::Return(code)),
        Err(outcome) => Ok(outcome),
    }
}

fn parse_export_readonly_opts<'a>(
    builtin_name: &str,
    args: &'a [BString],
    stderr: &mut dyn Write,
) -> Result<(bool, &'a [BString]), i32> {
    let mut parser = OptionParser::new(args);
    let mut has_p = false;

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'p', enable: true }) => has_p = true,
            Ok(OptionItem::Flag { flag, .. }) => {
                let _ = writeln!(stderr, "{}: Illegal option -{}", builtin_name, flag as char);
                return Err(EXIT_SYNTAX_ERROR);
            }
            _ => {
                let _ = writeln!(stderr, "{}: Illegal option", builtin_name);
                return Err(EXIT_SYNTAX_ERROR);
            }
        }
    }

    Ok((has_p, parser.rest()))
}

pub fn builtin_export(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let (has_p, operands) = match parse_export_readonly_opts("export", args, stderr) {
        Ok(res) => res,
        Err(status) => return status,
    };

    if has_p || operands.is_empty() {
        for name in state.exported().sorted() {
            if let Some(val) = state.get_var(name) {
                let _ = writeln!(stdout, "export {}={}", name, single_quote(val.as_bstr()));
            } else {
                let _ = writeln!(stdout, "export {}", name);
            }
        }
        EXIT_SUCCESS
    } else {
        for arg in operands {
            if let Some((name, val)) = split_key_value(arg.as_bytes()) {
                if !is_valid_var_name(name) {
                    let _ = writeln!(stderr, "export: {}: bad variable name", name);
                    return EXIT_SYNTAX_ERROR;
                }
                if state.is_readonly(name) {
                    let _ = writeln!(stderr, "export: {}: is read only", name);
                    return EXIT_SYNTAX_ERROR;
                }
                state.set_and_export_var(name, val);
            } else {
                if !is_valid_var_name(arg.as_bstr()) {
                    let _ = writeln!(stderr, "export: {}: bad variable name", arg);
                    return EXIT_SYNTAX_ERROR;
                }
                state.export_var(arg);
            }
        }
        EXIT_SUCCESS
    }
}

pub fn builtin_unset(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum UnsetMode {
        Variable,
        Function,
    }

    let mut mode = UnsetMode::Variable;
    let mut parser = OptionParser::new(args);

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'v', enable: true }) => mode = UnsetMode::Variable,
            Ok(OptionItem::Flag { flag: b'f', enable: true }) => mode = UnsetMode::Function,
            Ok(OptionItem::Flag { flag, .. }) => {
                let _ = writeln!(stderr, "unset: Illegal option -{}", flag as char);
                return EXIT_SYNTAX_ERROR;
            }
            _ => {
                let _ = writeln!(stderr, "unset: Illegal option");
                return EXIT_SYNTAX_ERROR;
            }
        }
    }

    for arg in parser.rest() {
        match mode {
            UnsetMode::Function => {
                state.remove_function(arg);
            }
            UnsetMode::Variable => {
                if state.is_readonly(arg) {
                    let _ = writeln!(stderr, "unset: {}: is read only", arg);
                    return EXIT_SYNTAX_ERROR;
                }
                state.unset_var(arg);
            }
        }
    }
    EXIT_SUCCESS
}

pub fn builtin_local(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if !state.is_in_function() {
        let _ = writeln!(stderr, "local: not in a function");
        return EXIT_SYNTAX_ERROR;
    }
    for arg in args {
        let (name, val) = match split_key_value(arg.as_bytes()) {
            Some((name, val)) => (name, Some(val)),
            None => (arg.as_bstr(), None),
        };
        if !is_valid_var_name(name) {
            let _ = writeln!(stderr, "local: {}: bad variable name", name);
            return EXIT_SYNTAX_ERROR;
        }
        if state.is_readonly(name) {
            let _ = writeln!(stderr, "local: {}: is read only", name);
            return EXIT_SYNTAX_ERROR;
        }
        state.declare_local(name, val);
    }
    EXIT_SUCCESS
}

pub fn builtin_set(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.is_empty() {
        for (name, val) in state.all_vars().sorted_entries() {
            let _ = writeln!(stdout, "{}={}", name, single_quote(val.as_bstr()));
        }
        return EXIT_SUCCESS;
    }

    let mut arg_idx = 0;
    let mut set_positional = false;
    let mut new_args = Vec::new();

    while arg_idx < args.len() {
        let arg = &args[arg_idx];
        if arg == "--" {
            set_positional = true;
            arg_idx += 1;
            new_args.extend(args[arg_idx..].iter().cloned());
            break;
        } else if arg == "-" {
            state.opt_xtrace = false;
            state.opt_verbose = false;
            arg_idx += 1;
            if arg_idx < args.len() {
                set_positional = true;
                new_args.extend(args[arg_idx..].iter().cloned());
            }
            break;
        } else if (arg.starts_with(b"-") || arg.starts_with(b"+")) && arg.len() > 1 {
            let enable = arg.starts_with(b"-");
            let bytes = arg.as_bytes();
            let mut char_idx = 1;
            while char_idx < bytes.len() {
                let flag_char = bytes[char_idx];
                if flag_char == b'o' {
                    char_idx += 1;
                    if arg_idx + 1 < args.len() {
                        let opt_name = &args[arg_idx + 1];
                        arg_idx += 1;
                        if state.set_option_by_name(opt_name.as_bstr(), enable).is_err() {
                            let _ = writeln!(stderr, "set: Illegal option -o {}", opt_name);
                            return EXIT_SYNTAX_ERROR;
                        }
                    } else {
                        let options_status = [
                            ("errexit", state.opt_errexit),
                            ("noglob", state.opt_noglob),
                            ("ignoreeof", state.opt_ignoreeof),
                            ("interactive", state.opt_interactive),
                            ("monitor", false),
                            ("noexec", state.opt_noexec),
                            ("stdin", false),
                            ("xtrace", state.opt_xtrace),
                            ("verbose", state.opt_verbose),
                            ("vi", false),
                            ("emacs", false),
                            ("noclobber", state.opt_noclobber),
                            ("allexport", state.opt_allexport),
                            ("notify", false),
                            ("nounset", state.opt_nounset),
                            ("nolog", false),
                            ("debug", false),
                        ];
                        if enable {
                            let _ = writeln!(stdout, "Current option settings");
                            for (name, is_on) in options_status {
                                let status = if is_on { "on" } else { "off" };
                                let _ = writeln!(stdout, "{:<16}{}", name, status);
                            }
                        } else {
                            for (name, is_on) in options_status {
                                let flag = if is_on { "-o" } else { "+o" };
                                let _ = writeln!(stdout, "set {} {}", flag, name);
                            }
                        }
                    }
                } else {
                    if state.set_option_by_flag(flag_char, enable).is_err() {
                        let _ = writeln!(stderr, "set: Illegal option -{}", flag_char as char);
                        return EXIT_SYNTAX_ERROR;
                    }
                    char_idx += 1;
                }
            }
            arg_idx += 1;
        } else {
            set_positional = true;
            new_args.extend(args[arg_idx..].iter().cloned());
            break;
        }
    }

    if set_positional {
        state.set_args(new_args);
    }
    EXIT_SUCCESS
}

pub fn builtin_shift(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let shift_count = if args.is_empty() {
        1
    } else {
        match parse_non_negative_int(args[0].as_bstr()) {
            Some(val) => val as usize,
            None => {
                let _ = writeln!(stderr, "shift: Illegal number: {}", args[0]);
                return EXIT_SYNTAX_ERROR;
            }
        }
    };
    let current_args = state.get_args();
    if shift_count > current_args.len() {
        let _ = writeln!(stderr, "shift: can't shift that many");
        return EXIT_SYNTAX_ERROR;
    }
    let mut new_args = current_args;
    new_args.drain(0..shift_count);
    state.set_args(new_args);
    EXIT_SUCCESS
}

const SIGNALS: &[&str] = &[
    "EXIT",   // 0
    "HUP",    // 1
    "INT",    // 2
    "QUIT",   // 3
    "ILL",    // 4
    "TRAP",   // 5
    "ABRT",   // 6
    "EMT",    // 7
    "FPE",    // 8
    "KILL",   // 9
    "BUS",    // 10
    "SEGV",   // 11
    "SYS",    // 12
    "PIPE",   // 13
    "ALRM",   // 14
    "TERM",   // 15
    "URG",    // 16
    "STOP",   // 17
    "TSTP",   // 18
    "CONT",   // 19
    "CHLD",   // 20
    "TTIN",   // 21
    "TTOU",   // 22
    "IO",     // 23
    "XCPU",   // 24
    "XFSZ",   // 25
    "VTALRM", // 26
    "PROF",   // 27
    "WINCH",  // 28
    "INFO",   // 29
    "USR1",   // 30
    "USR2",   // 31
];

fn resolve_signal(sig: &BStr) -> Option<&'static str> {
    let bytes = sig.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    if bytes.iter().all(|c| c.is_ascii_digit()) {
        if let Ok(num_str) = std::str::from_utf8(bytes) {
            if let Ok(n) = num_str.parse::<usize>() {
                if n < SIGNALS.len() {
                    return Some(SIGNALS[n]);
                }
            }
        }
        return None;
    }

    let upper = bytes.to_ascii_uppercase();
    let name_bytes =
        if let Some(stripped) = upper.strip_prefix(b"SIG") { stripped } else { upper.as_slice() };

    if name_bytes.is_empty() {
        return None;
    }

    for &sig_name in SIGNALS {
        if sig_name.as_bytes().eq_ignore_ascii_case(name_bytes) {
            return Some(sig_name);
        }
    }

    None
}

pub fn builtin_trap(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut operands = args;
    if let Some(first) = args.first() {
        if first == "--" {
            operands = &args[1..];
        } else if first.starts_with(b"-") && first != "-" && first.len() > 1 {
            let opt_char = first.as_bytes()[1] as char;
            let _ = writeln!(stderr, "trap: Illegal option -{}", opt_char);
            return EXIT_SYNTAX_ERROR;
        }
    }

    if operands.is_empty() {
        for (sig_name, action) in state.traps.sorted_entries() {
            let _ = writeln!(stdout, "trap -- {} {}", single_quote(action.as_bstr()), sig_name);
        }
        return EXIT_SUCCESS;
    }

    let (action, sig_args) =
        if operands.len() == 1 || parse_non_negative_int(operands[0].as_bstr()).is_some() {
            (None, operands)
        } else {
            let action_str = &operands[0];
            let action = if action_str == "-" { None } else { Some(action_str.clone()) };
            (action, &operands[1..])
        };

    for sig in sig_args {
        match resolve_signal(sig.as_bstr()) {
            Some(canonical_sig) => {
                if let Some(ref action_val) = action {
                    state.traps.insert(BString::from(canonical_sig), action_val.clone());
                } else {
                    state.traps.remove(BStr::new(canonical_sig));
                }
            }
            None => {
                let _ = writeln!(stderr, "trap: bad signal {}", sig);
                return EXIT_SYNTAX_ERROR;
            }
        }
    }

    EXIT_SUCCESS
}

pub fn builtin_read(
    args: &[BString],
    state: &mut ShellState,
    stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut raw = false;
    let mut prompt: Option<&BStr> = None;
    let mut parser = OptionParser::new(args);

    while let Some(opt_res) = parser.next_option(|f| f == b'p') {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'r', enable: true }) => raw = true,
            Ok(OptionItem::OptArg { flag: b'p', enable: true, value }) => prompt = Some(value),
            Ok(OptionItem::Flag { flag, .. }) => {
                let _ = writeln!(stderr, "read: Illegal option -{}", flag as char);
                return EXIT_SYNTAX_ERROR;
            }
            Err(msg) => {
                let _ = writeln!(stderr, "read: {}", msg);
                return EXIT_SYNTAX_ERROR;
            }
            _ => {
                let _ = writeln!(stderr, "read: Illegal option");
                return EXIT_SYNTAX_ERROR;
            }
        }
    }

    let vars = parser.rest();
    if vars.is_empty() {
        let _ = writeln!(stderr, "read: arg count");
        return EXIT_SYNTAX_ERROR;
    }

    for var in vars {
        if !is_valid_var_name(var.as_bstr()) {
            let _ = writeln!(stderr, "read: {}: bad variable name", var);
            return EXIT_SYNTAX_ERROR;
        }
        if state.is_readonly(var) {
            let _ = writeln!(stderr, "read: {}: is read only", var);
            return EXIT_SYNTAX_ERROR;
        }
    }

    if let Some(p) = prompt {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            let _ = stderr.write_all(p.as_bytes());
            let _ = stderr.flush();
        }
    }

    let mut line: Vec<LineChar> = Vec::new();
    let mut buf = [0u8; 1];
    let mut eof_reached = false;

    loop {
        let n = match stdin.read(&mut buf) {
            Ok(n) => n,
            Err(err) => {
                let _ = writeln!(stderr, "read: {}", err);
                return EXIT_FAILURE;
            }
        };
        if n == 0 {
            eof_reached = true;
            break;
        }
        let c = buf[0];

        if !raw && c == b'\\' {
            let next_n = match stdin.read(&mut buf) {
                Ok(n) => n,
                Err(err) => {
                    let _ = writeln!(stderr, "read: {}", err);
                    return EXIT_FAILURE;
                }
            };
            if next_n == 0 {
                eof_reached = true;
                break;
            }
            let next_c = buf[0];
            if next_c == b'\n' {
                continue;
            } else {
                line.push(LineChar { byte: next_c, escaped: true });
            }
        } else if c == b'\n' {
            break;
        } else {
            line.push(LineChar { byte: c, escaped: false });
        }
    }

    let ifs = state.get_var(b"IFS").unwrap_or_else(|| BString::from(" \t\n"));
    let fields = split_ifs_read(&line, ifs.as_ref(), vars.len());

    for (i, var) in vars.iter().enumerate() {
        let val = fields.get(i).cloned().unwrap_or_default();
        state.set_var(var, &val);
    }

    if eof_reached { EXIT_FAILURE } else { EXIT_SUCCESS }
}

pub fn builtin_eval(
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    if args.is_empty() {
        return Ok(EvalOutcome::Code(EXIT_SUCCESS));
    }
    let mut cmd_bytes = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            cmd_bytes.push(b' ');
        }
        cmd_bytes.extend_from_slice(arg.as_bytes());
    }
    let cmd_str = BString::from(cmd_bytes);
    eval_string(cmd_str.as_ref(), state, ctx)
}

fn execute_external_process(
    caller_name: &str,
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> EvalOutcome {
    let mut actions = Vec::new();
    for (fd_opt, target) in
        [(ctx.stdin(), Fd::STDIN), (ctx.stdout(), Fd::STDOUT), (ctx.stderr(), Fd::STDERR)]
    {
        if let Some(fd) = fd_opt {
            if let Some(action) = clone_fd_to_action(fd, target) {
                actions.push(action);
            }
        }
    }
    let proc = match spawn_command(args, &state.vars(), &mut actions) {
        Ok(proc) => proc,
        Err(status) => {
            if let Some(mut err) = ctx.stderr() {
                let _ = writeln!(
                    err,
                    "{}: failed to spawn {}: {}",
                    caller_name,
                    args[0],
                    zx_status_str(status)
                );
            }
            let code = if status == zx::Status::NOT_FOUND {
                EXIT_NOT_FOUND
            } else if status == zx::Status::ACCESS_DENIED {
                EXIT_CANNOT_EXEC
            } else {
                EXIT_FAILURE
            };
            return EvalOutcome::Code(code);
        }
    };
    match wait_for_process_to_exit(&proc, ctx) {
        Ok(code) => EvalOutcome::Code(code),
        Err(e) => {
            if let Some(mut err) = ctx.stderr() {
                let _ = writeln!(err, "{}: wait failed: {}", caller_name, e);
            }
            EvalOutcome::Code(EXIT_FAILURE)
        }
    }
}

pub fn builtin_exec(
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let mut arg_idx = 0;
    let mut custom_argv0: Option<BString> = None;
    let mut clear_env = false;

    while arg_idx < args.len() {
        let arg = &args[arg_idx];
        if arg == "--" {
            arg_idx += 1;
            break;
        }
        if arg.starts_with(b"-") && arg.len() > 1 {
            let bytes = arg.as_bytes();
            let mut i = 1;
            let mut opt_err = None;
            while i < bytes.len() {
                match bytes[i] {
                    b'c' => {
                        clear_env = true;
                        i += 1;
                    }
                    b'a' => {
                        if i + 1 < bytes.len() {
                            custom_argv0 = Some(BString::from(&bytes[i + 1..]));
                            i = bytes.len();
                        } else if arg_idx + 1 < args.len() {
                            arg_idx += 1;
                            custom_argv0 = Some(args[arg_idx].clone());
                            i = bytes.len();
                        } else {
                            opt_err = Some("exec: option requires an argument -- 'a'".to_string());
                            break;
                        }
                    }
                    ch => {
                        opt_err = Some(format!("exec: -{}: invalid option", ch as char));
                        break;
                    }
                }
            }
            if let Some(err_msg) = opt_err {
                write_err!(ctx, "{}", err_msg);
                return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR));
            }
            arg_idx += 1;
        } else {
            break;
        }
    }

    let remaining_args = &args[arg_idx..];
    if remaining_args.is_empty() {
        return Ok(EvalOutcome::Code(EXIT_SUCCESS));
    }

    let cmd_name = &remaining_args[0];
    let argv0 = custom_argv0.as_ref().unwrap_or(cmd_name);
    let mut proc_args = Vec::with_capacity(remaining_args.len());
    proc_args.push(argv0.clone());
    proc_args.extend_from_slice(&remaining_args[1..]);

    let mut actions = Vec::new();
    for (fd_opt, target) in
        [(ctx.stdin(), Fd::STDIN), (ctx.stdout(), Fd::STDOUT), (ctx.stderr(), Fd::STDERR)]
    {
        if let Some(fd) = fd_opt {
            if let Some(action) = clone_fd_to_action(fd, target) {
                actions.push(action);
            }
        }
    }

    let env = if clear_env { crate::eval::ShellEnv::new(Vec::new()) } else { state.vars() };

    let proc = match spawn_command_with_path(cmd_name.as_bstr(), &proc_args, &env, &mut actions) {
        Ok(proc) => proc,
        Err(status) => {
            if status == zx::Status::NOT_FOUND {
                write_err!(ctx, "exec: {}: not found", cmd_name);
                return Ok(EvalOutcome::Exit(EXIT_NOT_FOUND));
            } else if status == zx::Status::ACCESS_DENIED {
                write_err!(ctx, "exec: {}: Permission denied", cmd_name);
                return Ok(EvalOutcome::Exit(EXIT_CANNOT_EXEC));
            } else {
                write_err!(ctx, "exec: failed to spawn {}: {}", cmd_name, zx_status_str(status));
                return Ok(EvalOutcome::Exit(EXIT_FAILURE));
            }
        }
    };

    match wait_for_process_to_exit(&proc, ctx) {
        Ok(code) => Ok(EvalOutcome::Exit(code)),
        Err(e) => {
            write_err!(ctx, "exec: wait failed: {}", e);
            Ok(EvalOutcome::Exit(EXIT_FAILURE))
        }
    }
}

fn is_keyword(name: &[u8]) -> bool {
    matches!(
        name,
        b"!" | b"case"
            | b"do"
            | b"done"
            | b"elif"
            | b"else"
            | b"esac"
            | b"fi"
            | b"for"
            | b"if"
            | b"in"
            | b"then"
            | b"until"
            | b"while"
            | b"{"
            | b"}"
    )
}

fn is_special_builtin(name: &[u8]) -> bool {
    matches!(
        name,
        b"." | b":"
            | b"break"
            | b"continue"
            | b"eval"
            | b"exec"
            | b"exit"
            | b"export"
            | b"readonly"
            | b"return"
            | b"set"
            | b"shift"
            | b"times"
            | b"trap"
            | b"unset"
    )
}

fn describe_command(
    name: &BStr,
    use_default_path: bool,
    verbose: bool,
    state: &ShellState,
    stdout: &mut dyn Write,
) -> i32 {
    if is_keyword(name.as_bytes()) {
        if verbose {
            let _ = writeln!(stdout, "{} is a shell keyword", name);
        } else {
            let _ = writeln!(stdout, "{}", name);
        }
        return EXIT_SUCCESS;
    }

    if let Some(val) = state.aliases.get(name) {
        if verbose {
            let _ = writeln!(stdout, "{} is an alias for {}", name, val);
        } else {
            let _ = writeln!(stdout, "alias {}={}", name, single_quote(val.as_bstr()));
        }
        return EXIT_SUCCESS;
    }

    if state.get_function(name).is_some() {
        if verbose {
            let _ = writeln!(stdout, "{} is a shell function", name);
        } else {
            let _ = writeln!(stdout, "{}", name);
        }
        return EXIT_SUCCESS;
    }

    if is_builtin(name) {
        if verbose {
            if is_special_builtin(name.as_bytes()) {
                let _ = writeln!(stdout, "{} is a special shell builtin", name);
            } else {
                let _ = writeln!(stdout, "{} is a shell builtin", name);
            }
        } else {
            let _ = writeln!(stdout, "{}", name);
        }
        return EXIT_SUCCESS;
    }

    let path_obj = if use_default_path { ShellPath::default() } else { state.path() };

    if let Some(resolved) = path_obj.resolve(name) {
        if verbose {
            let _ = writeln!(stdout, "{} is {}", name, resolved);
        } else {
            let _ = writeln!(stdout, "{}", resolved);
        }
        return EXIT_SUCCESS;
    }

    if verbose {
        let _ = writeln!(stdout, "{}: not found", name);
    }
    EXIT_NOT_FOUND
}

pub fn builtin_type(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    _stderr: &mut dyn Write,
) -> i32 {
    let mut exit_code = EXIT_SUCCESS;
    for name in args {
        let res = describe_command(name.as_bstr(), false, true, state, stdout);
        if res != EXIT_SUCCESS {
            exit_code = res;
        }
    }
    exit_code
}

pub fn builtin_wait(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut arg_idx = 0;
    if arg_idx < args.len() {
        let arg = &args[arg_idx];
        if arg == "--" {
            arg_idx += 1;
        } else if arg.starts_with(b"-") && arg != "-" {
            let flag_char = arg.as_bytes()[1] as char;
            let _ = writeln!(stderr, "wait: Illegal option -{}", flag_char);
            return EXIT_SYNTAX_ERROR;
        }
    }

    let operands = &args[arg_idx..];

    if operands.is_empty() {
        for job in state.bg_jobs.drain(..) {
            let _ = job
                .process
                .wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::INFINITE);
        }
        return EXIT_SUCCESS;
    }

    let mut exit_code = EXIT_SUCCESS;
    for arg in operands {
        if arg.starts_with(b"%") {
            match resolve_job(Some(arg), state) {
                Ok(idx) => {
                    let job = state.bg_jobs.remove(idx);
                    if job
                        .process
                        .wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::INFINITE)
                        .to_result()
                        .is_ok()
                    {
                        if let Ok(info) = job.process.info() {
                            exit_code = info.return_code as i32;
                        }
                    }
                }
                Err(err) => {
                    let _ = writeln!(stderr, "wait: {}", err);
                    exit_code = EXIT_NOT_FOUND;
                }
            }
        } else if let Some(pid) = parse_int::<u64>(arg.as_bytes()) {
            let found_idx = state
                .bg_jobs
                .iter()
                .position(|j| j.process.koid().map(|k| k.raw_koid()).unwrap_or(0) == pid);
            if let Some(idx) = found_idx {
                let job = state.bg_jobs.remove(idx);
                if job
                    .process
                    .wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::INFINITE)
                    .to_result()
                    .is_ok()
                {
                    if let Ok(info) = job.process.info() {
                        exit_code = info.return_code as i32;
                    }
                }
            } else {
                let _ = writeln!(stderr, "wait: pid {}: no such job", pid);
                exit_code = EXIT_NOT_FOUND;
            }
        } else {
            let _ = writeln!(stderr, "wait: Illegal number: {}", arg);
            return EXIT_SYNTAX_ERROR;
        }
    }

    exit_code
}

pub fn builtin_dot(
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let mut remaining_args = args;
    if let Some(first) = args.first() {
        if first == "--" {
            remaining_args = &args[1..];
        } else if first.starts_with(b"-") && first.len() > 1 {
            let flag = first[1] as char;
            write_err!(ctx, ".: Illegal option -{}", flag);
            return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR));
        }
    }

    if remaining_args.is_empty() {
        write_err!(ctx, ".: filename argument required");
        return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR));
    }

    let script_path = &remaining_args[0];
    let script_args = remaining_args[1..].to_vec();

    let resolved_path = if script_path.find(b"/").is_some() {
        Some(script_path.clone())
    } else {
        state.path().resolve(script_path.as_ref())
    };

    let resolved_path = match resolved_path {
        Some(path) => path,
        None => {
            write_err!(ctx, ".: {}: not found", script_path);
            return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR));
        }
    };

    let path = match resolved_path.to_path() {
        Ok(path) => path,
        Err(err) => {
            write_err!(ctx, ".: invalid path {}: {}", resolved_path, err);
            return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR));
        }
    };
    let content = match std::fs::read(path) {
        Ok(content) => content,
        Err(err) => {
            if script_path.find(b"/").is_some() && err.kind() == std::io::ErrorKind::NotFound {
                write_err!(ctx, ".: cannot open {}: No such file", resolved_path);
            } else {
                write_err!(ctx, ".: cannot open {}: {}", resolved_path, err);
            }
            return Ok(EvalOutcome::Code(EXIT_NOT_FOUND));
        }
    };

    let mut old_args = None;
    if !script_args.is_empty() {
        old_args = Some(state.get_args());
        state.set_args(script_args);
    }

    let res = eval_string(BStr::new(&content), state, ctx);

    if let Some(args) = old_args {
        state.set_args(args);
    }

    match res {
        Ok(EvalOutcome::Return(code)) => Ok(EvalOutcome::Code(code)),
        other => other,
    }
}

pub fn builtin_getopts(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.len() < 2 {
        let _ = writeln!(stderr, "getopts: usage: getopts optstring name [arg...]");
        return EXIT_SYNTAX_ERROR;
    }
    let optstring = &args[0];
    let name_var = &args[1];

    let clean_args = if args.len() > 2 { args[2..].to_vec() } else { state.get_args() };

    let optind_str = state.get_var(b"OPTIND").unwrap_or_else(|| BString::from("1"));
    let mut optind = parse_int::<usize>(optind_str.as_bytes()).unwrap_or(1);

    if optind == 0 || optind > clean_args.len() + 1 {
        optind = 1;
        state.optopt_offset = 1;
        state.set_var(b"OPTIND", b"1");
    }

    if optind > clean_args.len() {
        state.set_var(name_var, b"?");
        state.unset_var(b"OPTARG");
        state.optopt_offset = 1;
        return EXIT_FAILURE;
    }

    let arg = &clean_args[optind - 1];
    if !arg.starts_with(b"-") || arg == "-" {
        state.set_var(name_var, b"?");
        state.unset_var(b"OPTARG");
        state.optopt_offset = 1;
        return EXIT_FAILURE;
    }

    if arg == "--" {
        let optind_next = (optind + 1).to_string();
        state.set_var(b"OPTIND", optind_next.as_bytes());
        state.set_var(name_var, b"?");
        state.unset_var(b"OPTARG");
        state.optopt_offset = 1;
        return EXIT_FAILURE;
    }

    let mut offset = state.optopt_offset;
    let bytes = arg.as_bytes();
    if offset < 1 || offset >= bytes.len() {
        offset = 1;
        state.optopt_offset = 1;
    }

    let opt_byte = bytes[offset];
    let is_silent = optstring.starts_with(b":");
    let optsearch_str = if is_silent { &optstring[1..] } else { optstring };

    if optsearch_str.find(&[opt_byte]).is_some() {
        let pos = optstring.find(&[opt_byte]).unwrap();
        let requires_arg = pos + 1 < optstring.len() && optstring.as_bytes()[pos + 1] == b':';
        if requires_arg {
            if offset + 1 < bytes.len() {
                let val = BStr::new(&bytes[offset + 1..]);
                state.set_var(b"OPTARG", val);
                let optind_next = (optind + 1).to_string();
                state.set_var(b"OPTIND", optind_next.as_bytes());
                state.optopt_offset = 1;
            } else if optind < clean_args.len() {
                let val = &clean_args[optind];
                state.set_var(b"OPTARG", val);
                let optind_next = (optind + 2).to_string();
                state.set_var(b"OPTIND", optind_next.as_bytes());
                state.optopt_offset = 1;
            } else {
                if is_silent {
                    state.set_var(name_var, b":");
                    let opt_char_str = (opt_byte as char).to_string();
                    state.set_var(b"OPTARG", opt_char_str.as_bytes());
                } else {
                    let _ = writeln!(
                        stderr,
                        "getopts: option requires an argument -- {}",
                        opt_byte as char
                    );
                    state.set_var(name_var, b"?");
                    state.unset_var(b"OPTARG");
                }
                let optind_next = (optind + 1).to_string();
                state.set_var(b"OPTIND", optind_next.as_bytes());
                state.optopt_offset = 1;
                return EXIT_SUCCESS;
            }
            let opt_char_str = (opt_byte as char).to_string();
            state.set_var(name_var, opt_char_str.as_bytes());
        } else {
            let opt_char_str = (opt_byte as char).to_string();
            state.set_var(name_var, opt_char_str.as_bytes());
            state.set_var(b"OPTARG", b"");
            if offset + 1 < bytes.len() {
                state.optopt_offset = offset + 1;
            } else {
                let optind_next = (optind + 1).to_string();
                state.set_var(b"OPTIND", optind_next.as_bytes());
                state.optopt_offset = 1;
            }
        }
    } else {
        if is_silent {
            state.set_var(name_var, b"?");
            let opt_char_str = (opt_byte as char).to_string();
            state.set_var(b"OPTARG", opt_char_str.as_bytes());
        } else {
            let _ = writeln!(stderr, "getopts: illegal option -- {}", opt_byte as char);
            state.set_var(name_var, b"?");
            state.unset_var(b"OPTARG");
        }
        if offset + 1 < bytes.len() {
            state.optopt_offset = offset + 1;
        } else {
            let optind_next = (optind + 1).to_string();
            state.set_var(b"OPTIND", optind_next.as_bytes());
            state.optopt_offset = 1;
        }
    }
    EXIT_SUCCESS
}

pub fn builtin_command(
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let mut use_default_path = false;
    let mut verify_brief = false;
    let mut verify_verbose = false;
    let mut parser = OptionParser::new(args);

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'p', enable: true }) => use_default_path = true,
            Ok(OptionItem::Flag { flag: b'v', enable: true }) => verify_brief = true,
            Ok(OptionItem::Flag { flag: b'V', enable: true }) => verify_verbose = true,
            Ok(OptionItem::Flag { flag, .. }) => {
                write_err!(ctx, "command: Illegal option -{}", flag as char);
                return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR));
            }
            _ => {
                write_err!(ctx, "command: Illegal option");
                return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR));
            }
        }
    }

    let remaining_args = parser.rest();

    if verify_brief || verify_verbose {
        if remaining_args.is_empty() {
            return Ok(EvalOutcome::Code(EXIT_SUCCESS));
        }
        let name = remaining_args[0].as_bstr();
        let verbose = verify_verbose;
        let mut default_out = ClosedWriter;
        let mut stdout_ref = ctx.stdout();
        let stdout: &mut dyn Write = match &mut stdout_ref {
            Some(file) => file,
            None => &mut default_out,
        };
        let code = describe_command(name, use_default_path, verbose, state, stdout);
        return Ok(EvalOutcome::Code(code));
    }

    if remaining_args.is_empty() {
        return Ok(EvalOutcome::Code(EXIT_SUCCESS));
    }

    let cmd_name = &remaining_args[0];
    if is_builtin(cmd_name.as_bstr()) {
        return run_builtin(cmd_name.as_bstr(), &remaining_args[1..], state, ctx);
    }

    if use_default_path {
        let path_obj = ShellPath::default();
        let mut resolved_args = remaining_args.to_vec();
        if let Some(resolved) = path_obj.resolve(cmd_name.as_bstr()) {
            resolved_args[0] = resolved;
        }
        Ok(execute_external_process("command", &resolved_args, state, ctx))
    } else {
        Ok(execute_external_process("command", remaining_args, state, ctx))
    }
}

fn parse_loop_count(name: &str, args: &[BString], ctx: &mut ExecutionContext) -> Option<u32> {
    if args.is_empty() {
        Some(1)
    } else {
        match parse_int::<i32>(args[0].as_bytes()) {
            Some(count) if count > 0 => Some(count as u32),
            _ => {
                write_err!(ctx, "{}: Illegal number: {}", name, args[0]);
                None
            }
        }
    }
}

pub fn builtin_break(
    args: &[BString],
    env: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let count = match parse_loop_count("break", args, ctx) {
        Some(count) => count,
        None => return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR)),
    };
    if env.loop_nest == 0 {
        return Ok(EvalOutcome::Code(EXIT_SUCCESS));
    }
    let n = count.min(env.loop_nest);
    Ok(EvalOutcome::Break(n))
}

pub fn builtin_continue(
    args: &[BString],
    env: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let count = match parse_loop_count("continue", args, ctx) {
        Some(count) => count,
        None => return Ok(EvalOutcome::Code(EXIT_SYNTAX_ERROR)),
    };
    if env.loop_nest == 0 {
        return Ok(EvalOutcome::Code(EXIT_SUCCESS));
    }
    let n = count.min(env.loop_nest);
    Ok(EvalOutcome::Continue(n))
}

pub fn builtin_alias(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if args.is_empty() {
        for (alias_name, alias_value) in state.aliases.sorted_entries() {
            let _ = writeln!(stdout, "{}={}", alias_name, single_quote(alias_value.as_bstr()));
        }
        EXIT_SUCCESS
    } else {
        let mut status = EXIT_SUCCESS;
        for arg in args {
            if let Some(rel_pos) =
                arg.as_bytes().get(1..).and_then(|sub| sub.iter().position(|&b| b == b'='))
            {
                let eq_idx = rel_pos + 1;
                let name = &arg.as_bytes()[..eq_idx];
                let value = &arg.as_bytes()[eq_idx + 1..];
                state.aliases.insert(BString::from(name), BString::from(value));
            } else {
                if let Some(value) = state.aliases.get(arg) {
                    let _ = writeln!(stdout, "{}={}", arg, single_quote(value.as_bstr()));
                } else {
                    let _ = writeln!(stderr, "alias: {} not found", arg);
                    status = EXIT_FAILURE;
                }
            }
        }
        status
    }
}

pub fn builtin_unalias(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            idx += 1;
            break;
        }
        if arg == "-" || !arg.starts_with(b"-") || arg.len() == 1 {
            break;
        }
        let mut remove_all = false;
        for &byte in arg.as_bytes().iter().skip(1) {
            if byte == b'a' {
                remove_all = true;
            } else {
                let _ = writeln!(stderr, "unalias: invalid option -- '{}'", byte as char);
                return EXIT_FAILURE;
            }
        }
        if remove_all {
            state.aliases.clear();
            return EXIT_SUCCESS;
        }
        idx += 1;
    }

    let operands = &args[idx..];
    if operands.is_empty() {
        return EXIT_FAILURE;
    }
    let mut status = EXIT_SUCCESS;
    for arg in operands {
        if state.aliases.remove(arg.as_bstr()).is_none() {
            let _ = writeln!(stderr, "unalias: {} not found", arg);
            status = EXIT_FAILURE;
        }
    }
    status
}

pub fn builtin_umask(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut parser = OptionParser::new(args);
    let mut symbolic_mode = false;

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'S', enable: true }) => symbolic_mode = true,
            Ok(OptionItem::Flag { flag, .. }) => {
                let _ = writeln!(stderr, "umask: Illegal option -{}", flag as char);
                return EXIT_SYNTAX_ERROR;
            }
            _ => {
                let _ = writeln!(stderr, "umask: Illegal option");
                return EXIT_SYNTAX_ERROR;
            }
        }
    }

    let operands = parser.rest();
    if operands.is_empty() {
        if symbolic_mode {
            let perm = 0o777 & !state.umask();
            let format_who = |shift: u32| {
                let mode_val = (perm >> shift) & 7;
                let mut perm_str = String::new();
                if mode_val & 4 != 0 {
                    perm_str.push('r');
                }
                if mode_val & 2 != 0 {
                    perm_str.push('w');
                }
                if mode_val & 1 != 0 {
                    perm_str.push('x');
                }
                perm_str
            };
            let _ = writeln!(stdout, "u={},g={},o={}", format_who(6), format_who(3), format_who(0));
        } else {
            let _ = writeln!(stdout, "{:04o}", state.umask());
        }
        return EXIT_SUCCESS;
    }

    let arg = &operands[0];
    let new_umask = match parse_mode_mask(arg.as_bytes(), state.umask()) {
        Some(val) => val,
        None => {
            let _ = writeln!(stderr, "umask: Illegal number: {}", arg);
            return EXIT_SYNTAX_ERROR;
        }
    };
    state.set_umask(new_umask);
    EXIT_SUCCESS
}

pub fn builtin_readonly(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let (has_p, operands) = match parse_export_readonly_opts("readonly", args, stderr) {
        Ok(res) => res,
        Err(status) => return status,
    };

    if has_p || operands.is_empty() {
        for name in state.readonly().sorted() {
            if let Some(val) = state.get_var(name) {
                let _ = writeln!(stdout, "readonly {}={}", name, single_quote(val.as_bstr()));
            } else {
                let _ = writeln!(stdout, "readonly {}", name);
            }
        }
        EXIT_SUCCESS
    } else {
        for arg in operands {
            if let Some((name, val)) = split_key_value(arg.as_bytes()) {
                if !is_valid_var_name(name) {
                    let _ = writeln!(stderr, "readonly: {}: bad variable name", name);
                    return EXIT_SYNTAX_ERROR;
                }
                if state.is_readonly(name) {
                    let _ = writeln!(stderr, "readonly: {}: is read only", name);
                    return EXIT_SYNTAX_ERROR;
                }
                state.set_var(name, val);
                state.make_readonly(name);
            } else {
                if !is_valid_var_name(arg.as_bstr()) {
                    let _ = writeln!(stderr, "readonly: {}: bad variable name", arg);
                    return EXIT_SYNTAX_ERROR;
                }
                state.make_readonly(arg);
            }
        }
        EXIT_SUCCESS
    }
}

pub fn builtin_hash(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut parser = OptionParser::new(args);
    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'r', enable: true }) => {
                state.clear_command_cache();
            }
            Ok(OptionItem::Flag { flag, .. }) => {
                let _ = writeln!(stderr, "hash: -{}: invalid option", flag as char);
                return EXIT_FAILURE;
            }
            _ => {
                let _ = writeln!(stderr, "hash: invalid option");
                return EXIT_FAILURE;
            }
        }
    }

    let cmd_args = parser.rest();
    if cmd_args.is_empty() {
        let cache = state.command_cache();
        for (_cmd_name, entry) in cache.sorted_entries() {
            let _ = writeln!(stdout, "{}", entry.path);
        }
        return EXIT_SUCCESS;
    }

    let mut status = EXIT_SUCCESS;
    for cmd in cmd_args {
        if cmd.find(b"/").is_some() {
            if let Ok(p) = cmd.to_path() {
                if !p.exists() {
                    let _ = writeln!(stderr, "hash: {}: not found", cmd);
                    status = EXIT_FAILURE;
                }
            } else {
                let _ = writeln!(stderr, "hash: {}: not found", cmd);
                status = EXIT_FAILURE;
            }
            continue;
        }
        if let Some(resolved) = state.path().resolve(cmd.as_ref()) {
            state.insert_command_cache(cmd.clone(), resolved, 0);
        } else {
            let _ = writeln!(stderr, "hash: {}: not found", cmd);
            status = EXIT_FAILURE;
        }
    }
    status
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LimitHow {
    Soft = 1,
    Hard = 2,
    Both = 3,
}

struct LimitSpec {
    name: &'static str,
    resource: i32,
    factor: u64,
    option: char,
}

const LIMITS: &[LimitSpec] = &[
    LimitSpec { name: "time(seconds)", resource: RLIMIT_CPU, factor: 1, option: 't' },
    LimitSpec { name: "file(blocks)", resource: RLIMIT_FSIZE, factor: 512, option: 'f' },
    LimitSpec { name: "data(kbytes)", resource: RLIMIT_DATA, factor: 1024, option: 'd' },
    LimitSpec { name: "stack(kbytes)", resource: RLIMIT_STACK, factor: 1024, option: 's' },
    LimitSpec { name: "coredump(blocks)", resource: RLIMIT_CORE, factor: 512, option: 'c' },
    LimitSpec { name: "memory(kbytes)", resource: RLIMIT_RSS, factor: 1024, option: 'm' },
    LimitSpec {
        name: "locked memory(kbytes)",
        resource: RLIMIT_MEMLOCK,
        factor: 1024,
        option: 'l',
    },
    LimitSpec { name: "process", resource: RLIMIT_NPROC, factor: 1, option: 'p' },
    LimitSpec { name: "nofiles", resource: RLIMIT_NOFILE, factor: 1, option: 'n' },
    LimitSpec { name: "vmemory(kbytes)", resource: RLIMIT_AS, factor: 1024, option: 'v' },
    LimitSpec { name: "locks", resource: RLIMIT_LOCKS, factor: 1, option: 'w' },
    LimitSpec { name: "rtprio", resource: RLIMIT_RTPRIO, factor: 1, option: 'r' },
];

fn printlim(how: LimitHow, limit: &Rlimit, l: &LimitSpec, stdout: &mut dyn Write) {
    let mut val = limit.hard;
    if (how as u8 & LimitHow::Soft as u8) != 0 {
        val = limit.soft;
    }

    if val == RLIM_INFINITY {
        let _ = writeln!(stdout, "unlimited");
    } else {
        let val_scaled = val / l.factor;
        let _ = writeln!(stdout, "{}", val_scaled);
    }
}

pub fn builtin_ulimit(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut how = LimitHow::Both;
    let mut what = 'f';
    let mut all = false;
    let mut parser = OptionParser::new(args);

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'H', enable: true }) => how = LimitHow::Hard,
            Ok(OptionItem::Flag { flag: b'S', enable: true }) => how = LimitHow::Soft,
            Ok(OptionItem::Flag { flag: b'a', enable: true }) => all = true,
            Ok(OptionItem::Flag {
                flag:
                    b @ (b't' | b'f' | b'd' | b's' | b'c' | b'm' | b'l' | b'p' | b'u' | b'n' | b'v'
                    | b'w' | b'r'),
                enable: true,
            }) => {
                what = if b == b'u' { 'p' } else { b as char };
            }
            Ok(OptionItem::Flag { flag, .. }) => {
                let _ = writeln!(stderr, "ulimit: Illegal option -{}", flag as char);
                return EXIT_SYNTAX_ERROR;
            }
            _ => {
                let _ = writeln!(stderr, "ulimit: Illegal option");
                return EXIT_SYNTAX_ERROR;
            }
        }
    }

    let positional_args = parser.rest();
    let set = !positional_args.is_empty();

    let target_limit_spec = LIMITS
        .iter()
        .find(|l| l.option == what || (what == 'u' && l.option == 'p'))
        .unwrap_or(&LIMITS[1]);

    let mut val = 0u64;

    if set {
        if all || positional_args.len() > 1 {
            let _ = writeln!(stderr, "ulimit: too many arguments");
            return EXIT_SYNTAX_ERROR;
        }

        let p = positional_args[0].as_bstr();
        if p == "unlimited" {
            val = RLIM_INFINITY;
        } else {
            let p_bytes = p.as_bytes();
            if p_bytes.is_empty() || !p_bytes.iter().all(|c| c.is_ascii_digit()) {
                let _ = writeln!(stderr, "ulimit: bad number");
                return EXIT_SYNTAX_ERROR;
            }
            let parsed_num =
                match std::str::from_utf8(p_bytes).ok().and_then(|s| s.parse::<u64>().ok()) {
                    Some(n) => n,
                    None => {
                        let _ = writeln!(stderr, "ulimit: bad number");
                        return EXIT_SYNTAX_ERROR;
                    }
                };
            if parsed_num > RLIM_INFINITY / target_limit_spec.factor {
                val = RLIM_INFINITY;
            } else {
                val = parsed_num * target_limit_spec.factor;
            }
        }
    }

    if all {
        for l in LIMITS {
            let limit = state
                .get_rlimit(l.resource)
                .unwrap_or(Rlimit { soft: RLIM_INFINITY, hard: RLIM_INFINITY });
            let _ = write!(stdout, "{:<20} ", l.name);
            printlim(how, &limit, l, stdout);
        }
        return EXIT_SUCCESS;
    }

    let mut limit = state
        .get_rlimit(target_limit_spec.resource)
        .unwrap_or(Rlimit { soft: RLIM_INFINITY, hard: RLIM_INFINITY });
    if set {
        if (how as u8 & LimitHow::Hard as u8) != 0 {
            limit.hard = val;
        }
        if (how as u8 & LimitHow::Soft as u8) != 0 {
            limit.soft = val;
        }
        if limit.soft > limit.hard {
            let _ = writeln!(stderr, "ulimit: error setting limit (Operation not permitted)");
            return EXIT_SYNTAX_ERROR;
        }
        state.set_rlimit(target_limit_spec.resource, limit);
    } else {
        printlim(how, &limit, target_limit_spec, stdout);
    }

    EXIT_SUCCESS
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobsMode {
    Normal,
    Pid,  // -l
    Pgid, // -p
}

fn parse_jobs_args(args: &[BString]) -> Result<(JobsMode, Vec<BString>), String> {
    let mut parser = OptionParser::new(args);
    let mut mode = JobsMode::Normal;

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'l', enable: true }) => mode = JobsMode::Pid,
            Ok(OptionItem::Flag { flag: b'p', enable: true }) => mode = JobsMode::Pgid,
            Ok(OptionItem::Flag { flag, .. }) => {
                return Err(format!("Illegal option -{}", flag as char));
            }
            _ => return Err("Illegal option".to_string()),
        }
    }

    Ok((mode, parser.rest().to_vec()))
}

fn parse_fg_bg_args(args: &[BString]) -> Result<Vec<BString>, String> {
    let mut parser = OptionParser::new(args);

    if let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag, .. }) => {
                return Err(format!("Illegal option -{}", flag as char));
            }
            _ => return Err("Illegal option".to_string()),
        }
    }

    Ok(parser.rest().to_vec())
}

fn resolve_job(arg: Option<&BString>, state: &ShellState) -> Result<usize, String> {
    let arg_bstr = match arg {
        Some(a) => a.as_bstr(),
        None => BStr::new(b""),
    };

    if arg_bstr.is_empty() || arg_bstr == b"%" || arg_bstr == b"%+" {
        if state.bg_jobs.is_empty() {
            return Err("No current job".to_string());
        }
        return Ok(state.bg_jobs.len() - 1);
    }

    if arg_bstr == b"%-" {
        if state.bg_jobs.len() < 2 {
            return Err("No previous job".to_string());
        }
        return Ok(state.bg_jobs.len() - 2);
    }

    if let Some(spec) = arg_bstr.strip_prefix(b"%") {
        if !spec.is_empty() && spec.iter().all(|b| b.is_ascii_digit()) {
            let num =
                parse_int::<usize>(spec).ok_or_else(|| format!("No such job: {}", arg_bstr))?;
            if num > 0 && num <= state.bg_jobs.len() {
                return Ok(num - 1);
            } else {
                return Err(format!("No such job: {}", arg_bstr));
            }
        }
        if let Some(needle) = spec.strip_prefix(b"?") {
            let matches: Vec<usize> = state
                .bg_jobs
                .iter()
                .enumerate()
                .filter(|(_, job)| job.cmd.contains_str(needle))
                .map(|(i, _)| i)
                .collect();
            if matches.len() == 1 {
                return Ok(matches[0]);
            } else if matches.len() > 1 {
                return Err(format!("{}: ambiguous", arg_bstr));
            } else {
                return Err(format!("No such job: {}", arg_bstr));
            }
        } else {
            let matches: Vec<usize> = state
                .bg_jobs
                .iter()
                .enumerate()
                .filter(|(_, job)| job.cmd.starts_with_str(spec))
                .map(|(i, _)| i)
                .collect();
            if matches.len() == 1 {
                return Ok(matches[0]);
            } else if matches.len() > 1 {
                return Err(format!("{}: ambiguous", arg_bstr));
            } else {
                return Err(format!("No such job: {}", arg_bstr));
            }
        }
    }

    Err(format!("No such job: {}", arg_bstr))
}

fn format_job_entry(
    job: &crate::eval::BgJob,
    job_idx: usize,
    total_jobs: usize,
    mode: JobsMode,
) -> String {
    let raw_pid = job.process.koid().map(|k| k.raw_koid()).unwrap_or(0);
    if mode == JobsMode::Pgid {
        return format!("{}\n", raw_pid);
    }

    let mark = if job_idx + 1 == total_jobs {
        '+'
    } else if total_jobs >= 2 && job_idx + 2 == total_jobs {
        '-'
    } else {
        ' '
    };

    let is_terminated =
        job.process.wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::ZERO).is_ok();

    let status_str = if is_terminated {
        if let Ok(info) = job.process.info() {
            if info.return_code == 0 {
                "Done".to_string()
            } else {
                format!("Done({})", info.return_code)
            }
        } else {
            "Done".to_string()
        }
    } else {
        "Running".to_string()
    };

    let prefix = if mode == JobsMode::Pid {
        format!("[{}] {} {} {}", job_idx + 1, mark, raw_pid, status_str)
    } else {
        format!("[{}] {} {}", job_idx + 1, mark, status_str)
    };

    let pad = if prefix.len() < 33 { 33 - prefix.len() } else { 0 };
    format!("{}{:width$}{}\n", prefix, "", job.cmd, width = pad)
}

fn cleanup_done_jobs(state: &mut ShellState) {
    state.bg_jobs.retain(|job| {
        job.process.wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::ZERO).is_err()
    });
}

pub fn builtin_jobs(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let (mode, job_specs) = match parse_jobs_args(args) {
        Ok(res) => res,
        Err(err) => {
            let _ = writeln!(stderr, "jobs: {}", err);
            return 2;
        }
    };

    if job_specs.is_empty() {
        let total = state.bg_jobs.len();
        for (i, job) in state.bg_jobs.iter().enumerate() {
            let line = format_job_entry(job, i, total, mode);
            let _ = write!(stdout, "{}", line);
        }
        cleanup_done_jobs(state);
        EXIT_SUCCESS
    } else {
        let mut exit_code = EXIT_SUCCESS;
        let total = state.bg_jobs.len();
        for spec in job_specs {
            match resolve_job(Some(&spec), state) {
                Ok(idx) => {
                    let line = format_job_entry(&state.bg_jobs[idx], idx, total, mode);
                    let _ = write!(stdout, "{}", line);
                }
                Err(err) => {
                    let _ = writeln!(stderr, "jobs: {}", err);
                    exit_code = EXIT_FAILURE;
                }
            }
        }
        cleanup_done_jobs(state);
        exit_code
    }
}

pub fn builtin_fg(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let job_specs = match parse_fg_bg_args(args) {
        Ok(specs) => specs,
        Err(err) => {
            let _ = writeln!(stderr, "fg: {}", err);
            return 2;
        }
    };

    let specs_to_run: Vec<Option<BString>> =
        if job_specs.is_empty() { vec![None] } else { job_specs.into_iter().map(Some).collect() };

    let mut exit_code = EXIT_SUCCESS;
    for spec in specs_to_run {
        let idx = match resolve_job(spec.as_ref(), state) {
            Ok(idx) => idx,
            Err(err) => {
                let _ = writeln!(stderr, "fg: {}", err);
                return 2;
            }
        };

        let job = state.bg_jobs.remove(idx);
        let _ = writeln!(stdout, "{}", job.cmd);
        if job
            .process
            .wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::INFINITE)
            .to_result()
            .is_ok()
        {
            if let Ok(info) = job.process.info() {
                exit_code = info.return_code as i32;
            }
        }
    }
    exit_code
}

pub fn builtin_bg(
    args: &[BString],
    state: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let job_specs = match parse_fg_bg_args(args) {
        Ok(specs) => specs,
        Err(err) => {
            let _ = writeln!(stderr, "bg: {}", err);
            return 2;
        }
    };

    let specs_to_run: Vec<Option<BString>> =
        if job_specs.is_empty() { vec![None] } else { job_specs.into_iter().map(Some).collect() };

    for spec in specs_to_run {
        let idx = match resolve_job(spec.as_ref(), state) {
            Ok(idx) => idx,
            Err(err) => {
                let _ = writeln!(stderr, "bg: {}", err);
                return 2;
            }
        };
        let job = &state.bg_jobs[idx];
        let job_num = idx + 1;
        let _ = writeln!(stdout, "[{}] {}", job_num, job.cmd);
    }
    EXIT_SUCCESS
}
