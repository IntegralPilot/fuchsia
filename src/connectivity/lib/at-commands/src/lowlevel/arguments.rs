// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module contains an AST for arguments for both AT commands and responses.
//!
//! The format of of these is not specifed in any one place in the spec, but they are
//! described thoughout HFP 1.8.

use crate::lowlevel::write_to::WriteTo;
use std::io;

/// An argument list set off from a command or response by a delimiter such as "=" or ":".
#[derive(Debug, Clone, PartialEq)]
pub struct DelimitedArguments {
    /// A string setting off arguments from the command or response.  This is normally `=`
    /// or ": ", but could be ">" or absent. This latter is currently only in a variants
    /// of  the `ATD` command, specified in HFP v1.8 4.19.
    pub delimiter: Option<String>,
    /// The actual arguments to the execute commmand.
    pub arguments: Arguments,
    /// An optional terminator for the arguments to the command, such as the ";" used to
    /// terminate ATD commands.
    pub terminator: Option<String>,
}

impl WriteTo for DelimitedArguments {
    fn write_to<W: io::Write>(&self, sink: &mut W) -> io::Result<()> {
        let DelimitedArguments { delimiter, arguments, terminator } = self;
        if let Some(string) = delimiter {
            if string == ":" {
                // Hack.  The parser ignores whitespace, but the colon in a response must be followed by a space, so special case this.
                sink.write_all(": ".as_bytes())?;
            } else {
                sink.write_all(string.as_bytes())?;
            }
        };
        arguments.write_to(sink)?;
        if let Some(terminator) = terminator {
            sink.write_all(terminator.as_bytes())?
        };

        Ok(())
    }
}
/// The collection of arguments to a given command or response.
///
/// AT supports multiple different formats, represented here by the different enum
/// branches.
#[derive(Debug, Clone, PartialEq)]
pub enum Arguments {
    /// A sequence of multiple arguments lists delimited by parentheses, like, `(1,2)(3,4)(a=1)`
    ParenthesisDelimitedArgumentLists(Vec<Vec<Argument>>),
    /// A single argument list delimited by commas, like, `1,2,a=3`
    ArgumentList(Vec<Argument>),
}

impl Arguments {
    fn write_comma_delimited_argument_list<W: io::Write>(
        &self,
        arguments: &[Argument],
        sink: &mut W,
    ) -> io::Result<()> {
        if !arguments.is_empty() {
            for argument in &arguments[0..arguments.len() - 1] {
                argument.write_to(sink)?;
                sink.write_all(b",")?;
            }
            arguments[arguments.len() - 1].write_to(sink)?;
        }

        Ok(())
    }

    fn write_paren_delimited_argument_lists<W: io::Write>(
        &self,
        argument_lists: &[Vec<Argument>],
        sink: &mut W,
    ) -> io::Result<()> {
        for arguments in argument_lists {
            sink.write_all(b"(")?;
            self.write_comma_delimited_argument_list(arguments, sink)?;
            sink.write_all(b")")?;
        }

        Ok(())
    }
}

impl WriteTo for Arguments {
    fn write_to<W: io::Write>(&self, sink: &mut W) -> io::Result<()> {
        match self {
            Arguments::ParenthesisDelimitedArgumentLists(argument_lists) => {
                self.write_paren_delimited_argument_lists(&argument_lists, sink)
            }
            Arguments::ArgumentList(argument_list) => {
                self.write_comma_delimited_argument_list(&argument_list, sink)
            }
        }
    }
}

/// An individual argument in a list.
#[derive(Debug, Clone, PartialEq)]
pub enum Argument {
    /// A primitive string or int.
    PrimitiveArgument(String),
    /// A key-value pair like `a=1`
    KeyValueArgument { key: String, value: String },
}

impl Argument {
    pub fn is_empty(&self) -> bool {
        match self {
            Argument::PrimitiveArgument(argument) => argument.is_empty(),
            Argument::KeyValueArgument { key, value } => key.is_empty() && value.is_empty(),
        }
    }
}

