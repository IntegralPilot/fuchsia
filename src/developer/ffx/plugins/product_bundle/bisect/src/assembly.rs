// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembled_system::AssembledSystem;
use assembly_artifact_cache::{ArtifactCache, ArtifactType, MOSIdentifier, Slot};
use assembly_cli_args::{ProductArgs, ValidationMode};
use assembly_config_schema::Architecture;
use assembly_container::AssemblyContainer;
use assembly_tool::PlatformToolProvider;
use camino::{Utf8Path, Utf8PathBuf};
use ffx_config::EnvironmentContext;
use product_bundle::{ProductBundleBuilder, Slot as PBSlot};
use tempfile::tempdir;

/// Run assembly with the given collection of assembly artifacts
/// to generate a flashable fuchsia image.
pub async fn assemble(
    artifacts: &[MOSIdentifier],
    cache: &ArtifactCache,
    env_context: &EnvironmentContext,
    pb_name: &str,
    outdir: &Utf8Path,
    gendir: Utf8PathBuf,
    slot: Slot,
    mut print_fn: impl FnMut(&str),
) -> Result<Utf8PathBuf> {
    // Find the specific artifacts required for assembly
    let platform_artifact = find_unique_artifact(artifacts, ArtifactType::Platform, slot)?;
    let product_artifact = find_unique_artifact(artifacts, ArtifactType::Product, slot)?;
    let board_artifact = find_unique_artifact(artifacts, ArtifactType::Board, slot)?;

    // Ensure all of the artifacts have been downloaded.
    let platform_config_path =
        cache.resolve_platform(Some(platform_artifact.id()), &Architecture::X64).await?;
    let product_config_path = cache.resolve_product(product_artifact.id()).await?;
    let board_config_path = cache.resolve_board(board_artifact.id()).await?;

    print_fn(&format!("Assembling into {} ...", outdir));

    let tools = PlatformToolProvider::new(platform_config_path.clone());

    let tmp = tempdir().context("Creating temporary directory for assembly")?;
    let tmp_path = Utf8Path::from_path(tmp.path())
        .context("Creating Utf8Path pointing to the temporary directory")?;
    let system_outdir = tmp_path.join("system");

    // Perform assembly
    let product_args = ProductArgs {
        product: product_config_path,
        board_config: board_config_path,
        outdir: system_outdir,
        gendir,
        platform_artifacts: Some(platform_config_path),
        input_bundles_dir: None,
        package_validation: Some(ValidationMode::Off),
        custom_kernel_aib: None,
        custom_boot_shim_aib: None,
        suppress_overrides_warning: false,
        developer_overrides: None,
        include_example_aib_for_tests: Some(false),
        mode: Default::default(),
        board_input_bundle_sets: vec![],
        product_input_bundles: vec![],
    };

    let create_system_outputs = assembly_api::assemble(env_context, product_args)?;
    let system = AssembledSystem::from_dir(&create_system_outputs.outdir)
        .context("Loading system instance from assembly output directory")?;

    let pb_slot = match slot {
        Slot::A => PBSlot::A,
        Slot::R => PBSlot::R,
    };
    let builder = ProductBundleBuilder::new(pb_name).system(system, pb_slot).update_package(1);

    builder.build(Box::new(tools), outdir).await?;
    print_fn("Assembly complete.");

    Ok(outdir.to_path_buf())
}

// Ensure there is only one artifact of type "artifact_type" in the given
// list of artifacts with the given slot.
fn find_unique_artifact<'a>(
    artifacts: &'a [MOSIdentifier],
    artifact_type: ArtifactType,
    slot: Slot,
) -> Result<&'a MOSIdentifier> {
    let mut matches =
        artifacts.iter().filter(|a| a.artifact_type == artifact_type && a.slot == slot);

    let artifact = matches.next().ok_or_else(|| {
        anyhow::anyhow!("No {} artifact found for slot {:?}", artifact_type, slot)
    })?;

    if matches.next().is_some() {
        anyhow::bail!(
            "Multiple {} artifacts found for slot {:?}, but exactly one is required.",
            artifact_type,
            slot
        );
    }

    Ok(artifact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mock_mos(artifact_type: ArtifactType, version: &str, slot: Slot) -> MOSIdentifier {
        MOSIdentifier {
            artifact_type,
            name: "test_artifact".to_string(),
            version: version.to_string(),
            repository: "fuchsia".to_string(),
            cipd: None,
            slot,
        }
    }

    #[test]
    fn test_find_unique_artifact_success() {
        let artifacts = vec![
            make_mock_mos(ArtifactType::Platform, "1", Slot::A),
            make_mock_mos(ArtifactType::Platform, "1", Slot::R), // Different slot
            make_mock_mos(ArtifactType::Product, "1", Slot::A),  // Different type
        ];

        let result = find_unique_artifact(&artifacts, ArtifactType::Platform, Slot::A);
        let artifact = result.unwrap();
        assert_eq!(artifact.version, "1");
        assert_eq!(artifact.slot, Slot::A);
        assert_eq!(artifact.slot, Slot::A);
    }

    #[test]
    fn test_find_unique_artifact_missing() {
        let artifacts = vec![
            make_mock_mos(ArtifactType::Platform, "1", Slot::R), // Only slot R exists
            make_mock_mos(ArtifactType::Product, "1", Slot::A),
        ];

        let result = find_unique_artifact(&artifacts, ArtifactType::Platform, Slot::A);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No platform artifact found"));
    }

    #[test]
    fn test_find_unique_artifact_multiple() {
        let artifacts = vec![
            make_mock_mos(ArtifactType::Platform, "1", Slot::A),
            make_mock_mos(ArtifactType::Platform, "2", Slot::A), // Duplicate slot A
        ];

        let result = find_unique_artifact(&artifacts, ArtifactType::Platform, Slot::A);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Multiple platform artifacts found"));
    }
}
