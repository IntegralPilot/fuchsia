// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use assembled_system::AssembledSystem;
use assembly_api::release_info::*;
use assembly_artifact_cache::{ArtifactCache, ArtifactError};
use assembly_cli_args::{AssemblyMode, ProductArgs, ValidationMode};
use assembly_container::AssemblyContainer;
use assembly_release_info::{BoardReleaseInfo, ProductReleaseInfo, ReleaseInfo};
use camino::Utf8PathBuf;
use ffx_config::EnvironmentContext;

pub struct Assembly {
    pub platform_path: Utf8PathBuf,
    pub platform_release_info: ReleaseInfo,
    pub product_config_path: Utf8PathBuf,
    pub product_config_release_info: ProductReleaseInfo,
    pub board_config_path: Utf8PathBuf,
    pub board_config_release_info: BoardReleaseInfo,
    pub bib_sets: Vec<Utf8PathBuf>,
    pub bib_set_release_infos: Vec<ReleaseInfo>,
    pub pibs: Vec<Utf8PathBuf>,
    pub pib_release_infos: Vec<ReleaseInfo>,
}

impl Assembly {
    pub async fn new(
        cache: &ArtifactCache,
        platform: Option<String>,
        product_config: String,
        board_config: String,
        bib_sets: Vec<String>,
        pibs: Vec<String>,
    ) -> Result<Self, ArtifactError> {
        let product_config_path = cache.resolve_product(product_config).await?;
        let product_config_release_info = load_product_release_info(&product_config_path)?;

        let board_config_path = cache.resolve_board(board_config).await?;

        let bib_sets: Vec<Utf8PathBuf> = futures::future::try_join_all(
            bib_sets.into_iter().map(|bib_set| async move { cache.resolve_bib_set(bib_set).await }),
        )
        .await?;

        let bib_set_release_infos =
            bib_sets.iter().map(|p| load_bib_set_release_info(p)).collect::<Result<Vec<_>, _>>()?;

        let pibs: Vec<Utf8PathBuf> = futures::future::try_join_all(
            pibs.into_iter().map(|pib| async move { cache.resolve_pib(pib).await }),
        )
        .await?;

        let pib_release_infos =
            pibs.iter().map(|p| load_pib_release_info(p)).collect::<Result<Vec<_>, _>>()?;

        let board_config_release_info = load_board_release_info(&board_config_path)?;
        let arch: assembly_config_schema::board_config::Architecture =
            load_board_arch(&board_config_path)?.parse()?;

        let platform_path = cache.resolve_platform(platform, &arch).await?;
        let platform_release_info = load_platform_release_info(&platform_path)?;

        Ok(Self {
            platform_path,
            platform_release_info,
            product_config_path,
            product_config_release_info,
            board_config_path,
            board_config_release_info,
            bib_sets,
            bib_set_release_infos,
            pibs,
            pib_release_infos,
        })
    }

    pub fn version_string(&self) -> String {
        let mut base = format!(
            "\tplatform: {}@{}\n\tproduct_config: {}@{}\n\tboard_config: {}@{}",
            self.platform_release_info.name,
            self.platform_release_info.version,
            self.product_config_release_info.info.name,
            self.product_config_release_info.info.version,
            self.board_config_release_info.info.name,
            self.board_config_release_info.info.version,
        );
        for bib_set in &self.bib_set_release_infos {
            base.push_str(&format!("\n\tbib_set: {}@{}", bib_set.name, bib_set.version));
        }
        for pib in &self.pib_release_infos {
            base.push_str(&format!("\n\tpib: {}@{}", pib.name, pib.version));
        }
        base
    }

    pub async fn create_system(
        self,
        context: &EnvironmentContext,
        should_configure_example: bool,
        zbi_only: bool,
        developer_overrides: Option<Utf8PathBuf>,
        outdir: &Utf8PathBuf,
    ) -> Result<AssembledSystem> {
        let gendir = tempfile::TempDir::new().unwrap();
        let gendir = Utf8PathBuf::from_path_buf(gendir.path().to_path_buf()).unwrap();

        let args = ProductArgs {
            product: self.product_config_path,
            board_config: self.board_config_path,
            outdir: outdir.clone(),
            gendir,
            platform_artifacts: Some(self.platform_path),
            input_bundles_dir: None,
            package_validation: Some(ValidationMode::Off),
            custom_kernel_aib: None,
            custom_boot_shim_aib: None,
            suppress_overrides_warning: false,
            developer_overrides,
            include_example_aib_for_tests: Some(should_configure_example),
            mode: if zbi_only { AssemblyMode::SkipFilesystems } else { Default::default() },
            board_input_bundle_sets: self.bib_sets.clone(),
            product_input_bundles: self.pibs,
        };
        let create_system_outputs = assembly_api::assemble(context, args)?;
        AssembledSystem::from_dir(create_system_outputs.outdir)
    }
}
