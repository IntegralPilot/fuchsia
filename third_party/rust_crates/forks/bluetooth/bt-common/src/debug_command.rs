// Copyright 2023 Google LLC
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! Debug command traits and helpers for defining commands for integration
/// into a debug tool.
use std::str::FromStr;

/// A CommandSet is a set of commands (usually an enum) that each represent an
/// action that can be performed.  i.e. 'list', 'volume' etc.  Each command can
/// take zero or more arguments and have zero or more flags.
/// Typically an Enum of commands would implement CommandSet trait.
pub trait CommandSet: FromStr + ::core::fmt::Display {
    /// Returns a vector of strings that are the commands supported by this.
    fn variants() -> Vec<String>;

    /// Returns a string listing the arguments that this command takes, in <>
    /// brackets
    fn arguments(&self) -> &'static str;

    /// Returns a string displaying the flags that this command supports, in []
    /// brackets
    fn flags(&self) -> &'static str;

    /// Returns a short description of this command
    fn desc(&self) -> &'static str;

    /// Help string for this variant (build from Display, arguments and flags by
    /// default)
    fn help_simple(&self) -> String {
        format!("{self} {} {} -- {}", self.flags(), self.arguments(), self.desc())
    }

    /// Possibly multi-line help string for all variants of this set.
    fn help_all() -> String {
        Self::variants()
            .into_iter()
            .filter_map(|s| FromStr::from_str(&s).ok())
            .map(|s: Self| format!("{}\n", s))
            .collect()
    }
}

/// Macro to help build CommandSets
#[macro_export]
macro_rules! gen_commandset {
    ($name:ident {
        $($variant:ident = ($val:expr, [$($flag:expr),*], [$($arg:expr),*], $help:expr)),*,
    }) => {
        /// Enum of all possible commands
        #[derive(PartialEq, Debug)]
        pub enum $name {
            $($variant),*
        }

        impl CommandSet for $name {
            fn variants() -> Vec<String> {
                let mut variants = Vec::new();
                $(variants.push($val.to_string());)*
                variants
            }

            fn arguments(&self) -> &'static str {
                match self {
                    $(
                        $name::$variant => concat!($("<", $arg, "> ",)*)
                    ),*
                }
            }

            fn flags(&self) -> &'static str {
                match self {
                    $(
                        $name::$variant => concat!($("[", $flag, "] ",)*)
                    ),*
                }
            }

            fn desc(&self) -> &'static str {
                match self {
                    $(
                        $name::$variant => $help
                    ),*
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match *self {
                    $($name::$variant => write!(f, $val)),* ,
                }
            }
        }

        impl ::std::str::FromStr for $name {
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

/// CommandRunner is used to perform a specific task based on the
pub trait CommandRunner {
    type Set: CommandSet;
    fn run(
        &self,
        cmd: Self::Set,
        args: Vec<String>,
    ) -> impl futures::Future<Output = Result<(), impl ::std::error::Error>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_commandset_simple() {
        gen_commandset! {
            TestCmd {
                One = ("one", [], [], "First Command"),
                WithFlags = ("with-flags", ["-1","-2"], [], "Command with flags"),
                WithArgs = ("with-args", [], ["arg", "two"], "Command with args"),
                WithBoth = ("with-both", ["-w"], ["simple"], "Command with both flags and args"),
            }
        }

        let cmd: TestCmd = "one".parse().unwrap();

        assert_eq!(cmd, TestCmd::One);

        let cmd2: TestCmd = "with-flags".parse().unwrap();

        assert_eq!(cmd2.arguments(), "");
        assert_eq!(cmd2.flags(), "[-1] [-2] ");
    }
}
