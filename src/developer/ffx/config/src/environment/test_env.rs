// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::logging::LogDestination;
use crate::nested::nested_set;
use crate::{ConfigMap, Environment, EnvironmentContext};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::{NamedTempFile, TempDir};

use super::{EnvVars, EnvironmentKind, ExecutableKind};

static LOG_INIT: std::sync::Once = std::sync::Once::new();

/// A structure that holds information about the test config environment for the duration
/// of a test. This object must continue to exist for the duration of the test, or the test
/// may fail.
#[must_use = "This object must be held for the duration of a test (ie. `let _env = ffx_config::test_init()`) for it to operate correctly."]
pub struct TestEnv {
    pub env_file: NamedTempFile,
    pub context: EnvironmentContext,
    pub isolate_root: TempDir,
    pub user_file: NamedTempFile,
    pub build_file: Option<NamedTempFile>,
    pub global_file: NamedTempFile,
}

impl TestEnv {
    fn new_isolated(
        env_vars: EnvVars,
        runtime_args: ConfigMap,
        user_config: ConfigMap,
        global_config: ConfigMap,
        build_config: ConfigMap,
        isolate_root: TempDir,
    ) -> Result<Self> {
        let mut env_file = NamedTempFile::new().context("tmp access failed")?;
        env_file.write_all(b"{}")?;
        env_file.flush()?;

        let context = EnvironmentContext::isolated(
            ExecutableKind::Test,
            isolate_root.path().to_owned(),
            env_vars,
            runtime_args,
            Some(env_file.path().to_owned()),
            None,
            false,
        )?;
        Self::build_test_env(
            context,
            env_file,
            isolate_root,
            user_config,
            global_config,
            build_config,
        )
    }

    fn new_intree(
        build_dir: &Path,
        env_vars: EnvVars,
        runtime_args: ConfigMap,
        user_config: ConfigMap,
        global_config: ConfigMap,
        build_config: ConfigMap,
        isolate_root: TempDir,
    ) -> Result<Self> {
        let mut env_file = NamedTempFile::new().context("tmp access failed")?;
        env_file.write_all(b"{}")?;
        env_file.flush()?;

        let context = EnvironmentContext::new(
            EnvironmentKind::InTree {
                tree_root: isolate_root.path().to_owned(),
                build_dir: Some(PathBuf::from(build_dir)),
            },
            ExecutableKind::Test,
            Some(env_vars),
            runtime_args,
            Some(env_file.path().to_owned()),
            false,
        )?;
        Self::build_test_env(
            context,
            env_file,
            isolate_root,
            user_config,
            global_config,
            build_config,
        )
    }

    fn build_test_env(
        context: EnvironmentContext,
        env_file: NamedTempFile,
        isolate_root: TempDir,
        user_config: ConfigMap,
        global_config: ConfigMap,
        build_config: ConfigMap,
    ) -> Result<Self> {
        let mut global_file = NamedTempFile::new().context("tmp access failed")?;
        serde_json::to_writer_pretty(&mut global_file, &Value::Object(global_config))?;
        global_file.flush()?;
        let global_file_path = global_file.path().to_owned();

        let mut user_file = NamedTempFile::new().context("tmp access failed")?;
        serde_json::to_writer_pretty(&mut user_file, &Value::Object(user_config))?;
        user_file.flush()?;
        let user_file_path = user_file.path().to_owned();

        let build_file = if let Some(_dir) = context.build_dir() {
            let mut f = NamedTempFile::new().context("tmp access failed")?;
            serde_json::to_writer_pretty(&mut f, &Value::Object(build_config))?;
            f.flush()?;
            Some(f)
        } else {
            None
        };

        LOG_INIT.call_once(|| {
            let logging = crate::logging::build_logger_with_destinations(
                &context,
                vec![LogDestination::TestWriter],
            );
            let logger = Box::new(logging.unwrap());

            let res =
                log::set_boxed_logger(logger).map(|()| log::set_max_level(log::LevelFilter::Trace));
            if res.is_err() {
                log::warn!("build_test_env: Logging already initialized");
            }
        });

        let mut test_env =
            TestEnv { env_file, context, user_file, build_file, global_file, isolate_root };

        let mut env = Environment::new_empty(test_env.context.clone());

        env.set_user(Some(&user_file_path));
        if let Some(ref build_file) = test_env.build_file {
            let build_file_path = build_file.path().to_owned();
            env.set_build(&build_file_path)?;
        }
        env.set_global(Some(&global_file_path));
        env.save().context("saving env file")?;

        test_env.reload_context()?;

        Ok(test_env)
    }

    pub fn load(&self) -> Environment {
        self.context.load().expect("opening test env file")
    }

