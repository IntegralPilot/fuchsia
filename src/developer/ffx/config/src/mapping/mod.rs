// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MappingError {
    #[error("path contains invalid UTF-8: {0:?}")]
    InvalidUtf8(PathBuf),

    #[error("Paths error: {0}")]
    Paths(#[from] crate::paths::PathsError),

    #[error("environment variable not found: {0}")]
    EnvironmentVariableNotFound(String),

    #[error("variable mapping (${0}) is ignored in strict mode")]
    StrictVariableIgnored(String),
}

mod file_check;
mod filter;
mod flatten;
mod workspace;

pub(crate) use file_check::file_check;
pub(crate) use filter::filter;
pub(crate) use flatten::flatten;

use crate::EnvironmentContext;
use std::sync::LazyLock;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BuiltinMacro {
    BuildDir,
    FindWorkspaceRoot,
    SharedData,
    Runtime,
    Cache,
    Data,
    Config,
    Home,
}

impl BuiltinMacro {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "BUILD_DIR" => Some(Self::BuildDir),
            "FIND_WORKSPACE_ROOT" => Some(Self::FindWorkspaceRoot),
            "SHARED_DATA" => Some(Self::SharedData),
            "RUNTIME" => Some(Self::Runtime),
            "CACHE" => Some(Self::Cache),
            "DATA" => Some(Self::Data),
            "CONFIG" => Some(Self::Config),
            "HOME" => Some(Self::Home),
            _ => None,
        }
    }

    pub fn is_strict_allowed(&self) -> bool {
        matches!(self, Self::BuildDir | Self::FindWorkspaceRoot | Self::SharedData)
    }

    pub fn resolve(&self, ctx: &EnvironmentContext) -> Result<Option<String>, MappingError> {
        match self {
            Self::BuildDir => to_utf8_path(ctx.build_dir().map(Path::to_path_buf)),
            Self::FindWorkspaceRoot => to_utf8_path(
                ctx.project_root().and_then(workspace::find_workspace_root).map(Path::to_path_buf),
            ),
            Self::SharedData => {
                to_utf8_path(Some(ctx.get_shared_data_path().map_err(MappingError::Paths)?))
            }
            Self::Runtime => to_utf8_path(ctx.get_runtime_path().ok()),
            Self::Cache => to_utf8_path(ctx.get_cache_path().ok()),
            Self::Data => to_utf8_path(ctx.get_data_path().ok()),
            Self::Config => to_utf8_path(ctx.get_config_path().ok()),
            Self::Home => to_utf8_path(home::home_dir()),
        }
    }
}

fn to_utf8_path(path: Option<PathBuf>) -> Result<Option<String>, MappingError> {
    match path {
        Some(p) => {
            let s = p.to_str().map(String::from).ok_or(MappingError::InvalidUtf8(p))?;
            Ok(Some(s))
        }
        None => Ok(None),
    }
}

/// Single-pass macro and environment variable expander for configuration strings.
///
/// Expands variables such as `$BUILD_DIR`, `$HOME`, `$CACHE`, `$DATA`, `$SHARED_DATA`,
/// `$CONFIG`, `$RUNTIME`, `$FIND_WORKSPACE_ROOT`, and environment variables `$ENV_VAR` in a
/// single scan. Escaped dollar signs (`$$`) are converted to a literal `$`.
///
/// Substituted variable values are appended directly to the output buffer without being
/// rescanned, preventing second-order macro injection.
fn expand_string_with_source(
    ctx: &EnvironmentContext,
    s: &str,
    strict: bool,
) -> Result<Option<(String, Option<String>)>, MappingError> {
    if !s.contains('$') {
        return Ok(Some((s.to_string(), None)));
    }

    static MACRO_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$\$|\$([A-Z][A-Z0-9_]*)").unwrap());

    let mut result = String::with_capacity(s.len());
    let mut last_end = 0;
    let mut unresolvable = false;
    let mut expanded_var = None;

    for caps in MACRO_REGEX.captures_iter(s) {
        let m = caps.get(0).unwrap();
        if !unresolvable {
            result.push_str(&s[last_end..m.start()]);
        }
        last_end = m.end();

        if m.as_str() == "$$" {
            if !unresolvable {
                result.push('$');
            }
            continue;
        }

        let Some(var_cap) = caps.get(1) else {
            continue;
        };
        let var_name = var_cap.as_str();

        if strict {
            match BuiltinMacro::parse(var_name) {
                Some(m) if m.is_strict_allowed() => {
                    if !unresolvable {
                        match m.resolve(ctx)? {
                            Some(val) => {
                                if expanded_var.is_none() {
                                    expanded_var = Some(var_name.to_string());
                                }
                                result.push_str(&val);
                            }
                            None => unresolvable = true,
                        }
                    }
                }
                Some(_) | None => {
                    return Err(MappingError::StrictVariableIgnored(var_name.to_string()));
                }
            }
        } else {
            let expanded = match BuiltinMacro::parse(var_name) {
                Some(m) => m.resolve(ctx)?,
                None => ctx.env_var(var_name).ok(),
            };

            match expanded {
                Some(val) => {
                    if expanded_var.is_none() {
                        expanded_var = Some(var_name.to_string());
                    }
                    result.push_str(&val);
                }
                None => return Ok(None),
            }
        }
    }

    if unresolvable {
        return Ok(None);
    }

    result.push_str(&s[last_end..]);
    Ok(Some((result, expanded_var)))
}

