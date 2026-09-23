// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use fxfs::serialized_types::{
    EARLIEST_SUPPORTED_VERSION, LATEST_VERSION, Version, get_type_fingerprints,
};
use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

#[derive(FromArgs, PartialEq, Debug)]
/// Generate type fingerprints for golden testing.
#[argh(subcommand, name = "type_fprint")]
pub struct TypeFprintSubCommand {
    #[argh(option)]
    /// path to output directory.
    output_dir: PathBuf,
    #[argh(option)]
    /// path to depfile.
    depfile: Option<PathBuf>,
    #[argh(option)]
    /// path to comparisons file.
    comparisons_file: Option<PathBuf>,
    #[argh(option)]
    /// path to golden directory.
    golden_dir: Option<PathBuf>,
}

#[derive(Serialize)]
struct Comparison {
    candidate: String,
    golden: String,
}

pub fn generate_all_fingerprints(output_dir: PathBuf) -> Result<Vec<PathBuf>, anyhow::Error> {
    if !output_dir.exists() {
        std::fs::create_dir_all(&output_dir)?;
    }

    let mut generated_files = Vec::new();
    for major in EARLIEST_SUPPORTED_VERSION.major..=LATEST_VERSION.major {
        let version = Version { major, minor: 0 };
        let map = get_type_fingerprints(version);
        let output_path = output_dir.join(format!("{}.json.golden", major));
        let mut file = File::create(&output_path)?;
        serde_json::to_writer_pretty(&mut file, &map)?;
        generated_files.push(output_path);
    }

    Ok(generated_files)
}

pub async fn run(args: TypeFprintSubCommand) -> Result<(), anyhow::Error> {
    let generated_files = generate_all_fingerprints(args.output_dir.clone())?;

    if let Some(depfile) = args.depfile {
        let mut file = File::create(depfile)?;
        for generated_file in &generated_files {
            write!(file, "{} ", generated_file.display())?;
        }
        writeln!(file, ":")?;
    }

    if let Some(comparisons_file) = args.comparisons_file {
        if let Some(golden_dir) = args.golden_dir {
            let mut comparisons = Vec::new();
            for generated_file in generated_files {
                let file_name = generated_file.file_name().unwrap().to_str().unwrap();
                comparisons.push(Comparison {
                    candidate: generated_file.to_str().unwrap().to_string(),
                    golden: golden_dir.join(file_name).to_str().unwrap().to_string(),
                });
            }
            let mut file = File::create(comparisons_file)?;
            serde_json::to_writer_pretty(&mut file, &comparisons)?;
        }
    }

    Ok(())
}
