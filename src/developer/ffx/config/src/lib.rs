// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use errors as _;

use crate::api::value::{ConfigValue, ValueStrategy};

use analytics::metrics_state::MetricsStatus;
use analytics::{set_new_opt_in_status, show_status_message};

use core::fmt;
use futures::future::LocalBoxFuture;
use std::fmt::Debug;
use std::io::Write;
use std::path::PathBuf;

pub mod api;
pub mod environment;
pub mod keys;
pub mod logging;
pub mod runtime;

mod aliases;
mod mapping;
mod nested;
mod paths;
mod storage;

pub use aliases::{
    is_analytics_disabled, is_mdns_autoconnect_disabled, is_mdns_discovery_disabled,
    is_usb_discovery_disabled,
};
pub use api::ConfigError;
pub use api::query::{ConfigQuery, ConfigQueryBuilder, SelectMode};
pub use config_macros::FfxConfigBacked;

pub use environment::{Environment, EnvironmentContext, TestEnv, test_env, test_init};
pub use paths::get_state_base as get_state_base_path;
pub use sdk::{self, Sdk, SdkRoot};
pub use storage::{AssertNoEnv, AssertNoEnvError, Config, ConfigMap};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LibError {
    #[error("SDK context error: {0}")]
    Sdk(#[from] crate::environment::ContextError),

    #[error("Override path for {0} set to {1:?}, but does not exist")]
    OverrideNotExist(String, PathBuf),

    #[error("SDK returned {0:?} for {1}, but does not exist")]
    SdkToolNotExist(PathBuf, String),

    #[error("Config error: {0}")]
    Config(#[from] crate::ConfigError),

    #[error("Environment error: {0}")]
    Environment(#[from] crate::environment::EnvironmentError),

    #[error("Config read guard failed")]
    ReadGuard,

    #[error("Displaying config failed: {0}")]
    DisplayConfig(#[source] std::io::Error),

    #[error("Failed to load host log directories from ffx config: {0}")]
    LoadLogDirs(String),

    #[error("Metrics error: {0}")]
    Metrics(String),

    #[error("SDK error: {0}")]
    SdkError(#[from] sdk::SdkError),
}

#[doc(hidden)]
pub mod macro_deps {
    pub use anyhow;
    pub use serde_json;
}

pub trait TryFromEnvContext: Sized + Debug {
    fn try_from_env_context<'a>(
        env: &'a EnvironmentContext,
    ) -> LocalBoxFuture<'a, ffx_command_error::Result<Self>>;
}

/// The levels of configuration possible
// If you edit this enum, make sure to also change the enum counter below to match.
#[derive(Debug, Eq, PartialEq, Copy, Clone, Hash)]
pub enum ConfigLevel {
    /// Default configurations are provided through GN build rules across all subcommands and are
    ///  hard-coded and immutable.
    Default,
    /// Global configuration is intended to be a system-wide configuration level. It is intended to
    /// be used when installing ffx on a host system to set organizational
    /// properties that would be the same for a collection of users.
    Global,
    /// Build configuration is associated with a build directory. It is intended to be used to set
    /// properties describing the output of the build. It should be generated as part of the build
    /// process, and is considered read-only by ffx.
    Build,
    /// User configuration is configuration set in the user's home directory and applies to all
    /// invocations of ffx by that user. User configuration can be overridden only at runtime.
    User,
    /// Runtime configuration is set by the user when invoking ffx, and can't be 'set' by any other means.
    Runtime,
}

/// Describes the source and provenance of a configuration value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigSource {
    /// The configuration level where the value was found.
    pub level: ConfigLevel,
    /// The file path of the configuration file, if the value came from a file-backed level.
    pub file_path: Option<PathBuf>,
    /// The name of the environment variable or macro expanded to produce this value, if any.
    ///
    /// When multiple variables are expanded in a single string (e.g. "$FOO/$BAR"),
    /// this records the first expanded variable.
    pub expanded_var: Option<String>,
}

impl ConfigSource {
    pub fn new(level: ConfigLevel) -> Self {
        Self { level, file_path: None, expanded_var: None }
    }

    pub fn with_file_path(mut self, path: Option<PathBuf>) -> Self {
        self.file_path = path;
        self
    }

    pub fn with_expanded_var(mut self, var: Option<String>) -> Self {
        self.expanded_var = var;
        self
    }
}
impl ConfigLevel {
    /// The number of elements in the above enum, used for tests.
    const _COUNT: usize = 5;

    /// Iterates over the config levels in priority order, starting from the most narrow scope if given None.
    /// Note this is not conformant to Iterator::next(), it's just meant to be a simple source of truth about ordering.
    pub(crate) fn next(current: Option<Self>) -> Option<Self> {
        use ConfigLevel::*;
        match current {
            Some(Default) => None,
            Some(Global) => Some(Default),
            Some(Build) => Some(Global),
            Some(User) => Some(Build),
            Some(Runtime) => Some(User),
            None => Some(Runtime),
        }
    }
}
impl fmt::Display for ConfigLevel {
    // Required method
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let val = match self {
            ConfigLevel::Default => "default",
            ConfigLevel::Global => "global",
            ConfigLevel::User => "user",
            ConfigLevel::Build => "build",
            ConfigLevel::Runtime => "runtime",
        };
        write!(f, "{}", val)
    }
}

impl argh::FromArgValue for ConfigLevel {
    fn from_arg_value(val: &str) -> Result<Self, String> {
        match val {
            "u" | "user" => Ok(ConfigLevel::User),
            "b" | "build" => Ok(ConfigLevel::Build),
            "g" | "global" => Ok(ConfigLevel::Global),
            _ => Err(String::from(
                "Unrecognized value. Possible values are \"user\",\"build\",\"global\".",
            )),
        }
    }
}

pub const SDK_OVERRIDE_KEY_PREFIX: &str = "sdk.overrides";
pub const SDK_ALLOW_BUILD_HOST_TOOLS: &str = keys::SDK_ALLOW_BUILD_HOST_TOOLS;

/// Returns whether build-level tool overrides (`sdk.overrides.*` in build configuration)
/// are enabled.
///
/// If explicitly configured at a trusted user configuration level ([`ConfigLevel::Runtime`],
/// [`ConfigLevel::User`], or [`ConfigLevel::Global`]), that value is used.
/// Otherwise, defaults to `true` if `ffx` itself is running from within the configured
/// build directory, and `false` otherwise.
pub fn allow_build_host_tools(ctx: &EnvironmentContext) -> bool {
    for level in [ConfigLevel::Runtime, ConfigLevel::User, ConfigLevel::Global] {
        match ctx.query(SDK_ALLOW_BUILD_HOST_TOOLS).level(Some(level)).build().get::<bool>(ctx) {
            Ok(allowed) => return allowed,
            Err(ConfigError::KeyNotFound | ConfigError::NoValueSet(_)) => continue,
            Err(_) => continue,
        }
    }
    ctx.is_self_in_build_dir()
}

/// Returns the path to the tool with the given name by first checking for configured
/// overrides with the key `sdk.overrides.{name}`, falling back to [`Sdk::get_host_tool`].
///
/// Tool binary overrides must come from explicit user-controlled levels ([`ConfigLevel::Runtime`],
/// [`ConfigLevel::User`], or [`ConfigLevel::Global`]), or from [`ConfigLevel::Build`] if
/// build overrides are enabled via `sdk.overrides.allow-build-host-tools` (which defaults to
/// `true` when `ffx` is running from within the build directory). Default-level configuration
/// (including project-local `fuchsia_env` files) is always excluded to prevent untrusted
/// repository directories from hijacking tool execution.
pub fn get_host_tool(ctx: &EnvironmentContext, name: &str) -> Result<PathBuf, LibError> {
    let sdk = ctx.get_sdk()?;
    let override_key = format!("{SDK_OVERRIDE_KEY_PREFIX}.{name}");
    let check_build_level = allow_build_host_tools(ctx);
    let levels: &[ConfigLevel] = if check_build_level {
        &[ConfigLevel::Runtime, ConfigLevel::User, ConfigLevel::Build, ConfigLevel::Global]
    } else {
        &[ConfigLevel::Runtime, ConfigLevel::User, ConfigLevel::Global]
    };
    for &level in levels {
        match ctx.query(&override_key).level(Some(level)).build().get::<PathBuf>(ctx) {
            Ok(tool_path) => {
                if tool_path.exists() {
                    log::info!("Using configured override for {name}: {tool_path:?}");
                    return Ok(tool_path);
                } else {
                    return Err(LibError::OverrideNotExist(name.to_string(), tool_path));
                }
            }
            Err(ConfigError::KeyNotFound | ConfigError::NoValueSet(_)) => continue,
            Err(e) => return Err(LibError::from(e)),
        }
    }

    if !check_build_level {
        if ctx
            .query(&override_key)
            .level(Some(ConfigLevel::Build))
            .build()
            .get::<PathBuf>(ctx)
            .is_ok()
        {
            eprintln!(
                "WARNING: Ignoring build-level override for '{name}' because build overrides are \
                 disabled. To enable, configure '{SDK_ALLOW_BUILD_HOST_TOOLS}=true'."
            );
        }
    }

    let tool_path = sdk.get_host_tool(name)?;
    if tool_path.exists() {
        log::info!("SDK returned {tool_path:?} for {name}");
        Ok(tool_path)
    } else {
        Err(LibError::SdkToolNotExist(tool_path, name.to_string()))
    }
}

pub fn print_config<W: Write>(ctx: &EnvironmentContext, mut writer: W) -> Result<(), LibError> {
    writeln!(writer, "{}", ctx.config).map_err(LibError::DisplayConfig)
}

fn get_log_dirs(ctx: &Option<EnvironmentContext>) -> Result<Vec<String>, LibError> {
    match ctx {
        None => {
            return Err(LibError::LoadLogDirs("No EnvironmentContext provided".into()));
        }
        Some(con) => match con.get("log.dir") {
            Ok(log_dirs) => Ok(log_dirs),
            Err(e) => return Err(LibError::LoadLogDirs(format!("{:?}", e))),
        },
    }
}

/// Print out useful hints about where important log information might be found after an error.
pub fn print_log_hint<W: std::io::Write>(ctx: &Option<EnvironmentContext>, writer: &mut W) {
    let msg = match get_log_dirs(ctx) {
        Ok(log_dirs) if log_dirs.is_empty() => {
            "More information may be available in ffx host logs, but no configured log directory locations were discovered.".to_string()
        }
        Ok(log_dirs) if log_dirs.len() == 1 => format!(
            "More information may be available in ffx host logs (ffx.log) in directory:\n    {}\nTo inspect, run: tail -f \"{}/ffx.log\"",
            log_dirs[0], log_dirs[0]
        ),
        Ok(log_dirs) => format!(
            "More information may be available in ffx host logs (ffx.log) in directories:\n    {}\nTo inspect, examine the ffx.log file within those paths.",
            log_dirs.join("\n    ")
        ),
        Err(err) => format!(
            "More information may be available in ffx host logs, but ffx failed to retrieve configured log file locations. Error:\n    {}",
            err,
        ),
    };
    if writeln!(writer, "{}", msg).is_err() {
        println!("{}", msg);
    }
}

pub async fn set_metrics_status(value: MetricsStatus) -> Result<(), crate::api::ConfigError> {
    set_new_opt_in_status(value).await?;
    Ok(())
}

pub async fn enable_basic_metrics() -> Result<(), crate::api::ConfigError> {
    set_new_opt_in_status(MetricsStatus::OptedIn).await?;
    Ok(())
}

pub async fn enable_enhanced_metrics() -> Result<(), crate::api::ConfigError> {
    set_new_opt_in_status(MetricsStatus::OptedInEnhanced).await?;
    Ok(())
}

pub async fn disable_metrics() -> Result<(), crate::api::ConfigError> {
    set_new_opt_in_status(MetricsStatus::OptedOut).await?;
    Ok(())
}

pub async fn show_metrics_status<W: Write>(mut writer: W) -> Result<(), LibError> {
    let status_message = show_status_message().await;
    writeln!(&mut writer, "{status_message}").map_err(LibError::DisplayConfig)?;
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// tests
#[cfg(test)]
mod test {
    use super::*;
    // This is to get the FfxConfigBacked derive to compile, as it
    // creates a token stream referencing `ffx_config` on the inside.
    use crate::{self as ffx_config};
    use api::value::TryConvert;
    use serde_json::{Value, json};
    use std::collections::HashSet;
    use std::fs;

    #[test]
    fn test_config_levels_make_sense_from_first() {
        let mut found_set = HashSet::new();
        let mut from_first = None;
        for _ in 0..ConfigLevel::_COUNT + 1 {
            if let Some(next) = ConfigLevel::next(from_first) {
                let entry = found_set.get(&next);
                assert!(entry.is_none(), "Found duplicate config level while iterating: {next:?}");
                found_set.insert(next);
                from_first = Some(next);
            } else {
                break;
            }
        }

        assert_eq!(
            ConfigLevel::_COUNT,
            found_set.len(),
            "A config level was missing from the forward iteration of levels: {found_set:?}"
        );
    }

    #[test]
    fn test_converting_array() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let c = |val: Value| -> ConfigValue { ConfigValue::from(Some(val)) };
        let conv_elem: Vec<String> = <_>::try_convert(c(json!("test")))?;
        assert_eq!(1, conv_elem.len());
        let conv_string: Vec<String> = <_>::try_convert(c(json!(["test", "test2"])))?;
        assert_eq!(2, conv_string.len());
        let conv_bool: Vec<bool> = <_>::try_convert(c(json!([true, "false", false])))?;
        assert_eq!(3, conv_bool.len());
        let conv_bool_2: Vec<bool> = <_>::try_convert(c(json!([36, "false", false])))?;
        assert_eq!(2, conv_bool_2.len());
        let conv_num: Vec<u64> = <_>::try_convert(c(json!([3, "36", 1000])))?;
        assert_eq!(3, conv_num.len());
        let conv_num_2: Vec<u64> = <_>::try_convert(c(json!([3, "false", 1000])))?;
        assert_eq!(2, conv_num_2.len());
        let bad_elem: Result<Vec<u64>, ConfigError> = <_>::try_convert(c(json!("test")));
        assert!(bad_elem.is_err());
        let bad_elem_2: Result<Vec<u64>, ConfigError> = <_>::try_convert(c(json!(["test"])));
        assert!(bad_elem_2.is_err());
        Ok(())
    }

    #[test]
    fn test_validating_types() {
        let c = |val: Value| -> ConfigValue { ConfigValue::from(Some(val)) };
        assert!(<String>::try_convert(c(json!("test"))).is_ok());
        assert!(<String>::try_convert(c(json!(1))).is_err());
        assert!(<String>::try_convert(c(json!(false))).is_err());
        assert!(<String>::try_convert(c(json!(true))).is_err());
        assert!(<String>::try_convert(c(json!({"test": "whatever"}))).is_err());
        assert!(<String>::try_convert(c(json!(["test", "test2"]))).is_err());
        assert!(<bool>::try_convert(c(json!(true))).is_ok());
        assert!(<bool>::try_convert(c(json!(false))).is_ok());
        assert!(<bool>::try_convert(c(json!("true"))).is_ok());
        assert!(<bool>::try_convert(c(json!("false"))).is_ok());
        assert!(<bool>::try_convert(c(json!(1))).is_err());
        assert!(<bool>::try_convert(c(json!("test"))).is_err());
        assert!(<bool>::try_convert(c(json!({"test": "whatever"}))).is_err());
        assert!(<bool>::try_convert(c(json!(["test", "test2"]))).is_err());
        assert!(<u64>::try_convert(c(json!(2))).is_ok());
        assert!(<u64>::try_convert(c(json!(100))).is_ok());
        assert!(<u64>::try_convert(c(json!("100"))).is_ok());
        assert!(<u64>::try_convert(c(json!("0"))).is_ok());
        assert!(<u64>::try_convert(c(json!(true))).is_err());
        assert!(<u64>::try_convert(c(json!("test"))).is_err());
        assert!(<u64>::try_convert(c(json!({"test": "whatever"}))).is_err());
        assert!(<u64>::try_convert(c(json!(["test", "test2"]))).is_err());
        assert!(<PathBuf>::try_convert(c(json!("/"))).is_ok());
        assert!(<PathBuf>::try_convert(c(json!("test"))).is_ok());
        assert!(<PathBuf>::try_convert(c(json!(true))).is_err());
        assert!(<PathBuf>::try_convert(c(json!({"test": "whatever"}))).is_err());
        assert!(<PathBuf>::try_convert(c(json!(["test", "test2"]))).is_err());
    }

    #[test]
    fn test_conversion_errors() {
        // Probably don't want so much in the way of hard-coded string comparison, but this might
        // at least simplify it a bit.
        let nv_str = |ty: &'static str| format!("No value set. Could not convert to {ty}");
        let badv_str = |from: &'static str, to: &'static str| {
            format!("Conversion to {to} not possible for value: {from}")
        };
        let no_val = ConfigValue::from(None);
        let err = <String>::try_convert(no_val.clone()).unwrap_err();
        assert_eq!(err.to_string(), nv_str("String"));

        let err = <u64>::try_convert(no_val.clone()).unwrap_err();
        assert_eq!(err.to_string(), nv_str("u64"));

        let err = <bool>::try_convert(no_val.clone()).unwrap_err();
        assert_eq!(err.to_string(), nv_str("bool"));

        let err = <PathBuf>::try_convert(no_val.clone()).unwrap_err();
        assert_eq!(err.to_string(), nv_str("PathBuf"));

        let wrong_val = ConfigValue::from(Some(json!(123)));
        let err = <String>::try_convert(wrong_val).unwrap_err();
        assert_eq!(err.to_string(), badv_str("123", "String"));

        let wrong_val = ConfigValue::from(Some(json!(true)));
        let err = <u64>::try_convert(wrong_val).unwrap_err();
        assert_eq!(err.to_string(), badv_str("true", "u64"));

        let wrong_val = ConfigValue::from(Some(json!(123)));
        let err = <bool>::try_convert(wrong_val).unwrap_err();
        assert_eq!(err.to_string(), badv_str("123", "bool"));

        let wrong_val = ConfigValue::from(Some(json!(false)));
        let err = <PathBuf>::try_convert(wrong_val).unwrap_err();
        assert_eq!(err.to_string(), badv_str("false", "PathBuf"));

        let wrong_val = ConfigValue::from(Some(json!("frog")));
        let err = <usize>::try_convert(wrong_val).unwrap_err();
        assert_eq!(err.to_string(), badv_str("\"frog\"", "usize"));

        let wrong_val = ConfigValue::from(Some(json!("frog")));
        let err = <i64>::try_convert(wrong_val).unwrap_err();
        assert_eq!(err.to_string(), badv_str("\"frog\"", "i64"));

        let wrong_val = ConfigValue::from(Some(json!("frog")));
        let err = <u16>::try_convert(wrong_val).unwrap_err();
        assert_eq!(err.to_string(), badv_str("\"frog\"", "u16"));

        let wrong_val = ConfigValue::from(Some(json!("frog")));
        let err = <f64>::try_convert(wrong_val).unwrap_err();
        assert_eq!(err.to_string(), badv_str("\"frog\"", "f64"));
    }

    #[test]
    fn test_string_fallback_conversion() {
        // This doesn't attempt to handle things like underflow/overflow, just plain
        // string conversions.
        let val = ConfigValue::from(Some(json!("2.0")));
        let res = <f64>::try_convert(val).unwrap();
        assert_eq!(res, 2.0);

        let val = ConfigValue::from(Some(json!("20")));
        let res = <u64>::try_convert(val).unwrap();
        assert_eq!(res, 20);

        let val = ConfigValue::from(Some(json!("20")));
        let res = <u16>::try_convert(val).unwrap();
        assert_eq!(res, 20);

        let val = ConfigValue::from(Some(json!("20")));
        let res = <i64>::try_convert(val).unwrap();
        assert_eq!(res, 20);

        let val = ConfigValue::from(Some(json!("20")));
        let res = <usize>::try_convert(val).unwrap();
        assert_eq!(res, 20);

        let val = ConfigValue::from(Some(json!("true")));
        let res = <bool>::try_convert(val).unwrap();
        assert_eq!(res, true);
    }

    #[derive(FfxConfigBacked, Default)]
    struct TestConfigBackedStruct {
        #[ffx_config_default(key = "test.test.thing", default = "thing")]
        value: Option<String>,

        #[ffx_config_default(default = "what", key = "oops")]
        reverse_value: Option<String>,

        #[ffx_config_default(key = "other.test.thing")]
        other_value: Option<f64>,
    }

    #[derive(FfxConfigBacked, Default)] // This should just compile despite having no config.
    struct TestEmptyBackedStruct {}

    #[fuchsia::test]
    fn test_config_backed_attribute_default() {
        let env = ffx_config::test_env().build().expect("create test config");
        let empty_config_struct = TestConfigBackedStruct::default();
        assert!(empty_config_struct.value.is_none());
        assert_eq!(empty_config_struct.value(&env.context).unwrap(), "thing");
        assert!(empty_config_struct.reverse_value.is_none());
        assert_eq!(empty_config_struct.reverse_value(&env.context).unwrap(), "what");
    }

    #[fuchsia::test]
    fn test_config_backed_attribute_override() {
        let env = ffx_config::test_env()
            .user_config("test.test.thing", "config_value_thingy")
            .user_config("other.test.thing", 2.0)
            .build()
            .expect("create test config");

        let mut empty_config_struct = TestConfigBackedStruct::default();

        // If this is set, this should pop up before the config values.
        empty_config_struct.value = Some("wat".to_owned());
        assert_eq!(empty_config_struct.value(&env.context).unwrap(), "wat");
        empty_config_struct.value = None;
        assert_eq!(empty_config_struct.value(&env.context).unwrap(), "config_value_thingy");
        assert_eq!(empty_config_struct.other_value(&env.context).unwrap().unwrap(), 2f64);

        // This should just compile and drop without panicking is all.
        let _ignore = TestEmptyBackedStruct {};
    }

    /// Writes the file to $root, with the path $path, from the source tree prefix $prefix
    /// (relative to this source file)
    macro_rules! put_file {
        ($root:expr, $prefix:literal, $name:literal) => {{
            fs::create_dir_all($root.join($name).parent().unwrap()).unwrap();
            fs::File::create($root.join($name))
                .unwrap()
                .write_all(include_bytes!(concat!($prefix, "/", $name)))
                .unwrap();
        }};
    }

    #[fuchsia::test]
    fn test_get_host_tool() {
        let mut builder = ffx_config::test_env();
        let sdk_root = builder.isolate_root().join("sdk");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        // Override the path via config
        let override_path = builder.isolate_root().join("a_override_host_tool");
        fs::write(&override_path, "a_override_tool_contents").expect("override file written");

        let env = builder
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .user_config(
                &format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool"),
                override_path.to_string_lossy(),
            )
            .build()
            .expect("create test config");

        let result = get_host_tool(&env.context, "a_host_tool").expect("a_host_tool");
        assert_eq!(result, override_path);
    }

    #[fuchsia::test]
    fn test_get_host_tool_override_no_exists() {
        let mut builder = ffx_config::test_env();
        let sdk_root = builder.isolate_root().join("sdk");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        // Override the path via config
        let override_path = builder.isolate_root().join("a_override_host_tool");

        // do not create file, this should report an error.

        let env = builder
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .user_config(
                &format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool"),
                override_path.to_string_lossy(),
            )
            .build()
            .expect("create test config");

        let result = get_host_tool(&env.context, "a_host_tool");
        assert_eq!(
            result.err().unwrap().to_string(),
            format!("Override path for a_host_tool set to {override_path:?}, but does not exist")
        );
    }

    #[fuchsia::test]
    fn test_get_host_tool_ignores_default_level_override() {
        let mut builder = ffx_config::test_env();
        let sdk_root = builder.isolate_root().join("sdk");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        let expected_sdk_tool = sdk_root.join("tools/x64/a-host-tool");
        fs::write(&expected_sdk_tool, "real_sdk_tool").expect("sdk file written");

        // Set an override in the Default level of configuration
        let override_path = builder.isolate_root().join("untrusted_override_host_tool");
        fs::write(&override_path, "untrusted_contents").expect("override file written");

        let mut env = builder
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .build()
            .expect("create test config");

        // Insert override directly into default level (simulating default-level injection)
        let override_key = format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool");
        let mut default_map =
            env.context.config.get_level(ConfigLevel::Default).cloned().unwrap_or_default();
        let key_vec: Vec<&str> = override_key.split('.').collect();
        crate::nested::nested_set(
            &mut default_map,
            key_vec[0],
            &key_vec[1..],
            serde_json::Value::String(override_path.to_string_lossy().into_owned()),
        );
        env.context.config.default = default_map;

        // get_host_tool must ignore the default-level override and return the SDK host tool
        let result = get_host_tool(&env.context, "a_host_tool").expect("a_host_tool");
        assert_eq!(result, expected_sdk_tool);
    }

    #[fuchsia::test]
    fn test_get_host_tool_runtime_override() {
        let mut builder = ffx_config::test_env();
        let sdk_root = builder.isolate_root().join("sdk");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        let override_path = builder.isolate_root().join("runtime_override_host_tool");
        fs::write(&override_path, "runtime_tool_contents").expect("override file written");

        let env = builder
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .runtime_config(
                &format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool"),
                override_path.to_string_lossy(),
            )
            .build()
            .expect("create test config");

        let result = get_host_tool(&env.context, "a_host_tool").expect("a_host_tool");
        assert_eq!(result, override_path);
    }

    #[fuchsia::test]
    fn test_get_host_tool_ignores_build_level_override_when_outside_build_dir() {
        let mut builder = ffx_config::test_env();
        let build_dir = builder.isolate_root().join("build");
        fs::create_dir_all(&build_dir).expect("build dir created");
        let sdk_root = builder.isolate_root().join("sdk");

        let external_bin = builder.isolate_root().join("external_bin");
        fs::create_dir_all(&external_bin).expect("external bin dir created");
        let external_ffx = external_bin.join("ffx");
        fs::write(&external_ffx, "").expect("external ffx created");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        let expected_sdk_tool = sdk_root.join("tools/x64/a-host-tool");
        fs::write(&expected_sdk_tool, "real_sdk_tool").expect("sdk file written");

        let build_override_path = builder.isolate_root().join("build_override_host_tool");
        fs::write(&build_override_path, "build_override_tool_contents")
            .expect("override file written");

        let env = builder
            .in_tree(&build_dir)
            .self_path(&external_ffx)
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .build_config(
                &format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool"),
                build_override_path.to_string_lossy(),
            )
            .build()
            .expect("create test config");

        // When running outside build dir and not opted in, build override is ignored
        let result = get_host_tool(&env.context, "a_host_tool").expect("a_host_tool");
        assert_eq!(result, expected_sdk_tool);
    }

    #[fuchsia::test]
    fn test_get_host_tool_accepts_build_level_override_when_inside_build_dir() {
        let mut builder = ffx_config::test_env();
        let build_dir = builder.isolate_root().join("build");
        fs::create_dir_all(&build_dir).expect("build dir created");
        let sdk_root = builder.isolate_root().join("sdk");

        let in_tree_host_tools = build_dir.join("host_x64");
        fs::create_dir_all(&in_tree_host_tools).expect("host_x64 dir created");
        let in_tree_ffx = in_tree_host_tools.join("ffx");
        fs::write(&in_tree_ffx, "").expect("in tree ffx created");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        let build_override_path = builder.isolate_root().join("build_override_host_tool");
        fs::write(&build_override_path, "build_override_tool_contents")
            .expect("override file written");

        let env = builder
            .in_tree(&build_dir)
            .self_path(&in_tree_ffx)
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .build_config(
                &format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool"),
                build_override_path.to_string_lossy(),
            )
            .build()
            .expect("create test config");

        // When running inside build dir, build override is accepted by default
        let result = get_host_tool(&env.context, "a_host_tool").expect("a_host_tool");
        assert_eq!(result, build_override_path);
    }

    #[fuchsia::test]
    fn test_get_host_tool_accepts_build_level_override_when_opted_in() {
        let mut builder = ffx_config::test_env();
        let build_dir = builder.isolate_root().join("build");
        fs::create_dir_all(&build_dir).expect("build dir created");
        let sdk_root = builder.isolate_root().join("sdk");

        let external_bin = builder.isolate_root().join("external_bin");
        fs::create_dir_all(&external_bin).expect("external bin dir created");
        let external_ffx = external_bin.join("ffx");
        fs::write(&external_ffx, "").expect("external ffx created");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        let build_override_path = builder.isolate_root().join("build_override_host_tool");
        fs::write(&build_override_path, "build_override_tool_contents")
            .expect("override file written");

        let env = builder
            .in_tree(&build_dir)
            .self_path(&external_ffx)
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .user_config(SDK_ALLOW_BUILD_HOST_TOOLS, true)
            .build_config(
                &format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool"),
                build_override_path.to_string_lossy(),
            )
            .build()
            .expect("create test config");

        // When opted in explicitly at user level, build-level override is used even if outside build dir
        let result = get_host_tool(&env.context, "a_host_tool").expect("a_host_tool");
        assert_eq!(result, build_override_path);
    }

    #[fuchsia::test]
    fn test_get_host_tool_ignores_build_level_override_when_opted_out_explicitly() {
        let mut builder = ffx_config::test_env();
        let build_dir = builder.isolate_root().join("build");
        fs::create_dir_all(&build_dir).expect("build dir created");
        let sdk_root = builder.isolate_root().join("sdk");

        let in_tree_host_tools = build_dir.join("host_x64");
        fs::create_dir_all(&in_tree_host_tools).expect("host_x64 dir created");
        let in_tree_ffx = in_tree_host_tools.join("ffx");
        fs::write(&in_tree_ffx, "").expect("in tree ffx created");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        let expected_sdk_tool = sdk_root.join("tools/x64/a-host-tool");
        fs::write(&expected_sdk_tool, "real_sdk_tool").expect("sdk file written");

        let build_override_path = builder.isolate_root().join("build_override_host_tool");
        fs::write(&build_override_path, "build_override_tool_contents")
            .expect("override file written");

        let env = builder
            .in_tree(&build_dir)
            .self_path(&in_tree_ffx)
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .user_config(SDK_ALLOW_BUILD_HOST_TOOLS, false)
            .build_config(
                &format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool"),
                build_override_path.to_string_lossy(),
            )
            .build()
            .expect("create test config");

        // Explicit user config false overrides the default true
        let result = get_host_tool(&env.context, "a_host_tool").expect("a_host_tool");
        assert_eq!(result, expected_sdk_tool);
    }

    #[fuchsia::test]
    fn test_get_host_tool_ignores_opt_in_from_build_config() {
        let mut builder = ffx_config::test_env();
        let build_dir = builder.isolate_root().join("build");
        fs::create_dir_all(&build_dir).expect("build dir created");
        let sdk_root = builder.isolate_root().join("sdk");

        let external_bin = builder.isolate_root().join("external_bin");
        fs::create_dir_all(&external_bin).expect("external bin dir created");
        let external_ffx = external_bin.join("ffx");
        fs::write(&external_ffx, "").expect("external ffx created");

        put_file!(sdk_root, "../test_data/sdk", "meta/manifest.json");
        put_file!(sdk_root, "../test_data/sdk", "tools/x64/a_host_tool-meta.json");

        let expected_sdk_tool = sdk_root.join("tools/x64/a-host-tool");
        fs::write(&expected_sdk_tool, "real_sdk_tool").expect("sdk file written");

        let build_override_path = builder.isolate_root().join("build_override_host_tool");
        fs::write(&build_override_path, "build_override_tool_contents")
            .expect("override file written");

        let env = builder
            .in_tree(&build_dir)
            .self_path(&external_ffx)
            .user_config("sdk.root", sdk_root.to_string_lossy())
            .build_config(SDK_ALLOW_BUILD_HOST_TOOLS, true)
            .build_config(
                &format!("{SDK_OVERRIDE_KEY_PREFIX}.a_host_tool"),
                build_override_path.to_string_lossy(),
            )
            .build()
            .expect("create test config");

        // Setting sdk.overrides.allow-build-host-tools in build_config itself must be ignored
        let result = get_host_tool(&env.context, "a_host_tool").expect("a_host_tool");
        assert_eq!(result, expected_sdk_tool);
    }

    #[fuchsia::test]
    fn test_config_source_runtime() {
        let env = ffx_config::test_env()
            .runtime_config("test.source.runtime", "runtime_val")
            .build()
            .expect("create test config");

        let (val, source): (String, ConfigSource) =
            env.context.get_with_source("test.source.runtime").expect("get_with_source");
        assert_eq!(val, "runtime_val");
        assert_eq!(source, ConfigSource::new(ConfigLevel::Runtime));
    }

    #[fuchsia::test]
    fn test_config_source_user() {
        let env = ffx_config::test_env()
            .user_config("test.source.user", "user_val")
            .build()
            .expect("create test config");

        let (val, source): (String, ConfigSource) =
            env.context.get_with_source("test.source.user").expect("get_with_source");
        assert_eq!(val, "user_val");
        assert_eq!(
            source,
            ConfigSource::new(ConfigLevel::User)
                .with_file_path(Some(env.user_file.path().to_path_buf()))
        );
    }

    #[fuchsia::test]
    fn test_config_source_build() {
        let mut builder = ffx_config::test_env();
        let build_dir = builder.isolate_root().join("build");
        fs::create_dir_all(&build_dir).expect("build dir created");

        let env = builder
            .in_tree(&build_dir)
            .build_config("test.source.build", "build_val")
            .build()
            .expect("create test config");

        let (val, source): (String, ConfigSource) =
            env.context.get_with_source("test.source.build").expect("get_with_source");
        assert_eq!(val, "build_val");
        assert_eq!(
            source,
            ConfigSource::new(ConfigLevel::Build)
                .with_file_path(Some(env.build_file.as_ref().unwrap().path().to_path_buf()))
        );
    }

    #[fuchsia::test]
    fn test_config_source_global() {
        let env = ffx_config::test_env()
            .global_config("test.source.global", "global_val")
            .build()
            .expect("create test config");

        let (val, source): (String, ConfigSource) =
            env.context.get_with_source("test.source.global").expect("get_with_source");
        assert_eq!(val, "global_val");
        assert_eq!(
            source,
            ConfigSource::new(ConfigLevel::Global)
                .with_file_path(Some(env.global_file.path().to_path_buf()))
        );
    }

    #[fuchsia::test]
    fn test_config_source_default_level() {
        let env = ffx_config::test_env().build().expect("create test config");

        // "log.level" has default "info"
        let (val, source): (String, ConfigSource) =
            env.context.get_with_source("log.level").expect("get_with_source");
        assert_eq!(val, "info");
        assert_eq!(source, ConfigSource::new(ConfigLevel::Default));
    }

    #[fuchsia::test]
    fn test_config_source_env_var_expansion() {
        let env = ffx_config::test_env()
            .env_var("TEST_FOO_VAR", "expanded_foo_val")
            .user_config("test.source.macro", "$TEST_FOO_VAR")
            .build()
            .expect("create test config");

        let (val, source): (String, ConfigSource) =
            env.context.get_with_source("test.source.macro").expect("get_with_source");
        assert_eq!(val, "expanded_foo_val");
        assert_eq!(
            source,
            ConfigSource::new(ConfigLevel::User)
                .with_file_path(Some(env.user_file.path().to_path_buf()))
                .with_expanded_var(Some("TEST_FOO_VAR".to_string()))
        );
    }

    #[fuchsia::test]
    fn test_config_source_array_env_var_fallback() {
        let env = ffx_config::test_env()
            .env_var("TEST_SECOND_VAR", "chosen_second_val")
            .user_config("test.source.fallback", vec!["$TEST_FIRST_UNSET_VAR", "$TEST_SECOND_VAR"])
            .build()
            .expect("create test config");

        let (val, source): (String, ConfigSource) =
            env.context.get_with_source("test.source.fallback").expect("get_with_source");
        assert_eq!(val, "chosen_second_val");
        assert_eq!(
            source,
            ConfigSource::new(ConfigLevel::User)
                .with_file_path(Some(env.user_file.path().to_path_buf()))
                .with_expanded_var(Some("TEST_SECOND_VAR".to_string()))
        );
    }

    #[fuchsia::test]
    fn test_config_source_get_optional_with_source() {
        let env = ffx_config::test_env()
            .user_config("test.source.opt_set", "found_val")
            .build()
            .expect("create test config");

        let (val, source): (Option<String>, Option<ConfigSource>) = env
            .context
            .get_optional_with_source("test.source.opt_set")
            .expect("get_optional_with_source");
        assert_eq!(val, Some("found_val".to_string()));
        assert_eq!(
            source,
            Some(
                ConfigSource::new(ConfigLevel::User)
                    .with_file_path(Some(env.user_file.path().to_path_buf()))
            )
        );

        let (val, source): (Option<String>, Option<ConfigSource>) = env
            .context
            .get_optional_with_source("test.source.nonexistent")
            .expect("get_optional_with_source");
        assert_eq!(val, None);
        assert_eq!(source, None);

        // Query with explicit level when key is missing at that level
        let (val, source): (Option<String>, Option<ConfigSource>) = env
            .context
            .query("test.source.opt_set")
            .level(Some(ConfigLevel::Build))
            .build()
            .get_optional_with_source(&env.context)
            .expect("get_optional_with_source");
        assert_eq!(val, None);
        assert_eq!(source, None);
    }

    #[fuchsia::test]
    fn test_config_source_select_all_has_no_single_source() {
        let env = ffx_config::test_env()
            .user_config("test.source.all_key", "user_val")
            .global_config("test.source.all_key", "global_val")
            .build()
            .expect("create test config");

        let (val, source): (Vec<String>, Option<ConfigSource>) = env
            .context
            .query("test.source.all_key")
            .select(SelectMode::All)
            .build()
            .get_optional_with_source(&env.context)
            .expect("get_optional_with_source");
        assert_eq!(val, vec!["user_val".to_string(), "global_val".to_string()]);
        assert_eq!(source, None);

        let res: Result<(Vec<String>, ConfigSource), ConfigError> = env
            .context
            .query("test.source.all_key")
            .select(SelectMode::All)
            .build()
            .get_with_source(&env.context);
        assert!(matches!(res, Err(ConfigError::NoSingleSource)));
    }
}
