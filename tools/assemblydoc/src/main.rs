// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use argh::FromArgs;
use assembly_config_schema::ProductConfig;
use camino::Utf8PathBuf;
use serdedoc::DocWriter;
use std::path::PathBuf;
use tempfile::TempDir;

/// Assembly documentation generator.
#[derive(Debug, FromArgs)]
struct AssemblyDocArgs {
    /// output directory. If not supplied a temporary directory is used.
    #[argh(option)]
    output_dir: Option<Utf8PathBuf>,
    /// archive output
    #[argh(option)]
    archive_output: Option<Utf8PathBuf>,
}

fn main() -> Result<()> {
    let args: AssemblyDocArgs = argh::from_env();
    let tmp_dir = TempDir::new().context("Creating tempdir")?;

    let output_dir = if let Some(output_dir) = &args.output_dir {
        // Remove the contents of the output directory to ensure there is no stale contents.
        if std::path::Path::new(&output_dir).exists() {
            std::fs::remove_dir_all(&output_dir).context("Clearing output directory")?;
        }
        output_dir.clone()
    } else {
        Utf8PathBuf::try_from(PathBuf::from(tmp_dir.path()))?
    };

    // Write the docs.
    let writer = DocWriter::new("/reference/assembly/".to_string());
    writer.write::<ProductConfig>(output_dir.clone()).context("Writing assembly docs")?;

    // Archive the docs if necessary.
    if let Some(archive_output) = &args.archive_output {
        let archive_file =
            std::fs::File::create(archive_output).context("Creating archive for assembly docs")?;
        let mut archive = tar::Builder::new(archive_file);
        archive
            .append_dir_all("assemblydoc", &output_dir)
            .context("Adding assembly docs to archive")?;
        archive.finish().context("Finalizing assembly docs archive")?;
    }

    Ok(())
}
