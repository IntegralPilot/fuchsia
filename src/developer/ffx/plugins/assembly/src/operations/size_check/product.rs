// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::operations::size_check::breakdown::{SizeBreakdown, SizeResult};
use crate::operations::size_check::visualization::generate_visualization;
use anyhow::{Context, Result, format_err};
use assembled_system::{AssembledSystem, BlobfsContents, Image};
use assembly_container::AssemblyContainer;
use camino::{Utf8Path, Utf8PathBuf};
use ffx_assembly_args::{AuthMode, ProductSizeCheckArgs};
use serde_json::json;
use std::fs;

use gcs::client::Client;
use gcs::gs_url::split_gs_url;

const TOTAL_BLOBFS_GERRIT_COMPONENT_NAME: &str = "Total BlobFS contents";
const TOTAL_RESOURCES_GERRIT_COMPONENT_NAME: &str = "Platform Resources";
const RESOURCES_CREEP_BUDGET: &u64 = &2097152; // 2.0 MiB

/// Verifies that the product budget is not exceeded.
pub async fn verify_product_budgets<W: std::io::Write>(
    writer: &mut W,
    args: ProductSizeCheckArgs,
) -> Result<bool> {
    let assembled_system = if args.assembly_manifest.starts_with("gs://") {
        let (bucket, object) = split_gs_url(args.assembly_manifest.as_str())?;
        let output_path = gcs_download(bucket, object, args.auth.clone())
            .await
            .context("download assembly manifest")?;
        AssembledSystem::from_config_path_relative_paths(output_path)?
    } else {
        AssembledSystem::from_config_path_relative_paths(&args.assembly_manifest)?
    };

    let blobfs_contents = match extract_blob_contents(&assembled_system) {
        Some(contents) => contents,
        None => {
            log::info!("No blobfs image was found in {}", args.assembly_manifest);
            return Ok(true);
        }
    };

    let max_contents_size = blobfs_contents.maximum_contents_size;
    let breakdown = SizeBreakdown::from_contents(blobfs_contents);
    let size = breakdown.calculate_size();
    let contents_fit = match max_contents_size {
        None => true,
        Some(max) => size.consumed_bytes <= max,
    };

    if let Some(size_breakdown_output) = args.size_breakdown_output {
        let lines = breakdown.get_print_lines();
        fs::write(
            size_breakdown_output,
            format!("{}\nTotal size: {} bytes", lines.join("\n"), size.consumed_bytes),
        )
        .context("writing size breakdown output")?;
    }

    if let Some(base_assembly_manifest) = args.base_assembly_manifest {
        let other_assembled_system = if base_assembly_manifest.starts_with("gs://") {
            let (bucket, object) = split_gs_url(base_assembly_manifest.as_str())?;
            let output_path = gcs_download(bucket, object, args.auth)
                .await
                .context("download base assembly manifest")?;
            AssembledSystem::from_config_path_relative_paths(output_path)?
        } else {
            AssembledSystem::from_config_path_relative_paths(&base_assembly_manifest)?
        };

        let other_blobfs_contents =
            extract_blob_contents(&other_assembled_system).ok_or_else(|| {
                format_err!(
                    "Attempted to diff with {} which does not contain a blobfs image",
                    base_assembly_manifest
                )
            })?;
        let other_breakdown = SizeBreakdown::from_contents(other_blobfs_contents);
        let diff = breakdown.diff(&other_breakdown);
        diff.print(writer).context("Failed to write product budget diff output")?;
    } else if args.verbose {
        breakdown.print(writer).context("Failed to write product budget breakdown output")?;
        writeln!(writer, "Total size: {} bytes", size.consumed_bytes)
            .context("Failed to write product budget output")?;
    }

    if let Some(gerrit_output) = args.gerrit_output {
        let max_contents_size = max_contents_size.ok_or_else(|| {
            format_err!("Cannot create gerrit report when max_contents_size is none")
        })?;
        let blobfs_creep_budget = args.blobfs_creep_budget.ok_or_else(|| {
            format_err!("Cannot create gerrit report when blobfs_creep_budget is none")
        })?;
        let platform_resources_budget = args.platform_resources_budget.ok_or_else(|| {
            format_err!("Cannot create gerrit report when platform_resources_budget is none")
        })?;
        fs::write(
            gerrit_output,
            serde_json::to_string(&create_gerrit_report(
                &size,
                max_contents_size,
                blobfs_creep_budget,
                platform_resources_budget,
            ))?,
        )
        .context("writing gerrit report")?;
    }

    if max_contents_size.is_none() && args.verbose {
        writeln!(
            writer,
            "Skipping size checks because maximum_contents_size is not specified for this product."
        )
        .context("Failed to write product budget output")?;
    }

    if let Some(visualization_dir) = args.visualization_dir {
        generate_visualization(&visualization_dir, &breakdown)?;
        if args.verbose {
            writeln!(writer, "Wrote visualization to {}", visualization_dir.join("index.html"))
                .context("Failed to write product budget output")?;
        }
    }

    if !contents_fit {
        writeln!(
            writer,
            "BlobFS contents size ({}) exceeds max_contents_size ({}).",
            size.consumed_bytes,
            max_contents_size.unwrap(), // Value is always present when budget is exceeded.
        )
        .context("Failed to write product budget output")?;
        if !args.verbose {
            writeln!(
                writer,
                "Run with --verbose to view the size breakdown of all packages and blobs, or run `fx size-check` in-tree."
            )
            .context("Failed to write product budget output")?;
        }
    }

    Ok(contents_fit)
}