impl WriteTo for Argument {
    fn write_to<W: io::Write>(&self, sink: &mut W) -> io::Result<()> {
        match self {
            Argument::PrimitiveArgument(argument) => write_string(sink, argument)?,
            Argument::KeyValueArgument { key, value } => {
                write_string(sink, key)?;
                sink.write_all(b"=")?;
                write_string(sink, value)?;
            }
        }
        Ok(())
    }
}

/// Writes a string, converting it to bytes first. Rejects any string containing
/// control characters (except tab).
fn write_string<W: io::Write>(sink: &mut W, string: &String) -> io::Result<()> {
    if string.chars().any(|c| c.is_control() && c != '\t') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "AT command string argument contains control characters",
        ));
    }
    sink.write_all(string.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lowlevel::write_to::WriteTo;

    #[test]
    fn write_string_rejects_various_control_characters() {
        let mut sink = Vec::new();
        let malicious_payloads = vec![
            "123;\rAT+CMGD=1",
            "123\nAT+CHUP",
            "123\0abc",
            "123\x1B[2Jabc", // ESC
            "abc\x7Fdef",    // DEL
            "abc\x08def",    // Backspace
            "abc\x0Cdef",    // Form feed
        ];

        for payload in malicious_payloads {
            let arg = Argument::PrimitiveArgument(String::from(payload));
            assert!(
                arg.write_to(&mut sink).is_err(),
                "Must reject argument with control characters: {:?}",
                payload
            );
        }
    }

    #[test]
    fn write_key_value_argument_rejects_control_characters_in_key_and_value() {
        let mut sink = Vec::new();

        // Malicious key
        let kv_bad_key = Argument::KeyValueArgument {
            key: String::from("KEY\rINJECT"),
            value: String::from("VAL"),
        };
        assert!(kv_bad_key.write_to(&mut sink).is_err());

        // Malicious value
        let kv_bad_val = Argument::KeyValueArgument {
            key: String::from("KEY"),
            value: String::from("VAL\rINJECT"),
        };
        assert!(kv_bad_val.write_to(&mut sink).is_err());

        // Both malicious
        let kv_bad_both =
            Argument::KeyValueArgument { key: String::from("KEY\n"), value: String::from("VAL\0") };
        assert!(kv_bad_both.write_to(&mut sink).is_err());
    }

    #[test]
    fn write_arguments_list_rejects_control_characters_in_any_element() {
        let mut sink = Vec::new();

        let args_list = Arguments::ArgumentList(vec![
            Argument::PrimitiveArgument(String::from("\"valid1\"")),
            Argument::PrimitiveArgument(String::from("123;\rAT+CMGD=1")),
            Argument::PrimitiveArgument(String::from("\"valid2\"")),
        ]);

        assert!(args_list.write_to(&mut sink).is_err());
    }

    #[test]
    fn write_parenthesis_delimited_argument_lists_rejects_control_characters() {
        let mut sink = Vec::new();

        let paren_list = Arguments::ParenthesisDelimitedArgumentLists(vec![
            vec![Argument::PrimitiveArgument(String::from("1"))],
            vec![Argument::PrimitiveArgument(String::from("2\rAT+CHUP"))],
        ]);

        assert!(paren_list.write_to(&mut sink).is_err());
    }

    #[test]
    fn write_string_accepts_valid_edge_case_strings() {
        let mut sink = Vec::new();

        let arg_empty = Argument::PrimitiveArgument(String::from(""));
        assert!(arg_empty.write_to(&mut sink).is_ok());
        assert_eq!(sink, b"");

        sink.clear();
        let arg_quotes = Argument::PrimitiveArgument(String::from("\"\""));
        assert!(arg_quotes.write_to(&mut sink).is_ok());
        assert_eq!(sink, b"\"\"");

        sink.clear();
        let arg_symbols = Argument::PrimitiveArgument(String::from("\"+1-800-555-0199;#*>\""));
        assert!(arg_symbols.write_to(&mut sink).is_ok());
        assert_eq!(sink, b"\"+1-800-555-0199;#*>\"");

        sink.clear();
        let arg_tab = Argument::PrimitiveArgument(String::from("foo\tbar"));
        assert!(arg_tab.write_to(&mut sink).is_ok());
        assert_eq!(sink, b"foo\tbar");
    }
}
