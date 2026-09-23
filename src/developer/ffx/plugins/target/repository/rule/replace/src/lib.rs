// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_fuchsia_pkg_rewrite::EngineProxy;
use fdomain_fuchsia_pkg_rewrite_ext::{RuleConfig, do_transaction};
use ffx_config::EnvironmentContext;
use ffx_target_repository_rule_replace_args::{JsonURI, ReplaceCommand};
use ffx_writer::VerifiedMachineWriter;
use fho::{Error, FfxMain, FfxTool, FhoEnvironment, Result, bug, return_user_error, user_error};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json;
use std::fs::File;
use std::io;
use target_holders::toolbox;
use url::Url;

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successfully waited for the target (either to come up or shut down).
    Ok {},
    /// Unexpected error with string denoting error message.
    UnexpectedError { message: String },
    /// A known error that can be reported to the user.
    UserError { message: String },
}

#[derive(FfxTool)]
pub struct ReplaceTool {
    #[command]
    cmd: ReplaceCommand,
    _fho_env: FhoEnvironment,
    _context: EnvironmentContext,
    #[with(toolbox())]
    engine_proxy: EngineProxy,
}

fho::embedded_plugin!(ReplaceTool);

#[async_trait(?Send)]
impl FfxMain for ReplaceTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        match self.replace_cmd().await {
            Ok(()) => {
                writer.machine(&CommandStatus::Ok {})?;
                Ok(())
            }
            Err(e @ Error::User(_)) => {
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(e)
            }
            Err(e) => {
                writer.machine(&CommandStatus::UnexpectedError { message: e.to_string() })?;
                Err(e)
            }
        }
    }
}

impl ReplaceTool {
    pub async fn replace_cmd(&self) -> Result<()> {
        let RuleConfig::Version1(ref rules) = match (&self.cmd.json_uri, &self.cmd.rule) {
            (Some(_), Some(_)) => {
                return_user_error!(
                    "the `--json-uri` and `--rule` arguments are mutually exclusive."
                );
            }
            (Some(uri), None) => rule_from_uri(uri).await?,
            (None, Some(s)) => serde_json::from_slice(s.as_bytes())
                .map_err(|e| user_error!("error parsing rule definition: {}", e))?,
            _ => {
                return Ok(());
            }
        };

        do_transaction(&self.engine_proxy, |transaction| {
            async move {
                transaction.reset_all()?;
                // add() inserts rules as highest priority, hence reverse iterate
                for rule in rules.iter().rev() {
                    let () = transaction.add(rule.clone()).await?;
                }
                Ok(transaction)
            }
        })
        .await
        .map_err(|err| bug!("failed to create transactions: {:#?}", err))?;

        Ok(())
    }
}

async fn rule_from_uri(json_uri: &JsonURI) -> Result<RuleConfig> {
    match json_uri {
        JsonURI::LocalFile(path) => {
            serde_json::from_reader(io::BufReader::new(File::open(path).map_err(|e| {
                user_error!("error reading rule definition file {}: {}", path.display(), e)
            })?))
            .map_err(|e| {
                user_error!("error parsing rule definition file {}: {}", path.display(), e)
            })
        }
        JsonURI::WebURL(url) => read_literal_rule_from_url(url).await,
    }
}

async fn read_literal_rule_from_url(url: &Url) -> Result<RuleConfig> {
    let https_client = fuchsia_hyper::new_https_client();
    let req = Request::builder()
        .method(Method::GET)
        .uri(url.as_str())
        .body(Full::<Bytes>::default())
        .map_err(|e| bug!("error building GET request for {}: {}", url, e))?;
    let res = https_client
        .request(req)
        .await
        .map_err(|e| user_error!("error fetching config file from {}: {}", url, e))?;
    if !res.status().is_success() {
        return_user_error!("http(s) request failed for {}: status {}", url, res.status());
    }
    let bytes = res
        .into_body()
        .collect()
        .await
        .map_err(|e| user_error!("error getting body from response after fetching {}: {}", url, e))?
        .to_bytes();
    serde_json::from_slice(&bytes)
        .map_err(|e| user_error!("error parsing config file from {}: {}", url, e))
}

#[cfg(test)]
mod test {
    use super::*;

    use fdomain_fuchsia_pkg_rewrite::{
        EditTransactionRequest, EngineRequest, Rule, RuleIteratorRequest,
    };

    use ffx_config::keys::TARGET_DEFAULT_KEY;
    use ffx_writer::TestBuffers;

    use futures::TryStreamExt;
    use std::path::PathBuf;
    use std::str::FromStr;
    use std::sync::Arc;
    use target_behavior::{ConnectionBehavior, setup_fake_resolution, target_interface};
    use target_holders::fake_proxy;

