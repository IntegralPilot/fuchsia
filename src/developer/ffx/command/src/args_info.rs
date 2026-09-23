// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::CommandInfoWithArgs;
use serde::{Deserialize, Serialize};

/// These help with deserializing and merging the command line
/// argument information from argh::ArgsInfo in order to
/// support JSON encoded help output.

#[derive(Clone, Default, Debug, Deserialize, Serialize)]
pub struct CliArgsInfo {
    /// The name of the command.
    pub name: String,
    /// A short description of the command's functionality.
    pub description: String,
    /// Examples of usage
    pub examples: Vec<String>,
    /// Flags
    pub flags: Vec<FlagInfo>,
    /// Notes about usage
    pub notes: Vec<String>,
    /// The subcommands.
    pub commands: Vec<SubCommandInfo>,
    /// Positional args
    pub positionals: Vec<PositionalInfo>,
    /// Error code information
    pub error_codes: Vec<ErrorCodeInfo>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub enum FlagKind {
    Option {
        arg_name: String,
    },
    #[default]
    Switch,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Optionality {
    Required,
    #[default]
    Optional,
    Repeating,
    Greedy,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FlagInfo {
    pub kind: FlagKind,
    /// The optionality of the flag.
    pub optionality: Optionality,
    /// The long string of the flag.
    pub long: String,
    /// The single character short indicator
    /// for trhis flag.
    pub short: Option<char>,
    /// The description of the flag.
    pub description: String,
    /// Visibility in the help for this argument.
    /// `false` indicates this argument will not appear
    /// in the help message.
    pub hidden: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct PositionalInfo {
    /// Name of the argument.
    pub name: String,
    /// Description of the argument.
    pub description: String,
    /// Optionality of the argument.
    pub optionality: Optionality,
    /// Visibility in the help for this argument.
    /// `false` indicates this argument will not appear
    /// in the help message.
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SubCommandInfo {
    /// The subcommand name.
    pub name: String,
    /// The information about the subcommand.
    pub command: CliArgsInfo,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ErrorCodeInfo {
    /// The code value.
    pub code: i32,
    /// Short description about what this code indicates.
    pub description: String,
}

impl From<CommandInfoWithArgs> for CliArgsInfo {
    fn from(value: CommandInfoWithArgs) -> Self {
        CliArgsInfo {
            name: value.name.into(),
            description: value.description.into(),
            examples: value.examples.iter().map(|s| s.to_string()).collect(),
            flags: value.flags.iter().map(|f| f.into()).collect(),
            notes: value.notes.iter().map(|s| s.to_string()).collect(),
            commands: value.commands.iter().map(|c| c.into()).collect(),
            positionals: value.positionals.iter().map(|p| p.into()).collect(),
            error_codes: value.error_codes.iter().map(|e| e.into()).collect(),
        }
    }
}

impl From<&argh::ErrorCodeInfo<'_>> for ErrorCodeInfo {
    fn from(value: &argh::ErrorCodeInfo<'_>) -> Self {
        Self { code: value.code, description: value.description.into() }
    }
}

impl From<&argh::FlagInfoKind<'_>> for FlagKind {
    fn from(value: &argh::FlagInfoKind<'_>) -> Self {
        match value {
            argh::FlagInfoKind::Switch => FlagKind::Switch,
            argh::FlagInfoKind::Option { arg_name } => {
                FlagKind::Option { arg_name: arg_name.to_string() }
            }
        }
    }
}

impl From<&argh::Optionality> for Optionality {
    fn from(value: &argh::Optionality) -> Self {
        match value {
            argh::Optionality::Required => Optionality::Required,
            argh::Optionality::Optional => Optionality::Optional,
            argh::Optionality::Repeating => Optionality::Repeating,
            argh::Optionality::Greedy => Optionality::Greedy,
        }
    }
}

impl From<&argh::FlagInfo<'_>> for FlagInfo {
    fn from(value: &argh::FlagInfo<'_>) -> Self {
        FlagInfo {
            kind: (&value.kind).into(),
            optionality: (&value.optionality).into(),
            long: value.long.into(),
            short: value.short,
            description: value.description.into(),
            hidden: value.hidden,
        }
    }
}

impl From<&argh::PositionalInfo<'_>> for PositionalInfo {
    fn from(value: &argh::PositionalInfo<'_>) -> Self {
        PositionalInfo {
            name: value.name.into(),
            description: value.description.into(),
            optionality: (&value.optionality).into(),
            hidden: value.hidden,
        }
    }
}

impl From<&argh::SubCommandInfo> for SubCommandInfo {
    fn from(value: &argh::SubCommandInfo) -> Self {
        SubCommandInfo { name: value.name.into(), command: value.command.clone().into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flagkind_conversion() {
        let test_data = vec![
            (argh::FlagInfoKind::Switch, FlagKind::Switch),
            (
                argh::FlagInfoKind::Option { arg_name: "option_name" },
                FlagKind::Option { arg_name: "option_name".to_string() },
            ),
        ];

        for (orig, converted) in test_data {
            let actual: FlagKind = (&orig).into();
            assert_eq!(converted, actual);
        }
    }

    #[test]
    fn test_optionality_conversion() {
        let test_data = vec![(argh::Optionality::Required, Optionality::Required)];

        for (orig, converted) in test_data {
            let actual: Optionality = (&orig).into();
            assert_eq!(converted, actual);
        }
    }

    #[test]
    fn test_positional_conversion() {
        const TEST_NAME: &str = "test_name";
        const TEST_DESCRIPTION: &str = "test description";

        let orig = argh::PositionalInfo {
            name: TEST_NAME,
            description: TEST_DESCRIPTION,
            optionality: argh::Optionality::Optional,
            hidden: false,
        };

        let converted: PositionalInfo = (&orig).into();
        assert_eq!(TEST_NAME, converted.name);
        assert_eq!(TEST_DESCRIPTION, converted.description);
        assert_eq!(Optionality::Optional, converted.optionality);
        assert_eq!(false, converted.hidden);
    }

    #[test]
    fn test_errorcode_conversion() {
        const TEST_DESCRIPTION: &str = "test description";

        let orig = argh::ErrorCodeInfo { code: 1000, description: TEST_DESCRIPTION };

        let converted: ErrorCodeInfo = (&orig).into();
        assert_eq!(1000, converted.code);
        assert_eq!(TEST_DESCRIPTION, converted.description);
    }

    #[test]
    fn test_flaginfo_conversion() {
        const LONG_OPTION: &str = "--test-option";
        const TEST_DESCRIPTION: &str = "test description";

        let orig = argh::FlagInfo {
            kind: argh::FlagInfoKind::Option { arg_name: "test_option" },
            optionality: argh::Optionality::Required,
            long: LONG_OPTION,
            short: Some('t'),
            description: TEST_DESCRIPTION,
            hidden: true,
        };

        let converted: FlagInfo = (&orig).into();
        assert_eq!(FlagKind::Option { arg_name: "test_option".to_string() }, converted.kind);
        assert_eq!(Optionality::Required, converted.optionality);
        assert_eq!(LONG_OPTION, converted.long);
        assert_eq!(Some('t'), converted.short);
        assert_eq!(TEST_DESCRIPTION, converted.description);
        assert_eq!(true, converted.hidden);
    }

    #[test]
    fn test_convert_empty_cli() {
        const TEST_NAME: &str = "test_name";

        let orig = argh::CommandInfoWithArgs {
            name: TEST_NAME,
            description: "",
            short: &'\0',
            examples: &[],
            flags: &[],
            notes: &[],
            commands: vec![],
            positionals: &[],
            error_codes: &[],
        };
        let converted: CliArgsInfo = orig.into();
        assert_eq!(TEST_NAME, converted.name);
        assert_eq!("", converted.description);
        assert!(converted.examples.is_empty());
    }

    #[test]
    fn test_convert_cli_no_subcommands() {
        const TEST_NAME: &str = "test_name";
        const TEST_DESCRIPTION: &str = "test description";
        const TEST_EXAMPLES: [&str; 2] = ["ex1", "ex2"];
        const TEST_NOTES: [&str; 2] = ["note1", "note2"];
        const POS1: argh::PositionalInfo<'_> = argh::PositionalInfo {
            name: "pos1",
            description: "positional arg 1",
            optionality: argh::Optionality::Optional,
            hidden: false,
        };
        const ERR1: argh::ErrorCodeInfo<'_> =
            argh::ErrorCodeInfo { code: 123, description: "Some error code" };
        let orig = argh::CommandInfoWithArgs {
            name: TEST_NAME,
            short: &'\0',
            description: TEST_DESCRIPTION,
            examples: &TEST_EXAMPLES,
            flags: &[],
            notes: &TEST_NOTES,
            commands: vec![],
            positionals: &[POS1],
            error_codes: &[ERR1],
        };
        let converted: CliArgsInfo = orig.into();
        assert_eq!(TEST_NAME, converted.name);
        assert_eq!(TEST_DESCRIPTION, converted.description);
        assert_eq!(
            TEST_EXAMPLES.iter().map(|e| e.to_string()).collect::<Vec<String>>(),
            converted.examples
        );
        // Don't use into() here since that is what we are testing.

        assert_eq!(
            vec![PositionalInfo {
                name: "pos1".into(),
                description: "positional arg 1".into(),
                hidden: false,
                optionality: Optionality::Optional
            }],
            converted.positionals
        );

        assert_eq!(
            vec![ErrorCodeInfo { code: 123, description: "Some error code".into() }],
            converted.error_codes
        );
    }

    #[test]
    fn test_convert_subcommand() {
        const TEST_NAME: &str = "test_name";
        const TEST_DESCRIPTION: &str = "test description";
        const TEST_EXAMPLES: [&str; 2] = ["ex1", "ex2"];
        const TEST_NOTES: [&str; 2] = ["note1", "note2"];
        const POS1: argh::PositionalInfo<'_> = argh::PositionalInfo {
            name: "pos1",
            description: "positional arg 1",
            optionality: argh::Optionality::Optional,
            hidden: false,
        };
        const ERR1: argh::ErrorCodeInfo<'_> =
            argh::ErrorCodeInfo { code: 123, description: "Some error code" };
        const SUB1: argh::CommandInfoWithArgs = argh::CommandInfoWithArgs {
            name: TEST_NAME,
            description: TEST_DESCRIPTION,
            short: &'\0',
            examples: &TEST_EXAMPLES,
            flags: &[],
            notes: &TEST_NOTES,
            commands: vec![],
            positionals: &[POS1],
            error_codes: &[ERR1],
        };

        let orig = argh::SubCommandInfo { name: "subcommand_name", command: SUB1 };

        let converted: SubCommandInfo = (&orig).into();
        assert_eq!("subcommand_name", converted.name);
        assert_eq!(TEST_DESCRIPTION, converted.command.description);
        assert_eq!(TEST_NAME, converted.command.name);
        assert_eq!(
            TEST_EXAMPLES.iter().map(|e| e.to_string()).collect::<Vec<String>>(),
            converted.command.examples
        );
        // Don't use into() here since that is what we are testing.

        assert_eq!(
            vec![PositionalInfo {
                name: "pos1".into(),
                description: "positional arg 1".into(),
                hidden: false,
                optionality: Optionality::Optional
            }],
            converted.command.positionals
        );

        assert_eq!(
            vec![ErrorCodeInfo { code: 123, description: "Some error code".into() }],
            converted.command.error_codes
        );
    }
}
