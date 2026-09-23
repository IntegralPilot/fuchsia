// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod assembly;

use anyhow::{Context, Result, anyhow};
use assembly::Assembly;
use assembly_artifact_cache::{ArtifactCache, ArtifactError};
use assembly_tool::PlatformToolProvider;
use assembly_util::{fast_copy, shorten_path};
use camino::Utf8PathBuf;
use delivery_blob::DeliveryBlobType;
use errors::FfxError;
use ffx_config::EnvironmentContext;
use ffx_product_bundle_create_args::CreateCommand;
use ffx_writer::{SimpleWriter, ToolIO};
use fho::{FfxMain, FfxTool};
use gcs;
use pbms;
use product_bundle::{ProductBundleBuilder, Slot};
use structured_ui;
use tempfile::tempdir;

#[derive(FfxTool)]
pub struct ProductBundleCreateTool {
    #[command]
    pub cmd: CreateCommand,
    ctx: EnvironmentContext,
}

fho::embedded_plugin!(ProductBundleCreateTool);

/// Create a fuchsia product bundle.
#[async_trait::async_trait(?Send)]
impl FfxMain for ProductBundleCreateTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let build_dir = self
            .ctx
            .build_dir()
            .map(|p| {
                Utf8PathBuf::from_path_buf(p.to_path_buf()).map_err(|build_dir| {
                    fho::bug!("Failed to parse build_dir as utf8: {}", build_dir.display())
                })
            })
            .transpose()?;
        product_bundle_create(&self.ctx, self.cmd, build_dir, writer)
            .await
            .map_err(flatten_error_sources)
    }
}

/// Create a fuchsia product bundle and return an anyhow result.
/// This allows us to work with anyhow, and map to a fho result above.
async fn product_bundle_create(
    context: &EnvironmentContext,
    cmd: CreateCommand,
    build_dir: Option<Utf8PathBuf>,
    writer: SimpleWriter,
) -> Result<(), ArtifactError> {
    let sanitized_cmd = cmd.try_into()?;
    Box::pin(sanitized_product_bundle_create(context, sanitized_cmd, build_dir, writer)).await
}

/// Convert the anyhow error into a pretty/stacked fho error.
fn flatten_error_sources(e: ArtifactError) -> fho::Error {
    let suggestion =
        e.suggestion.map(|s| format!("\n\nSuggestion: {}", s)).unwrap_or_else(|| "".to_string());
    FfxError::Error(
        anyhow::anyhow!(
            "Failed: {}{}{}",
            e.error,
            e.error
                .chain()
                .skip(1)
                .enumerate()
                .map(|(i, e)| format!("\n  {: >3}.  {}", i + 1, e))
                .collect::<Vec<String>>()
                .concat(),
            suggestion,
        ),
        -1,
    )
    .into()
}

/// All the inputs necessary to run `ffx product-bundle create` after checking
/// that the arguments were properly given.
#[derive(Debug)]
struct SanitizedCreateCommand {
    /// The platform artifacts to use.
    /// If None, then we use the default local artifacts.
    pub platform: Option<String>,

    /// The product config to use.
    pub product_config: String,

    /// The product config to use for the recovery image.
    pub recovery_product_config: Option<String>,

    /// The board config to use.
    pub board_config: String,

    /// The board config to use for the recovery image.
    pub recovery_board_config: Option<String>,

    /// The name to add to the output product bundle.
    pub output_name: Option<String>,

    /// The authentication flow to use to access googleapis.
    pub auth: pbms::AuthFlowChoice,

    /// The tuf keys to use.
    pub tuf_keys: Option<Utf8PathBuf>,

    /// path to a file specifying developer-level overrides for assembly.
    pub developer_overrides: Option<Utf8PathBuf>,

    /// What result we want from running `ffx product create`.
    pub result: CreateResult,

    /// Whether to only build the ZBI and skip filesystems.
    pub zbi_only: bool,

    /// The board input bundle sets to use.
    pub bib_sets: Vec<String>,

    /// The product input bundles to use.
    pub pibs: Vec<String>,
}

/// What result we want from running `ffx product create`.
#[derive(Debug)]
enum CreateResult {
    /// Stage the inputs, but do not run assembly.
    Stage,

    /// Run assembly and generate the outputs to this path.
    Out(Utf8PathBuf),

    /// Run assembly and output only what's required for the ZBI to this path.
    ZbiOnly(Utf8PathBuf),

    /// Run assembly and generates the outputs to the default location.
    Default,
}

