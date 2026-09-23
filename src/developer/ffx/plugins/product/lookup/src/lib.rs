// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A tool to:
//! - lookup product bundle description information to find the transfer URL.

use ffx_config::EnvironmentContext;
use ffx_product::{CommandStatus, MachineOutput, MachineUi};
use ffx_product_list::{ProductBundle, pb_list_impl};
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, Result, bug, return_user_error};
use pbms::AuthFlowChoice;
use safe_string::TermSafe;
use std::io::{Write, stdin, stdout};

mod args;
pub use args::LookupCommand;

#[derive(FfxTool)]
pub struct PbLookupTool {
    #[command]
    cmd: LookupCommand,

    context: EnvironmentContext,
}

#[async_trait::async_trait(?Send)]
impl FfxMain for PbLookupTool {
    type Writer = VerifiedMachineWriter<MachineOutput<ProductBundle>>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> Result<()> {
        if writer.is_machine() {
            self.do_machine_main(writer).await
        } else {
            self.do_text_main(writer).await
        }
    }
}

impl PbLookupTool {
    async fn do_machine_main(&self, writer: <PbLookupTool as fho::FfxMain>::Writer) -> Result<()> {
        let ui = MachineUi::new(writer);

        match pb_lookup_impl(
            &self.cmd.auth,
            &self.cmd.base_url,
            &self.cmd.name,
            &self.cmd.version,
            &ui,
            &self.context,
        )
        .await
        {
            Ok(product_bundle) => {
                ui.machine(MachineOutput::Data(product_bundle))?;
                return Ok(());
            }

            Err(e) => {
                ui.machine(MachineOutput::CommandStatus(CommandStatus::UnexpectedError {
                    message: e.to_string(),
                }))?;
                return Err(e.into());
            }
        }
    }

    async fn do_text_main(&self, mut writer: <PbLookupTool as fho::FfxMain>::Writer) -> Result<()> {
        let mut output = stdout();
        let mut err_out = writer.stderr();
        let mut input = stdin();
        let ui = structured_ui::TextUi::new(&mut input, &mut output, &mut err_out);
        let product = pb_lookup_impl(
            &self.cmd.auth,
            &self.cmd.base_url,
            &self.cmd.name,
            &self.cmd.version,
            &ui,
            &self.context,
        )
        .await?;

        writeln!(writer, "{}", TermSafe::from_str_escaped(&product.transfer_manifest_url))
            .map_err(|e| bug!("{e}"))?;
        Ok(())
    }
}

