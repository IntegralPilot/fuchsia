// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bstr::{BStr, BString, ByteSlice};

/// An item yielded by `OptionParser`.
#[derive(Debug, PartialEq, Eq)]
pub enum OptionItem<'a> {
    /// A single character flag (e.g., `-f`, `+x`).
    Flag { enable: bool, flag: u8 },

    /// A flag with an associated string value (e.g., `-c "cmd"` or `-cecho`).
    OptArg { enable: bool, flag: u8, value: &'a BStr },
}

/// A zero-allocation parser for POSIX-compatible shell command options.
#[derive(Debug)]
pub struct OptionParser<'a> {
    args: &'a [BString],
    idx: usize,
    char_idx: usize,
    stopped: bool,
    allow_plus: bool,
}

impl<'a> OptionParser<'a> {
    /// Creates a new `OptionParser` over `args`.
    pub fn new(args: &'a [BString]) -> Self {
        Self { args, idx: 0, char_idx: 1, stopped: false, allow_plus: false }
    }

    /// Enables parsing options prefixed with `+` in addition to `-`.
    pub fn allow_plus_options(mut self, allow: bool) -> Self {
        self.allow_plus = allow;
        self
    }

    /// Yields the next option or flag from the argument list.
    pub fn next_option<F>(&mut self, takes_arg: F) -> Option<Result<OptionItem<'a>, String>>
    where
        F: Fn(u8) -> bool,
    {
        if self.stopped || self.idx >= self.args.len() {
            return None;
        }

        let arg = self.args[self.idx].as_bytes();
        let enable = arg.first() == Some(&b'-');

        if self.char_idx == 1 {
            if arg.len() < 2 || (!enable && (!self.allow_plus || arg.first() != Some(&b'+'))) {
                self.stopped = true;
                return None;
            }
            if arg == b"--" {
                self.idx += 1;
                self.stopped = true;
                return None;
            }
        }

        let flag = arg[self.char_idx];
        self.char_idx += 1;

        if takes_arg(flag) {
            let value = if self.char_idx < arg.len() {
                let rem = &arg[self.char_idx..];
                self.idx += 1;
                self.char_idx = 1;
                BStr::new(rem)
            } else if self.idx + 1 < self.args.len() {
                self.idx += 2;
                self.char_idx = 1;
                self.args[self.idx - 1].as_bstr()
            } else {
                let opt_prefix = if enable { "-" } else { "+" };
                return Some(Err(format!("{}{} requires an argument", opt_prefix, flag as char)));
            };
            return Some(Ok(OptionItem::OptArg { enable, flag, value }));
        }

        if self.char_idx >= arg.len() {
            self.idx += 1;
            self.char_idx = 1;
        }

        Some(Ok(OptionItem::Flag { enable, flag }))
    }

    /// Returns the remaining unparsed positional arguments.
    pub fn rest(&self) -> &'a [BString] {
        if self.idx < self.args.len() { &self.args[self.idx..] } else { &[] }
    }
}

/// Parsed command line arguments for the shell.
#[derive(Default, Debug, Clone)]
pub struct Args {
    /// Command to execute. If set, the shell executes this command and exits.
    /// Maps to the `-c` command-line option.
    pub command: Option<BString>,

    /// Read commands from the standard input.
    /// Maps to the `-s` command-line option, or triggers if no script name is provided
    /// and standard input is not a TTY.
    pub stdin: bool,

    /// Run as a login shell.
    /// Maps to the `-l` command-line option.
    pub login: bool,

    /// Shell options to enable.
    /// Maps to the `-o option_name` command-line option.
    pub options_to_set: Vec<BString>,

    /// Shell options to disable.
    /// Maps to the `+o option_name` command-line option.
    pub options_to_clear: Vec<BString>,

    // Single letter flags (can be set with `-` or cleared with `+`)
    /// Automatically export all variables that are defined or modified.
    /// Maps to the `-a` / `+a` option.
    pub opt_allexport: Option<bool>,

    /// Report the status of terminated background jobs immediately.
    /// Maps to the `-b` / `+b` option.
    pub opt_notify: Option<bool>,

    /// Prevent existing files from being overwritten by redirection.
    /// Maps to the `-C` / `+C` option.
    pub opt_noclobber: Option<bool>,

    /// Exit immediately if a command exits with a non-zero status.
    /// Maps to the `-e` / `+e` option.
    pub opt_errexit: Option<bool>,

    /// Disable pathname expansion (globbing).
    /// Maps to the `-f` / `+f` option.
    pub opt_noglob: Option<bool>,

    /// Force the shell to run interactively.
    /// Maps to the `-i` command-line option.
    pub opt_interactive: bool,

    /// Ignore EOF (Ctrl-D) when reading from stdin in an interactive shell.
    /// Maps to the `-I` / `+I` option.
    pub opt_ignoreeof: Option<bool>,

    /// Enable job control.
    /// Maps to the `-m` / `+m` option.
    pub opt_monitor: Option<bool>,

    /// Read commands but do not execute them. Useful for syntax checking.
    /// Maps to the `-n` / `+n` option.
    pub opt_noexec: Option<bool>,

