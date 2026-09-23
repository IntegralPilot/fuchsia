// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use buf_read_ext::BufReadExt as _;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::path::{Path, PathBuf};

/// Compares either match or they have a set of mismatch errors.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum CompareResult {
    Matches,
    Mismatch { errors: Vec<String> },
}

/// A golden file consists of lines of input that annotate expected data. Each
/// line may contain a '?' prefix which indicates an optional entry or a '#'
/// prefix which indicates the entry is a comment and should be ignored.
pub struct GoldenFile {
    path: PathBuf,
    required: HashSet<String>,
    optional: HashSet<String>,
    required_prefix: HashSet<String>,
    optional_prefix: HashSet<String>,
}

/// Returns the element of `prefixes` that `name` matched, or `None`.
fn matches_prefix(name: &String, prefixes: &HashSet<String>) -> Option<String> {
    for p in prefixes.iter() {
        if name.starts_with(p) {
            // The wildcard prefix cannot match if the remaining suffix
            // contains the path separator.
            let remainder = &name[p.len()..];
            if !remainder.contains('/') {
                return Some(p.to_string());
            }
        }
    }
    None
}

impl GoldenFile {
    pub fn from_files<P: AsRef<Path>>(files: &Vec<P>) -> Result<Self> {
        if files.is_empty() {
            bail!("files must not be empty");
        }
        let mut golden = Self::open(&files[0])?;
        for index in 1..files.len() {
            golden.merge(Self::open(&files[index])?);
        }
        Ok(golden)
    }

