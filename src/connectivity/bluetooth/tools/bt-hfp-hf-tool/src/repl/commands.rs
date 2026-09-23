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

/// Arguments for a command, used by the tab completion hinter.
enum Argument {
    Required(&'static str),
    Optional(&'static str),
}

impl From<&Argument> for String {
    fn from(arg: &Argument) -> String {
        match arg {
            Required(str) => format!("[{str}]"),
            Optional(str) => format!("<{str}>"),
        }
    }
}

/// Macro to generate a command enum and its impl.  A command consists of the
/// command name, arguments, and a help description.  Arguments have type
/// Vec<Argument>, where the order of the arguments in the vector is the order
/// they should be given in; they are used only by the tab completion hinter.
macro_rules! gen_commands {
    ($name:ident {
        $($variant:ident = ($val:expr, $args:expr, $help:expr)),*,
    }) => {
        /// Enum of all possible commands
        #[derive(PartialEq)]
        pub enum $name {
            $($variant),*
        }

        impl $name {
            /// Returns a list of the string representations of all variants
            #[allow(clippy::vec_init_then_push)]
            pub fn variants() -> Vec<String> {
                let mut variants = Vec::new();
                $(variants.push($val.to_string());)*
                variants
            }

            pub fn arguments(&self) -> String {
                match self {
                    $(
                        $name::$variant => {
                            let args: Vec<Argument> = $args;
                            args
                                .iter()
                                .map(Into::into)
                                .collect::<Vec<String>>()
                                .join(" ")
                        }
                    ),*
                }
            }

            /// Help string for a given variant.
            /// The format is "command arg* -- help message".
            pub fn cmd_help(&self) -> String {
                match self {
                    $(
                        $name::$variant => format!("{} {} -- {}", $val, $name::$variant.arguments(), $help)
                    ),*
                }
            }

            /// Multiline help string for `$name` including usage of all variants.
            pub fn help_msg() -> String {
                let mut string = String::from("Commands:\n");
                $(
                    let s = format!("\t{}\n", $name::$variant.cmd_help());
                    string.push_str(&s);
                )*
                string
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

use Argument::*;

// `Command` is the declarative specification of all commands that bt-cli accepts.
gen_commands! {
    Command {
        Help = (
            "help",
            vec![],
            "Print this help message"
        ),
        ListPeers = (
            "list-peers",
            vec![Optional("peer_id")],
            "List known peers with HFP AG support"
        ),
        DisplayNetworkInfo = (
            "display-network-info",
            vec![Required("peer_id")],
            "Request network info from a peer"
        ),
        ListCalls = (
            "list-calls",
            vec![Optional("call_id")],
            "List calls or info about one call"
        ),
        DialFromNumber = (
            "dial-from-number",
            vec![Required("peer_id"), Required("string")],
            "Start a call by dialing a number"
        ),
        DialFromMemoryLocation = (
            "dial-from-memory-location",
            vec![Required("peer_id"), Required("number")],
            "Start a call by dialing from a memory location"
        ),
        RedialLast = (
            "redial-last",
            vec![Required("peer_id")],
            "Start a call by redialing the last number"
        ),
        TransferToHf = (
            "transfer-to-hf",
            vec![Required("peer_id")],
            "Transfer a call from the AG to the HF (This is the transfer_active FIDL call action.)"
        ),
        QueryOperator = (
            "query-operator",
            vec![Required("peer_id")],
            "Query for info about the network carrier"
        ),
        SubsctiberNubmerInfo = (
            "subscriber-number-info",
            vec![Required("peer_id")],
            "Request info about the current phone number"
        ),
        SetNrecMode = (
            "set-nrec-mode",
            vec![Required("peer_id"), Required("bool")],
            "Set the noise reduction and echo mode"
        ),
        ReportHeadsetBatteryLevel = (
            "report-headset-battery-level",
            vec![Required("peer_id"), Required("level")],
            "Report the battery level"
        ),
        RequestHold = (
            "request-hold",
            vec![Required("call_id")],
            "Request the AG put a call on hold"
        ),
        RequestActive = (
            "request-active",
            vec![Required("call_id")],
            "Request the AG set a call as active"
        ),
        RequestTerminate = (
            "request-terminate",
            vec![Required("call_id")],
            "Request the AG terminate a call"
        ),
        RequestTransferToAg = (
            "request-transfer-to-ag",
            vec![Required("call_id")],
            "Request the AG transfer the call to the AG (This is the RequestTransferAudio FIDL method.)"
        ),
        SendDtmfCode = (
            "send-dtmf-code",
            vec![Required("call_id"), Required("code")],
            "Send a DTMF code"
        ),
        GainContol = (
            "gain-control",
            vec![Required("peer_id")],
            "Set the microphone gain"
        ),
        Exit = (
            "exit",
            vec![],
            "Exit the REPL"
        ),
    }
}

/// CommandHelper provides completion, hints, and highlighting for bt-cli
pub struct CommandHelper;

impl CommandHelper {
    pub fn new() -> CommandHelper {
        CommandHelper {}
    }
}

impl Completer for CommandHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        _pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<String>), ReadlineError> {
        let mut variants = Vec::new();
        for variant in Command::variants() {
            if variant.starts_with(line) {
                variants.push(variant)
            }
        }
        Ok((0, variants))
    }
}

impl Hinter for CommandHelper {
    type Hint = String;

    /// CommandHelper provides hints for commands with arguments
    fn hint(&self, line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        let needs_space = !line.ends_with(" ");
        line.trim()
            .parse::<Command>()
            .map(|cmd| format!("{}{}", if needs_space { " " } else { "" }, cmd.arguments(),))
            .ok()
    }
}

impl Highlighter for CommandHelper {
    /// CommandHelper provides highlights for commands with hints
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        if hint.trim().is_empty() {
            Borrowed(hint)
        } else {
            Owned(format!("\x1b[90m{}\x1b[0m", hint))
        }
    }
}

impl Validator for CommandHelper {}

/// CommandHelper can be used as an `Editor` helper for entering input commands
impl Helper for CommandHelper {}

#[cfg(test)]
impl CommandHelper {
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
        assert!(Command::variants().contains(&"list-peers".to_string()));
        assert_eq!(Command::ListPeers.arguments(), "<peer_id>");
        assert_eq!(Command::DialFromNumber.arguments(), "[peer_id] [string]");
        assert!(Command::help_msg().starts_with("Commands:\n"));
    }

    #[test]
    fn test_completer() {
        let cmdhelper = CommandHelper::new();

        assert!(cmdhelper.complete("list-p", 0).unwrap().1.contains(&"list-peers".to_string()));
        assert!(cmdhelper.complete("he", 0).unwrap().1.contains(&"help".to_string()));
        assert!(cmdhelper.complete("dial", 0).unwrap().1.contains(&"dial-from-number".to_string()));
    }
}
