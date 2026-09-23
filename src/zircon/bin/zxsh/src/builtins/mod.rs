// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io::{Read, Write};

use crate::eval::{
    ClosedReader, ClosedWriter, EXIT_FAILURE, EXIT_SUCCESS, EvalOutcome, ExecutionContext,
    ShellState, VarName,
};
use bstr::{BString, ByteSlice};

pub mod dump;
pub mod echo;
pub mod essential;
pub mod file_utils;
pub mod fuchsia;
pub mod list;
pub mod msleep;
pub mod printf;
pub mod test;
pub mod times;

#[derive(Clone, Copy)]
enum BuiltinType {
    Alias,
    Bg,
    Break,
    Cd,
    Chdir,
    Colon,
    Command,
    Continue,
    Cp,
    Dm,
    Dot,
    Dump,
    Echo,
    Eval,
    Exec,
    Exit,
    Export,
    False,
    Fg,
    Getopts,
    Hash,
    Jobs,
    K,
    List,
    LeftBracket,
    Local,
    Ls,
    Mkdir,
    Msleep,
    Mv,
    Power,
    Pwd,
    Printf,
    Read,
    Readonly,
    Return,
    Rm,
    Set,
    Shift,
    Test,
    Times,
    Trap,
    True,
    Type,
    Ulimit,
    Umask,
    Unalias,
    Unset,
    Wait,
}

struct BuiltinEntry {
    name: [u8; 18],
    len: u8,
    func: BuiltinType,
}

const fn make_entry(name: &[u8], func: BuiltinType) -> BuiltinEntry {
    let mut name_arr = [0u8; 18];
    let len = name.len();
    if len > 18 {
        panic!("Builtin name too long");
    }
    let mut i = 0;
    while i < len {
        name_arr[i] = name[i];
        i += 1;
    }
    BuiltinEntry { name: name_arr, len: len as u8, func }
}

static BUILTINS: &[BuiltinEntry] = &[
    make_entry(b".", BuiltinType::Dot),
    make_entry(b":", BuiltinType::Colon),
    make_entry(b"[", BuiltinType::LeftBracket),
    make_entry(b"alias", BuiltinType::Alias),
    make_entry(b"bg", BuiltinType::Bg),
    make_entry(b"break", BuiltinType::Break),
    make_entry(b"cd", BuiltinType::Cd),
    make_entry(b"chdir", BuiltinType::Chdir),
    make_entry(b"command", BuiltinType::Command),
    make_entry(b"continue", BuiltinType::Continue),
    make_entry(b"cp", BuiltinType::Cp),
    make_entry(b"dm", BuiltinType::Dm),
    make_entry(b"dump", BuiltinType::Dump),
    make_entry(b"echo", BuiltinType::Echo),
    make_entry(b"eval", BuiltinType::Eval),
    make_entry(b"exec", BuiltinType::Exec),
    make_entry(b"exit", BuiltinType::Exit),
    make_entry(b"export", BuiltinType::Export),
    make_entry(b"false", BuiltinType::False),
    make_entry(b"fg", BuiltinType::Fg),
    make_entry(b"getopts", BuiltinType::Getopts),
    make_entry(b"hash", BuiltinType::Hash),
    make_entry(b"jobs", BuiltinType::Jobs),
    make_entry(b"k", BuiltinType::K),
    make_entry(b"list", BuiltinType::List),
    make_entry(b"local", BuiltinType::Local),
    make_entry(b"ls", BuiltinType::Ls),
    make_entry(b"mkdir", BuiltinType::Mkdir),
    make_entry(b"msleep", BuiltinType::Msleep),
    make_entry(b"mv", BuiltinType::Mv),
    make_entry(b"power", BuiltinType::Power),
    make_entry(b"printf", BuiltinType::Printf),
    make_entry(b"pwd", BuiltinType::Pwd),
    make_entry(b"read", BuiltinType::Read),
    make_entry(b"readonly", BuiltinType::Readonly),
    make_entry(b"return", BuiltinType::Return),
    make_entry(b"rm", BuiltinType::Rm),
    make_entry(b"set", BuiltinType::Set),
    make_entry(b"shift", BuiltinType::Shift),
    make_entry(b"test", BuiltinType::Test),
    make_entry(b"times", BuiltinType::Times),
    make_entry(b"trap", BuiltinType::Trap),
    make_entry(b"true", BuiltinType::True),
    make_entry(b"type", BuiltinType::Type),
    make_entry(b"ulimit", BuiltinType::Ulimit),
    make_entry(b"umask", BuiltinType::Umask),
    make_entry(b"unalias", BuiltinType::Unalias),
    make_entry(b"unset", BuiltinType::Unset),
    make_entry(b"wait", BuiltinType::Wait),
];

