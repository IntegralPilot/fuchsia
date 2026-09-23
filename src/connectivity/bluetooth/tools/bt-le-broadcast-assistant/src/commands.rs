// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_broadcast_assistant::debug::AssistantCmd;
use bt_common::debug_command::CommandSet;
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};
use std::borrow::Cow::{self, Borrowed, Owned};
use std::fmt;
use std::str::FromStr;

/// Macro to generate a command enum and its impl.
/// A command consists of the command name, optional flags, arguments, and a help description.
macro_rules! gen_commands {
    ($name:ident {
        $($variant:ident = ($val:expr, [$($flag:expr),*], [$($arg:expr),*], $help:expr)),*,
    }) => {
        /// Enum of all possible commands
        #[derive(Debug, PartialEq)]
        pub enum $name {
            $($variant),*
        }

        impl $name {
            /// Returns a list of the string representations of all variants
            pub fn variants() -> Vec<String> {
                vec![$($val.to_string()),*]
            }

            pub fn arguments(&self) -> &'static str {
                match self {
                    $(
                        $name::$variant => concat!($("<", $arg, "> ",)*)
                    ),*
                }
            }

            pub fn flags(&self) -> &'static str {
                match self {
                    $(
                        $name::$variant => concat!($("[", $flag, "] ",)*)
                    ),*
                }
            }

            /// Multiline help string for `$name` including usage of all variants.
            pub fn help_msg() -> &'static str {
                concat!("Commands:\n", $(
                    "\t", $val, " ", $("[", $flag, "] ",)* $("<", $arg, "> ",)* "-- ", $help, "\n"
                ),*)
            }

            /// Simple one-line usage string for a command.
            pub fn help_simple(&self) -> String {
                format!("{} {} {}", self.to_string(), self.flags(), self.arguments())
            }

        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match *self {
                    $($name::$variant => write!(f, $val)),* ,
                }
            }
        }

        impl FromStr for $name {
            type Err = ();

            fn from_str(s: &str) -> Result<$name, ()> {
                match s {
                    $($val => Ok($name::$variant)),* ,
                    _ => Err(()),
                }
            }
        }

    }
}

// `Cmd` is the declarative specification of all commands that bt-le-broadcast-assistant accepts.
gen_commands! {
    Cmd {
        Help = ("help", [], [], "Print this help message"),
        Scan = ("scan", [], ["[duration]"], "Scan for scan delegators for a specified duration in seconds (default is 10)"),
        SetPeerAddr = ("set-peer-addr", [], ["peer_id", "address", "Public|Random"], "Set the address for a given peer ID (only for --use-static-address mode)."),
        Exit = ("exit", [], [], "Quit and disconnect"),
    }
}

/// CmdHelper provides completion, hints, and highlighting for bt-cli
pub struct CmdHelper;

impl CmdHelper {
    pub fn new() -> CmdHelper {
        CmdHelper {}
    }
}

impl Completer for CmdHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        _pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<String>), ReadlineError> {
        let mut variants = Cmd::variants();
        variants.extend(AssistantCmd::variants());
        let mut unique_variants: Vec<String> = variants.into_iter().collect();
        unique_variants.sort();
        unique_variants.dedup();

        let mut matching_variants = Vec::new();
        for variant in unique_variants {
            if variant.starts_with(line) {
                matching_variants.push(variant)
            }
        }
        Ok((0, matching_variants))
    }
}

impl Hinter for CmdHelper {
    type Hint = String;

    /// CmdHelper provides hints for commands with arguments
    fn hint(&self, line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        let needs_space = !line.ends_with(' ');
        let trimmed_line = line.trim();

        let get_hint = |flags: &str, args: &str| {
            if flags.is_empty() && args.is_empty() {
                return None;
            }
            Some(format!("{}{}{}", if needs_space { " " } else { "" }, flags, args))
        };

        // First, try to parse as a local `Cmd`.
        if let Ok(cmd) = trimmed_line.parse::<Cmd>() {
            return get_hint(cmd.flags(), cmd.arguments());
        }

        // If that fails, try to parse as an `AssistantCmd`.
        if let Ok(cmd) = trimmed_line.parse::<AssistantCmd>() {
            return get_hint(cmd.flags(), cmd.arguments());
        }

        None
    }
}

impl Highlighter for CmdHelper {
    /// CmdHelper provides highlights for commands with hints
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        if hint.trim().is_empty() {
            Borrowed(hint)
        } else {
            Owned(format!("\x1b[90m{}\x1b[0m", hint))
        }
    }
}

impl Validator for CmdHelper {}

/// CmdHelper can be used as an `Editor` helper for entering input commands
impl Helper for CmdHelper {}

#[cfg(test)]
impl CmdHelper {
    fn complete(&self, line: &str, pos: usize) -> Result<(usize, Vec<String>), ReadlineError> {
        let history = rustyline::history::DefaultHistory::new();
        let ctx = Context::new(&history);
        Completer::complete(self, line, pos, &ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmd_variants() {
        let variants = Cmd::variants();
        assert!(variants.contains(&"help".to_string()));
        assert!(variants.contains(&"exit".to_string()));
    }

    #[test]
    fn test_cmd_from_str() {
        assert_eq!(Cmd::from_str("help"), Ok(Cmd::Help));
        assert_eq!(Cmd::from_str("exit"), Ok(Cmd::Exit));
        assert_eq!(Cmd::from_str("invalid"), Err(()));
    }

    #[test]
    fn test_cmd_help_msg() {
        assert!(!Cmd::help_msg().is_empty());
    }

    #[test]
    fn test_cmd_helper_completer() {
        let helper = CmdHelper::new();

        // Test completion for a local command
        let (_pos, candidates) = helper.complete("he", 2).unwrap();
        assert_eq!(candidates, vec!["help".to_string()]);

        // Test completion for an assistant command
        let (_pos, candidates) = helper.complete("conn", 4).unwrap();
        assert_eq!(candidates, vec!["connect".to_string()]);

        // Test completion for a shared prefix "set"
        let (_pos, candidates) = helper.complete("set", 3).unwrap();
        // The implementation sorts the combined list
        assert_eq!(candidates, vec!["set-broadcast-code".to_string(), "set-peer-addr".to_string()]);

        // Test completion for an unknown command
        let (_pos, candidates) = helper.complete("foo", 3).unwrap();
        assert!(candidates.is_empty());
    }
}
