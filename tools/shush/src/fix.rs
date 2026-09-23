// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use rustfix::{Filter, Suggestion};

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::BufRead;

pub fn fix<R: BufRead>(
    lints: &mut R,
    filter: &[String],
    rustfix_filter: Filter,
    solution_message: Option<&str>,
    dryrun: bool,
) -> Result<(), Error> {
    let mut all_lints = String::new();
    lints.read_to_string(&mut all_lints)?;
    let categories = crate::lint::get_categories();
    // If a lint category is given, add all lints in that category to the filter
    let mut filter_lints: HashSet<String> = HashSet::new();
    for f in filter {
        if let Some(lints) = categories.get(f) {
            filter_lints.extend(lints.iter().cloned());
        } else {
            filter_lints.insert(f.to_owned());
        }
    }

    let suggestions =
        rustfix::get_suggestions_from_json(&all_lints, &filter_lints, rustfix_filter)?;
    if suggestions.is_empty() {
        return Err(anyhow!("Couldn't find any fixable occurances of those lints"));
    }

    let mut source_files: HashMap<String, Vec<Suggestion>> = Default::default();
    for suggestion in suggestions {
        assert!(!suggestion.solutions.is_empty());

        let solution = if let Some(message) = solution_message {
            if let Some(solution) = suggestion.solutions.iter().find(|s| s.message == message) {
                solution
            } else {
                return Err(anyhow!(
                    "Suggestion does not have a solution with message {}",
                    message
                ));
            }
        } else if suggestion.solutions.len() > 1 {
            return Err(anyhow!(
                "Suggestion has multiple solutions, use `--solution-message` to select one:\n{}",
                suggestion
                    .solutions
                    .iter()
                    .map(|solution| format!("  - {}", solution.message))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        } else {
            &suggestion.solutions[0]
        };

        // there should be only one file per suggestion
        let files = solution
            .replacements
            .iter()
            .map(|r| r.snippet.file_name.clone())
            .collect::<BTreeSet<_>>();

        if files.len() > 1 {
            return Err(anyhow!(
                "Solution applies to multiple files:\n{}",
                files.iter().map(|file| format!("  - {}", file)).collect::<Vec<_>>().join("\n")
            ));
        }

        assert_eq!(files.len(), 1);

        let file = solution.replacements[0].snippet.file_name.clone();
        source_files.entry(file).or_default().push(suggestion);
    }

    for (source_file, suggestions) in &source_files {
        let source = fs::read_to_string(source_file)?;
        let mut fix = rustfix::CodeFix::new(&source);
        for suggestion in suggestions.iter().rev() {
            if let Some(message) = solution_message {
                let solution = suggestion.solutions.iter().find(|s| s.message == message).unwrap();

                if let Err(e) = fix.apply_solution(solution) {
                    eprintln!("Failed to apply solution to {}: {}", source_file, e);
                }
            } else {
                assert_eq!(suggestion.solutions.len(), 1);
                if let Err(e) = fix.apply(suggestion) {
                    eprintln!("Failed to apply suggestion to {}: {}", source_file, e);
                }
            }
        }
        let fixes = fix.finish()?;
        println!("{} fixes in {}", suggestions.len(), source_file);
        if !dryrun {
            fs::write(source_file, fixes)?;
        }
    }
    Ok(())
}
