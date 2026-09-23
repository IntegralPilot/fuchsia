// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use argh::FromArgs;

#[derive(FromArgs)]
/// Reads test case names from stdin and outputs an expectation table based on
/// on each of the provided expectations files.
///
/// Note that this tool doesn't preprocess expectations files in order to
/// resolve includes. It requires a fully baked expectations file, typically
/// acquired directly from a test package.
///
/// The output is a comma-separated list where each column after the first
/// matches the outcome for the test case matching the position of the
/// expectations file provided.
///
/// E.g.:
/// ```
/// $ list_test_expectations always_passes.json2 always_fails.json5 < test_cases.txt
/// test1,Pass,Fail
/// test2,Pass,Fail
/// ```
struct Args {
    /// path to expectations json5 files.
    #[argh(positional)]
    expectations: Vec<String>,
}

fn process_expectations<W: std::io::Write, R: IntoIterator<Item = anyhow::Result<String>>>(
    expectations: &[ser::Expectations],
    writer: &mut W,
    reader: R,
) -> anyhow::Result<()> {
    for line in reader {
        let line = line.context("failed to read line")?;
        write!(writer, "{line}").context("output")?;
        for expectations in expectations.iter() {
            let outcome = expectations_matcher::expected_outcome(&line, expectations)
                .ok_or_else(|| anyhow::anyhow!("no expectation matches '{line}'"))?;
            write!(writer, ",{outcome:?}").context("output")?;
        }
        write!(writer, "\n").context("output")?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let Args { expectations } = argh::from_env();
    if expectations.is_empty() {
        return Err(anyhow::anyhow!("no expectations provided"));
    }

    let expectations = expectations
        .into_iter()
        .map(|expectations| {
            let file = std::fs::File::open(&expectations)
                .with_context(|| format!("failed to open '{expectations}'"))?;
            let mut reader = std::io::BufReader::new(file);
            serde_json5::from_reader(&mut reader)
                .with_context(|| format!("failed to parse expectations from '{expectations}'"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    process_expectations(
        &expectations,
        &mut std::io::stdout(),
        std::io::stdin().lines().map(|r| r.context("reading from stdin")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    #[test]
    fn process_output() {
        let all_glob: glob::Pattern = "*".parse().expect("glob parse");
        let expectations = [
            ser::Expectations {
                expectations: vec![ser::Expectation::ExpectPass(ser::Matchers {
                    matchers: vec![all_glob.clone()],
                })],
                cases_to_run: ser::CasesToRun::All,
            },
            ser::Expectations {
                expectations: vec![ser::Expectation::ExpectFailure(ser::Matchers {
                    matchers: vec![all_glob],
                })],
                cases_to_run: ser::CasesToRun::All,
            },
        ];
        let mut output = Vec::new();
        let tests = ["foo", "bar"];
        process_expectations(
            &expectations[..],
            &mut output,
            tests.iter().map(|s| Ok(s.to_string())),
        )
        .expect("process expectations");
        for (output, test) in std::io::BufReader::new(&output[..]).lines().zip(tests.iter()) {
            let output = output.unwrap();
            assert_eq!(
                output,
                format!(
                    "{test},{:?},{:?}",
                    expectations_matcher::Outcome::Pass,
                    expectations_matcher::Outcome::Fail
                )
            );
        }
    }
}
