// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_fuchsia_pkg::RepositoryManagerProxy;
use fdomain_fuchsia_pkg_rewrite::EngineProxy;
use ffx_config::EnvironmentContext;
use ffx_ssh::parse::HostAddr;
use ffx_target_repository_register_args::{JsonURI, RegisterCommand};
use ffx_writer::VerifiedMachineWriter;
use fho::{
    Error, FfxContext, FfxMain, FfxTool, FhoEnvironment, Result, TryFromEnv, bug,
    return_user_error, user_error,
};
use fidl_fuchsia_pkg_ext::{
    RepositoryConfig, RepositoryRegistrationAliasConflictMode, RepositoryTarget,
};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use pkg::repo::{RepoHostAddr, register_target_with_repo_instance};
use pkg::{PkgServerInfo, PkgServerInstanceInfo as _, PkgServerInstances};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::BTreeSet;
use std::fs::File;
use std::io;
use target_holders::{HostAddrHolder, toolbox};
use url::Url;
use zx_types::{ZX_ERR_ACCESS_DENIED, ZX_ERR_ALREADY_EXISTS, ZX_ERR_INVALID_ARGS};

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
pub struct RegisterTool {
    #[command]
    cmd: RegisterCommand,
    fho_env: FhoEnvironment,
    context: EnvironmentContext,
    #[with(toolbox())]
    repo_proxy: RepositoryManagerProxy,
    #[with(toolbox())]
    engine_proxy: EngineProxy,
}

fho::embedded_plugin!(RegisterTool);

#[async_trait(?Send)]
impl FfxMain for RegisterTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        match self.register_cmd().await {
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

const TUNNEL_REQUIRED_ERROR: &'static str = "Tunnel required";

impl RegisterTool {
    pub async fn register_cmd(&self) -> Result<()> {
        // If a config file or URL is provided, get the repository configuration onto the device.
        // This is equivalent to running `ssh target.device.tld pkgctl repo add ...`
        if let Some(json_uri) = &self.cmd.json_uri {
            let RegisterCommand {
                repository,
                port,
                address_override,
                storage_type,
                alias,
                alias_conflict_mode,
                json_uri: _,
            } = &self.cmd;
            if repository.is_none()
                && port.is_none()
                && address_override.is_none()
                && storage_type.is_none()
                && alias.is_empty()
                // alias_conflict_mode is always provided. args.gn passes "Replace" by default.
                && *alias_conflict_mode == RepositoryRegistrationAliasConflictMode::Replace
            {
                return self.register_target_with_json_config(&json_uri).await;
            } else {
                return_user_error!(
                    "the json_uri argument can only be used when the other options are omitted"
                );
            }
        }

        // Get the repository that should be registered.
        let instance_root = self
            .context
            .get("repository.process_dir")
            .map_err(|e: ffx_config::api::ConfigError| bug!(e))?;
        let mgr = PkgServerInstances::new(instance_root);

        let mut repo_name = if let Some(name) = &self.cmd.repository {
            Some(name.to_string())
        } else {
            pkg::config::get_default_repository(&self.context)
                .map_err(ffx_config::macro_deps::anyhow::Error::from)?
        }
        .ok_or_else(|| {
            user_error!(
                "A repository must be specfied via the --repository flag or \
            configured using 'ffx repository default set'"
            )
        })?;
        let repo_port = self.cmd.port;

        // if none was found, check for a product bundle repo server which has the prefix of repo_name.
        let pkg_server_info = match mgr
            .get_instance(repo_name.clone(), repo_port)
            .map_err(ffx_config::macro_deps::anyhow::Error::from)?
        {
            Some(instance) => Some(instance),
            None => {
                let instances =
                    mgr.list_instances().map_err(ffx_config::macro_deps::anyhow::Error::from)?;
                instances
                    .iter()
                    .find(|s| s.name.starts_with(&format!("{repo_name}.")))
                    .and_then(|s| Some(s.clone()))
            }
        };

        let target_spec = ffx_target::get_target_specifier(&self.context)
            .user_message("getting target specifier from config")?;

        // update the repo name if we matched a product bundle repo.
        if let Some(info) = pkg_server_info.as_ref() {
            repo_name = info.name.clone();
        }

        let repository_target = RepositoryTarget {
            repo_name: repo_name.clone(),
            target_identifier: target_spec.clone(),
            aliases: Some(BTreeSet::from_iter(self.cmd.alias.iter().map(|a| a.to_string()))),
            storage_type: self.cmd.storage_type.clone(),
        };

        if let Some(server_info) = pkg_server_info {
            self.register_standalone(&server_info, repository_target).await
        } else {
            return_user_error!(
                "{repo_name} is not a running repository, nor a daemon based repository."
            )
        }
    }