pub(crate) fn expand_macros_with_recorder<F: FnMut(&str)>(
    ctx: &EnvironmentContext,
    value: Value,
    mut on_var_expanded: F,
) -> Result<Option<Value>, MappingError> {
    match value {
        Value::String(s) => match expand_string_with_source(ctx, &s, false)? {
            Some((expanded, var)) => {
                if let Some(v) = var {
                    on_var_expanded(&v);
                }
                Ok(Some(Value::String(expanded)))
            }
            None => Ok(None),
        },
        other => Ok(Some(other)),
    }
}

pub(crate) fn expand_macros_strict_with_recorder<F: FnMut(&str)>(
    ctx: &EnvironmentContext,
    value: Value,
    mut on_var_expanded: F,
) -> Result<Option<Value>, MappingError> {
    match value {
        Value::String(s) => match expand_string_with_source(ctx, &s, true)? {
            Some((expanded, var)) => {
                if let Some(v) = var {
                    on_var_expanded(&v);
                }
                Ok(Some(Value::String(expanded)))
            }
            None => Ok(None),
        },
        other => Ok(Some(other)),
    }
}

#[cfg(test)]
pub(crate) fn expand_macros(
    ctx: &EnvironmentContext,
    value: Value,
) -> Result<Option<Value>, MappingError> {
    expand_macros_with_recorder(ctx, value, |_| {})
}

