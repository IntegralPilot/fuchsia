// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A tool to:
//! - list product bundle name to for specific SDK version.

use ::gcs::client::{Client, ProgressResponse};
use anyhow::{Context, Result};
use ffx_config::EnvironmentContext;
use ffx_config::sdk::{SdkVersion, in_tree_sdk_version};
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool, bug, return_user_error};
use gcs::gs_url::split_gs_url;
use maplit::hashmap;
use omaha_client::version::Version;
use pbms::{AuthFlowChoice, list_from_gcs, string_from_url};
use safe_string::TermSafe;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Write, stderr, stdin, stdout};
use std::str::FromStr;
use std::sync::LazyLock;
use structured_ui::{Notice, Presentation};

mod args;
pub use args::ListCommand;

const PB_MANIFEST_NAME: &'static str = "product_bundles.json";
const CONFIG_BASE_URLS: &'static str = "pbms.base_urls";

static BRANCH_TO_PREFIX_MAPPING: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        hashmap! {
            "f12" => "12.20230611.1",
            "f13" => "13.20230724.3",
            "f14" => "14.202308",
            "f15" => "15.20231018.3",
            "f16" => "16.20231130.3",
            "f17" => "17.20240122.0",
            "f18" => "18.20240225.3",
            "f19" => "19.20240327.3",
            "f20" => "20.20240604.3",
            "f21" => "21.20240626.3",
            "f22" => "22.20240807.5",
            "f23" => "23.20240903.6",
            "f24" => "24.20241015.6",
            "f25" => "25.20241125.0",
            "f26" => "26.20250124",
            "f27" => "27.20250501",
            "f28" => "28.20250730",
            "f29" => "29.20251028",
            "f30" => "30.20260128",
            "f31" => "31.20260504",
            "LATEST" => "32",
        }
    });

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
pub struct ProductBundle {
    pub name: String,
    pub product_version: String,
    pub transfer_manifest_url: String,
}

type ProductManifest = Vec<ProductBundle>;

/// `ffx product list` sub-command.
#[derive(FfxTool)]
pub struct ProductListTool {
    #[command]
    pub cmd: ListCommand,

    pub context: EnvironmentContext,
}

/// This plugin will get list the available product bundles.
#[async_trait::async_trait(?Send)]
impl FfxMain for ProductListTool {
    type Writer = MachineWriter<Vec<ProductBundle>>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let mut input = stdin();
        // Emit machine progress info to stderr so users can redirect it to /dev/null.
        let mut output = if writer.is_machine() {
            Box::new(stderr()) as Box<dyn Write + Send + Sync>
        } else {
            Box::new(stdout())
        };
        let mut err_out = stderr();
        let ui = structured_ui::TextUi::new(&mut input, &mut output, &mut err_out);

        let pbs = pb_list_impl(
            &self.cmd.auth,
            self.cmd.base_url,
            self.cmd.version,
            self.cmd.branch,
            &ui,
            &self.context,
        )
        .await?;
        if writer.is_machine() {
            writer.machine(&pbs)?;
        } else {
            let pb_names = pbs
                .iter()
                .map(|x| TermSafe::from_str_escaped(&x.name).to_string())
                .collect::<Vec<_>>();
            let pb_string = pb_names.join("\n");
            writeln!(writer, "{}", pb_string).map_err(|e| bug!(e))?;
        }

        Ok(())
    }
}

pub async fn resolve_branch_to_base_urls<I>(
    version: Option<String>,
    branch: Option<String>,
    auth: &AuthFlowChoice,
    ui: &I,
    client: &Client,
    context: &EnvironmentContext,
) -> fho::Result<Vec<String>>
where
    I: structured_ui::Interface,
{
    let mut use_in_tree_sdk = false;
    let prefix = match (branch, version) {
        (Some(_), Some(_)) => {
            return_user_error!("Cannot provide version and branch at the same time")
        }
        (None, Some(version)) => version,
        (Some(branch), None) => match BRANCH_TO_PREFIX_MAPPING.get(branch.as_str()) {
            Some(version) => version.to_string(),
            None => return_user_error!("Branch value is not supported!"),
        },
        (None, None) => {
            let sdk = context.get_sdk().context("getting sdk env context")?;
            match sdk.get_version() {
                // For version of SDK, we trim the version to date section.
                // e.g. if the version is 24.20241023.2.1, we will trim it
                // into 24.20241023. This is because the product version
                // does not always align with the sdk version.
                SdkVersion::Version(version) => version[..11].to_string(),
                SdkVersion::InTree => {
                    use_in_tree_sdk = true;
                    in_tree_sdk_version()
                }
                SdkVersion::Unknown => {
                    return_user_error!(
                        "Unable to determine SDK version. Please specify the version through '--version'"
                    )
                }
            }
        }
    };

    let mut result = Vec::new();
    let mut base_urls: Vec<String> = context.get(CONFIG_BASE_URLS).unwrap_or(vec![]);
    base_urls.sort();
    base_urls.dedup();
    for base_url in base_urls {
        if base_url.starts_with("file://") {
            result.push(base_url.to_owned());
            continue;
        }
        if use_in_tree_sdk {
            continue;
        }

        let (bucket, path) = split_gs_url(&base_url).context("Splitting gs URL.")?;
        let prefix = format!("{}/{}", path, prefix);
        if let Some(version) = get_latest_version(bucket, &prefix, auth, ui, &client).await? {
            result.push(format!("{}/{}", base_url, version));
        } else {
            log::debug!("No version found for {base_url}");
        }
    }

    Ok(result)
}