    async fn register_standalone(
        &self,
        info: &PkgServerInfo,
        mut repo_target_info: RepositoryTarget,
    ) -> Result<()> {
        repo_target_info.aliases = match repo_target_info.aliases {
            Some(aliases) if aliases.is_empty() => {
                Some(info.aliases().iter().map(ToString::to_string).collect())
            }
            None => Some(info.aliases().iter().map(ToString::to_string).collect()),
            Some(aliases) => Some(aliases),
        };

        let repo_host_addr = match &self.cmd.address_override {
            Some(addr_override) => addr_override.to_string(),
            None => {
                let host_addr = HostAddrHolder::try_from_env(&self.fho_env).await?;
                let host_address: Option<HostAddr> = host_addr.into();
                let host_address = host_address.map(|t| t.0);
                match pkg::repo::create_repo_host(info.address, host_address)
                    .bug_context("Failed to discover repository host")?
                {
                    RepoHostAddr::Direct(addr) => addr,
                    RepoHostAddr::Tunnel => {
                        return Err(fho::user_error!(TUNNEL_REQUIRED_ERROR));
                    }
                }
            }
        };

        register_target_with_repo_instance(
            self.repo_proxy.clone(),
            self.engine_proxy.clone(),
            &repo_target_info,
            &repo_host_addr,
            &info,
            self.cmd.alias_conflict_mode.clone(),
        )
        .await
        .map_err(|e| bug!("Failed to register repository: {:?}", e))
    }