/// Checks if the given command name corresponds to an internal shell builtin.
pub fn is_builtin(name: impl VarName) -> bool {
    let name = name.to_bstr();
    BUILTINS
        .binary_search_by_key(&name.as_bytes(), |entry| &entry.name[..entry.len as usize])
        .is_ok()
}

fn with_io(
    ctx: &mut ExecutionContext,
    args: &[BString],
    state: &mut ShellState,
    f: fn(&[BString], &mut ShellState, &mut dyn Read, &mut dyn Write, &mut dyn Write) -> i32,
) -> Result<EvalOutcome, String> {
    let mut default_in = ClosedReader;
    let mut default_out = ClosedWriter;
    let mut default_err = ClosedWriter;

    let mut stdin_ref = ctx.stdin();
    let in_file: &mut dyn Read = match &mut stdin_ref {
        Some(f) => f,
        None => &mut default_in,
    };
    let mut stdout_ref = ctx.stdout();
    let out_file: &mut dyn Write = match &mut stdout_ref {
        Some(f) => f,
        None => &mut default_out,
    };
    let mut stderr_ref = ctx.stderr();
    let err_file: &mut dyn Write = match &mut stderr_ref {
        Some(f) => f,
        None => &mut default_err,
    };
    Ok(EvalOutcome::Code(f(args, state, in_file, out_file, err_file)))
}

/// Executes an internal shell builtin command with the provided argument list and execution
/// context.
pub fn run_builtin(
    name: impl VarName,
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    run_builtin_impl(name.to_bstr(), args, state, ctx)
}