pub async fn get_latest_version<I>(
    bucket: &str,
    prefix: &str,
    auth: &AuthFlowChoice,
    ui: &I,
    client: &Client,
) -> Result<Option<String>>
where
    I: structured_ui::Interface,
{
    let list = list_from_gcs(bucket, prefix, auth, ui, &client)
        .await
        .with_context(|| "Listing the objects")?;
    let mut filtered_list = list
        .iter()
        .filter(|x| x.contains("product_bundles.json"))
        .map(|x| {
            let v = x
                .trim_start_matches("development/")
                .trim_end_matches("/product_bundles.json")
                .trim_end_matches("/sdk");
            Version::from_str(&v).expect("version cannot be parsed")
        })
        .collect::<Vec<_>>();
    filtered_list.sort();
    Ok(filtered_list.last().map(|v| v.to_string()))
}

pub async fn pb_list_impl<I>(
    auth: &AuthFlowChoice,
    override_base_url: Option<String>,
    version: Option<String>,
    branch: Option<String>,
    ui: &I,
    context: &EnvironmentContext,
) -> Result<Vec<ProductBundle>>
where
    I: structured_ui::Interface,
{
    let mut products = Vec::new();
    let client = Client::initial()?;

    let base_urls = if let Some(base_url) = override_base_url {
        vec![base_url]
    } else {
        resolve_branch_to_base_urls(version.clone(), branch, auth, ui, &client, &context).await?
    };

    for base_url in &base_urls {
        let mut prods =
            pb_gather_from_url(base_url, auth, ui, &client).await.unwrap_or_else(|_| {
                let mut notice: Notice = Notice::builder();
                notice.message(format!("Failed to fetch from base_url: {base_url}"));
                ui.present(&Presentation::Notice(notice)).expect("presenting to work");
                Vec::new()
            });
        if let Some(product_version) = &version {
            prods.retain(|p| p.product_version == *product_version);
        }
        products.extend(prods);
    }
    products.sort_by(|a, b| {
        format!("{}@{}", a.name, a.product_version)
            .cmp(&format!("{}@{}", b.name, b.product_version))
    });
    products.dedup_by_key(|i| format!("{}@{}", i.name, i.product_version));
    products.sort_by_key(|i| i.product_version.clone());
    products.reverse();
    Ok(products)
}

