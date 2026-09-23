// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembly_platform_artifacts::PlatformArtifacts;
use assembly_release_info::ReleaseInfo;
use assembly_util::{copy_dir, fast_copy};
use camino::Utf8PathBuf;
use depfile::Depfile;
use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;

use crate::PlatformArtifactsArgs;

#[derive(Deserialize)]
struct AssemblyInputBundleEntry {
    name: String,
    path: Utf8PathBuf,
}

pub fn new(args: &PlatformArtifactsArgs) -> Result<()> {
    if args.output.exists() {
        std::fs::remove_dir_all(&args.output).context("removing output directory")?;
    }
    std::fs::create_dir_all(&args.output).context("creating output directory")?;
    let tools_dir = args.output.join("tools");
    std::fs::create_dir(&tools_dir).context("creating tools directory")?;

    let file = File::open(&args.aib_list).context("opening aib list")?;
    let reader = BufReader::new(file);
    let bundles: Vec<AssemblyInputBundleEntry> =
        serde_json::from_reader(reader).context("reading aib list")?;

    let mut deps = vec![];
    let mut names = vec![];

    for bundle in bundles {
        let dest = args.output.join(bundle.path.file_name().context("getting file name")?);
        link_or_copy(&bundle.path, &dest)
            .with_context(|| format!("copying aib: {}", bundle.name))?;

        deps.push(bundle.path.join("assembly_config.json"));
        names.push(bundle.name);
    }

    for tool in &args.tools {
        let tool_name = tool.file_name().context("getting tool name")?;
        let dest = tools_dir.join(tool_name);
        link_or_copy(tool, &dest).with_context(|| format!("copying tool: {}", tool))?;
    }

    let platform_artifacts = PlatformArtifacts {
        assembly_input_bundles: names,
        release_info: ReleaseInfo {
            name: args.name.clone(),
            repository: args.repo.clone(),
            version: args.version.clone(),
        },
        platform_input_bundle_dir: Utf8PathBuf::default(),
    };

    let config_path = args.output.join("platform_artifacts.json");
    let file = File::create(&config_path).context("creating config file")?;
    serde_json::to_writer_pretty(file, &platform_artifacts).context("writing config file")?;

    let mut depfile_builder = Depfile::new_with_output(&config_path);
    depfile_builder.add_inputs(deps);
    depfile_builder.write_to(&args.depfile).context("writing depfile")?;

    Ok(())
}

fn link_or_copy(src: &Utf8PathBuf, dst: &Utf8PathBuf) -> Result<()> {
    if src.is_dir() { copy_dir(src, dst) } else { fast_copy(src, dst) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn test_platform_artifacts() {
        let temp_dir = TempDir::new().unwrap();
        let root = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).unwrap();

        let aib_list_path = root.join("aib_list.json");
        let aib_dir = root.join("aib_dir");
        let tool_path = root.join("my_tool");
        let output_dir = root.join("output");
        let depfile_path = root.join("output.d");

        // Create inputs
        std::fs::create_dir(&aib_dir).unwrap();
        std::fs::write(aib_dir.join("assembly_config.json"), "{}").unwrap();
        std::fs::write(&tool_path, "tool binary").unwrap();

        let aib_list = json!([
            {
                "name": "aib_name",
                "path": aib_dir,
            }
        ]);
        std::fs::write(&aib_list_path, serde_json::to_string(&aib_list).unwrap()).unwrap();

        let args = PlatformArtifactsArgs {
            name: "test_platform".into(),
            aib_list: aib_list_path,
            repo: "test_repo".into(),
            version: "1.2.3".into(),
            tools: vec![tool_path],
            output: output_dir.clone(),
            depfile: depfile_path.clone(),
        };

        new(&args).unwrap();

        // Verify output
        let config_path = output_dir.join("platform_artifacts.json");
        let config_file = File::open(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_reader(config_file).unwrap();

        assert_eq!(config["release_info"]["name"], "test_platform");
        assert_eq!(config["release_info"]["version"], "1.2.3");
        assert_eq!(config["release_info"]["repository"], "test_repo");
        assert_eq!(config["assembly_input_bundles"][0], "aib_name");

        assert!(output_dir.join("aib_dir").exists());
        assert!(output_dir.join("aib_dir/assembly_config.json").exists());
        assert!(output_dir.join("tools/my_tool").exists());

        // Verify depfile
        let depfile_content = std::fs::read_to_string(&depfile_path).unwrap();
        assert!(depfile_content.contains("output/platform_artifacts.json:"));
        assert!(depfile_content.contains("aib_dir/assembly_config.json"));
    }
}
