// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use camino::Utf8Path;
use std::collections::BTreeSet;
use std::io::Write;

/// A depfile is a file that lists all the input files that are necessary for
/// producing a specific output. This file is handed to the ninja build system
/// for tracking build dependencies.
pub struct Depfile {
    inputs: BTreeSet<String>,
    outputs: BTreeSet<String>,
}

impl Depfile {
    /// Construct a new depfile.
    pub fn new() -> Self {
        Self { inputs: BTreeSet::default(), outputs: BTreeSet::default() }
    }

    /// Construct a new depfile for the output file.
    pub fn new_with_output(output: impl AsRef<str>) -> Self {
        let mut depfile = Self::new();
        depfile.add_output(output);
        depfile
    }

    /// Add additional input files that are used to construct the output.
    pub fn add_inputs<I: IntoIterator<Item = impl AsRef<str>>>(&mut self, iter: I) {
        self.inputs.extend(iter.into_iter().map(|s| s.as_ref().to_string()));
    }

    /// Add a single input file that is used to construct the output.
    pub fn add_input(&mut self, input: impl AsRef<str>) {
        self.inputs.insert(input.as_ref().to_string());
    }

    /// Add additional output files that are produced.
    pub fn add_outputs<I: IntoIterator<Item = impl AsRef<str>>>(&mut self, iter: I) {
        self.outputs.extend(iter.into_iter().map(|s| s.as_ref().to_string()));
    }

    /// Add a single output file that is produced.
    pub fn add_output(&mut self, output: impl AsRef<str>) {
        self.outputs.insert(output.as_ref().to_string());
    }

    /// Write the depfile.
    pub fn write_to(self, path: impl AsRef<Utf8Path>) -> Result<()> {
        let mut writer = std::io::BufWriter::new(std::fs::File::create(path.as_ref())?);
        let inputs: Vec<String> = self.inputs.into_iter().collect();
        let outputs: Vec<String> = self.outputs.into_iter().collect();
        if inputs.is_empty() {
            write!(writer, "{}:\n", outputs.join(" "))?;
        } else {
            write!(writer, "{}: \\\n  {}\n", outputs.join(" "), inputs.join(" \\\n  "))?;
        }
        writer.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use std::io::Read;
    use tempfile::tempdir;

    #[test]
    fn test_write() {
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let depfile_path = dir_path.join("depfile");

        let mut d = Depfile::new_with_output("a");
        d.add_inputs(vec!["b", "c"]);
        d.add_input("d");
        d.add_input("c");
        d.write_to(&depfile_path).unwrap();

        let mut contents = String::new();
        let mut depfile = std::fs::File::open(depfile_path).unwrap();
        depfile.read_to_string(&mut contents).unwrap();
        let expected = r#"a: \
  b \
  c \
  d
"#
        .to_string();
        assert_eq!(expected, contents);
    }

    #[test]
    fn test_write_multiple_outputs() {
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let depfile_path = dir_path.join("depfile");

        let mut d = Depfile::new_with_output("a");
        d.add_inputs(vec!["b", "c"]);
        d.add_input("d");
        d.add_input("c");
        d.add_output("e");
        d.write_to(&depfile_path).unwrap();

        let mut contents = String::new();
        let mut depfile = std::fs::File::open(depfile_path).unwrap();
        depfile.read_to_string(&mut contents).unwrap();
        let expected = r#"a e: \
  b \
  c \
  d
"#
        .to_string();
        assert_eq!(expected, contents);
    }

    #[test]
    fn test_write_no_deps() {
        let dir = tempdir().unwrap();
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let depfile_path = dir_path.join("depfile");

        let d = Depfile::new_with_output("a");
        d.write_to(&depfile_path).unwrap();

        let mut contents = String::new();
        let mut depfile = std::fs::File::open(depfile_path).unwrap();
        depfile.read_to_string(&mut contents).unwrap();
        let expected = r#"a:
"#
        .to_string();
        assert_eq!(expected, contents);
    }
}