    const TARGET_NAME: &str = "some-target";
    const VALID_TEST_RULE: &str = r#"{
    "version": "1",
    "content": [
        {
            "host_match": "fuchsia.com",
            "host_replacement": "some.host",
            "path_prefix_match": "/",
            "path_prefix_replacement": "/"
        }
    ]
}"#;

    async fn setup_fake_engine_proxy(
        client: Arc<fdomain_client::Client>,
        expected_rule: Option<Rule>,
    ) -> EngineProxy {
        let repos = fake_proxy(client, move |req| match req {
            EngineRequest::StartEditTransaction { transaction, control_handle: _ } => {
                let expected_rule = expected_rule.clone();
                fuchsia_async::Task::local(async move {
                    let mut tx_stream = transaction.into_stream();

                    while let Some(req) = tx_stream.try_next().await.unwrap() {
                        match req {
                            EditTransactionRequest::ResetAll { control_handle: _ } => (),
                            EditTransactionRequest::ListDynamic { iterator, control_handle: _ } => {
                                let mut stream = iterator.into_stream();

                                while let Some(req) = stream.try_next().await.unwrap() {
                                    let RuleIteratorRequest::Next { responder } = req;
                                    responder.send(&[]).unwrap();
                                }
                            }
                            EditTransactionRequest::Add { rule, responder } => {
                                if let Some(Rule::Literal(ref expected)) = expected_rule {
                                    if let Rule::Literal(actual) = rule {
                                        if expected.host_match != actual.host_match {
                                            log::error!(
                                                "host_match expected {:?} got {:?}",
                                                expected.host_match,
                                                actual.host_match
                                            );
                                            responder.send(Err(-100)).unwrap();
                                            return;
                                        }
                                        if expected.host_replacement != actual.host_replacement {
                                            log::error!(
                                                "host_replacement expected {:?} got {:?}",
                                                expected.host_replacement,
                                                actual.host_replacement
                                            );
                                            responder.send(Err(-101)).unwrap();
                                            return;
                                        }
                                        if expected.path_prefix_match != actual.path_prefix_match {
                                            log::error!(
                                                "path_prefix_match expected {:?} got {:?}",
                                                expected.path_prefix_match,
                                                actual.path_prefix_match
                                            );
                                            responder.send(Err(-102)).unwrap();
                                            return;
                                        }
                                        if expected.path_prefix_replacement
                                            != actual.path_prefix_replacement
                                        {
                                            log::error!(
                                                "path_prefix_replacement expected {:?} got {:?}",
                                                expected.path_prefix_replacement,
                                                actual.path_prefix_replacement
                                            );
                                            responder.send(Err(-103)).unwrap();
                                            return;
                                        }
                                    }
                                }
                                responder.send(Ok(())).unwrap();
                            }
                            EditTransactionRequest::Commit { responder } => {
                                responder.send(Ok(())).unwrap();
                            }
                        }
                    }
                })
                .detach()
            }
            other => panic!("Unexpected request: {:?}", other),
        });
        repos
    }

    #[fuchsia::test]
    async fn test_empty_args() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env()
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let engine_proxy = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tool = ReplaceTool {
            cmd: ReplaceCommand { json_uri: None, rule: None },
            _fho_env: fho_env,
            _context: env.context.clone(),
            engine_proxy,
        };
        let buffers = TestBuffers::default();
        let writer = <ReplaceTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("replace ok");
    }

    #[fuchsia::test]
    async fn test_incomplete_json_structure() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env()
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let engine_proxy = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tool = ReplaceTool {
            cmd: ReplaceCommand {
                json_uri: None,
                rule: Some(
                    r#"{"version":"1","content":[{"host_match":"fuchsia.com"}]}"#.to_string(),
                ),
            },
            _fho_env: fho_env,
            _context: env.context.clone(),
            engine_proxy,
        };
        let buffers = TestBuffers::default();
        let writer = <ReplaceTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect_err("replace fail");
    }

    #[fuchsia::test]
    async fn test_valid_rule_via_command_line() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env()
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let engine_proxy = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tool = ReplaceTool {
            cmd: ReplaceCommand { json_uri: None, rule: Some(VALID_TEST_RULE.into()) },
            _fho_env: fho_env,
            _context: env.context.clone(),
            engine_proxy,
        };
        let buffers = TestBuffers::default();
        let writer = <ReplaceTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("replace ok");
    }

    #[fuchsia::test]
    async fn test_mutually_exclusive_command_line() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env()
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let engine_proxy = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tool = ReplaceTool {
            cmd: ReplaceCommand {
                json_uri: Some(JsonURI::LocalFile(PathBuf::from_str("some-file").unwrap())),
                rule: Some(VALID_TEST_RULE.into()),
            },
            _fho_env: fho_env,
            _context: env.context.clone(),
            engine_proxy,
        };
        let buffers = TestBuffers::default();
        let writer = <ReplaceTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect_err("replace fail");
    }

    #[fuchsia::test]
    async fn test_valid_rule_via_uri() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env()
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let engine_proxy = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let rule_file = tempfile::NamedTempFile::new().unwrap();

        std::fs::write(&rule_file, VALID_TEST_RULE).unwrap();

        let tool = ReplaceTool {
            cmd: ReplaceCommand {
                json_uri: Some(JsonURI::LocalFile(rule_file.path().into())),
                rule: None,
            },
            _fho_env: fho_env,
            _context: env.context.clone(),
            engine_proxy,
        };
        let buffers = TestBuffers::default();
        let writer = <ReplaceTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("replace ok");
    }
}
