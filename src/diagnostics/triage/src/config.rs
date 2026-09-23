// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Options;
use crate::file_io::{config_from_files, diagnostics_from_directory};
use anyhow::{Error, bail, format_err};
use fuchsia_triage::{ActionTagDirective, DiagnosticData, ParseResult};
use std::path::Path;
use std::str::FromStr;

// TODO(https://fxbug.dev/42127533): Add support for CSV.
#[derive(Debug, PartialEq, Clone)]
pub enum OutputFormat {
    Structured,
    Text,
    VerboseText,
}

impl FromStr for OutputFormat {
    type Err = anyhow::Error;
    fn from_str(output_format: &str) -> Result<Self, Self::Err> {
        match output_format {
            "structured" => Ok(OutputFormat::Structured),
            "text" => Ok(OutputFormat::Text),
            "verbose-text" => Ok(OutputFormat::VerboseText),
            incorrect => Err(format_err!(
                "Invalid output type '{}' - must be 'text' or 'verbose-text' or 'structured",
                incorrect
            )),
        }
    }
}

/// Complete program execution context.
pub struct ProgramStateHolder {
    pub parse_result: ParseResult,
    pub diagnostic_data: Vec<DiagnosticData>,
    pub output_format: OutputFormat,
}

/// Parses the inspect.json file and all the config files.
pub fn initialize(options: Options) -> Result<ProgramStateHolder, Error> {
    let Options { data_directory, output_format, config_files, tags, exclude_tags } = options;

    if config_files.is_empty() {
        bail!("Need at least one config file; use --config");
    }

    let parse_result =
        config_from_files(&config_files, &ActionTagDirective::from_tags(tags, exclude_tags))?;
    parse_result.validate()?;

    let diagnostic_data = diagnostics_from_directory(Path::new(&data_directory))?;

    Ok(ProgramStateHolder { parse_result, diagnostic_data, output_format })
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    fn output_format_from_string() -> Result<(), Error> {
        assert_eq!(OutputFormat::from_str("text")?, OutputFormat::Text);
        assert_eq!(OutputFormat::from_str("verbose-text")?, OutputFormat::VerboseText);
        assert_eq!(OutputFormat::from_str("structured")?, OutputFormat::Structured);
        assert!(OutputFormat::from_str("").is_err(), "Should have returned 'Err' on ''");
        assert!(OutputFormat::from_str("CSV").is_err(), "Should have returned 'Err' on 'CSV'");
        assert!(OutputFormat::from_str("Text").is_err(), "Should have returned 'Err' on 'Text'");
        assert!(OutputFormat::from_str("verbose_text").is_err(), "Should reject underscore");
        assert!(
            OutputFormat::from_str("GARBAGE").is_err(),
            "Should have returned 'Err' on 'GARBAGE'"
        );
        Ok(())
    }
}
