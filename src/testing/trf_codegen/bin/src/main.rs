// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// Entry point for the TRF Codegen CLI. Orchestrates parsing, linting, and artifact generation.

mod consistency_checker;
mod generator;
mod parser;

use anyhow::{Context, Result};
use argh::FromArgs;
use consistency_checker::{ComponentManifest, check_consistency};
use generator::generate_artifacts;
use parser::parse_source;
use std::io::{self, Read};

#[derive(FromArgs, Default)]
/// TRF Codegen Tool
struct Args {
    /// source file path (if not provided, reads from stdin)
    #[argh(option)]
    source: Option<String>,

    /// manifest file path
    #[argh(option)]
    manifest: Option<String>,

    /// out dir
    #[argh(option)]
    out_dir: Option<String>,

    /// target name for GN integration
    #[argh(option)]
    target_name: Option<String>,

    /// component under test name
    #[argh(option)]
    cut_component_name: Option<String>,

    /// mock paths
    #[argh(option)]
    mock: Vec<String>,

    /// fallback positional manifest file path
    #[argh(positional)]
    manifest_pos: Option<String>,
}

fn load_primary_source(source_path: Option<&String>) -> Result<String> {
    if let Some(path) = source_path {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read source code file at {}", path))
    } else {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .context("Failed to read Rust source code from standard input")?;
        Ok(input)
    }
}

fn load_mock_sources(mock_paths: &[String]) -> Result<Vec<String>> {
    let mut mock_sources = Vec::with_capacity(mock_paths.len());
    for path in mock_paths {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read mock source file at {}", path))?;
        mock_sources.push(content);
    }
    Ok(mock_sources)
}

struct CodeGenInputs<'a> {
    source_code: &'a str,
    mock_sources: &'a [String],
    manifest_content: Option<&'a str>,
    target_name: Option<&'a str>,
    cut_component_name: Option<&'a str>,
    mock_paths: &'a [String],
}

fn process_pipeline(inputs: CodeGenInputs<'_>) -> Result<generator::GeneratedArtifacts> {
    let parse_result = parse_source(inputs.source_code, inputs.mock_sources)
        .map_err(|err| anyhow::anyhow!("Failed to parse TRF AST annotations: {}", err))?;

    let manifest = inputs
        .manifest_content
        .map(|content| {
            serde_json::from_str::<ComponentManifest>(content)
                .context("Failed to parse component manifest JSON")
        })
        .transpose()?;

    let auto_routes = check_consistency(&parse_result, manifest.as_ref()).map_err(|errors| {
        let error_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        anyhow::anyhow!("Manifest linting errors:\n{}", error_messages.join("\n"))
    })?;

    let generated_artifacts = generate_artifacts(
        &parse_result,
        &auto_routes,
        inputs.target_name,
        inputs.cut_component_name,
        inputs.mock_paths,
    )
    .context("Failed to generate code and manifest artifacts")?;

    Ok(generated_artifacts)
}

fn run_pipeline(args: Args) -> Result<()> {
    let source_code = load_primary_source(args.source.as_ref())?;
    let mock_sources = load_mock_sources(&args.mock)?;

    let manifest_content = args
        .manifest
        .as_ref()
        .or(args.manifest_pos.as_ref())
        .map(|path| {
            std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read manifest file at {}", path))
        })
        .transpose()?;

    let inputs = CodeGenInputs {
        source_code: &source_code,
        mock_sources: &mock_sources,
        manifest_content: manifest_content.as_deref(),
        target_name: args.target_name.as_deref(),
        cut_component_name: args.cut_component_name.as_deref(),
        mock_paths: &args.mock,
    };

    let generated_artifacts = process_pipeline(inputs)?;

    if let Some(dir) = args.out_dir {
        let out_path = std::path::Path::new(&dir);
        let fidl_dir = out_path.join("fidl");
        let src_dir = out_path.join("src");
        let cml_dir = out_path.join("cml");

        std::fs::create_dir_all(&fidl_dir)
            .with_context(|| format!("Failed to create fidl directory in {}", dir))?;
        std::fs::create_dir_all(&src_dir)
            .with_context(|| format!("Failed to create src directory in {}", dir))?;
        std::fs::create_dir_all(&cml_dir)
            .with_context(|| format!("Failed to create cml directory in {}", dir))?;

        for (i, (path, mock_source)) in args.mock.iter().zip(mock_sources).enumerate() {
            if let Some(stem) = std::path::Path::new(path).file_stem().and_then(|s| s.to_str()) {
                let filename = format!("mock_{i}_{stem}.rs");
                std::fs::write(src_dir.join(filename), mock_source)
                    .context("Failed to write mock source file")?;
            }
        }

        std::fs::write(fidl_dir.join("mock_control.fidl"), &generated_artifacts.mock_control_fidl)
            .context("Failed to write mock_control.fidl")?;
        std::fs::write(
            src_dir.join("injectable_universe.rs"),
            &generated_artifacts.injectable_universe_rs,
        )
        .context("Failed to write injectable_universe.rs")?;
        std::fs::write(
            cml_dir.join("injectable_universe.cml"),
            &generated_artifacts.injectable_universe_cml,
        )
        .context("Failed to write injectable_universe.cml")?;
        std::fs::write(cml_dir.join("test_driver.cml"), &generated_artifacts.test_driver_cml)
            .context("Failed to write test_driver.cml")?;
        std::fs::write(
            src_dir.join("realm_builder_generated.rs"),
            &generated_artifacts.realm_builder_rs,
        )
        .context("Failed to write realm_builder_generated.rs")?;
        std::fs::write(src_dir.join("realm_factory.rs"), &generated_artifacts.realm_factory_rs)
            .context("Failed to write realm_factory.rs")?;
        std::fs::write(cml_dir.join("realm_factory.cml"), &generated_artifacts.realm_factory_cml)
            .context("Failed to write realm_factory.cml")?;
        std::fs::write(cml_dir.join("test_root.cml"), &generated_artifacts.test_root_cml)
            .context("Failed to write test_root.cml")?;
    }

    Ok(())
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();
    run_pipeline(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_mock_sources_empty() {
        let result = load_mock_sources(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_process_pipeline_validates_basic_flow() {
        let source_code = "
            #[trf::test]
            fn test_foo() {}
        ";
        let inputs = CodeGenInputs {
            source_code,
            mock_sources: &[],
            manifest_content: None,
            target_name: Some("test_foo"),
            cut_component_name: Some("test_foo"),
            mock_paths: &[],
        };
        let result = process_pipeline(inputs);
        assert!(result.is_ok());
    }
}
