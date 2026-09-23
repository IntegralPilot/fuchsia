// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::EnvironmentContext;
use crate::environment::EnvironmentKind;

use camino::Utf8Path;
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};

use std::env::var;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathsError {
    #[error("cannot find home directory")]
    HomeDirectoryNotFound,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("'shared_data' config: {0} is not a string")]
    SharedDataNotString(String),

    #[error("SHARED_DATA must be specified in strict mode. Use `ffx ... -c shared_data=<dir>.`")]
    SharedDataRequiredInStrictMode,
}

pub const ENV_FILE: &str = ".ffx_env";
pub const USER_FILE: &str = ".ffx_user_config.json";
pub const DEFAULT_BUILD_CONFIG_FILE: &str = "ffx-config.json";

impl EnvironmentKind {
    pub fn get_default_user_file_path(&self) -> std::result::Result<PathBuf, PathsError> {
        match self.isolate_root() {
            Some(isolate_root) => Ok(isolate_root.join(USER_FILE)),
            _ => get_default_user_file_path(),
        }
    }

    pub fn get_default_env_path(&self) -> std::result::Result<PathBuf, PathsError> {
        match self.isolate_root() {
            Some(isolate_root) => Ok(isolate_root.join(ENV_FILE)),
            _ => default_env_path(),
        }
    }

    pub fn get_default_build_dir_config_path(
        &self,
        build_dir: &Path,
    ) -> std::result::Result<PathBuf, PathsError> {
        let filename = build_dir.join(DEFAULT_BUILD_CONFIG_FILE);
        Ok(filename)
    }

    pub fn get_build_config_file(&self) -> Option<&Utf8Path> {
        match self {
            EnvironmentKind::ConfigDomain { domain, .. } => domain.get_build_config_file(),
            _ => None,
        }
    }
}

impl EnvironmentContext {
    pub fn get_default_user_file_path(&self) -> std::result::Result<PathBuf, PathsError> {
        self.env_kind().get_default_user_file_path()
    }

    pub fn get_default_env_path(&self) -> std::result::Result<PathBuf, PathsError> {
        self.env_kind().get_default_env_path()
    }

    pub fn get_default_build_dir_config_path(
        &self,
        build_dir: &Path,
    ) -> std::result::Result<PathBuf, PathsError> {
        self.env_kind().get_default_build_dir_config_path(build_dir)
    }

    /// If this environment context has an explicitly set build config path,
    /// return it. Otherwise None.
    pub fn get_build_config_file(&self) -> Option<&Utf8Path> {
        self.env_kind().get_build_config_file()
    }

    pub fn get_default_ascendd_path(&self) -> std::result::Result<PathBuf, PathsError> {
        match (self.env_var("ASCENDD"), self.env_kind()) {
            (Ok(path), _) => Ok(PathBuf::from(&path)),
            (_, EnvironmentKind::InTree { build_dir: Some(p), .. }) => {
                Ok(p.join(".ffx-daemon/daemon.sock"))
            }
            (_, EnvironmentKind::Isolated { isolate_root }) => Ok(isolate_root.join("daemon.sock")),
            (_, EnvironmentKind::ConfigDomain { isolate_root: Some(isolate_root), .. }) => {
                Ok(isolate_root.join("daemon.sock").into())
            }
            (_, _) => Ok(default_ascendd_path()),
        }
    }

    pub fn get_runtime_path(&self) -> std::result::Result<PathBuf, PathsError> {
        match self.env_kind().isolate_root() {
            Some(isolate_root) => Ok(isolate_root.join("runtime")),
            _ => get_runtime_base_path(),
        }
    }

    pub fn get_cache_path(&self) -> std::result::Result<PathBuf, PathsError> {
        match self.env_kind().isolate_root() {
            Some(isolate_root) => Ok(isolate_root.join("cache")),
            _ => get_cache_base_path(),
        }
    }

    pub fn get_config_path(&self) -> std::result::Result<PathBuf, PathsError> {
        match self.env_kind().isolate_root() {
            Some(isolate_root) => Ok(isolate_root.join("config")),
            _ => get_config_base_path(),
        }
    }

    pub fn get_data_path(&self) -> std::result::Result<PathBuf, PathsError> {
        match self.env_kind().isolate_root() {
            Some(isolate_root) => Ok(isolate_root.join("data")),
            _ => get_data_base_path(),
        }
    }