fn run_builtin_impl(
    name: &bstr::BStr,
    args: &[BString],
    state: &mut ShellState,
    ctx: &mut ExecutionContext,
) -> Result<EvalOutcome, String> {
    let name = name.to_bstr();
    let idx = BUILTINS
        .binary_search_by_key(&name.as_bytes(), |entry| &entry.name[..entry.len as usize])
        .map_err(|_| format!("builtin not found: {}", name))?;

    match BUILTINS[idx].func {
        BuiltinType::Alias => with_io(ctx, args, state, essential::builtin_alias),
        BuiltinType::Bg => with_io(ctx, args, state, essential::builtin_bg),
        BuiltinType::Break => essential::builtin_break(args, state, ctx),
        BuiltinType::Cd | BuiltinType::Chdir => with_io(ctx, args, state, essential::builtin_cd),
        BuiltinType::Colon | BuiltinType::True => Ok(EvalOutcome::Code(EXIT_SUCCESS)),
        BuiltinType::Command => essential::builtin_command(args, state, ctx),
        BuiltinType::Continue => essential::builtin_continue(args, state, ctx),
        BuiltinType::Cp => with_io(ctx, args, state, file_utils::builtin_cp),
        BuiltinType::Dm => with_io(ctx, args, state, fuchsia::builtin_dm),
        BuiltinType::Dot => essential::builtin_dot(args, state, ctx),
        BuiltinType::Dump => with_io(ctx, args, state, dump::builtin_dump),
        BuiltinType::Echo => with_io(ctx, args, state, echo::builtin_echo),
        BuiltinType::Eval => essential::builtin_eval(args, state, ctx),
        BuiltinType::Exec => essential::builtin_exec(args, state, ctx),
        BuiltinType::Exit => essential::builtin_exit(args, state, ctx),
        BuiltinType::Export => with_io(ctx, args, state, essential::builtin_export),
        BuiltinType::False => Ok(EvalOutcome::Code(EXIT_FAILURE)),
        BuiltinType::Fg => with_io(ctx, args, state, essential::builtin_fg),
        BuiltinType::Getopts => with_io(ctx, args, state, essential::builtin_getopts),
        BuiltinType::Hash => with_io(ctx, args, state, essential::builtin_hash),
        BuiltinType::Jobs => with_io(ctx, args, state, essential::builtin_jobs),
        BuiltinType::K => with_io(ctx, args, state, fuchsia::builtin_k),
        BuiltinType::List => with_io(ctx, args, state, list::builtin_list),
        BuiltinType::Local => with_io(ctx, args, state, essential::builtin_local),
        BuiltinType::Ls => with_io(ctx, args, state, file_utils::builtin_ls),
        BuiltinType::Mkdir => with_io(ctx, args, state, file_utils::builtin_mkdir),
        BuiltinType::Msleep => with_io(ctx, args, state, msleep::builtin_msleep),
        BuiltinType::Mv => with_io(ctx, args, state, file_utils::builtin_mv),
        BuiltinType::Power => with_io(ctx, args, state, fuchsia::builtin_power),
        BuiltinType::Pwd => with_io(ctx, args, state, essential::builtin_pwd),
        BuiltinType::Printf => with_io(ctx, args, state, printf::builtin_printf),
        BuiltinType::Read => with_io(ctx, args, state, essential::builtin_read),
        BuiltinType::Readonly => with_io(ctx, args, state, essential::builtin_readonly),
        BuiltinType::Return => essential::builtin_return(args, state, ctx),
        BuiltinType::Rm => with_io(ctx, args, state, file_utils::builtin_rm),
        BuiltinType::Set => with_io(ctx, args, state, essential::builtin_set),
        BuiltinType::LeftBracket => with_io(ctx, args, state, test::builtin_left_bracket),
        BuiltinType::Shift => with_io(ctx, args, state, essential::builtin_shift),
        BuiltinType::Test => with_io(ctx, args, state, test::builtin_test),
        BuiltinType::Times => with_io(ctx, args, state, times::builtin_times),
        BuiltinType::Trap => with_io(ctx, args, state, essential::builtin_trap),
        BuiltinType::Type => with_io(ctx, args, state, essential::builtin_type),
        BuiltinType::Ulimit => with_io(ctx, args, state, essential::builtin_ulimit),
        BuiltinType::Umask => with_io(ctx, args, state, essential::builtin_umask),
        BuiltinType::Unalias => with_io(ctx, args, state, essential::builtin_unalias),
        BuiltinType::Unset => with_io(ctx, args, state, essential::builtin_unset),
        BuiltinType::Wait => with_io(ctx, args, state, essential::builtin_wait),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtins_sorted() {
        for i in 1..BUILTINS.len() {
            let name1 = &BUILTINS[i - 1].name[..BUILTINS[i - 1].len as usize];
            let name2 = &BUILTINS[i].name[..BUILTINS[i].len as usize];
            assert!(
                name1 < name2,
                "BUILTINS table is not sorted: {:?} vs {:?}",
                std::str::from_utf8(name1),
                std::str::from_utf8(name2)
            );
        }
    }

    #[test]
    fn test_make_entry_runtime() {
        let name_bytes = vec![b'f', b'o', b'o'];
        let entry = make_entry(&name_bytes, BuiltinType::Alias);
        assert_eq!(entry.len, 3);
        assert_eq!(&entry.name[..3], b"foo");
    }

    #[test]
    #[should_panic(expected = "Builtin name too long")]
    fn test_make_entry_too_long_panics() {
        let long_name = vec![b'a'; 20];
        let _ = make_entry(&long_name, BuiltinType::Alias);
    }
}
