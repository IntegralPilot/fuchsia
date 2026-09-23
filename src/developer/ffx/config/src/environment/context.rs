// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{EnvironmentFiles, EnvironmentKind, ExecutableKind};
use crate::api::ConfigError;
use crate::api::value::{TryConvert, ValueStrategy};
use crate::storage::{AssertNoEnv, Config, StorageError};
use crate::{ConfigMap, ConfigQueryBuilder, Environment, is_analytics_disabled};

use camino::{Utf8Path, Utf8PathBuf};
use ffx_config_domain::ConfigDomain;
use sdk::{Sdk, SdkRoot};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("SDK error: {0}")]
    Sdk(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Config domain root not found for path: {0}")]
    DomainRootNotFound(Utf8PathBuf),

    #[error("Failed to load config domain: {0}")]
    DomainLoad(#[from] ffx_config_domain::FileError),

    #[error("SDK load error: {0}")]
    SdkLoad(String),

    #[error("Unable to load SDK while searching for the 'main' ffx binary: {0}")]
    LoadSdkForMainFfx(String),

    #[error(
        "Failed to get 'ffx' host tool from SDK while searching for the 'main' ffx binary: {0}"
    )]
    GetHostToolForMainFfx(String),

    #[error("SDK tool error: {0}")]
    SdkTool(String),

    #[error("Environment detect error: {0}")]
    Detect(#[from] EnvironmentDetectError),
    #[error("Paths error: {0}")]
    Paths(#[from] crate::paths::PathsError),

    #[error("AssertNoEnv error: {0}")]
    AssertNoEnv(#[from] crate::storage::AssertNoEnvError),

    #[error("Config load error: {0}")]
    ConfigLoad(#[from] StorageError),

    #[error("Path error: {0}")]
    Path(#[from] camino::FromPathError),
}

/// A name for the type used as an environment variable mapping for isolation override
pub(crate) type EnvVars = HashMap<String, String>;

/// Contextual information about where this instance of ffx is running
#[derive(Clone, Debug)]
pub struct EnvironmentContext {
    pub(crate) kind: EnvironmentKind,
    exe_kind: ExecutableKind,
    pub(crate) env_vars: Option<EnvVars>,
    pub(crate) runtime_args: ConfigMap,
    env_file_path: Option<PathBuf>,
    pub(crate) config: Config,
    pub(crate) self_path: PathBuf,
    // A target spec is an Option<String>. The extra Option<> indicates whether it has been set.
    override_target_spec: Option<Option<String>>,
    /// if true, do not read or write any environment files.
    pub(crate) no_environment: bool,
}

impl Default for EnvironmentContext {
    fn default() -> Self {
        Self {
            kind: EnvironmentKind::NoContext,
            exe_kind: ExecutableKind::Test,
            env_vars: Default::default(),
            runtime_args: Default::default(),
            env_file_path: Default::default(),
            config: Config::default(),
            override_target_spec: None,
            self_path: std::env::current_exe().unwrap(),
            no_environment: false,
        }
    }
}

impl std::cmp::PartialEq for EnvironmentContext {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.exe_kind == other.exe_kind
            && self.env_vars == other.env_vars
            && self.runtime_args == other.runtime_args
            && self.env_file_path == other.env_file_path
    }
}

#[derive(Error, Debug)]
pub enum EnvironmentDetectError {
    #[error("Error reading metadata or data from the filesystem")]
    FileSystem(#[from] std::io::Error),
    #[error("Invalid path, not utf8-safe")]
    Path(#[from] camino::FromPathError),
    #[error("Error in config domain environment file")]
    ConfigDomain(#[from] ffx_config_domain::FileError),
}

impl EnvironmentContext {
    fn load_env_files(path: &Path) -> Result<EnvironmentFiles, ContextError> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(EnvironmentFiles::default());
            }
            Err(e) => return Err(e.into()),
        };
        serde_json::from_reader(std::io::BufReader::new(file)).map_err(Into::into)
    }

    /// Initializes a new environment type with the given kind and runtime arguments.
    pub(crate) fn new(
        kind: EnvironmentKind,
        exe_kind: ExecutableKind,
        env_vars: Option<EnvVars>,
        runtime_args: ConfigMap,
        env_file_path: Option<PathBuf>,
        no_environment: bool,
    ) -> Result<Self, ContextError> {
        let config = if !no_environment {
            let env_path = match &env_file_path {
                Some(path) => path.clone(),
                None => kind.get_default_env_path()?,
            };
            let env_files = Self::load_env_files(&env_path)?;

            let user_conf = env_files.user.or_else(|| kind.get_default_user_file_path().ok());
            let build_conf = if let Some(build_config_path) = kind.get_build_config_file() {
                Some(build_config_path.into())
            } else {
                let dir = kind.build_dir();
                dir.and_then(|d| {
                    env_files
                        .build
                        .as_ref()
                        .and_then(|dirs| dirs.get(d).cloned())
                        .or_else(|| kind.get_default_build_dir_config_path(d).ok())
                })
            };
            let global_conf = env_files.global;

            let is_isolated = kind.is_isolated();
            Config::from_paths(
                user_conf,
                build_conf,
                global_conf,
                runtime_args.clone(),
                kind.get_default_overrides(),
                is_isolated,
            )?
        } else {
            Config::new(None, None, None, runtime_args.clone(), kind.get_default_overrides())
        };

        Ok(Self {
            kind,
            exe_kind,
            env_vars,
            runtime_args,
            env_file_path,
            config,
            override_target_spec: None,
            self_path: std::env::current_exe().unwrap(),
            no_environment,
        })
    }

    /// Initializes an environment type that is just the bare minimum, containing no ambient configuration, only
    /// the runtime args.
    pub fn strict(exe_kind: ExecutableKind, runtime_args: ConfigMap) -> Result<Self, ContextError> {
        let mut res = Self {
            kind: EnvironmentKind::StrictContext,
            exe_kind: exe_kind.clone(),
            // For simplicity, the runtime_args will be kept empty.
            runtime_args: ConfigMap::new(),
            env_vars: None,
            env_file_path: None,
            no_environment: true,
            override_target_spec: None,
            config: Config::new(None, None, None, runtime_args.clone(), ConfigMap::new()),
            self_path: std::env::current_exe().unwrap(),
        };

        // Since environment variables won't be
        // expanded even when specified at run-time, we verify that there are no
        // environment variables specified on the command line.  Values in the
        // default config that refer to environment variables will be ignored,
        // so such values will look like unspecified config values, and will
        // need to be specified on the command-line. For example:
        // `ffx ... --config ssh.priv=path/to/ssh-key --strict target echo`
        runtime_args.assert_no_env(None, &res)?;
        res.runtime_args = runtime_args;
        // TODO(b/368058956): Print a sanitized config into the logs so we can use it for
        // debugging.
        Ok(res)
    }

    /// Initialize an environment type for a config domain context, with a
    /// `fuchsia_env` file at its root.
    pub fn config_domain(
        exe_kind: ExecutableKind,
        domain: ConfigDomain,
        runtime_args: ConfigMap,
        isolate_root: Option<PathBuf>,
        no_environment: bool,
    ) -> Result<Self, ContextError> {
        Self::new(
            EnvironmentKind::ConfigDomain { domain: Box::new(domain), isolate_root },
            exe_kind,
            None,
            runtime_args,
            None,
            no_environment,
        )
    }

    /// Initialize an environment type for a config domain context, looking for
    /// a fuchsia_env file at the given path.
    pub fn config_domain_root(
        exe_kind: ExecutableKind,
        domain_root: Utf8PathBuf,
        runtime_args: ConfigMap,
        isolate_root: Option<PathBuf>,
        no_environment: bool,
    ) -> Result<Self, ContextError> {
        let domain_config = ConfigDomain::find_root(&domain_root)
            .ok_or_else(|| ContextError::DomainRootNotFound(domain_root.clone()))?;
        let domain = ConfigDomain::load_from(&domain_config)?;
        Self::config_domain(exe_kind, domain, runtime_args, isolate_root, no_environment)
    }

    /// Initialize an environment type for an in tree context, rooted at `tree_root` and if
    /// a build directory is currently set at `build_dir`.
    pub fn in_tree(
        exe_kind: ExecutableKind,
        tree_root: PathBuf,
        build_dir: Option<PathBuf>,
        runtime_args: ConfigMap,
        env_file_path: Option<PathBuf>,
        no_environment: bool,
    ) -> Result<Self, ContextError> {
        Self::new(
            EnvironmentKind::InTree { tree_root, build_dir },
            exe_kind,
            None,
            runtime_args,
            env_file_path,
            no_environment,
        )
    }

    /// Initialize an environment with an isolated root under which things should be stored/used/run.
    pub fn isolated(
        exe_kind: ExecutableKind,
        isolate_root: PathBuf,
        env_vars: EnvVars,
        runtime_args: ConfigMap,
        env_file_path: Option<PathBuf>,
        current_dir: Option<&Utf8Path>,
        no_environment: bool,
    ) -> Result<Self, ContextError> {
        if let Some(domain_path) = current_dir.and_then(ConfigDomain::find_root) {
            let domain = ConfigDomain::load_from(&domain_path)?;
            Self::config_domain(exe_kind, domain, runtime_args, Some(isolate_root), no_environment)
        } else {
            // Isolate dirs should be absolute paths
            let isolate_root = std::path::absolute(&isolate_root)?;
            Self::new(
                EnvironmentKind::Isolated { isolate_root },
                exe_kind,
                Some(env_vars),
                runtime_args,
                env_file_path,
                no_environment,
            )
        }
    }

    pub fn is_strict(&self) -> bool {
        match self.kind {
            EnvironmentKind::StrictContext => true,
            _ => false,
        }
    }

    pub fn is_isolated(&self) -> bool {
        match self.kind {
            EnvironmentKind::ConfigDomain { isolate_root: Some(..), .. }
            | EnvironmentKind::Isolated { .. } => true,
            _ => false,
        }
    }

    pub fn is_in_tree(&self) -> bool {
        match self.kind {
            EnvironmentKind::InTree { .. } => true,
            _ => false,
        }
    }

    pub fn has_no_environment(&self) -> bool {
        self.no_environment
    }

    /// Initialize an environment type that has no meaningful context, using only global and
    /// user level configuration.
    pub fn no_context(
        exe_kind: ExecutableKind,
        runtime_args: ConfigMap,
        env_file_path: Option<PathBuf>,
        no_environment: bool,
    ) -> Result<Self, ContextError> {
        Self::new(
            EnvironmentKind::NoContext,
            exe_kind,
            None,
            runtime_args,
            env_file_path,
            no_environment,
        )
    }

    /// Detects what kind of environment we're in, based on the provided arguments,
    /// and returns the context found. If None is given for `env_file_path`, the default for
    /// the kind of environment will be used. Note that this will never automatically detect
    /// an isolated environment, that has to be chosen explicitly.
    pub fn detect(
        exe_kind: ExecutableKind,
        runtime_args: ConfigMap,
        current_dir: &Path,
        env_file_path: Option<PathBuf>,
        no_environment: bool,
    ) -> Result<Self, ContextError> {
        // strong signals that we're running...
        if let Some(domain_path) = ConfigDomain::find_root(current_dir.try_into()?) {
            // - a config-domain: we found a fuchsia-env file
            let domain = ConfigDomain::load_from(&domain_path)?;
            Self::config_domain(exe_kind, domain, runtime_args, None, no_environment)
        } else if let Some(tree_root) = Self::find_fx_root(current_dir)? {
            // - in-tree: we found `.fx-root`, and...
            // look for a .fx-build-dir file and use that instead.
            let build_dir = Self::load_fx_build_dir(&tree_root)?;

            Self::in_tree(
                exe_kind,
                tree_root,
                build_dir,
                runtime_args,
                env_file_path,
                no_environment,
            )
        } else {
            // - no particular context: any other situation
            Self::no_context(exe_kind, runtime_args, env_file_path, no_environment)
        }
    }

    pub fn exe_kind(&self) -> ExecutableKind {
        self.exe_kind
    }

    pub fn analytics_enabled(&self) -> bool {
        use EnvironmentKind::*;
        if let Isolated { .. } = self.kind {
            false
        } else {
            // note: double negative to turn this into an affirmative
            !is_analytics_disabled(self)
        }
    }

    pub fn env_file_path(&self) -> Result<PathBuf, ContextError> {
        match &self.env_file_path {
            Some(path) => Ok(path.clone()),
            None => Ok(self.get_default_env_path()?),
        }
    }

    /// Returns the overridden path to the environment config file, if explicitly configured.
    pub fn custom_env_file_path(&self) -> Option<&Path> {
        self.env_file_path.as_deref()
    }

    /// Returns the CLI argument flags representing this environment context
    /// (such as `--no-environment`, `--isolate-dir <path>`, `--env <path>`, `--strict`).
    pub fn env_args(&self) -> Result<Vec<String>, ContextError> {
        let mut args = Vec::new();
        if self.has_no_environment() {
            args.push("--no-environment".to_string());
        }
        if let Some(isolate_root) = self.env_kind().isolate_root() {
            let root_str: &Utf8Path = isolate_root.try_into()?;
            args.extend(["--isolate-dir".to_string(), root_str.to_string()]);
        }
        if let Some(env_file) = self.custom_env_file_path() {
            let env_str: &Utf8Path = env_file.try_into()?;
            args.extend(["--env".to_string(), env_str.to_string()]);
        }
        if self.is_strict() {
            args.push("--strict".to_string());
        }
        Ok(args)
    }
    /// Returns the context's project root, if it makes sense for its
    /// [`EnvironmentKind`].
    pub fn project_root(&self) -> Option<&Path> {
        match &self.kind {
            EnvironmentKind::InTree { tree_root, .. } => Some(&tree_root),
            EnvironmentKind::ConfigDomain { domain, .. } => Some(domain.root().as_std_path()),
            _ => None,
        }
    }

    /// Returns the path to the currently active build output directory
    pub fn build_dir(&self) -> Option<&Path> {
        self.kind.build_dir()
    }

    /// Returns version info about the running ffx binary
    pub fn build_info(&self) -> ffx_build_version::VersionInfo {
        ffx_build_version::build_info()
    }

    pub fn env_kind(&self) -> &EnvironmentKind {
        &self.kind
    }

    pub fn load(&self) -> Result<Environment, crate::environment::EnvironmentError> {
        Environment::load(self)
    }

    /// Gets an environment variable, either from the system environment or from the isolation-configured
    /// environment.
    pub fn env_var(&self, name: &str) -> Result<String, std::env::VarError> {
        match &self.env_vars {
            Some(env_vars) => env_vars.get(name).cloned().ok_or(std::env::VarError::NotPresent),
            _ => std::env::var(name),
        }
    }

    // Some tests need to clear out the env
    pub fn remove_var(&mut self, name: &str) {
        if let Some(env_vars) = &mut self.env_vars {
            env_vars.remove(name);
        }
    }

    /// Creates a [`ConfigQuery`] against the global config cache and
    /// this environment.
    ///
    /// Example:
    ///
    /// ```no_run
    /// use ffx_config::ConfigLevel;
    /// use ffx_config::BuildSelect;
    /// use ffx_config::SelectMode;
    ///
    /// let ctx = EnvironmentContext::default();
    /// let query = ctx.build()
    ///     .name("testing")
    ///     .level(Some(ConfigLevel::Build))
    ///     .build(Some(BuildSelect::Path("/tmp/build.json")))
    ///     .select(SelectMode::All);
    /// let value = query.get().await?;
    /// ```
    pub fn build<'a>(&'a self) -> ConfigQueryBuilder<'a> {
        ConfigQueryBuilder::default()
    }

    /// Creates a [`ConfigQuery`] against the global config cache and this
    /// environment, using the provided value converted in to a base query.
    ///
    /// Example:
    ///
    /// ```no_run
    /// let ctx = EnvironmentContext::default();
    /// ctx.query("a_key").get();
    /// ctx.query(ffx_config::ConfigLevel::User).get();
    /// ```
    pub fn query<'a>(&'a self, with: impl Into<ConfigQueryBuilder<'a>>) -> ConfigQueryBuilder<'a> {
        with.into()
    }

    /// A shorthand for the very common case of querying a value from the global config
    /// cache and this environment, using the provided value converted into a query.
    pub fn get<'a, T, U>(&'a self, with: U) -> Result<T, ConfigError>
    where
        T: TryConvert + ValueStrategy,
        U: Into<ConfigQueryBuilder<'a>>,
    {
        self.query(with).build().get(self)
    }

    /// A shorthand for the very common case of querying a value from the global config
    /// cache and this environment, using the provided value converted into a query.
    pub fn get_optional<'a, T, U>(&'a self, with: U) -> Result<T, ConfigError>
    where
        T: TryConvert + ValueStrategy,
        U: Into<ConfigQueryBuilder<'a>>,
    {
        self.query(with).build().get_optional(self)
    }

    /// A shorthand for querying a value and its source information.
    pub fn get_with_source<'a, T, U>(
        &'a self,
        with: U,
    ) -> Result<(T, crate::ConfigSource), ConfigError>
    where
        T: TryConvert + ValueStrategy,
        U: Into<ConfigQueryBuilder<'a>>,
    {
        self.query(with).build().get_with_source(self)
    }

    /// A shorthand for querying an optional value and its source information.
    pub fn get_optional_with_source<'a, T, U>(
        &'a self,
        with: U,
    ) -> Result<(T, Option<crate::ConfigSource>), ConfigError>
    where
        T: TryConvert + ValueStrategy,
        U: Into<ConfigQueryBuilder<'a>>,
    {
        self.query(with).build().get_optional_with_source(self)
    }

    /// Find the appropriate sdk root for this invocation of ffx, looking at configuration
    /// values and the current environment context to determine the correct place to find it.
    pub fn get_sdk_root(&self) -> Result<SdkRoot, ContextError> {
        // some in-tree tooling directly overrides sdk.root. But if that's not done, the 'root' is just the
        // build directory.
        // Out of tree, we will always want to pull the config from the normal config path, which
        // we can defer to the SdkRoot's mechanisms for.
        let runtime_root: Option<PathBuf> = self.get("sdk.root").ok();
        match (&self.kind, runtime_root) {
            (EnvironmentKind::InTree { build_dir: Some(build_dir), .. }, None) => {
                let host_arch = match std::env::consts::ARCH {
                    "x86_64" => "host_x64",
                    "aarch64" => "host_arm64",
                    _ => "host_x64",
                };
                let host_tools_dir = build_dir.join(host_arch);
                if host_tools_dir.exists() {
                    Ok(SdkRoot::HostTools { root: host_tools_dir })
                } else {
                    SdkRoot::from_paths(None).map_err(|e| ContextError::Sdk(e.to_string()))
                }
            }
            (EnvironmentKind::InTree { .. }, None) => {
                SdkRoot::from_paths(None).map_err(|e| ContextError::Sdk(e.to_string()))
            }
            (_, runtime_root) => SdkRoot::from_paths(runtime_root.as_deref())
                .map_err(|e| ContextError::Sdk(e.to_string())),
        }
    }

    /// Load the sdk configured for this environment context
    pub fn get_sdk(&self) -> Result<Sdk, ContextError> {
        self.get_sdk_root()?.get_sdk().map_err(|e| ContextError::Sdk(e.to_string()))
    }

    /// The environment variable we search for
    pub const FFX_BIN_ENV: &'static str = "FFX_BIN";

    /// Gets the path to the top level binary for use when re-running ffx.
    ///
    /// - This will first check the environment variable in [`Self::FFX_BIN_ENV`],
    /// which should be set by a top level ffx invocation if we were run by one.
    /// - If that isn't set, it will use the current executable if this
    /// context's `ExecutableType` is MainFfx.
    /// - If neither of those are found, and an sdk is configured, search the
    /// sdk manifest for the ffx host-tool entry and use that.
    pub fn rerun_bin(&self) -> Result<PathBuf, ContextError> {
        if let Some(bin_from_env) = self.env_var(Self::FFX_BIN_ENV).ok() {
            return Ok(bin_from_env.into());
        }

        if let ExecutableKind::MainFfx = self.exe_kind {
            return Ok(self.self_path.clone());
        }

        let sdk = self.get_sdk().map_err(|e| ContextError::LoadSdkForMainFfx(e.to_string()))?;
        sdk.get_host_tool("ffx").map_err(|e| ContextError::GetHostToolForMainFfx(e.to_string()))
    }

    /// Creates a command builder that starts with everything necessary to re-run ffx within the same context,
    /// without any subcommands.
    pub fn rerun_prefix(&self) -> Result<Command, ContextError> {
        // we may have been run by a wrapper script, so we want to make sure we're using the 'real' executable.
        let mut ffx_path = self.rerun_bin()?;
        // if we daemonize, our path will change to /, so get the canonical path before that occurs.
        ffx_path = std::fs::canonicalize(ffx_path)?;

        let mut cmd = Command::new(&ffx_path);
        match &self.kind {
            EnvironmentKind::Isolated { isolate_root } => {
                cmd.arg("--isolate-dir").arg(isolate_root);

                // for isolation we're always going to clear the environment,
                // because it's better to fail than poison the isolation with something
                // external.
                // But an isolated context without an env var hash shouldn't be
                // constructable anyways.
                cmd.env_clear();
                if let Some(env_vars) = &self.env_vars {
                    for (k, v) in env_vars {
                        cmd.env(k, v);
                    }
                }
            }
            _ => {}
        }
        cmd.env(Self::FFX_BIN_ENV, &ffx_path);
        cmd.arg("--config").arg(serde_json::to_string(&self.runtime_args)?);
        if let Some(e) = self.env_file_path.as_ref() {
            cmd.arg("--env").arg(e);
        }
        Ok(cmd)
    }

    /// Searches for the .fx-root that should be at the top of the tree. Returns
    /// Ok(Some(parent_of_fx_root)) if one is found.
    fn find_fx_root(from: &Path) -> Result<Option<PathBuf>, EnvironmentDetectError> {
        let mut from = Some(std::fs::canonicalize(from)?);
        while let Some(next) = from {
            let fx_root_path = next.join(".fx-root");
            if fx_root_path.is_file() {
                return Ok(Some(next));
            } else {
                from = next.parent().map(Path::to_owned);
            }
        }
        Ok(None)
    }

    /// Looks for an fx-configured .fx-build-dir file in the tree root and returns the path
    /// presented there if the directory exists.
    fn load_fx_build_dir(from: &Path) -> Result<Option<PathBuf>, EnvironmentDetectError> {
        let build_dir_file = from.join(".fx-build-dir");
        if build_dir_file.is_file() {
            let mut dir = String::default();
            File::open(build_dir_file)?.read_to_string(&mut dir)?;
            Ok(from.join(dir.trim()).canonicalize().ok())
        } else {
            Ok(None)
        }
    }

    pub fn get_default_overrides(&self) -> ConfigMap {
        self.kind.get_default_overrides()
    }

    /// Returns the configuration domain for the current invocation, if there
    /// is one.
    pub fn get_config_domain(&self) -> Option<&ConfigDomain> {
        match &self.kind {
            EnvironmentKind::ConfigDomain { domain, .. } => Some(domain),
            _ => None,
        }
    }

    /// Returns a mutable reference to the configuration domain for the current
    /// invocation, if there is one. This can be used in bootstrapping to
    /// refresh the project-local configuration.
    pub fn get_config_domain_mut(&mut self) -> Option<&mut ConfigDomain> {
        match &mut self.kind {
            EnvironmentKind::ConfigDomain { domain, .. } => Some(domain),
            _ => None,
        }
    }

    /// Returns whether we are configured to make a direct connection to the
    /// target, rather than going through the daemon. Defaults to true. Can be
    /// overridden by setting the config option `connectivity.direct` (e.g. with
    /// `-c connectivity.direct=false` on the command line).
    pub fn get_direct_connection_mode(&self) -> bool {
        self.get(crate::keys::DIRECT_CONNECTIONS).unwrap_or(true)
    }

    /// Set a specific target spec. Used in unusual circumstances (e.g. in `ffx
    /// doctor`) when we want to create an alternate context with a specific
    /// target specified. When this is None, the `target.default` option is
    /// used.
    pub fn override_target_specifier(&mut self, spec: &Option<String>) {
        self.override_target_spec = Some(spec.clone());
    }

    pub fn get_overridden_target_specifier(&self) -> Option<Option<String>> {
        self.override_target_spec.clone()
    }

    /// Returns whether the running ffx executable is located within the active build directory.
    pub fn is_self_in_build_dir(&self) -> bool {
        if let Some(build_dir) = self.build_dir() {
            if let (Ok(canonical_build_dir), Ok(canonical_self_path)) =
                (std::fs::canonicalize(build_dir), std::fs::canonicalize(&self.self_path))
            {
                return canonical_self_path.starts_with(canonical_build_dir);
            }
        }
        false
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use assert_matches::assert_matches;
    use tempfile::tempdir;

    const DOMAINS_TEST_DATA_PATH: &str = env!("DOMAINS_TEST_DATA_PATH");

    fn domains_test_data_path() -> &'static Utf8Path {
        Utf8Path::new(DOMAINS_TEST_DATA_PATH)
    }

    #[fuchsia::test]
    fn test_config_domain_context() {
        let domain_root = domains_test_data_path().join("basic_example");
        let context = EnvironmentContext::config_domain_root(
            ExecutableKind::Test,
            domain_root.clone(),
            Default::default(),
            None,
            false,
        )
        .expect("config domain context");

        check_config_domain_paths(&context, &domain_root);
        assert!(!context.is_isolated());
    }

    #[fuchsia::test]
    fn test_strict_context() {
        // For the time being, these are all the values that default (built into the binary,
        // specifically) to being env variables. These must be overwritten to allow for allocating
        // of a strict env context.
        let mut config_map = ConfigMap::new();
        config_map.insert(
            "target".to_string(),
            serde_json::json!({
                "default": "127.0.0.1"
            }),
        );
        config_map.insert(
            "ssh".to_string(),
            serde_json::json!({
                "pub": "/tmp/whatever",
                "priv": "/tmp/whatever2"
            }),
        );
        config_map.insert(
            "log".to_string(),
            serde_json::json!({
                "dir": "/tmp/loggodoggo"
            }),
        );
        config_map.insert(
            "fastboot".to_string(),
            serde_json::json!({
                "devices_file": {
                    "path": "/tmp/fastboot_thing_I_guess"
                }
            }),
        );
        let context =
            EnvironmentContext::strict(ExecutableKind::Test, config_map).expect("strict context");
        assert!(context.is_strict());
    }

    #[fuchsia::test]
    fn test_config_domain_context_isolated() {
        let isolate_dir = tempdir().expect("tempdir");
        let domain_root = domains_test_data_path().join("basic_example");
        println!("check with explicit config domain path");
        let context = EnvironmentContext::config_domain_root(
            ExecutableKind::Test,
            domain_root.clone(),
            Default::default(),
            Some(isolate_dir.path().to_owned()),
            false,
        )
        .expect("isolated config domain context");

        check_config_domain_paths(&context, &domain_root);
        check_isolated_paths(&context, &isolate_dir.path());

        println!("check with implied config domain path");
        let context = EnvironmentContext::isolated(
            ExecutableKind::Test,
            isolate_dir.path().to_owned(),
            Default::default(),
            Default::default(),
            None,
            Some(&domain_root),
            false,
        )
        .expect("Isolated context");

        check_config_domain_paths(&context, &domain_root);
        check_isolated_paths(&context, &isolate_dir.path());
    }

    #[test]
    fn test_config_isolated_context() {
        let isolate_dir = tempdir().expect("tempdir");
        let context = EnvironmentContext::isolated(
            ExecutableKind::Test,
            isolate_dir.path().to_owned(),
            Default::default(),
            Default::default(),
            None,
            None,
            false,
        )
        .expect("Isolated context");

        check_isolated_paths(&context, &isolate_dir.path());
    }

    #[fuchsia::test]
    fn test_in_tree_context() {
        let tree_root = tempdir().expect("output directory");
        let context = EnvironmentContext::in_tree(
            ExecutableKind::Test,
            tree_root.path().to_path_buf(),
            None,
            Default::default(),
            None,
            false,
        )
        .expect("in tree context");

        assert!(context.is_in_tree());
    }

    fn check_config_domain_paths(context: &EnvironmentContext, domain_root: &Utf8Path) {
        let domain_root = domain_root.canonicalize().expect("canonicalized domain root");
        assert_eq!(context.build_dir().unwrap(), domain_root.join("bazel-out"));
        assert_eq!(
            context.get_build_config_file().unwrap(),
            domain_root.join(".fuchsia-build-config.json")
        );
        assert_matches!(context.get_sdk_root().unwrap(), SdkRoot::Full{root:path, manifest:None} if path == domain_root.join("bazel-out/external/fuchsia_sdk"));
    }

    fn check_isolated_paths(context: &EnvironmentContext, isolate_dir: &Path) {
        assert_eq!(
            context.get_default_user_file_path().unwrap(),
            isolate_dir.join(crate::paths::USER_FILE)
        );
        assert_eq!(
            context.get_default_env_path().unwrap(),
            isolate_dir.join(crate::paths::ENV_FILE)
        );
        assert_eq!(context.get_default_ascendd_path().unwrap(), isolate_dir.join("daemon.sock"));
        assert_eq!(context.get_runtime_path().unwrap(), isolate_dir.join("runtime"));
        assert_eq!(context.get_cache_path().unwrap(), isolate_dir.join("cache"));
        assert_eq!(context.get_config_path().unwrap(), isolate_dir.join("config"));
        assert_eq!(context.get_data_path().unwrap(), isolate_dir.join("data"));
        assert!(context.is_isolated());
    }

    #[fuchsia::test]
    fn direct_connection_mode() {
        // Defaults to true
        let ctx =
            EnvironmentContext::no_context(ExecutableKind::Test, ConfigMap::new(), None, true)
                .unwrap();
        assert!(ctx.get_direct_connection_mode());

        // True if connectivity.direct=true
        let mut connectivity = ConfigMap::new();
        connectivity.insert("direct".into(), true.into());
        let mut runtime_args = ConfigMap::new();
        runtime_args.insert("connectivity".into(), serde_json::Value::Object(connectivity));
        let ctx =
            EnvironmentContext::no_context(ExecutableKind::Test, runtime_args, None, true).unwrap();
        assert!(ctx.get_direct_connection_mode());

        // False if connectivity.direct=false
        let mut connectivity = ConfigMap::new();
        connectivity.insert("direct".into(), false.into());
        let mut runtime_args = ConfigMap::new();
        runtime_args.insert("connectivity".into(), serde_json::Value::Object(connectivity));
        let ctx =
            EnvironmentContext::no_context(ExecutableKind::Test, runtime_args, None, true).unwrap();
        assert!(!ctx.get_direct_connection_mode());
    }

    #[fuchsia::test]
    fn test_override_target_spec() {
        let mut ctx =
            EnvironmentContext::no_context(ExecutableKind::Test, ConfigMap::new(), None, true)
                .unwrap();
        assert_eq!(ctx.get_overridden_target_specifier(), None);
        ctx.override_target_specifier(&Some("foo".to_string()));
        assert_eq!(
            ctx.get_overridden_target_specifier().expect("target spec unset"),
            Some("foo".to_string())
        )
    }

    #[fuchsia::test]
    fn test_env_args() {
        // Default / empty
        let ctx =
            EnvironmentContext::no_context(ExecutableKind::Test, ConfigMap::new(), None, false)
                .unwrap();
        assert_eq!(ctx.env_args().unwrap(), Vec::<String>::new());

        // No environment flag
        let ctx =
            EnvironmentContext::no_context(ExecutableKind::Test, ConfigMap::new(), None, true)
                .unwrap();
        assert_eq!(ctx.env_args().unwrap(), vec!["--no-environment".to_string()]);

        // Isolated environment
        let isolate_dir = tempdir().expect("tempdir");
        let isolate_path = isolate_dir.path().to_owned();
        let expected_isolate_path = std::path::absolute(&isolate_path).unwrap();
        let expected_isolate_str = expected_isolate_path.to_str().unwrap().to_string();
        let ctx = EnvironmentContext::isolated(
            ExecutableKind::Test,
            isolate_path,
            Default::default(),
            Default::default(),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            ctx.env_args().unwrap(),
            vec!["--isolate-dir".to_string(), expected_isolate_str]
        );

        // Custom env file path
        let env_path = PathBuf::from("/tmp/custom_env_file.json");
        let ctx = EnvironmentContext::no_context(
            ExecutableKind::Test,
            ConfigMap::new(),
            Some(env_path.clone()),
            false,
        )
        .unwrap();
        assert_eq!(
            ctx.env_args().unwrap(),
            vec!["--env".to_string(), "/tmp/custom_env_file.json".to_string()]
        );

        // Strict context
        let mut config_map = ConfigMap::new();
        config_map.insert("target".to_string(), serde_json::json!({ "default": "127.0.0.1" }));
        config_map.insert(
            "ssh".to_string(),
            serde_json::json!({ "pub": "/tmp/whatever", "priv": "/tmp/whatever2" }),
        );
        config_map.insert("log".to_string(), serde_json::json!({ "dir": "/tmp/loggodoggo" }));
        config_map.insert(
            "fastboot".to_string(),
            serde_json::json!({ "devices_file": { "path": "/tmp/fastboot_thing_I_guess" } }),
        );
        let ctx = EnvironmentContext::strict(ExecutableKind::Test, config_map).unwrap();
        assert_eq!(
            ctx.env_args().unwrap(),
            vec!["--no-environment".to_string(), "--strict".to_string()]
        );
    }
}