    pub fn reload_context(&mut self) -> Result<()> {
        let context = match self.context.env_kind() {
            EnvironmentKind::Isolated { .. } => EnvironmentContext::isolated(
                self.context.exe_kind(),
                self.isolate_root.path().to_owned(),
                self.context.env_vars.clone().unwrap_or_default(),
                self.context.runtime_args.clone(),
                Some(self.env_file.path().to_owned()),
                None,
                self.context.no_environment,
            )?,
            EnvironmentKind::InTree { tree_root, build_dir } => EnvironmentContext::new(
                EnvironmentKind::InTree {
                    tree_root: tree_root.clone(),
                    build_dir: build_dir.clone(),
                },
                self.context.exe_kind(),
                self.context.env_vars.clone(),
                self.context.runtime_args.clone(),
                Some(self.env_file.path().to_owned()),
                self.context.no_environment,
            )?,
            _ => anyhow::bail!("Cannot reload context for this environment kind"),
        };
        self.context = context;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct TestEnvBuilder {
    build_dir: Option<PathBuf>,
    self_path: Option<PathBuf>,
    env_vars: EnvVars,
    runtime_config: ConfigMap,
    user_config: ConfigMap,
    global_config: ConfigMap,
    build_config: ConfigMap,
    isolate_root: Option<tempfile::TempDir>,
}

/// Creates a TestEnvBuilder with the following defaults:
///  - Backed by `EnvironmentKind::Isolated`,
///  - Does not inherit any environment variables, and
///  - is initialized with an empty runtime configuration.
pub fn test_env() -> TestEnvBuilder {
    TestEnvBuilder { ..Default::default() }
}

/// Creates a TestEnvBuilder that inherits environment variables from the real
/// test environment.
fn test_builder_with_envs() -> TestEnvBuilder {
    let env_vars: HashMap<String, String> = std::env::vars()
        .filter(|(key, _)| !key.starts_with("FFX_") && !key.starts_with("FUCHSIA_"))
        .collect();
    TestEnvBuilder { env_vars, ..Default::default() }
}

impl TestEnvBuilder {
    /// Switches the final built TestEnv to be backed by
    /// `EnvironmentKind::in_tree`.
    /// This also allows ConfigLevel::Build to be used in tests.
    pub fn in_tree(mut self, build_dir: &Path) -> Self {
        self.build_dir = Some(build_dir.into());
        self
    }

    /// Sets the executable path on the resulting `TestEnv.context.self_path`.
    pub fn self_path(mut self, path: &Path) -> Self {
        self.self_path = Some(path.into());
        self
    }

    /// Sets a single environment variable on the resulting
    /// `TestEnv.context.env_vars`.
    pub fn env_var(mut self, key: &str, value: &str) -> Self {
        self.env_vars.insert(key.into(), value.into());
        self
    }

    /// Returns the path to the isolate root, creating it if it doesn't exist.
    pub fn isolate_root(&mut self) -> PathBuf {
        if self.isolate_root.is_none() {
            self.isolate_root = Some(tempfile::tempdir().unwrap());
        }
        self.isolate_root.as_ref().unwrap().path().to_owned()
    }

    /// Sets a key to a value in the runtime config.
    /// Keys are allowed to be nested, meaning that keys like "target.default"
    /// or "repository.server.mode" are valid.
    pub fn runtime_config<T>(mut self, key: &str, value: T) -> Self
    where
        T: Into<Value>,
    {
        let key_vec: Vec<&str> = key.split('.').collect();
        nested_set(&mut self.runtime_config, key_vec[0], &key_vec[1..], value.into());
        self
    }

    /// Sets a key to a value in the user config.
    pub fn user_config<T>(mut self, key: &str, value: T) -> Self
    where
        T: Into<Value>,
    {
        let key_vec: Vec<&str> = key.split('.').collect();
        nested_set(&mut self.user_config, key_vec[0], &key_vec[1..], value.into());
        self
    }

    /// Sets a key to a value in the global config.
    pub fn global_config<T>(mut self, key: &str, value: T) -> Self
    where
        T: Into<Value>,
    {
        let key_vec: Vec<&str> = key.split('.').collect();
        nested_set(&mut self.global_config, key_vec[0], &key_vec[1..], value.into());
        self
    }

    /// Sets a key to a value in the build config.
    pub fn build_config<T>(mut self, key: &str, value: T) -> Self
    where
        T: Into<Value>,
    {
        let key_vec: Vec<&str> = key.split('.').collect();
        nested_set(&mut self.build_config, key_vec[0], &key_vec[1..], value.into());
        self
    }

    /// Builds a TestEnv backed by EnvironmentKind::Isolated by default, else
    /// EnvironmentKind::InTree if a `.in_tree()` is specified.
    ///
    /// You must hold the returned object object for the duration of the test.
    /// Not doing so will result in strange behaviour.
    pub fn build(mut self) -> Result<TestEnv> {
        let isolate_root = self.isolate_root.take().unwrap_or_else(|| tempfile::tempdir().unwrap());
        let mut env = match self.build_dir {
            Some(build_dir) => TestEnv::new_intree(
                build_dir.as_path(),
                self.env_vars,
                self.runtime_config,
                self.user_config,
                self.global_config,
                self.build_config,
                isolate_root,
            ),
            None => TestEnv::new_isolated(
                self.env_vars,
                self.runtime_config,
                self.user_config,
                self.global_config,
                self.build_config,
                isolate_root,
            ),
        }?;
        if let Some(self_path) = self.self_path {
            env.context.self_path = self_path;
        }

        Ok(env)
    }
}

/// When running tests we typically want to initialize a blank slate
/// configuration, so use this for tests.
///
/// For more complex use-cases (eg: in-tree, environment variables, runtime
/// configuration), use `test_env()` instead.
///
/// You must hold the returned object object for the duration of the test.
/// Not doing so will result in strange behaviour.
///
/// FIXME(https://fxbug.dev/411199300): This function inherits environment
/// variables from the real test environment.
pub fn test_init() -> Result<TestEnv> {
    // TODO(https://fxbug.dev/411199300): Use `test_env()` when we don't need to
    // implicitly inherit environment variables from the real test environment
    // environment anymore.
    test_builder_with_envs().build()
}