pub(crate) fn expand_macros_strict(
    ctx: &EnvironmentContext,
    value: Value,
) -> Result<Option<Value>, MappingError> {
    expand_macros_strict_with_recorder(ctx, value, |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn in_tree_env(build_dir: Option<PathBuf>) -> EnvironmentContext {
        EnvironmentContext::in_tree(
            crate::environment::ExecutableKind::Test,
            "/tmp".into(),
            build_dir,
            Default::default(),
            Default::default(),
            false,
        )
        .unwrap()
    }

    fn isolated_env(env_vars: &[(&str, &str)]) -> EnvironmentContext {
        let env_map = env_vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        EnvironmentContext::isolated(
            crate::environment::ExecutableKind::Test,
            "/tmp".into(),
            env_map,
            Default::default(),
            None,
            None,
            false,
        )
        .unwrap()
    }

    #[test]
    fn test_expand_macros_passthrough() {
        let ctx = in_tree_env(Some("/tmp/build".into()));

        for value in [
            json!(123),
            json!(false),
            json!(""),
            json!("hello world"),
            json!("foo$"),
            json!("$123"),
            json!("$foo"),
        ] {
            assert_eq!(expand_macros(&ctx, value.clone()).unwrap(), Some(value));
        }
    }

    #[test]
    fn test_expand_macros_escaping() {
        let ctx = in_tree_env(Some("/tmp/build".into()));

        for (input, expected) in [
            ("$$BUILD_DIR/logs", "$BUILD_DIR/logs"),
            ("$$HOME/path", "$HOME/path"),
            ("$$$$", "$$"),
            ("$$$BUILD_DIR", "$/tmp/build"),
        ] {
            assert_eq!(expand_macros(&ctx, json!(input)).unwrap(), Some(json!(expected)));
        }
    }

    #[test]
    fn test_expand_macros_no_second_order_expansion() {
        let ctx = in_tree_env(Some("/tmp/$SECRET_VAR".into()));
        // $BUILD_DIR expands to /tmp/$SECRET_VAR, but $SECRET_VAR is NOT expanded by env_var!
        assert_eq!(
            expand_macros(&ctx, json!("$BUILD_DIR/logs")).unwrap(),
            Some(json!("/tmp/$SECRET_VAR/logs"))
        );
    }

    #[test]
    fn test_expand_macros_builtins_and_env_vars() {
        let ctx = isolated_env(&[("MY_VAR", "val")]);

        let data = ctx.get_data_path().unwrap().to_string_lossy().to_string();
        let cache = ctx.get_cache_path().unwrap().to_string_lossy().to_string();
        let config = ctx.get_config_path().unwrap().to_string_lossy().to_string();
        let runtime = ctx.get_runtime_path().unwrap().to_string_lossy().to_string();
        let shared = ctx.get_shared_data_path().unwrap().to_string_lossy().to_string();
        let home = home::home_dir().and_then(|p| p.to_str().map(String::from));

        for (macro_str, expected) in [
            ("$DATA", Some(data.clone())),
            ("$CACHE", Some(cache)),
            ("$CONFIG", Some(config)),
            ("$RUNTIME", Some(runtime)),
            ("$SHARED_DATA", Some(shared)),
            ("$HOME", home),
            ("$DATA/$DATA", Some(format!("{data}/{data}"))),
            ("prefix-$MY_VAR-suffix", Some("prefix-val-suffix".to_string())),
        ] {
            assert_eq!(
                expand_macros(&ctx, json!(macro_str)).unwrap(),
                expected.map(Value::String),
                "failed for macro: {macro_str}"
            );
        }
    }

    #[test]
    fn test_expand_macros_unresolvable_returns_none() {
        let ctx = isolated_env(&[]);

        for input in [
            "$NONEXISTENT_VAR",
            "$BUILD_DIR/file",
            "$FIND_WORKSPACE_ROOT/file",
            "$DATA/$NONEXISTENT_VAR",
        ] {
            assert_eq!(
                expand_macros(&ctx, json!(input)).unwrap(),
                None,
                "expected None for: {input}"
            );
        }
    }

    #[test]
    fn test_expand_macros_workspace_resolution() {
        let workspace_dir = tempfile::tempdir().expect("tempdir");
        std::fs::File::create(workspace_dir.path().join("WORKSPACE")).expect("workspace file");
        let config_domain = ffx_config_domain::ConfigDomain::load_from_contents(
            workspace_dir.path().join("fuchsia_env.toml").try_into().unwrap(),
            Default::default(),
        )
        .unwrap();
        let ctx = EnvironmentContext::config_domain(
            crate::environment::ExecutableKind::Test,
            config_domain,
            Default::default(),
            Some(workspace_dir.path().to_owned()),
            false,
        )
        .unwrap();

        let ws = workspace_dir.path().to_string_lossy().to_string();
        assert_eq!(
            expand_macros(&ctx, json!("$FIND_WORKSPACE_ROOT/sub")).unwrap(),
            Some(json!(format!("{ws}/sub")))
        );
        assert_eq!(
            expand_macros(&ctx, json!("$FIND_WORKSPACE_ROOT/$FIND_WORKSPACE_ROOT")).unwrap(),
            Some(json!(format!("{ws}/{ws}")))
        );
    }

    #[test]
    fn test_expand_macros_strict_mode() {
        let ctx_with_build = in_tree_env(Some("/tmp/build".into()));

        // Allowed macro expands properly in strict mode
        assert_eq!(
            expand_macros_strict(&ctx_with_build, json!("$BUILD_DIR/logs")).unwrap(),
            Some(json!("/tmp/build/logs"))
        );

        // Disallowed built-in macros and env vars are rejected in strict mode
        for disallowed in ["HOME", "RUNTIME", "CACHE", "DATA", "CONFIG", "CUSTOM_VAR"] {
            let err = expand_macros_strict(&ctx_with_build, json!(format!("${disallowed}/path")))
                .unwrap_err();
            assert!(
                matches!(err, MappingError::StrictVariableIgnored(ref v) if v == disallowed),
                "expected StrictVariableIgnored for {disallowed}, got {err:?}"
            );
        }

        // Order-independent validation when an allowed macro evaluates to None
        let ctx_no_build = in_tree_env(None);
        for input in ["$BUILD_DIR/$HOME", "$HOME/$BUILD_DIR", "$BUILD_DIR/$UNKNOWN_VAR"] {
            let err = expand_macros_strict(&ctx_no_build, json!(input)).unwrap_err();
            assert!(
                matches!(err, MappingError::StrictVariableIgnored(_)),
                "expected strict error for {input}, got {err:?}"
            );
        }

        // Two allowed macros where one is None evaluates to None without error
        assert_eq!(
            expand_macros_strict(&ctx_no_build, json!("$BUILD_DIR/$SHARED_DATA")).unwrap(),
            None
        );
    }
}
