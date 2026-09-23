// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use clap::Parser;
use component_id_index::Index;
use fidl::persist;
use fidl_fuchsia_component_internal as fcomponent_internal;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(about = "Validate and merge component ID index files.")]
struct CommandLineOpts {
    #[arg(
        long,
        help = "Path to a manifest text file containing a list of index files, one on each line. All index files are merged into a single index, written to the supplied --output_index_json and --output_index_fidl"
    )]
    input_manifest: PathBuf,

    #[arg(long, help = "Where to write the merged index file, encoded in JSON.")]
    output_index_json: PathBuf,

    #[arg(long, help = "Where to write the merged index file, encoded in FIDL wire-format.")]
    output_index_fidl: PathBuf,

    #[arg(
        short,
        long,
        help = "Where to write a dep file (.d) file of indices from --input_manifest."
    )]
    depfile: PathBuf,
}

fn run(opts: CommandLineOpts) -> anyhow::Result<()> {
    let input_manifest =
        fs::File::open(opts.input_manifest).context("Could not open input manifest")?;
    let input_files = BufReader::new(input_manifest)
        .lines()
        .map(|result| result.map(Into::into))
        .collect::<Result<Vec<Utf8PathBuf>, _>>()
        .context("Could not read input manifest")?;
    let merged_index = Index::merged_from_json5_files(input_files.as_slice())
        .context("Could not merge index files")?;

    let serialized_output_json =
        serde_json::to_string(&merged_index).context("Could not json-encode merged index")?;
    fs::write(&opts.output_index_json, serialized_output_json.as_bytes())
        .context("Could not write merged JSON-encoded index to file")?;

    let merged_index_fidl: fcomponent_internal::ComponentIdIndex = merged_index.into();
    let serialized_output_fidl =
        persist(&merged_index_fidl).context("Could not fidl-encode merged index")?;
    fs::write(&opts.output_index_fidl, serialized_output_fidl)
        .context("Could not write merged FIDL-encoded index to file")?;

    // write out the depfile
    let input_files_str =
        input_files.iter().map(|path| path.as_str()).collect::<Vec<_>>().join(" ");
    fs::write(
        &opts.depfile,
        format!(
            "{}: {}\n{}: {}\n",
            opts.output_index_json.to_str().unwrap(),
            input_files_str,
            opts.output_index_fidl.to_str().unwrap(),
            input_files_str,
        ),
    )
    .context("Could not write to depfile")
}

fn main() -> anyhow::Result<()> {
    let opts = CommandLineOpts::parse();
    run(opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_id_index::{IndexEntry, InstanceId};
    use moniker::Moniker;
    use pretty_assertions::assert_eq;
    use std::io::Write;

    fn gen_index(start: u32, end: u32) -> Index {
        let mut index = Index::default();
        for i in start..end {
            let moniker = Moniker::parse_str(&format!("/a/b/c/{i}")).unwrap();
            let instance_id =
                format!("00000000000000000000000000000000000000000000000000000000000000{:02x}", i)
                    .parse::<InstanceId>()
                    .unwrap();
            index.insert(IndexEntry { moniker, instance_id, ignore_duplicate_id: false }).unwrap();
        }
        index
    }

    #[test]
    fn multiple_indices_in_manifest() {
        let mut tmp_input_manifest = tempfile::NamedTempFile::new().unwrap();
        let mut tmp_input_index1 = tempfile::NamedTempFile::new().unwrap();
        let mut tmp_input_index2 = tempfile::NamedTempFile::new().unwrap();
        let tmp_output_index_json = tempfile::NamedTempFile::new().unwrap();
        let tmp_output_index_fidl = tempfile::NamedTempFile::new().unwrap();
        let tmp_output_depfile = tempfile::NamedTempFile::new().unwrap();

        // the manifest lists two index files:
        write!(
            tmp_input_manifest,
            "{}\n{}",
            tmp_input_index1.path().display(),
            tmp_input_index2.path().display()
        )
        .unwrap();

        // write the first index file
        let index1 = gen_index(0, 2);
        tmp_input_index1.write_all(serde_json5::to_string(&index1).unwrap().as_bytes()).unwrap();

        // write the second index file
        let index2 = gen_index(2, 4);
        tmp_input_index2.write_all(serde_json5::to_string(&index2).unwrap().as_bytes()).unwrap();

        assert!(matches!(
            run(CommandLineOpts {
                input_manifest: tmp_input_manifest.path().to_path_buf(),
                output_index_json: tmp_output_index_json.path().to_path_buf(),
                output_index_fidl: tmp_output_index_fidl.path().to_path_buf(),
                depfile: tmp_output_depfile.path().to_path_buf(),
            }),
            Ok(_)
        ));

        // assert that the output index file contains the merged index.
        let index_files = [Utf8PathBuf::from(tmp_output_index_json.path().to_str().unwrap())];
        let merged_index = Index::merged_from_json5_files(&index_files).unwrap();
        for i in 0..4 {
            let instance_id =
                format!("00000000000000000000000000000000000000000000000000000000000000{:02x}", i)
                    .parse::<InstanceId>()
                    .unwrap();
            assert!(merged_index.contains_id(&instance_id));
        }

        // assert the structure of the dependency file:
        //  <merged_output_index>: <input index 1> <input index 2>\n
        assert_eq!(
            format!(
                "{}: {} {}\n{}: {} {}\n",
                tmp_output_index_json.path().to_str().unwrap(),
                tmp_input_index1.path().to_str().unwrap(),
                tmp_input_index2.path().to_str().unwrap(),
                tmp_output_index_fidl.path().to_str().unwrap(),
                tmp_input_index1.path().to_str().unwrap(),
                tmp_input_index2.path().to_str().unwrap()
            ),
            fs::read_to_string(tmp_output_depfile.path()).unwrap()
        )
    }
}