    async fn register_target_with_json_config(&self, json_uri: &JsonURI) -> Result<()> {
        let repo = match json_uri {
            JsonURI::LocalFile(path) => {
                let repo: RepositoryConfig =
                    serde_json::from_reader(io::BufReader::new(File::open(path).map_err(|e| {
                        user_error!("error reading config file {}: {}", path.display(), e)
                    })?))
                    .map_err(|e| {
                        user_error!("error parsing config file {}: {}", path.display(), e)
                    })?;
                repo.into()
            }
            JsonURI::WebURL(url) => {
                let repo = fetch_config_file_from_url(url).await?;
                repo.into()
            }
        };
        self.repo_proxy
            .add(&repo).await
            .map_err(|e| user_error!("error registering repository with target: {}", e))?
            .map_err(|e| {
                match e {
                    ZX_ERR_ACCESS_DENIED => user_error!("editing repos on the target is disabled or the repository to be edited matches a statically configured repository"),
                    ZX_ERR_ALREADY_EXISTS => user_error!("the repository already exists"),
                    ZX_ERR_INVALID_ARGS => user_error!("the repository config is malformed"),
                    _ => user_error!("unexpected ZX_ERR return code received: {}", e),
                }
            }
        )
    }
}

async fn fetch_config_file_from_url(url: &Url) -> Result<RepositoryConfig> {
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
    use camino::Utf8PathBuf;

    use target_behavior::{ConnectionBehavior, setup_fake_resolution, target_interface};
    use target_holders::fake_proxy;
    use tempfile::TempDir;

    use fdomain_fuchsia_pkg::{MirrorConfig, RepositoryConfig, RepositoryManagerRequest};
    use fdomain_fuchsia_pkg_rewrite::{
        EditTransactionRequest, EngineRequest, LiteralRule, Rule, RuleIteratorRequest,
    };

    use ffx_config::keys::TARGET_DEFAULT_KEY;
    use ffx_target_repository_register_args::parse_json_uri;
    use ffx_writer::{Format, TestBuffers};

    use fidl_fuchsia_pkg_ext::{
        RepositoryConfigBuilder, RepositoryRegistrationAliasConflictMode, RepositoryStorageType,
    };

    use fuchsia_repo::repository::RepositorySpec;
    use fuchsia_url::RepositoryUrl;
    use futures::TryStreamExt;
    use futures::channel::oneshot::{Receiver, channel};
    use pkg::ServerMode;
    use std::collections::BTreeSet;
    use std::fs;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    const REPO_NAME: &str = "some-name";
    const TARGET_NAME: &str = "some-target";
    const TEST_CFG: &str = r#"{
  "repo_url": "fuchsia-pkg://some-repo",
  "root_version": 9,
  "root_threshold": 1,
  "root_keys": [
    {
      "type": "ed25519",
      "value": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  ],
  "mirrors": [
    {
      "mirror_url": "http://some-repo.org:8083/repo",
      "subscribe": true
    }
  ],
  "use_local_mirror": false,
  "repo_storage_type": "ephemeral"
}"#;

    async fn setup_fake_repo_proxy(
        client: Arc<fdomain_client::Client>,
        expected_config: Option<RepositoryConfig>,
        return_error: bool,
    ) -> (RepositoryManagerProxy, Receiver<Result<(), i32>>) {
        let (sender, receiver) = channel();
        let mut _sender = Some(sender);
        let repos = fake_proxy(client, move |req| match req {
            RepositoryManagerRequest::Add { repo, responder } => {
                if let Some(expected) = &expected_config {
                    if expected.repo_url != repo.repo_url {
                        log::error!("expected {:?} got {:?}", expected.repo_url, repo.repo_url);
                        responder.send(Err(-100)).unwrap();
                        return;
                    } else if expected.root_keys != repo.root_keys {
                        log::error!("expected {:?} got {:?}", expected.root_keys, repo.root_keys);
                        responder.send(Err(-101)).unwrap();
                        return;
                    } else if expected.mirrors != repo.mirrors {
                        log::error!("expected {:?} got {:?}", expected.mirrors, repo.mirrors);
                        responder.send(Err(-102)).unwrap();
                        return;
                    } else if expected.root_version != repo.root_version {
                        log::error!(
                            "expected {:?} got {:?}",
                            expected.root_version,
                            repo.root_version
                        );
                        responder.send(Err(-103)).unwrap();
                        return;
                    } else if expected.root_threshold != repo.root_threshold {
                        log::error!(
                            "expected {:?} got {:?}",
                            expected.root_threshold,
                            repo.root_threshold
                        );
                        responder.send(Err(-104)).unwrap();
                        return;
                    } else if expected.use_local_mirror != repo.use_local_mirror {
                        log::error!(
                            "expected {:?} got {:?}",
                            expected.use_local_mirror,
                            repo.use_local_mirror
                        );
                        responder.send(Err(-105)).unwrap();
                        return;
                    } else if expected.storage_type != repo.storage_type {
                        log::error!(
                            "expected {:?} got {:?}",
                            expected.storage_type,
                            repo.storage_type
                        );
                        responder.send(Err(-106)).unwrap();
                        return;
                    }
                }
                if return_error {
                    let err: i32 = 2;
                    responder.send(Err(err)).unwrap();
                } else {
                    responder.send(Ok(())).unwrap();
                }
            }
            other => panic!("Unexpected request: {:?}", other),
        });
        (repos, receiver)
    }

    async fn setup_fake_engine_proxy(
        client: Arc<fdomain_client::Client>,
        expected_rule: Option<Rule>,
    ) -> (EngineProxy, Receiver<Result<(), i32>>) {
        let (sender, receiver) = channel();
        let mut _sender = Some(sender);
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
        (repos, receiver)
    }

    async fn make_server_instance(
        root: &std::path::Path,
        server_mode: ServerMode,
        name: &str,
        aliases: BTreeSet<String>,
    ) -> Result<()> {
        let instance_root = root.join("repo_servers");

        let mgr = PkgServerInstances::new(instance_root);
        let repo_config = RepositoryConfigBuilder::new(
            RepositoryUrl::parse_host("name".into()).expect("repo url"),
        )
        .into();

        mgr.write_instance(&PkgServerInfo {
            name: name.into(),
            address: ([0, 0, 0, 0], 8888).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::from("/some/repo/path"), aliases },
            registration_storage_type: fidl_fuchsia_pkg_ext::RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut
                .into(),
            server_mode,
            pid: std::process::id(),
            repo_config,
        })
        .map_err(ffx_config::macro_deps::anyhow::Error::from)?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_register_standalone() {
        let client = fdomain_local::local_client_empty();

        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let aliases = vec![String::from("my-alias")];

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Foreground,
            REPO_NAME,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: Some(REPO_NAME.to_string()),
                port: None,
                alias: aliases.clone(),
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("register ok");
    }

    #[fuchsia::test]
    async fn test_register_standalone_product_bundle() {
        let client = fdomain_local::local_client_empty();
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .user_config("repository.default", "test-repo")
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("127.7.7.1".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let expected_config = RepositoryConfig {
            repo_url: Some("fuchsia-pkg://test-repo.fuchsia.com".into()),
            root_keys: Some(vec![]),
            mirrors: Some(vec![MirrorConfig {
                mirror_url: Some("http://127.7.7.1:8888/test-repo.fuchsia.com".into()),
                subscribe: Some(false),
                blob_mirror_url: None,
                ..Default::default()
            }]),
            root_version: Some(1),
            root_threshold: Some(1),
            use_local_mirror: Some(false),
            storage_type: Some(fdomain_fuchsia_pkg::RepositoryStorageType::Ephemeral),
            ..Default::default()
        };

        let expected_rule = Rule::Literal(LiteralRule {
            host_match: "fuchsia.com".into(),
            host_replacement: "test-repo.fuchsia.com".into(),
            path_prefix_match: "/".into(),
            path_prefix_replacement: "/".into(),
        });

        let (repo_proxy, _) =
            setup_fake_repo_proxy(Arc::clone(&client), Some(expected_config), false).await;
        let (engine_proxy, _) =
            setup_fake_engine_proxy(Arc::clone(&client), Some(expected_rule)).await;

        let mut aliases = BTreeSet::new();
        aliases.insert("fuchsia.com".into());
        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Foreground,
            "test-repo.fuchsia.com",
            aliases,
        )
        .await
        .expect("repo server instance");

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: None,
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        let res = tool.main(writer).await;
        match res {
            Ok(_) => (),
            Err(e) => assert!(false, "Unexpected error {e:?}"),
        }
    }

    #[fuchsia::test]
    async fn test_register_default_repository() {
        let client = fdomain_local::local_client_empty();
        let default_repo_name = "default-repo";
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.default", default_repo_name)
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Daemon,
            default_repo_name,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: None,
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("register ok");
    }

    #[fuchsia::test]
    async fn test_register_storage_type() {
        let client = fdomain_local::local_client_empty();
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let aliases = vec![String::from("my-alias")];

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Daemon,
            REPO_NAME,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: Some(REPO_NAME.to_string()),
                port: None,
                alias: aliases.clone(),
                storage_type: Some(RepositoryStorageType::Persistent),
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("register ok");
    }

    #[fuchsia::test]
    async fn test_register_empty_aliases() {
        let client = fdomain_local::local_client_empty();
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Daemon,
            REPO_NAME,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: Some(REPO_NAME.to_string()),
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("register ok");
    }

    #[fuchsia::test]
    async fn test_register_returns_error() {
        let client = fdomain_local::local_client_empty();
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Daemon,
            REPO_NAME,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, true).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: Some(REPO_NAME.to_string()),
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        let err = tool.main(writer).await.expect_err("register error");
        let want = "BUG: An internal command error occurred.\nError: Failed to register repository: RepositoryManagerError";
        assert_eq!(err.to_string(), want)
    }

