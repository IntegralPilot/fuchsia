// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod args;
mod builtins;
mod collections;
mod errors;
mod eval;
mod fd;
mod parser;
mod path;
mod process;
mod relative;
mod repl;
mod runner;
mod serialization;
mod sort;
mod string;
mod subshell;
mod tty;
use bstr::{BString, ByteSlice};

const SUBSHELL_PAYLOAD: fuchsia_runtime::HandleInfo =
    fuchsia_runtime::HandleInfo::new(fuchsia_runtime::HandleType::User0, 0);

fn main() {
    // This program operates in several modes.
    //
    // - Running a subshell from a handle on startup.
    // - Running a script file passed as an argument.
    // - Running a command passed as an argument.
    // - Running an interactive REPL.
    //
    // We check for the subshell handle first, and if it's present, we run the subshell.
    // Otherwise, we parse the arguments and run the appropriate mode.
    if let Some(handle) = fuchsia_runtime::take_startup_handle(SUBSHELL_PAYLOAD) {
        let vmo = zx::Vmo::from(handle);
        let status = match subshell::run_subshell(vmo) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("Subshell error: {}", e);
                1
            }
        };
        std::process::exit(status);
    }

    let mut args = Vec::new();
    for s in std::env::args_os() {
        use std::os::unix::ffi::OsStrExt;
        args.push(BString::from(s.as_bytes().to_vec()));
    }

    let parsed_args = match args::parse_args(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("zxsh: {}", e);
            std::process::exit(2);
        }
    };

    let state = match eval::ShellState::with_args(
        parsed_args.clone(),
        eval::ShellState::inherited_vars(),
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("zxsh: {}", e);
            std::process::exit(2);
        }
    };

    if let Some(ref cmd) = parsed_args.command {
        let status = runner::run_string(cmd.as_bstr(), state);
        std::process::exit(status);
    }

    if let Some(ref path) = parsed_args.script_name {
        if !parsed_args.stdin {
            let status = runner::run_script(path.as_bstr(), state);
            std::process::exit(status);
        }
    }

    repl::run_repl(state);
}

#[cfg(test)]
mod tests;
