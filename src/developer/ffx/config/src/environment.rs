// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{ConfigLevel, ConfigMap};

use fuchsia_lockfile::{Lockfile, LockfileCreateError};
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EnvironmentError {
    #[error("reading environment from disk: {0}")]
    ReadEnv(#[source] serde_json::Error),

    #[error("loading host log directories from ffx config: {0}")]
    LoadLogDirs(#[source] std::io::Error),

    #[error("writing environment to disk: {0}")]
    WriteEnv(#[source] serde_json::Error),

    #[error("flushing environment: {0}")]
    FlushEnv(#[source] std::io::Error),

    #[error("persist error: {0}")]
    Persist(String),

    #[error("Tried to set unknown build directory")]
    UnknownBuildDir,

    #[error("writing default user configuration file: {0}")]
    WriteDefaultUserConfig(#[source] std::io::Error),

    #[error("syncing default user configuration file to filesystem: {0}")]
    SyncDefaultUserConfig(#[source] std::io::Error),

    #[error("creating default user configuration file: {0}")]
    CreateDefaultUserConfig(#[source] std::io::Error),

    #[error(
        "Global configuration not set. Use 'ffx config env set' command to setup the environment."
    )]
    GlobalConfigNotSet,

    #[error("Cannot set a build configuration without a build directory.")]
    NoBuildDir,

    #[error("This config level is not writable.")]
    NotWritable,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Config error: {0}")]
    Config(#[from] crate::ConfigError),

    #[error("Paths error: {0}")]
    Paths(#[from] crate::paths::PathsError),

    #[error("Context error: {0}")]
    Context(#[from] crate::environment::ContextError),
}

mod context;
mod kind;
mod test_env;

pub use context::*;
pub use kind::*;
pub use test_env::*;

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub(crate) struct EnvironmentFiles {
    pub(crate) user: Option<PathBuf>,
    pub(crate) build: Option<HashMap<PathBuf, PathBuf>>,
    pub(crate) global: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Environment {
    files: EnvironmentFiles,
    context: EnvironmentContext,
}

impl Environment {
    /// Creates a new empty env that will be saved to a specific path, but is initialized
    /// with no settings. For internal use only, when loading the global environment fails,
    /// or when --no-environment is specified.
    pub(crate) fn new_empty(context: EnvironmentContext) -> Self {
        let files = EnvironmentFiles::default();
        Self { context, files }
    }

    fn load(context: &EnvironmentContext) -> std::result::Result<Self, EnvironmentError> {
        Ok(if context.has_no_environment() {
            Self::new_empty(context.clone())
        } else {
            let path = context.env_file_path()?;
            Self::load_env_file(&path, context)?
        })
    }

    pub fn context(&self) -> &EnvironmentContext {
        &self.context
    }

    /// Checks if we can manage to open the given environment file's lockfile,
    /// as well as each configuration file referenced by it, and returns the lockfile
    /// owner if we can't. Will return a normal error via result if any non-lockfile
    /// error is encountered while processing the files.
    ///
    /// Used to implement diagnostics for `ffx doctor`.
    pub async fn check_locks(
        context: &EnvironmentContext,
    ) -> std::result::Result<
        Vec<(PathBuf, std::result::Result<PathBuf, Box<LockfileCreateError>>)>,
        EnvironmentError,
    > {
        let path = context.env_file_path()?.clone();
        let env = Self::load_env_file(&path, context)?;

        let mut checked = vec![];

        if let Some(user) = env.files.user {
            let res = Lockfile::lock_for(&user, Duration::from_secs(1)).await;
            checked.push((user, res.map(|lock| lock.path().to_owned())));
        }
        if let Some(global) = env.files.global {
            let res = Lockfile::lock_for(&global, Duration::from_secs(1)).await;
            checked.push((global, res.map(|lock| lock.path().to_owned())));
        }
        for (_, build) in env.files.build.unwrap_or_default() {
            let res = Lockfile::lock_for(&build, Duration::from_secs(1)).await;
            checked.push((build, res.map(|lock| lock.path().to_owned())));
        }

        Ok(checked)
    }

    pub fn save(&self) -> std::result::Result<(), EnvironmentError> {
        let path = self.context.env_file_path()?;

        Self::save_env_file(path, &self.files)?;

        Ok(())
    }

    fn load_env_file(
        path: &Path,
        context: &EnvironmentContext,
    ) -> std::result::Result<Self, EnvironmentError> {
        let files = match File::open(path) {
            Ok(file) => {
                serde_json::from_reader(BufReader::new(file)).map_err(EnvironmentError::ReadEnv)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(EnvironmentFiles::default()),
            Err(e) => Err(EnvironmentError::LoadLogDirs(e)),
        }?;

        Ok(Self { files, context: context.clone() })
    }

    fn save_env_file(
        path: PathBuf,
        files: &EnvironmentFiles,
    ) -> std::result::Result<(), EnvironmentError> {
        // First save the config to a temp file in the same location as the file, then atomically
        // rename the file to the final location to avoid partially written files.
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;

        serde_json::to_writer_pretty(&mut tmp, files).map_err(EnvironmentError::WriteEnv)?;

        tmp.flush().map_err(EnvironmentError::FlushEnv)?;

        tmp.persist(path).map_err(|e| EnvironmentError::Persist(e.to_string()))?;

        Ok(())
    }

    pub fn get_user(&self) -> Option<PathBuf> {
        if self.context.no_environment {
            return None;
        }
        self.files
            .user
            .as_ref()
            .map(PathBuf::clone)
            .or_else(|| self.context.get_default_user_file_path().ok())
    }
    pub fn set_user(&mut self, to: Option<&Path>) {
        assert!(
            !self.context.no_environment,
            "Cannot set user configuration with --no-environment"
        );
        self.files.user = to.map(Path::to_owned);
    }

    pub fn get_global(&self) -> Option<PathBuf> {
        if self.context.no_environment {
            return None;
        }
        self.files.global.as_ref().map(PathBuf::clone)
    }
    pub fn set_global(&mut self, to: Option<&Path>) {
        assert!(
            !self.context.no_environment,
            "Cannot set global configuration with --no-environment"
        );
        self.files.global = to.map(Path::to_owned);
    }

    pub fn get_runtime_args(&self) -> &ConfigMap {
        &self.context.runtime_args
    }

    pub fn build_dir(&self) -> Option<&Path> {
        self.context.build_dir()
    }

    /// returns either the directory indicated by the override or the one configured in this
    /// environment.
    pub fn override_build_dir<'a>(&'a self, build_override: Option<&'a Path>) -> Option<&'a Path> {
        match (build_override, self.build_dir()) {
            (Some(path), _) => Some(path),
            (_, maybe_path) => maybe_path,
        }
    }

    /// Returns the path to the build level config file, if there is one.
    /// If it is not explicit, the path is generated bnased on configured build_dir().
    pub fn get_build(&self) -> Option<PathBuf> {
        if self.context.no_environment {
            return None;
        }
        if let Some(build_config_path) = self.context.get_build_config_file() {
            Some(build_config_path.into())
        } else {
            let dir = self.build_dir()?;
            self.files
                .build
                .as_ref()
                .and_then(|dirs| dirs.get(dir).cloned())
                .or_else(|| self.context.get_default_build_dir_config_path(dir).ok())
        }
    }

    pub fn set_build(&mut self, to: &Path) -> std::result::Result<(), EnvironmentError> {
        assert!(
            !self.context.no_environment,
            "Cannot set build configuration with --no-environment"
        );
        let build_dir =
            self.override_build_dir(None).ok_or(EnvironmentError::UnknownBuildDir)?.to_owned();
        let build_dirs = match &mut self.files.build {
            Some(build_dirs) => build_dirs,
            None => self.files.build.get_or_insert_with(Default::default),
        };
        build_dirs.insert(build_dir, to.to_owned());
        Ok(())
    }

    fn display_user(&self) -> String {
        match self.files.user.as_ref() {
            Some(file) => format!(" User: {}\n", file.display()),
            None => match self.context.get_default_user_file_path() {
                Ok(default) => format!(" User: {} (default)\n", default.display()),
                Err(_) => format!(" User: none"),
            },
        }
    }

    /// Returns the default build config file for the build dir if there is one,
    /// None otherwise.
    fn display_build_default(&self, current_build_dir: Option<&Path>) -> Option<String> {
        let current = current_build_dir?;
        let default = self.context.get_default_build_dir_config_path(current).ok()?;
        Some(format!("  {} => {} (default)\n", current.display(), default.display()))
    }

    fn display_build(&self) -> String {
        let mut current_build_dir = self.build_dir();
        let mut res = format!(" Build:");
        if let Some(m) = self.files.build.as_ref() {
            if !m.is_empty() {
                res.push_str(&format!("\n"));
                for (key, val) in m.iter() {
                    // if we have an explicitly set path for the current build
                    // dir then prevent displaying the default later by removing
                    // it from the option.
                    if Some(key.as_ref()) == current_build_dir {
                        current_build_dir.take();
                    }
                    res.push_str(&format!("  {} => {}\n", key.display(), val.display()));
                }
            }
        }
        res.push_str(
            self.display_build_default(current_build_dir).as_deref().unwrap_or("  none\n"),
        );
        res
    }

    fn display_global(&self) -> String {
        self.files
            .global
            .as_ref()
            .map_or_else(|| format!(" Global: none\n"), |g| format!(" Global: {}\n", g.display()))
    }

    pub fn display(&self, level: &Option<ConfigLevel>) -> String {
        level.map_or_else(
            || {
                let mut res = format!("\nEnvironment:\n");
                res.push_str(&self.display_user());
                res.push_str(&self.display_build());
                res.push_str(&self.display_global());
                res
            },
            |l| match l {
                ConfigLevel::User => self.display_user(),
                ConfigLevel::Build => self.display_build(),
                ConfigLevel::Global => self.display_global(),
                _ => format!(" This level is not saved in the environment file."),
            },
        )
    }

    /// Checks the config files at the requested level to make sure they exist and are configured
    /// properly.
    pub fn populate_defaults(
        &mut self,
        level: &ConfigLevel,
    ) -> std::result::Result<(), EnvironmentError> {
        match level {
            ConfigLevel::User => {
                log::debug!("Populating user defaults");
                if let None = self.files.user {
                    log::debug!("Getting user default file");
                    let default_path = self.context.get_default_user_file_path()?;

                    log::debug!("Creating new user default file");
                    match create_new_file(&default_path) {
                        Ok(mut file) => {
                            log::debug!("Doing write_all");
                            file.write_all(b"{}")
                                .map_err(EnvironmentError::WriteDefaultUserConfig)?;
                            log::debug!("Syncing block cache with underlying storage");
                            if !self.context().env_kind().is_isolated() {
                                file.sync_all().map_err(EnvironmentError::SyncDefaultUserConfig)?;
                            }
                            log::debug!("Done syncing");
                        }
                        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                            log::debug!("File already exists");
                        }
                        other => {
                            other.map_err(EnvironmentError::CreateDefaultUserConfig).map(|_| ())?
                        }
                    }
                    self.files.user = Some(default_path);
                }
            }
            ConfigLevel::Global => {
                log::debug!("Populating global defaults");
                if let None = self.files.global {
                    return Err(EnvironmentError::GlobalConfigNotSet);
                }
            }
            ConfigLevel::Build => match self.build_dir().map(Path::to_owned) {
                Some(b_dir) => {
                    log::debug!("Populating build defaults");
                    let build_dirs = match &mut self.files.build {
                        Some(build_dirs) => build_dirs,
                        None => self.files.build.get_or_insert_with(Default::default),
                    };
                    #[allow(
                        clippy::map_entry,
                        reason = "mass allow for https://fxbug.dev/381896734"
                    )]
                    if !build_dirs.contains_key(&b_dir) {
                        log::debug!("Getting build dir config path");
                        let config = self.context.get_default_build_dir_config_path(&b_dir)?;
                        if !config.is_file() {
                            info!(
                                "Build configuration file for '{b_dir}' does not exist yet, will create it by default at '{config}' if a value is set",
                                b_dir = b_dir.display(),
                                config = config.display()
                            );
                        }
                        log::debug!("Saving build dir config path");
                        build_dirs.insert(b_dir, config);
                    }
                    log::debug!("Build defaults populated");
                }
                None => return Err(EnvironmentError::NoBuildDir),
            },
            _ => return Err(EnvironmentError::NotWritable),
        }
        Ok(())
    }
}

/// polyfill for File::create_new, which is currently nightly-only.
fn create_new_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).write(true).create_new(true).open(path)
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.display(&None))
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use std::fs;

    const ENVIRONMENT: &'static str = r#"
        {
            "user": "/tmp/user.json",
            "build": {
                "/tmp/build/1": "/tmp/build/1/build.json"
            },
            "global": "/tmp/global.json"
        }"#;

    #[fuchsia::test]
    fn test_loading_and_saving_environment() {
        let mut test_env = test_init().expect("initializing test environment");
        let env: EnvironmentFiles = serde_json::from_str(ENVIRONMENT).unwrap();

        // Write out the initial test environment.
        let tmp_path = test_env.env_file.path().to_owned();
        let mut env_file = File::create(&tmp_path).unwrap();
        serde_json::to_writer(&mut env_file, &env).unwrap();
        env_file.flush().unwrap();

        // Load the environment back in, and make sure it's correct.
        let env_load = test_env.load();
        assert_eq!(env, env_load.files);

        // Remove the file to prevent a spurious success
        std::fs::remove_file(&tmp_path).expect("Temporary env file wasn't available to remove");

        // Save the environment, then read the saved file and make sure it's correct.
        env_load.save().unwrap();
        test_env.env_file.flush().unwrap();

        let env_file = fs::read(&tmp_path).unwrap();
        let env_save: EnvironmentFiles = serde_json::from_slice(&env_file).unwrap();

        assert_eq!(env, env_save);
    }

    #[fuchsia::test]
    fn build_config_autoconfigure() {
        let temp = tempfile::tempdir().expect("temporary build directory");
        let temp_dir = std::fs::canonicalize(temp.path()).expect("canonical temp path");
        let build_dir_path = temp_dir.join("build");
        let build_dir_config = build_dir_path.join("ffx-config.json");
        let env_file_path = temp_dir.join("env.json");
        let context = EnvironmentContext::in_tree(
            ExecutableKind::Test,
            temp_dir.clone(),
            Some(build_dir_path.clone()),
            ConfigMap::default(),
            Some(env_file_path.clone()),
            false,
        )
        .unwrap();
        assert!(!env_file_path.is_file(), "Environment file shouldn't exist yet");
        let mut env = context.load().expect("Should be able to load the environment");

        env.populate_defaults(&ConfigLevel::Build)
            .expect("Setting build level environment to automatic path should work");
        drop(env);
        let config = context
            .load()
            .expect("should be able to load the test-configured env-file.")
            .get_build()
            .expect("should be a default build configuration");

        assert_eq!(config, build_dir_config, "build config for {build_dir_path:?}");
    }

    #[fuchsia::test]
    fn build_config_manual_configure() {
        let temp = tempfile::tempdir().expect("temporary build directory");
        let temp_dir = std::fs::canonicalize(temp.path()).expect("canonical temp path");
        let build_dir_path = temp_dir.join("build");
        let build_dir_config = temp_dir.join("build-manual.json");
        let env_file_path = temp_dir.join("env.json");
        let context = EnvironmentContext::in_tree(
            ExecutableKind::Test,
            temp_dir.clone(),
            Some(build_dir_path.clone()),
            ConfigMap::default(),
            Some(env_file_path.clone()),
            false,
        )
        .unwrap();

        assert!(!env_file_path.is_file(), "Environment file shouldn't exist yet");
        let mut env = Environment::new_empty(context.clone());
        let mut config_map = std::collections::HashMap::new();
        config_map.insert(build_dir_path.clone(), build_dir_config.clone());
        env.files.build = Some(config_map);
        env.save().expect("Should be able to save the configured environment");

        env.populate_defaults(&ConfigLevel::Build)
            .expect("Setting build level environment to automatic path should work");
        drop(env);

        if let Some(build_configs) = context
            .load()
            .expect("should be able to load the manually configured env-file.")
            .files
            .build
        {
            match build_configs.get(&build_dir_path) {
                Some(config) if config == &build_dir_config => (),
                Some(config) => panic!(
                    "Build directory config file was wrong. Expected: {build_dir_config}, got: {config})",
                    build_dir_config = build_dir_config.display(),
                    config = config.display()
                ),
                None => panic!("No build directory config was set"),
            }
        } else {
            panic!("No build configurations set after setting a configuration value");
        }
    }

    #[fuchsia::test]
    fn test_env_does_not_inherit_ffx_or_fuchsia_vars() {
        // SAFETY: `std::env::set_var` is unsafe because it can race with concurrent reads
        // or writes of the environment in other threads. This test modifies unique, test-specific
        // variables that are not accessed by other tests, minimizing the risk of practical races.
        unsafe {
            std::env::set_var("FFX_TEST_VAR_FOR_TEST", "1");
            std::env::set_var("FUCHSIA_TEST_VAR_FOR_TEST", "2");
            std::env::set_var("OTHER_TEST_VAR_FOR_TEST", "3");
        }

        let test_env = test_init().expect("initializing test environment");

        // SAFETY: `std::env::remove_var` is unsafe for the same reasons as `set_var`.
        // We are removing the same unique, test-specific variables we set above to clean up
        // the process environment.
        unsafe {
            std::env::remove_var("FFX_TEST_VAR_FOR_TEST");
            std::env::remove_var("FUCHSIA_TEST_VAR_FOR_TEST");
            std::env::remove_var("OTHER_TEST_VAR_FOR_TEST");
        }

        let context = test_env.context;
        let env_vars = context.env_vars.expect("env_vars should be set");

        assert!(!env_vars.contains_key("FFX_TEST_VAR_FOR_TEST"));
        assert!(!env_vars.contains_key("FUCHSIA_TEST_VAR_FOR_TEST"));
        assert!(env_vars.contains_key("OTHER_TEST_VAR_FOR_TEST"));
        assert_eq!(env_vars.get("OTHER_TEST_VAR_FOR_TEST").unwrap(), "3");
    }
}