    /// Print commands and their arguments as they are executed.
    /// Maps to the `-x` / `+x` option (xtrace).
    pub opt_xtrace: Option<bool>,

    /// Print shell input lines as they are read.
    /// Maps to the `-v` / `+v` option.
    pub opt_verbose: Option<bool>,

    /// Enable vi-style line editing.
    /// Maps to the `-V` / `+V` option.
    pub opt_vi: Option<bool>,

    /// Enable emacs-style line editing.
    /// Maps to the `-E` / `+E` option.
    pub opt_emacs: Option<bool>,

    /// Treat unset variables as an error when performing parameter expansion.
    /// Maps to the `-u` / `+u` option.
    pub opt_nounset: Option<bool>,

    /// The name of the script to run, if not running a command via `-c`.
    /// This becomes `$0` in the script.
    pub script_name: Option<BString>,

    /// Positional arguments passed to the script or command.
    /// These become `$1`, `$2`, etc.
    pub positional_args: Vec<BString>,
}

/// Parses the command line arguments into an `Args` structure.
///
/// POSIX shell argument parsing rules are followed:
/// - Arguments starting with `-` set options, while arguments starting with `+` clear them.
/// - Options can be grouped (e.g. `-xive`).
/// - The `-c` and `-o` options require arguments. They can be attached (e.g. `-cecho`)
///   or detached (e.g. `-c "echo"`).
/// - `--` marks the end of options. Subsequent arguments are treated as positionals.
/// - `-` forces reading from stdin and ends option parsing.
pub fn parse_args(args: &[BString]) -> Result<Args, String> {
    let mut result = Args::default();
    let slice = if !args.is_empty() { &args[1..] } else { args };
    let mut parser = OptionParser::new(slice).allow_plus_options(true);

    while let Some(opt_res) = parser.next_option(|flag| flag == b'c' || flag == b'o') {
        let item = opt_res?;
        match item {
            OptionItem::OptArg { enable: true, flag: b'c', value } => {
                result.command = Some(BString::from(value));
            }
            OptionItem::OptArg { enable: false, flag: b'c', .. } => {
                return Err("cannot unset -c".to_string());
            }
            OptionItem::OptArg { enable, flag: b'o', value } => {
                if enable {
                    result.options_to_set.push(BString::from(value));
                } else {
                    result.options_to_clear.push(BString::from(value));
                }
            }
            OptionItem::Flag { enable, flag: b'a' } => result.opt_allexport = Some(enable),
            OptionItem::Flag { enable, flag: b'b' } => result.opt_notify = Some(enable),
            OptionItem::Flag { enable, flag: b'C' } => result.opt_noclobber = Some(enable),
            OptionItem::Flag { enable, flag: b'e' } => result.opt_errexit = Some(enable),
            OptionItem::Flag { enable, flag: b'f' } => result.opt_noglob = Some(enable),
            OptionItem::Flag { enable, flag: b'I' } => result.opt_ignoreeof = Some(enable),
            OptionItem::Flag { enable, flag: b'i' } => result.opt_interactive = enable,
            OptionItem::Flag { enable, flag: b'm' } => result.opt_monitor = Some(enable),
            OptionItem::Flag { enable, flag: b'n' } => result.opt_noexec = Some(enable),
            OptionItem::Flag { enable, flag: b's' } => result.stdin = enable,
            OptionItem::Flag { enable, flag: b'x' } => result.opt_xtrace = Some(enable),
            OptionItem::Flag { enable, flag: b'v' } => result.opt_verbose = Some(enable),
            OptionItem::Flag { enable, flag: b'V' } => result.opt_vi = Some(enable),
            OptionItem::Flag { enable, flag: b'E' } => result.opt_emacs = Some(enable),
            OptionItem::Flag { enable, flag: b'u' } => result.opt_nounset = Some(enable),
            OptionItem::Flag { enable, flag: b'l' } => result.login = enable,
            OptionItem::Flag { enable, flag } => {
                let opt_prefix = if enable { "-" } else { "+" };
                return Err(format!("unknown option: {}{}", opt_prefix, flag as char));
            }
            _ => unreachable!(),
        }
    }

    let mut pos = parser.rest();
    if !pos.is_empty() && pos[0] == "-" {
        result.stdin = true;
        pos = &pos[1..];
    }

    if !pos.is_empty() {
        if result.command.is_some() {
            result.script_name = Some(pos[0].clone());
            result.positional_args = pos[1..].to_vec();
        } else if result.stdin {
            result.positional_args = pos.to_vec();
        } else {
            result.script_name = Some(pos[0].clone());
            result.positional_args = pos[1..].to_vec();
        }
    }

    Ok(result)
}

#[cfg(test)]
impl Args {
    /// Creates an `Args` structure with only positional arguments.
    /// Useful for testing and programmatic shell execution.
    pub fn with_positionals(script_name: BString, positional_args: Vec<BString>) -> Self {
        Self { script_name: Some(script_name), positional_args, ..Default::default() }
    }
}