/// Parse the input command, and ensure that the user formatted it correctly,
/// then return a new command with all the requirements for running
/// `ffx product create`.
impl TryFrom<CreateCommand> for SanitizedCreateCommand {
    type Error = anyhow::Error;

    fn try_from(cmd: CreateCommand) -> Result<Self> {
        let platform = cmd.platform;

        // Determine what result we want from this command.
        let result = match (cmd.stage, cmd.out, cmd.zbi_only) {
            (true, Some(_), _) => {
                anyhow::bail!("--stage and --out cannot be used together.")
            }
            (true, _, true) => {
                anyhow::bail!("--stage and --zbi-only cannot be used together.")
            }
            (_, None, true) => {
                anyhow::bail!("--out is required when --zbi-only is specified.");
            }
            (true, None, false) => CreateResult::Stage,
            (false, Some(out), true) => CreateResult::ZbiOnly(out),
            (false, Some(out), false) => CreateResult::Out(out),
            (false, None, false) => CreateResult::Default,
        };

        // Choose between a product_config.board_config combo and --product --board flags.
        let (product_config, board_config) =
            if let Some(combo) = cmd.product_config_board_config_combo {
                let (p, b) = combo
                    .split_once(".")
                    .context("product_config.board_config combo must have a period")?;
                (p.to_string(), b.to_string())
            } else {
                let p = cmd.product_config.context("--product-config is missing")?;
                let b = cmd.board_config.context("--board-config is missing")?;
                (p, b)
            };

        let CreateCommand {
            output_name,
            tuf_keys,
            developer_overrides,
            auth,
            recovery_product_config,
            recovery_board_config,
            zbi_only,
            bib_set,
            pib,
            ..
        } = cmd;

        if recovery_board_config.is_some() && recovery_product_config.is_none() {
            anyhow::bail!(
                "--recovery-product-config is required if --recovery-board-config is specified."
            );
        }

        Ok(Self {
            platform,
            product_config,
            recovery_product_config,
            board_config,
            recovery_board_config,
            output_name,
            auth,
            tuf_keys,
            developer_overrides,
            result,
            zbi_only,
            bib_sets: bib_set,
            pibs: pib,
        })
    }
}

fn default_path_for_product_bundle_name(name: impl AsRef<str>) -> Result<Utf8PathBuf> {
    let home = std::env::home_dir().context("Getting the home dir")?;
    let home = Utf8PathBuf::from_path_buf(home)
        .map_err(|e| anyhow!("error converting path to utf8: {:?}", e))?;
    Ok(home.join(".fuchsia").join("product_bundles").join(name.as_ref()))
}