/// Fetch product bundle descriptions from a base URL.
async fn pb_gather_from_url<I>(
    base_url: &str,
    auth_flow: &AuthFlowChoice,
    ui: &I,
    client: &Client,
) -> fho::Result<Vec<ProductBundle>>
where
    I: structured_ui::Interface,
{
    log::debug!("transfer_manifest_url Url::parse");
    let mut manifest_url = match url::Url::parse(&base_url) {
        Ok(p) => p,
        _ => {
            return_user_error!("The lookup location must be a URL, failed to parse {:?}", base_url)
        }
    };
    gcs::gs_url::extend_url_path(&mut manifest_url, PB_MANIFEST_NAME)
        .with_context(|| format!("joining URL {:?} with file name", manifest_url))?;

    let pm = string_from_url(
        &manifest_url,
        auth_flow,
        &|state| {
            let mut progress = structured_ui::Progress::builder();
            progress.title("Getting product descriptions");
            progress.entry(&state.name, state.at, state.of, state.units);
            ui.present(&structured_ui::Presentation::Progress(progress))?;
            Ok(ProgressResponse::Continue)
        },
        ui,
        client,
    )
    .await
    .map_err(|e| bug!("string from gcs: {:?}: {e}", manifest_url))?;

    Ok(serde_json::from_str::<ProductManifest>(&pm)
        .map_err(|e| bug!("Parsing json {:?}: {e}", pm))?)
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_config::TestEnv;
    use ffx_writer::{Format, TestBuffers};
    use std::fs::File;

    async fn setup_test_env() -> TestEnv {
        ffx_config::test_env().user_config(CONFIG_BASE_URLS, serde_json::json!([])).build().unwrap()
    }

    #[fuchsia::test]
    async fn test_pb_list_impl() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env = setup_test_env().await;
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
        let pbs = pb_list_impl(
            &AuthFlowChoice::Default,
            Some(format!("file:{}", tmp.path().display())),
            None,
            None,
            &ui,
            &env.context,
        )
        .await
        .expect("testing list");
        assert_eq!(
            vec![ProductBundle {
                name: String::from("fake_name"),
                product_version: String::from("fake_version"),
                transfer_manifest_url: String::from("fake_url")
            }],
            pbs
        );
    }

    #[fuchsia::test]
    async fn test_pb_list_impl_filter_based_on_version() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env = setup_test_env().await;
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
        let pbs = pb_list_impl(
            &AuthFlowChoice::Default,
            None,
            Some("24.20240910.2.1".into()),
            None,
            &ui,
            &env.context,
        )
        .await
        .expect("testing list");
        assert!(pbs.is_empty());
    }

    #[fuchsia::test]
    async fn test_pb_list_impl_machine_code() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env = setup_test_env().await;
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
        let writer = MachineWriter::new_test(Some(Format::Json), &buffers);
        let tool = ProductListTool {
            cmd: ListCommand {
                auth: AuthFlowChoice::Default,
                base_url: Some(format!("file:{}", tmp.path().display())),
                version: None,
                branch: None,
            },
            context: env.context.clone(),
        };

        tool.main(writer).await.expect("testing list");

        let pbs: Vec<ProductBundle> = serde_json::from_str(&buffers.into_stdout_str()).unwrap();
        assert_eq!(
            vec![ProductBundle {
                name: String::from("fake_name"),
                product_version: String::from("fake_version"),
                transfer_manifest_url: String::from("fake_url")
            }],
            pbs
        );
    }

    #[fuchsia::test]
    async fn test_pb_list_impl_machine_code_ignore_unknown_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env: TestEnv = setup_test_env().await;
        let mut f = File::create(&path).expect("file create");
        f.write_all(
            r#"[{
            "name": "fake_name",
            "product_version": "fake_version",
            "transfer_manifest_url": "fake_url",
            "transfer_manifest_path": "not_used_path"
            }]"#
            .as_bytes(),
        )
        .expect("write_all");

        let buffers = TestBuffers::default();
        let writer = MachineWriter::new_test(Some(Format::Json), &buffers);
        let tool = ProductListTool {
            cmd: ListCommand {
                auth: AuthFlowChoice::Default,
                base_url: Some(format!("file:{}", tmp.path().display())),
                version: Some("fake_version".into()),
                branch: None,
            },
            context: env.context.clone(),
        };

        tool.main(writer).await.expect("testing list");

        let pbs: Vec<ProductBundle> = serde_json::from_str(&buffers.into_stdout_str()).unwrap();
        assert_eq!(
            vec![ProductBundle {
                name: String::from("fake_name"),
                product_version: String::from("fake_version"),
                transfer_manifest_url: String::from("fake_url")
            }],
            pbs
        );
    }

    #[fuchsia::test]
    async fn test_pb_list_impl_text_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env: TestEnv = setup_test_env().await;
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
        let writer = MachineWriter::new_test(None, &buffers);
        let tool = ProductListTool {
            cmd: ListCommand {
                auth: AuthFlowChoice::Default,
                base_url: Some(format!("file:{}", tmp.path().display())),
                version: None,
                branch: None,
            },
            context: env.context.clone(),
        };

        tool.main(writer).await.expect("testing list");
        assert_eq!(buffers.into_stdout_str(), "fake_name\n");
    }

    #[fuchsia::test]
    async fn test_pb_list_impl_text_mode_sanitizes_escape_sequences() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PB_MANIFEST_NAME);
        let env: TestEnv = setup_test_env().await;
        let mut f = File::create(&path).expect("file create");
        f.write_all(
            r#"[{
            "name": "\u001b[31mmalicious_name\u001b[0m\r\n",
            "product_version": "fake_version",
            "transfer_manifest_url": "fake_url"
            }]"#
            .as_bytes(),
        )
        .expect("write_all");

        let buffers = TestBuffers::default();
        let writer = MachineWriter::new_test(None, &buffers);
        let tool = ProductListTool {
            cmd: ListCommand {
                auth: AuthFlowChoice::Default,
                base_url: Some(format!("file:{}", tmp.path().display())),
                version: None,
                branch: None,
            },
            context: env.context.clone(),
        };

        tool.main(writer).await.expect("testing list");
        let output = buffers.into_stdout_str();
        assert!(!output.contains('\x1b'), "output should not contain raw escape characters");
        assert_eq!(output, "\\u{1b}[31mmalicious_name\\u{1b}[0m\\r\\n\n");
    }
}
