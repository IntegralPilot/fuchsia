// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_config_domain::ConfigDomain;
use std::fmt;
use std::path::{Path, PathBuf};

/// The type of environment we're running in, along with relevant information about
/// that environment.
#[derive(Clone, Debug, PartialEq)]
pub enum EnvironmentKind {
    /// In a project with a fuchsia_env file at its root with config domain info
    /// in it.
    ConfigDomain { domain: Box<ConfigDomain>, isolate_root: Option<PathBuf> },
    /// In a fuchsia.git build tree with a jiri root and possibly a build directory.
    InTree { tree_root: PathBuf, build_dir: Option<PathBuf> },
    /// Isolated within a particular directory for testing or consistency purposes
    Isolated { isolate_root: PathBuf },
    /// An environment context in which the config should:
    /// a.) Be readonly.
    /// b.) Return an error if any config values are not able to be found.
    /// c.) No shell env substitution occurs (things like $FUCHSIA_DIR are not allowed).
    StrictContext,
    /// all (non-global/default) configuration.
    NoContext,
}

impl EnvironmentKind {
    /// Get the isolate root of this environment
    pub fn isolate_root(&self) -> Option<&Path> {
        match self {
            Self::ConfigDomain { isolate_root, .. } => isolate_root.as_deref(),
            Self::Isolated { isolate_root } => Some(&isolate_root),
            _ => None,
        }
    }

    /// Whether this is an isolated context.
    pub fn is_isolated(&self) -> bool {
        matches!(
            self,
            EnvironmentKind::Isolated { .. }
                | EnvironmentKind::ConfigDomain { isolate_root: Some(_), .. }
        )
    }

    pub fn build_dir(&self) -> Option<&Path> {
        match self {
            EnvironmentKind::InTree { build_dir, .. } => build_dir.as_deref(),
            EnvironmentKind::ConfigDomain { domain, .. } => {
                Some(domain.get_build_dir()?.as_std_path())
            }
            _ => None,
        }
    }

    pub fn get_default_overrides(&self) -> crate::ConfigMap {
        use EnvironmentKind::*;
        let mut cm = match self {
            ConfigDomain { domain, .. } => {
                let mut defaults = domain.get_config_defaults().clone();
                sanitize_untrusted_domain_defaults(&mut defaults);
                defaults
            }
            _ => crate::ConfigMap::default(),
        };
        if self.is_isolated() {
            crate::aliases::add_isolation_default(&mut cm);
        }
        cm
    }
}

/// Strips high-risk execution and path redirection configuration keys from project-level
/// fuchsia_env default configuration. Tool binary overrides (sdk.overrides), subtool
/// search paths (ffx.subtool-search-paths), log directories (log.dir), emulator scripts
/// (emu.upscript), and SSH credentials (ssh.priv/pub) must be configured via user configuration
/// or explicit CLI flags.
fn sanitize_untrusted_domain_defaults(map: &mut crate::ConfigMap) {
    const SENSITIVE_PREFIXES: &[&str] = &[
        "sdk.overrides",
        "sdk.override",
        "ffx.subtool-search-paths",
        "ffx.subtool_search_paths",
        "ffx.subtool-manifest",
        "ffx.subtool_manifest",
        "log.dir",
        "emu.upscript",
        "ssh.priv",
        "ssh.pub",
        "ssh.auth-sock",
        "ssh.auth_sock",
    ];

    let is_match = |k: &str, prefix: &str| -> bool {
        k.strip_prefix(prefix).is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('.'))
    };
    let is_sensitive_key =
        |k: &str| -> bool { SENSITIVE_PREFIXES.iter().any(|&prefix| is_match(k, prefix)) };
    map.retain(|k, _| !is_sensitive_key(k));

    if let Some(serde_json::Value::Object(sdk_map)) = map.get_mut("sdk") {
        sdk_map.retain(|k, _| !is_match(k, "overrides") && !is_match(k, "override"));
    }
    if let Some(serde_json::Value::Object(ffx_map)) = map.get_mut("ffx") {
        ffx_map.retain(|k, _| {
            !is_match(k, "subtool-search-paths")
                && !is_match(k, "subtool_search_paths")
                && !is_match(k, "subtool-manifest")
                && !is_match(k, "subtool_manifest")
        });
    }
    if let Some(serde_json::Value::Object(log_map)) = map.get_mut("log") {
        log_map.retain(|k, _| !is_match(k, "dir"));
    }
    if let Some(serde_json::Value::Object(emu_map)) = map.get_mut("emu") {
        emu_map.retain(|k, _| !is_match(k, "upscript"));
    }
    if let Some(serde_json::Value::Object(ssh_map)) = map.get_mut("ssh") {
        ssh_map.retain(|k, _| {
            !is_match(k, "priv")
                && !is_match(k, "pub")
                && !is_match(k, "auth-sock")
                && !is_match(k, "auth_sock")
        });
    }
}

