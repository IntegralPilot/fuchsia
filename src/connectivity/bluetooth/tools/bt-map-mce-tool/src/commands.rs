// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

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
/// A command consists of the command name, arguments, and a help description.
macro_rules! gen_commands {
    ($name:ident {
        $($variant:ident = ($val:expr, [$($arg:expr),*], $help:expr)),*,
    }) => {
        /// Enum of all possible commands
        #[derive(PartialEq)]
        pub enum $name {
            $($variant),*
        }

        impl $name {
            /// Returns a list of the string representations of all variants
            pub fn variants() -> Vec<String> {
                vec![$($val.to_string(),)*]
            }

            pub fn arguments(&self) -> &'static str {
                match self {
                    $(
                        $name::$variant => concat!($("<", $arg, "> ",)*)
                    ),*
                }
            }

            /// Help string for a given variant.
            /// The format is "command [flag].. <arg>.. -- help message".
            pub fn cmd_help(&self) -> &'static str {
                match self {
                    $(
                        $name::$variant =>
                            concat!($val, " ", $("<", $arg, "> ",)* "-- ", $help)
                    ),*
                }
            }

            /// Multiline help string for `$name` including usage of all variants.
            pub fn help_msg() -> &'static str {
                concat!("Commands:\n", $(
                    "\t", $val, " ", $("<", $arg, "> ",)* "-- ", $help, "\n"
                ),*)
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

// `Cmd` is the declarative specification of all commands that bt-cli accepts.
gen_commands! {
    Cmd {
        ListAllMasInstances = ("list", [], "lists all the message access service instances available at this remote MSE peer"),
        TurnOnNotifications = ("turn-on-notifications", ["[repo_uid]"], "registers to receive notifications from the specified repo. If `repo_uid` isn't specified, will receive notifications from all known MAS instances"),
        TurnOffNotifications = ("turn-off-notifications", [], "unregister for notifications from the connected remote MSE peer"),
        Help = ("help", [], "This message"),
        Exit = ("exit", [], "Close REPL"),
    }
}

/// CmdHelper provides completion, hints, and highlighting for bt-map-mce-tool
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
        let mut variants = Vec::new();
        for variant in Cmd::variants() {
            if variant.starts_with(line) {
                variants.push(variant)
            }
        }
        Ok((0, variants))
    }
}

impl Hinter for CmdHelper {
    type Hint = String;

    /// CmdHelper provides hints for commands with arguments
    fn hint(&self, line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        let needs_space = !line.ends_with(" ");
        line.trim()
            .parse::<Cmd>()
            .map(|cmd| format!("{}{}", if needs_space { " " } else { "" }, cmd.arguments()))
            .ok()
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
    fn test_gen_commands_macro() {
        assert!(Cmd::variants().contains(&"turn-on-notifications".to_string()));
        assert_eq!(Cmd::ListAllMasInstances.arguments(), "");
        assert_eq!(Cmd::Help.arguments(), "");
        assert_eq!(Cmd::Exit.arguments(), "");
        assert!(Cmd::help_msg().starts_with("Commands:\n"));
    }

    #[test]
    fn test_completer() {
        let cmdhelper = CmdHelper::new();

        assert!(cmdhelper.complete("li", 0).unwrap().1.contains(&"list".to_string()));
        assert!(cmdhelper.complete("he", 0).unwrap().1.contains(&"help".to_string()));
        assert!(cmdhelper.complete("ex", 0).unwrap().1.contains(&"exit".to_string()));
    }
}
