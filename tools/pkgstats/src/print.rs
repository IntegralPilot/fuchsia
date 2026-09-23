// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::OutputSummary;
use anyhow::Result;
use argh::FromArgs;
use camino::Utf8PathBuf;
use std::fs::File;
use std::io::Write;

#[derive(FromArgs)]
#[argh(subcommand, name = "print")]
/// print all package contents in order, for diff
pub struct PrintCommand {
    /// input file generated using "process" command
    #[argh(option)]
    input: Utf8PathBuf,

    /// output file name, if absent print to stdout
    #[argh(option)]
    output: Option<Utf8PathBuf>,
}

impl PrintCommand {
    pub fn execute(self) -> Result<()> {
        let data: OutputSummary = {
            let file = File::open(self.input)?;
            serde_json::from_reader(file)?
        };

        let mut packages = data
            .packages
            .iter()
            .flat_map(|(_hash, package_contents)| {
                package_contents
                    .files
                    .iter()
                    .map(|f| (package_contents.url.name(), &f.name, &f.hash))
            })
            .collect::<Vec<_>>();
        packages.sort();

        let mut write = match self.output {
            None => StdoutOrFile::Stdout,
            Some(path) => StdoutOrFile::File(File::open(path)?),
        };

        for (pkg, file, hash) in packages {
            writeln!(&mut write, "{pkg} {file}={hash}")?;
        }

        Ok(())
    }
}

enum StdoutOrFile {
    Stdout,
    File(File),
}

impl Write for StdoutOrFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            StdoutOrFile::Stdout => std::io::stdout().write(buf),
            StdoutOrFile::File(f) => f.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            StdoutOrFile::Stdout => std::io::stdout().flush(),
            StdoutOrFile::File(f) => f.flush(),
        }
    }
}