impl std::fmt::Display for EnvironmentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use EnvironmentKind::*;
        match self {
            ConfigDomain { domain, isolate_root: None } => {
                write!(f, "Fuchsia Project Rooted at {}", domain.root(),)
            }
            ConfigDomain { domain, isolate_root: Some(isolation_root) } => write!(
                f,
                "Fuchsia Project Rooted at {} using isolation root at {}",
                domain.root(),
                isolation_root.display(),
            ),
            InTree { tree_root, build_dir: Some(build_dir) } => write!(
                f,
                "Fuchsia.git In-Tree Rooted at {root}, with default build directory of {build}",
                root = tree_root.display(),
                build = build_dir.display()
            ),
            InTree { tree_root, build_dir: None } => write!(
                f,
                "Fuchsia.git In-Tree Root at {root} with no default build directory",
                root = tree_root.display()
            ),
            Isolated { isolate_root } => write!(
                f,
                "Isolated environment with an isolated root of {root}",
                root = isolate_root.display()
            ),
            StrictContext => write!(f, "ffx-core Env Context"),
            NoContext => write!(f, "Global user context"),
        }
    }
}

/// What kind of executable target this environment was created in
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExecutableKind {
    /// A main "launcher" ffx that can be used to run other ffx commands.
    MainFfx,
    /// A subtool ffx that only knows its own command.
    Subtool,
    /// In a unit or integration test
    Test,
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sanitize_untrusted_domain_defaults() {
        let mut map: crate::ConfigMap = match json!({
            "sdk.overrides.fastboot": "/evil/fastboot",
            "sdk.override.gsutil": "/evil/gsutil",
            "sdk": {
                "root": "/good/sdk",
                "overrides": {
                    "emulator": "/evil/emu"
                },
                "override.dotted": "/evil/dotted_tool",
                "override": {
                    "tool": "/evil/tool"
                },
                "version": "1.0.0"
            },
            "ffx.subtool-search-paths": ["/evil/subtools"],
            "ffx.subtool-search-paths.0": "/evil/indexed_subtool",
            "ffx.subtool_manifest": "/evil/manifest2.json",
            "ffx": {
                "subtool-search-paths": ["/evil/subtools2"],
                "subtool-manifest": "/evil/manifest.json",
                "subtool_manifest": "/evil/manifest_nested.json",
                "ui": { "mode": "text" }
            },
            "log.dir": "/evil/log",
            "log.directory": "/good/dir",
            "log.dir_size": 1024,
            "log": {
                "dir": "/evil/log2",
                "enabled": true
            },
            "emu.upscript": "/evil/upscript.sh",
            "emu": {
                "upscript": "/evil/nested_upscript.sh",
                "device": "qemu-x64"
            },
            "ssh.priv": "/evil/key",
            "ssh.auth_sock": "/evil/sock2",
            "ssh": {
                "priv": "/evil/key2",
                "pub": "/evil/key2.pub",
                "auth-sock": "/evil/sock",
                "auth_sock": "/evil/sock_nested",
                "authorized_keys_server_port": 9797
            },
            "repository.default": "my-repo"
        }) {
            serde_json::Value::Object(map) => map,
            _ => panic!("expected object"),
        };

        sanitize_untrusted_domain_defaults(&mut map);

        assert!(!map.contains_key("sdk.overrides.fastboot"));
        assert!(!map.contains_key("sdk.override.gsutil"));
        assert!(!map.contains_key("ffx.subtool-search-paths"));
        assert!(!map.contains_key("ffx.subtool-search-paths.0"));
        assert!(!map.contains_key("ffx.subtool_manifest"));
        assert!(!map.contains_key("log.dir"));
        assert_eq!(map.get("log.directory").unwrap(), "/good/dir");
        assert_eq!(map.get("log.dir_size").unwrap(), 1024);
        assert!(!map.contains_key("emu.upscript"));
        assert!(!map.contains_key("ssh.priv"));
        assert!(!map.contains_key("ssh.auth_sock"));
        assert_eq!(map.get("repository.default").unwrap(), "my-repo");

        let sdk = map.get("sdk").unwrap().as_object().unwrap();
        assert_eq!(sdk.get("root").unwrap(), "/good/sdk");
        assert!(!sdk.contains_key("overrides"));
        assert!(!sdk.contains_key("override"));
        assert!(!sdk.contains_key("override.dotted"));
        assert_eq!(sdk.get("version").unwrap(), "1.0.0");

        let ffx = map.get("ffx").unwrap().as_object().unwrap();
        assert!(!ffx.contains_key("subtool-search-paths"));
        assert!(!ffx.contains_key("subtool-manifest"));
        assert!(!ffx.contains_key("subtool_manifest"));
        assert!(ffx.contains_key("ui"));

        let log = map.get("log").unwrap().as_object().unwrap();
        assert!(!log.contains_key("dir"));
        assert_eq!(log.get("enabled").unwrap(), true);

        let emu = map.get("emu").unwrap().as_object().unwrap();
        assert!(!emu.contains_key("upscript"));
        assert_eq!(emu.get("device").unwrap(), "qemu-x64");

        let ssh = map.get("ssh").unwrap().as_object().unwrap();
        assert!(!ssh.contains_key("priv"));
        assert!(!ssh.contains_key("pub"));
        assert!(!ssh.contains_key("auth-sock"));
        assert!(!ssh.contains_key("auth_sock"));
        assert_eq!(ssh.get("authorized_keys_server_port").unwrap(), 9797);
    }
}