    pub fn get_shared_data_path(&self) -> std::result::Result<PathBuf, PathsError> {
        // Special handling for $SHARED_DATA: it can be specified on the command
        // line with "-c shared_data=<dir>". Then, we allow the expansion of
        // $SHARED_DATA in strict mode, but _only_ if the actual shared-data
        // directory has been specified on the command line. Note that this
        // function will only be called for queries that actually contain
        // "$SHARED_DATA".
        if let Some(v) = self.runtime_args.get("shared_data") {
            if let Some(s) = v.as_str() {
                Ok(PathBuf::from(s))
            } else {
                return Err(PathsError::SharedDataNotString(format!("{v:?}")));
            }
        } else {
            if self.is_strict() {
                return Err(PathsError::SharedDataRequiredInStrictMode);
            } else {
                Ok(get_shared_data_base_path()?.join("shared"))
            }
        }
    }

    pub fn get_analytics_path(&self) -> Option<PathBuf> {
        if self.has_no_environment() {
            return None;
        }
        match self.env_kind().isolate_root() {
            Some(isolate_root) => Some(isolate_root.join("metrics")),
            _ => get_analytics_base_path().ok(),
        }
    }
}

fn get_runtime_base() -> std::result::Result<PathBuf, PathsError> {
    if cfg!(target_os = "macos") {
        let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
        home.push("Library");
        Ok(home)
    } else {
        var("XDG_RUNTIME_HOME").map(PathBuf::from).or_else(|_| {
            let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
            home.push(".local");
            home.push("share");
            Ok(home)
        })
    }
}

fn default_ascendd_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push("ascendd");
    path
}

fn get_runtime_base_path() -> std::result::Result<PathBuf, PathsError> {
    let mut path = get_runtime_base()?;
    path.push("Fuchsia");
    path.push("ffx");
    path.push("runtime");
    create_dir_all(&path)?;
    Ok(path)
}

fn get_cache_base() -> std::result::Result<PathBuf, PathsError> {
    if cfg!(target_os = "macos") {
        let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
        home.push("Library");
        home.push("Caches");
        Ok(home)
    } else {
        var("XDG_CACHE_HOME").map(PathBuf::from).or_else(|_| {
            let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
            home.push(".local");
            home.push("share");
            Ok(home)
        })
    }
}

fn get_cache_base_path() -> std::result::Result<PathBuf, PathsError> {
    let mut path = get_cache_base()?;
    path.push("Fuchsia");
    path.push("ffx");
    path.push("cache");
    create_dir_all(&path)?;
    Ok(path)
}

fn get_config_base() -> std::result::Result<PathBuf, PathsError> {
    if cfg!(target_os = "macos") {
        let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
        home.push("Library");
        home.push("Preferences");
        Ok(home)
    } else {
        var("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|_| {
            let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
            home.push(".local");
            home.push("share");
            Ok(home)
        })
    }
}

fn get_config_base_path() -> std::result::Result<PathBuf, PathsError> {
    let mut path = get_config_base()?;
    path.push("Fuchsia");
    path.push("ffx");
    path.push("config");
    create_dir_all(&path)?;
    Ok(path)
}

fn get_analytics_base_path() -> std::result::Result<PathBuf, PathsError> {
    let mut path = get_data_base()?;
    path.push("Fuchsia");
    path.push("metrics");
    create_dir_all(&path)?;
    Ok(path)
}

fn default_env_path() -> std::result::Result<PathBuf, PathsError> {
    // Environment file that keeps track of configuration files
    get_config_base_path().map(|mut path| {
        path.push(ENV_FILE);
        path
    })
}

fn get_data_base() -> std::result::Result<PathBuf, PathsError> {
    if cfg!(target_os = "macos") {
        let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
        home.push("Library");
        Ok(home)
    } else {
        var("XDG_DATA_HOME").map(PathBuf::from).or_else(|_| {
            let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
            home.push(".local");
            home.push("share");
            Ok(home)
        })
    }
}

pub fn get_state_base() -> std::result::Result<PathBuf, PathsError> {
    if cfg!(target_os = "macos") {
        let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
        home.push("Library");
        Ok(home)
    } else {
        var("XDG_STATE_HOME").map(PathBuf::from).or_else(|_| {
            let mut home = home::home_dir().ok_or(PathsError::HomeDirectoryNotFound)?;
            home.push(".local");
            home.push("share");
            Ok(home)
        })
    }
}

fn get_data_base_path() -> std::result::Result<PathBuf, PathsError> {
    let mut path = get_data_base()?;
    path.push("Fuchsia");
    path.push("ffx");
    create_dir_all(&path)?;
    Ok(path)
}

fn get_shared_data_base_path() -> std::result::Result<PathBuf, PathsError> {
    let mut path = get_state_base()?;
    path.push("Fuchsia");
    path.push("ffx");
    create_dir_all(&path)?;
    Ok(path)
}

fn get_default_user_file_path() -> std::result::Result<PathBuf, PathsError> {
    // Default user configuration file
    const DEFAULT_USER_CONFIG: &str = ".ffx_user_config.json";

    let mut default_path = get_config_base_path()?;
    default_path.push(DEFAULT_USER_CONFIG);
    Ok(default_path)
}