    #[fuchsia::test]
    async fn test_register_returns_error_machine() {
        let client = fdomain_local::local_client_empty();
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Foreground,
            REPO_NAME,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, true).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: Some(REPO_NAME.to_string()),
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;
        let want = "BUG: An internal command error occurred.\nError: Failed to register repository: RepositoryManagerError";

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_err(), "expected error: {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <RegisterTool as FfxMain>::Writer::verify_schema(&json).expect(&err);

        assert_eq!(json, serde_json::json!({"unexpected_error":{"message": want}}));
    }

    #[fuchsia::test]
    async fn test_register_machine() {
        let client = fdomain_local::local_client_empty();
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(Some("1.2.3.4".to_string())).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Daemon,
            REPO_NAME,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let aliases = vec![String::from("my-alias")];

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: Some(REPO_NAME.to_string()),
                port: None,
                alias: aliases.clone(),
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_ok(), "expected ok: {stdout} {stderr}");

        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <RegisterTool as FfxMain>::Writer::verify_schema(&json).expect(&err);

        assert_eq!(json, serde_json::json!({"ok":{}}));
    }

    #[fuchsia::test]
    async fn test_tunnel_required() {
        let client = fdomain_local::local_client_empty();
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);
        let target_env = target_interface(&fho_env);
        let resolution = setup_fake_resolution(None).await;
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(resolution));

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Daemon,
            REPO_NAME,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: Some(REPO_NAME.to_string()),
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        let err = tool.main(writer).await.expect_err("register error");
        assert_eq!(err.to_string(), TUNNEL_REQUIRED_ERROR);
    }

    #[fuchsia::test]
    async fn test_address_override() {
        let client = fdomain_local::local_client_empty();
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_servers");
        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NAME)
            .build()
            .expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);

        make_server_instance(
            env.isolate_root.path(),
            ServerMode::Daemon,
            REPO_NAME,
            BTreeSet::<String>::new(),
        )
        .await
        .expect("repo server instance");

        let addr_override = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 1313);
        let expected_config = RepositoryConfig {
            repo_url: Some("fuchsia-pkg://some-name".into()),
            root_keys: Some(vec![]),
            mirrors: Some(vec![MirrorConfig {
                mirror_url: Some(format!("http://{addr_override}/some-name")),
                subscribe: Some(false),
                blob_mirror_url: None,
                ..Default::default()
            }]),
            root_version: Some(1),
            root_threshold: Some(1),
            use_local_mirror: Some(false),
            storage_type: Some(fdomain_fuchsia_pkg::RepositoryStorageType::Ephemeral),
            ..Default::default()
        };

        let (repo_proxy, _) =
            setup_fake_repo_proxy(Arc::clone(&client), Some(expected_config), false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;
        // A target with no ssh host address would require a tunnel.
        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: Some(REPO_NAME.to_string()),
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: Some(addr_override),
                json_uri: None,
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);
        tool.main(writer).await.expect("succeeds");
    }

    #[fuchsia::test]
    async fn test_register_config_from_json_file() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env().build().expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("cfg.json");
        fs::write(&cfg, TEST_CFG.as_bytes()).unwrap();

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: None,
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: Some(JsonURI::LocalFile(cfg)),
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("register ok");
    }

    #[fuchsia::test]
    async fn test_register_config_from_json_file_via_url_scheme() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env().build().expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("cfg.json");
        fs::write(&cfg, TEST_CFG.as_bytes()).unwrap();

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: None,
                port: None,
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: parse_json_uri(format!("file://{}", cfg.display()).as_str()).ok(),
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("register ok");
    }

    #[fuchsia::test]
    async fn test_register_config_from_json_file_invalid_arguments() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env().build().expect("test env");
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "repo", "test"]);

        let (repo_proxy, _) = setup_fake_repo_proxy(Arc::clone(&client), None, false).await;
        let (engine_proxy, _) = setup_fake_engine_proxy(Arc::clone(&client), None).await;

        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("cfg.json");
        fs::write(&cfg, TEST_CFG.as_bytes()).unwrap();

        let tool = RegisterTool {
            cmd: RegisterCommand {
                repository: None,
                port: Some(12345),
                alias: vec![],
                storage_type: None,
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                address_override: None,
                json_uri: Some(JsonURI::LocalFile(cfg)),
            },
            context: env.context.clone(),
            repo_proxy,
            engine_proxy,
            fho_env,
        };
        let buffers = TestBuffers::default();
        let writer = <RegisterTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect_err("register fail");
    }
}