pub async fn pb_lookup_impl<I>(
    auth: &AuthFlowChoice,
    override_base_url: &Option<String>,
    name: &str,
    version: &str,
    ui: &I,
    context: &EnvironmentContext,
) -> Result<ProductBundle>
where
    I: structured_ui::Interface,
{
    let start = std::time::Instant::now();
    log::info!("---------------------- Lookup Begin ----------------------------");

    let products =
        pb_list_impl(auth, override_base_url.clone(), Some(version.to_string()), None, ui, context)
            .await?;

    log::debug!("Looking for product bundle {}, version {}", name, version);
    let mut products = products
        .iter()
        .filter(|x| x.name == name)
        .filter(|x| x.product_version == version)
        .map(|x| x.to_owned());

    let Some(product) = products.next() else {
        log::debug!("products {:?}", products);
        return_user_error!("Error: No product matching name {}, version {} found.", name, version);
    };

    if products.next().is_some() {
        log::debug!("products {:?}", products);
        return_user_error!(
            "More than one matching product found. The base-url may have poorly formed data."
        );
    }

    log::debug!("Total ffx product lookup runtime {} seconds.", start.elapsed().as_secs_f32());
    log::debug!("End");

    Ok(product)
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_config::TestEnv;
    use ffx_writer::{Format, TestBuffers};
    use std::fs::File;
    use std::path::Path;

    const PB_MANIFEST_NAME: &'static str = "product_bundles.json";
    const PRODUCT_BUNDLE_INDEX_KEY: &str = "product.index";

    async fn setup_test_env(path: &Path) -> TestEnv {
        ffx_config::test_env()
            .user_config(PRODUCT_BUNDLE_INDEX_KEY, path.to_str().unwrap())
            .build()
            .unwrap()
    }

    #[fuchsia::test]
    async fn test_pb_lookup_impl() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env = setup_test_env(&path).await;
        let mut f = File::create(&path).expect("file create");
        f.write_all(
            r#"[{
            "name": "fake_name",
            "product_version": "fake_version",
            "transfer_manifest_url": "fake_url"
            }]"#
            .as_bytes(),
        )
        .expect("write_all");

        let ui = structured_ui::MockUi::new();
        let product = pb_lookup_impl(
            &AuthFlowChoice::Default,
            &Some(format!("file:{}", tmp.path().display())),
            "fake_name",
            "fake_version",
            &ui,
            &env.context,
        )
        .await
        .expect("testing lookup");

        assert_eq!(
            product,
            ProductBundle {
                name: "fake_name".into(),
                product_version: "fake_version".into(),
                transfer_manifest_url: "fake_url".into(),
            },
        );
    }

    #[fuchsia::test]
    async fn test_bp_lookup_machine_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env = setup_test_env(&path).await;
        let mut f = File::create(&path).expect("file create");
        f.write_all(
            r#"[{
            "name": "fake_name",
            "product_version": "fake_version",
            "transfer_manifest_url": "fake_url"
            }]"#
            .as_bytes(),
        )
        .expect("write_all");

        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(Some(Format::Json), &buffers);
        let tool = PbLookupTool {
            cmd: LookupCommand {
                auth: AuthFlowChoice::Default,
                base_url: Some(format!("file:{}", tmp.path().display())),
                name: "fake_name".into(),
                version: "fake_version".into(),
            },
            context: env.context.clone(),
        };

        tool.main(writer).await.expect("testing lookup");

        let expected = serde_json::to_string(&MachineOutput::Data(ProductBundle {
            name: "fake_name".into(),
            product_version: "fake_version".into(),
            transfer_manifest_url: "fake_url".into(),
        }))
        .expect("serialize data");
        assert_eq!(buffers.into_stdout_str(), format!("{expected}\n"));
    }

    #[fuchsia::test]
    async fn test_pb_lookup_text_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env = setup_test_env(&path).await;
        let mut f = File::create(&path).expect("file create");
        f.write_all(
            r#"[{
            "name": "fake_name",
            "product_version": "fake_version",
            "transfer_manifest_url": "fake_url"
            }]"#
            .as_bytes(),
        )
        .expect("write_all");

        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(None, &buffers);
        let tool = PbLookupTool {
            cmd: LookupCommand {
                auth: AuthFlowChoice::Default,
                base_url: Some(format!("file:{}", tmp.path().display())),
                name: "fake_name".into(),
                version: "fake_version".into(),
            },
            context: env.context.clone(),
        };

        tool.main(writer).await.expect("testing lookup");
        assert_eq!(buffers.into_stdout_str(), "fake_url\n");
    }

    #[fuchsia::test]
    async fn test_pb_lookup_text_mode_sanitizes_escape_sequences() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env = setup_test_env(&path).await;
        let mut f = File::create(&path).expect("file create");
        f.write_all(
            r#"[{
            "name": "fake_name",
            "product_version": "fake_version",
            "transfer_manifest_url": "https://example.com/\u001b[31mmalicious\u001b[0m\r\n"
            }]"#
            .as_bytes(),
        )
        .expect("write_all");

        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(None, &buffers);
        let tool = PbLookupTool {
            cmd: LookupCommand {
                auth: AuthFlowChoice::Default,
                base_url: Some(format!("file:{}", tmp.path().display())),
                name: "fake_name".into(),
                version: "fake_version".into(),
            },
            context: env.context.clone(),
        };

        tool.main(writer).await.expect("testing lookup");
        let output = buffers.into_stdout_str();
        assert!(!output.contains('\x1b'), "output should not contain raw escape characters");
        assert_eq!(output, "https://example.com/\\u{1b}[31mmalicious\\u{1b}[0m\\r\\n\n");
    }
}
