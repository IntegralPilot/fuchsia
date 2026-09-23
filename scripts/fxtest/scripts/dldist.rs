// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::FromArgs;
use std::fs::read_to_string;
use std::path::PathBuf;

/// Command to match a list of strings using Damerau–Levenshtein distance.
#[derive(Debug, FromArgs)]
struct Args {
    /// value to search for
    #[argh(option)]
    needle: String,

    /// path of file containing strings to match, one per line
    #[argh(option)]
    input: PathBuf,

    /// if set, return a perfect match if needle is a prefix of any input
    #[argh(switch)]
    match_prefixes: bool,

    /// if set, return a perfect match if needle is contained in any input
    #[argh(switch)]
    match_contains: bool,

    /// if set, print verbose debugging to stderr
    #[argh(switch, short = 'v')]
    verbose: bool,
}

const PERFECT_MATCH: usize = 0;

fn process_line(args: &Args, line: &str) -> usize {
    if args.match_prefixes && line.starts_with(&args.needle) {
        PERFECT_MATCH
    } else if args.match_contains && line.contains(&args.needle) {
        PERFECT_MATCH
    } else {
        strsim::damerau_levenshtein(&args.needle, line)
    }
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    let contents = read_to_string(&args.input)?;
    for line in contents.lines() {
        let val = process_line(&args, &line);
        println!("{val}");
        if args.verbose {
            eprintln!("'{line}' = {val}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Case {
        line: &'static str,
        output: usize,
        args: Args,
    }

    impl Case {
        fn with_prefixes(line: &'static str, needle: &'static str, output: usize) -> Self {
            Self {
                line,
                output,
                args: Args {
                    needle: needle.to_string(),
                    input: PathBuf::new(),
                    match_prefixes: true,
                    match_contains: false,
                    verbose: false,
                },
            }
        }

        fn with_contains(line: &'static str, needle: &'static str, output: usize) -> Self {
            Self {
                line,
                output,
                args: Args {
                    needle: needle.to_string(),
                    input: PathBuf::new(),
                    match_prefixes: false,
                    match_contains: true,
                    verbose: false,
                },
            }
        }

        fn fuzzy(line: &'static str, needle: &'static str, output: usize) -> Self {
            Self {
                line,
                output,
                args: Args {
                    needle: needle.to_string(),
                    input: PathBuf::new(),
                    match_prefixes: false,
                    match_contains: false,
                    verbose: false,
                },
            }
        }

        fn to_explanation(&self) -> String {
            format!(
                "Searching for \"{}\" in \"{}\". Prefix?: {}. Contains?: {}. Expected {}",
                self.args.needle,
                self.line,
                self.args.match_prefixes,
                self.args.match_contains,
                self.output
            )
        }
    }

    #[test]
    fn test_fuzzy_matching() {
        for case in [
            Case::fuzzy("test_value", "test_value", 0),
            Case::fuzzy("test_value", "test_valuez", 1),
            Case::fuzzy("tset_value", "test_value", 1),
            Case::fuzzy("test_value", "test", 6),
            Case::fuzzy("test_value", "value", 5),
        ] {
            let actual = process_line(&case.args, case.line);
            assert_eq!(actual, case.output, "{}. Got {}", case.to_explanation(), actual);
        }
    }

    #[test]
    fn test_prefix_matching() {
        for case in [
            Case::with_prefixes("test_value", "test_value", 0),
            Case::with_prefixes("test_value", "test", 0),
            Case::with_prefixes("test_value", "value", 5),
        ] {
            let actual = process_line(&case.args, case.line);
            assert_eq!(actual, case.output, "{}. Got {}", case.to_explanation(), actual);
        }
    }

    #[test]
    fn test_contains_matching() {
        for case in [
            Case::with_contains("test_value", "test_value", 0),
            Case::with_contains("test_value", "test", 0),
            Case::with_contains("test_value", "value", 0),
        ] {
            let actual = process_line(&case.args, case.line);
            assert_eq!(actual, case.output, "{}. Got {}", case.to_explanation(), actual);
        }
    }
}