    fn merge(&mut self, golden: GoldenFile) {
        self.required.extend(golden.required);
        self.optional.extend(golden.optional);
        self.required_prefix.extend(golden.required_prefix);
        self.optional_prefix.extend(golden.optional_prefix);
    }

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let golden_file = File::open(path).context("failed to open golden file")?;
        Self::parse(path, BufReader::new(golden_file))
    }

    pub fn from_contents<P: AsRef<Path>>(path: P, contents: Vec<u8>) -> Result<Self> {
        Self::parse(path, BufReader::new(Cursor::new(contents)))
    }

    /// Parses the lines of `reader` as follows:
    ///
    /// * lines beginning with "#" are ignored
    /// * blank lines are ignored
    /// * lines beginning with "?" are treated as optional: the system may or
    ///   may not include the named file
    /// * lines ending with "*" are prefixes by which file names should be
    ///   matched, rather than matched exactly
    ///
    /// For prefix matching, the suffixes cannot contain the path separator
    /// character '/'. For example, a golden file with this line:
    ///
    ///   ?/bin/goat*
    ///
    /// indicates that the system image may or may not contain /bin/goat,
    /// /bin/goats, or /bin/goat_teleporter, but it says nothing about whether
    /// /bin/goats/Buttermilk is allowed.
    fn parse<P: AsRef<Path>, R: Read>(path: P, mut reader: BufReader<R>) -> Result<Self> {
        let mut required: HashSet<String> = HashSet::new();
        let mut optional: HashSet<String> = HashSet::new();
        let mut required_prefix: HashSet<String> = HashSet::new();
        let mut optional_prefix: HashSet<String> = HashSet::new();

        let mut lines = reader.lending_lines();
        while let Some(line) = lines.next() {
            let line = line?;
            match line.chars().next() {
                // Skip comment lines.
                Some('#') => {}
                // Optional entry
                Some('?') => {
                    let mut stripped = line.chars();
                    stripped.next();
                    let name = String::from(stripped.as_str());
                    if name.ends_with("*") {
                        optional_prefix.insert(name.strip_suffix("*").unwrap().to_string());
                    } else {
                        optional.insert(name);
                    };
                }
                // Normal entry
                Some(_) => {
                    let name = line.to_string();
                    if name.ends_with("*") {
                        required_prefix.insert(name.strip_suffix("*").unwrap().to_string());
                    } else {
                        required.insert(name);
                    }
                }
                // Skip empty lines
                None => {}
            };
        }

        Ok(Self {
            path: path.as_ref().to_path_buf(),
            required,
            optional,
            required_prefix,
            optional_prefix,
        })
    }

    /// Returns the set of differences between the golden file and the content
    /// provided. This should be an empty vector unless there is a mismatch.
    pub fn compare(&self, content: Vec<String>) -> CompareResult {
        let mut not_permitted: Vec<String> = Vec::new();

        // Take copies of all the sets and work on those, so that this function
        // does not mutate `self`.
        let mut required = self.required.clone();
        let mut optional = self.optional.clone();
        let mut required_prefix = self.required_prefix.clone();
        let mut optional_prefix = self.optional_prefix.clone();

        for line in content.iter() {
            if required.contains(line) {
                required.remove(line);
            } else if optional.contains(line) {
                optional.remove(line);
            } else if let Some(found) = matches_prefix(line, &required_prefix) {
                required_prefix.remove(&found);
            } else if let Some(found) = matches_prefix(line, &optional_prefix) {
                optional_prefix.remove(&found);
            } else {
                not_permitted.push(line.clone());
            }
        }

        not_permitted.sort();

        let mut errors: Vec<String> = Vec::new();
        for entry in not_permitted.iter() {
            errors.push(format!(
                "{0} is not listed in {1:?} but was found in the build. If the addition to the build was intended, add a line '{0}' to {1:?}.",
                entry,
                self.path,
            ));
        }
        for entry in required.iter().chain(required_prefix.iter()) {
            errors.push(format!(
                "{0} was declared as required in {1:?} but was not found in the build. If the removal from the build was intended, update {1:?} to remove the line '{0}'.",
                entry,
                self.path,
            ));
        }

        if errors.is_empty() { CompareResult::Matches } else { CompareResult::Mismatch { errors } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_required_golden_files() {
        let temp = tempdir().unwrap();
        let golden_path = temp.path().join("golden.txt");
        let mut golden_file = File::create(&golden_path).expect("failed to create golden");
        writeln!(golden_file, "foo").expect("failed to write");
        writeln!(golden_file, "bar").expect("failed to write");
        writeln!(golden_file, "baz").expect("failed to write");
        writeln!(golden_file, "goat*").expect("failed to write");
        drop(golden_file);

        let golden = GoldenFile::open(golden_path).expect("failed to open golden");
        let result = golden.compare(vec![
            "foo".to_string(),
            "bar".to_string(),
            "baz".to_string(),
            "goats".to_string(),
        ]);
        assert_eq!(result, CompareResult::Matches);

        let extra_entry_result = golden.compare(vec![
            "foo".to_string(),
            "bar".to_string(),
            "baz".to_string(),
            "extra".to_string(),
        ]);
        assert_ne!(extra_entry_result, CompareResult::Matches);

        let extra_entry_result = golden.compare(vec![
            "foo".to_string(),
            "bar".to_string(),
            "baz".to_string(),
            "goats/Buttermilk".to_string(),
        ]);
        assert_ne!(extra_entry_result, CompareResult::Matches);

        let omitted_entry_result = golden.compare(vec!["foo".to_string(), "bar".to_string()]);
        assert_ne!(omitted_entry_result, CompareResult::Matches);
    }

    #[test]
    fn test_optional_golden_files() {
        let temp = tempdir().unwrap();
        let golden_path = temp.path().join("golden.txt");
        let mut golden_file = File::create(&golden_path).expect("failed to create golden");
        writeln!(golden_file, "foo").expect("failed to write");
        writeln!(golden_file, "bar").expect("failed to write");
        writeln!(golden_file, "?baz").expect("failed to write");
        drop(golden_file);

        let golden = GoldenFile::open(golden_path).expect("failed to open golden");
        let result = golden.compare(vec!["foo".to_string(), "bar".to_string(), "baz".to_string()]);
        assert_eq!(result, CompareResult::Matches);

        let omitted_entry_result = golden.compare(vec!["foo".to_string(), "bar".to_string()]);
        assert_eq!(omitted_entry_result, CompareResult::Matches);

        let extra_entry_result = golden.compare(vec![
            "foo".to_string(),
            "bar".to_string(),
            "baz".to_string(),
            "extra".to_string(),
        ]);
        assert_ne!(extra_entry_result, CompareResult::Matches);
    }

    #[test]
    fn test_from_files() {
        let temp = tempdir().unwrap();
        let mut golden_files = Vec::<PathBuf>::new();
        for i in 0..3 {
            let golden_path = temp.path().join(format!("golden_{}.txt", i));
            let mut golden_file = File::create(&golden_path).expect("failed to create golden");
            writeln!(golden_file, "foo_{}", i).expect("failed to write");
            drop(golden_file);
            golden_files.push(golden_path);
        }

        let golden = GoldenFile::from_files(&golden_files).expect("failed to open goldens");
        let result =
            golden.compare(vec!["foo_0".to_string(), "foo_1".to_string(), "foo_2".to_string()]);
        assert_eq!(result, CompareResult::Matches);
    }
}