/// Construct a product bundle using sanitized inputs.
async fn sanitized_product_bundle_create(
    context: &EnvironmentContext,
    cmd: SanitizedCreateCommand,
    build_dir: Option<Utf8PathBuf>,
    mut writer: SimpleWriter,
) -> Result<(), ArtifactError> {
    let tmp = tempdir().unwrap();
    let tmp_path = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

    let gcs_client = gcs::client::Client::initial().map_err(|e| {
        ArtifactError::new(anyhow::anyhow!("Failed to initialize GCS client: {}", e))
    })?;
    if !matches!(cmd.auth, pbms::AuthFlowChoice::NoAuth) {
        let token = pbms::handle_new_access_token(&cmd.auth, &structured_ui::MockUi::new())
            .await
            .context("Failed to handle or retrieve new access token")?;
        gcs_client.set_access_token(token).await;
    }

    let cache = ArtifactCache::new(build_dir, gcs_client)?;
    let assembly = Assembly::new(
        &cache,
        cmd.platform.clone(),
        cmd.product_config,
        cmd.board_config.clone(),
        cmd.bib_sets.clone(),
        cmd.pibs.clone(),
    )
    .await?;
    writer
        .line(format!("Staged the artifacts\n{}", assembly.version_string()))
        .map_err(|e| ArtifactError::new(anyhow::anyhow!("{}", e)))?;

    let name = cmd.output_name.unwrap_or_else(|| {
        let product_name = assembly.product_config_release_info.info.name.clone();
        let board_name = assembly.board_config_release_info.info.name.clone();
        format!("{}.{}", product_name, board_name)
    });

    // Return early if we are only staging the inputs.
    let out = match cmd.result {
        CreateResult::Stage => return Ok(()),
        CreateResult::Out(out) => Ok(out),
        CreateResult::ZbiOnly(out) => Ok(out),
        CreateResult::Default => default_path_for_product_bundle_name(&name),
    }?;

    writer
        .line(format!("Assembling into {} ...", out))
        .map_err(|e| ArtifactError::new(anyhow::anyhow!("{}", e)))?;
    let tools = PlatformToolProvider::new(assembly.platform_path.clone());
    let should_configure_example =
        context.get::<bool, _>("assembly_example_enabled").unwrap_or_default();
    let system = Box::pin(assembly.create_system(
        context,
        should_configure_example,
        cmd.zbi_only,
        cmd.developer_overrides.clone(),
        &tmp_path.join("system"),
    ))
    .await?;

    if cmd.zbi_only {
        let zbi_path = system
            .images
            .iter()
            .find_map(|i| match i {
                assembled_system::Image::ZBI { path, .. } => Some(path.clone()),
                _ => None,
            })
            .ok_or_else(|| anyhow!("No ZBI found in assembled system"))?;

        if !out.is_dir() {
            return Err(ArtifactError::new(anyhow::anyhow!("Zbi output path is not a directory")));
        }

        let dest_path = out.join("fuchsia.zbi");

        fast_copy(&zbi_path, &dest_path)
            .with_context(|| format!("copying zbi from {} to {}", zbi_path, dest_path))?;
        println!("{}", dest_path);
        return Ok(());
    }
    let mut builder =
        ProductBundleBuilder::new(name.clone()).system(system, Slot::A).update_package(1);

    if let Some(tuf_keys) = cmd.tuf_keys {
        builder = builder.repository(DeliveryBlobType::Type1, tuf_keys);
    }

    // Build the recovery image if requested.
    if let Some(recovery_product_config) = cmd.recovery_product_config {
        let recovery_assembly = Assembly::new(
            &cache,
            cmd.platform.clone(),
            recovery_product_config,
            cmd.recovery_board_config.unwrap_or_else(|| cmd.board_config.clone()),
            cmd.bib_sets.clone(),
            vec![],
        )
        .await?;
        let recovery_system = Box::pin(recovery_assembly.create_system(
            context,
            should_configure_example,
            /* zbi_only=*/ false,
            cmd.developer_overrides.clone(),
            &tmp_path.join("recovery_system"),
        ))
        .await?;
        builder = builder.system(recovery_system, Slot::R);
    }

    let _ = builder
        .build(Box::new(tools), &out)
        .await
        .map_err(|e| ArtifactError::new(anyhow::anyhow!("{}", e)))?;
    cache.purge()?;

    println!(
        "\nNext, try flashing this product bundle:\n\tffx target flash -b {}",
        shorten_path(&out)
    );
    println!(
        "\nOr archive the product bundle to share it with someone else:\
         \n\t(cd {} && zip -r ~/{}.zip *)",
        shorten_path(&out),
        name.clone(),
    );

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use pbms::AuthFlowChoice;

    #[test]
    fn test_recovery_board_config_requires_recovery_product_config() {
        let cmd = CreateCommand {
            product_config_board_config_combo: None,
            platform: None,
            product_config: Some("product".to_string()),
            recovery_product_config: None,
            board_config: Some("board".to_string()),
            recovery_board_config: Some("recovery_board".to_string()),
            output_name: None,
            tuf_keys: None,
            developer_overrides: None,
            stage: false,
            out: None,
            auth: AuthFlowChoice::Default,
            zbi_only: false,
            bib_set: vec![],
            pib: vec![],
        };

        let result = SanitizedCreateCommand::try_from(cmd);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "--recovery-product-config is required if --recovery-board-config is specified."
        );
    }

    #[test]
    fn test_recovery_board_config_success() {
        let cmd = CreateCommand {
            product_config_board_config_combo: None,
            platform: None,
            product_config: Some("product".to_string()),
            recovery_product_config: Some("recovery_product".to_string()),
            board_config: Some("board".to_string()),
            recovery_board_config: Some("recovery_board".to_string()),
            output_name: None,
            tuf_keys: None,
            developer_overrides: None,
            stage: false,
            out: None,
            auth: AuthFlowChoice::Default,
            zbi_only: false,
            bib_set: vec![],
            pib: vec![],
        };

        let result = SanitizedCreateCommand::try_from(cmd);
        assert!(result.is_ok());
        let sanitized = result.unwrap();
        assert_eq!(sanitized.recovery_board_config, Some("recovery_board".to_string()));
    }
}
