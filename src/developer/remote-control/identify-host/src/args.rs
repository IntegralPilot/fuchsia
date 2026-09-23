// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    PrettyJson,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace(['-', '_'], "").as_str() {
            "text" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            "prettyjson" | "pretttyjson" => Ok(OutputFormat::PrettyJson),
            other => {
                Err(format!("invalid format '{other}': expected 'text', 'json', or 'prettyjson'"))
            }
        }
    }
}

#[derive(FromArgs, Debug, PartialEq, Eq)]
/// Identify a host target via the Remote Control Service (RCS).
pub struct IdentifyHostArgs {
    /// format for output: 'text' (default), 'json', or 'prettyjson'
    #[argh(option, short = 'f', default = "OutputFormat::Text")]
    pub format: OutputFormat,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_from_str() {
        assert_eq!("text".parse::<OutputFormat>(), Ok(OutputFormat::Text));
        assert_eq!("TEXT".parse::<OutputFormat>(), Ok(OutputFormat::Text));
        assert_eq!("Text".parse::<OutputFormat>(), Ok(OutputFormat::Text));
        assert_eq!("json".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("JSON".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("Json".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("prettyjson".parse::<OutputFormat>(), Ok(OutputFormat::PrettyJson));
        assert_eq!("prettyJson".parse::<OutputFormat>(), Ok(OutputFormat::PrettyJson));
        assert_eq!("pretttyJson".parse::<OutputFormat>(), Ok(OutputFormat::PrettyJson));
        assert_eq!("pretty_json".parse::<OutputFormat>(), Ok(OutputFormat::PrettyJson));
        assert_eq!("pretty-json".parse::<OutputFormat>(), Ok(OutputFormat::PrettyJson));
        assert!("xml".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_args_default() {
        let args = IdentifyHostArgs::from_args(&["identify-host"], &[]).expect("parse empty args");
        assert_eq!(args.format, OutputFormat::Text);
    }

    #[test]
    fn test_args_format_text() {
        let args = IdentifyHostArgs::from_args(&["identify-host"], &["--format", "text"])
            .expect("parse text format");
        assert_eq!(args.format, OutputFormat::Text);
    }

    #[test]
    fn test_args_format_json() {
        let args = IdentifyHostArgs::from_args(&["identify-host"], &["--format", "json"])
            .expect("parse json format");
        assert_eq!(args.format, OutputFormat::Json);

        let args_short = IdentifyHostArgs::from_args(&["identify-host"], &["-f", "JSON"])
            .expect("parse short json format");
        assert_eq!(args_short.format, OutputFormat::Json);
    }

    #[test]
    fn test_args_format_pretty_json() {
        let args = IdentifyHostArgs::from_args(&["identify-host"], &["--format", "prettyjson"])
            .expect("parse prettyjson format");
        assert_eq!(args.format, OutputFormat::PrettyJson);

        let args_camel = IdentifyHostArgs::from_args(&["identify-host"], &["-f", "prettyJson"])
            .expect("parse prettyJson format");
        assert_eq!(args_camel.format, OutputFormat::PrettyJson);
    }
}