async fn get_gcs_client_with_auth(auth_mode: AuthMode) -> Result<Client> {
    use pbms::{AuthFlowChoice, handle_new_access_token};

    let mut input = std::io::stdin();
    let mut output = std::io::stdout();
    let mut err_out = std::io::stderr();
    let ui = structured_ui::TextUi::new(&mut input, &mut output, &mut err_out);

    let auth_flow = match auth_mode {
        AuthMode::Default => AuthFlowChoice::Default,
        AuthMode::Pkce => AuthFlowChoice::Pkce,
        AuthMode::Exec(p) => AuthFlowChoice::Exec(p),
    };

    let access_token = handle_new_access_token(&auth_flow, &ui).await?;

    let client = Client::initial()?;
    client.set_access_token(access_token).await;
    Ok(client)
}

async fn gcs_download(bucket: &str, object: &str, auth_mode: AuthMode) -> Result<Utf8PathBuf> {
    let client = get_gcs_client_with_auth(auth_mode).await.context("getting gcs client")?;
    let output_path = Utf8Path::new("/tmp/fuchsia.json");
    client.fetch_without_progress(bucket, object, &output_path).await.context("Downloading")?;
    Ok(output_path.to_path_buf())
}

/// Extracts the blob contents from the assembled system.
fn extract_blob_contents(assembled_system: &AssembledSystem) -> Option<&BlobfsContents> {
    for image in &assembled_system.images {
        match image {
            Image::BlobFS { contents, .. } => {
                return Some(contents);
            }
            Image::FxfsSparse { contents, .. } => {
                return Some(contents);
            }
            _ => {}
        }
    }
    None
}

/// Builds a report with the gerrit size format.
fn create_gerrit_report(
    size: &SizeResult,
    max_contents_size: u64,
    blobfs_creep_budget: u64,
    platform_resources_budget: u64,
) -> serde_json::Value {
    json!({
        TOTAL_BLOBFS_GERRIT_COMPONENT_NAME: size.consumed_bytes - size.consumed_bytes_resources,
        format!("{TOTAL_BLOBFS_GERRIT_COMPONENT_NAME}.budget"): max_contents_size - platform_resources_budget,
        format!("{TOTAL_BLOBFS_GERRIT_COMPONENT_NAME}.creepBudget"): blobfs_creep_budget,
        TOTAL_RESOURCES_GERRIT_COMPONENT_NAME: size.consumed_bytes_resources,
        format!("{TOTAL_RESOURCES_GERRIT_COMPONENT_NAME}.budget"): platform_resources_budget,
        format!("{TOTAL_RESOURCES_GERRIT_COMPONENT_NAME}.creepBudget"): RESOURCES_CREEP_BUDGET,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembled_system::{PackageMetadata, PackageSetMetadata, PackagesMetadata};
    use assembly_release_info::SystemReleaseInfo;
    use serde_json::json;

    #[test]
    fn extract_blob_contents_test() -> Result<()> {
        let blobfs_contents = BlobfsContents {
            packages: PackagesMetadata {
                base: PackageSetMetadata {
                    metadata: vec![PackageMetadata {
                        name: "hello".to_string(),
                        manifest: "path".into(),
                        blobs: Default::default(),
                        abi_revision: Some(version_history::AbiRevision::from_u64(1234)),
                    }],
                },
                cache: PackageSetMetadata { metadata: vec![] },
            },
            maximum_contents_size: Some(1234),
        };
        let mut assembled_system = AssembledSystem {
            images: vec![Image::VBMeta("a/b/c".into()), Image::FVM("x/y/z".into())],
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        assert_eq!(extract_blob_contents(&assembled_system), None);
        assembled_system
            .images
            .push(Image::BlobFS { path: "path/to/blob.blk".into(), contents: blobfs_contents });
        let blobfs_contents =
            extract_blob_contents(&assembled_system).expect("blobfs contents is found");
        assert_eq!(blobfs_contents.maximum_contents_size, Some(1234));
        Ok(())
    }

    #[test]
    fn gerrit_report_test() {
        let gerrit_report = create_gerrit_report(
            &SizeResult { consumed_bytes: 151, consumed_bytes_resources: 30 },
            200,
            20,
            50,
        );
        assert_eq!(
            gerrit_report,
            json!({
                TOTAL_BLOBFS_GERRIT_COMPONENT_NAME: 121,
                format!("{TOTAL_BLOBFS_GERRIT_COMPONENT_NAME}.budget"): 150,
                format!("{TOTAL_BLOBFS_GERRIT_COMPONENT_NAME}.creepBudget"): 20,
                TOTAL_RESOURCES_GERRIT_COMPONENT_NAME: 30,
                format!("{TOTAL_RESOURCES_GERRIT_COMPONENT_NAME}.budget"): 50,
                format!("{TOTAL_RESOURCES_GERRIT_COMPONENT_NAME}.creepBudget"): 2097152,
            })
        )
    }
}
