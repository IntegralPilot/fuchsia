// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::api::ConfigError;
use crate::api::query::SelectMode;
use crate::api::value::merge_map;
use crate::environment::Environment;
use crate::nested::{nested_get, nested_remove, nested_set};
use crate::{ConfigLevel, ConfigSource, EnvironmentContext};

use config_macros::include_default;
use fuchsia_lockfile::{LockContext, Lockfile};
use log::error;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::fmt;
use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("writing config file: {0}")]
    WriteConfig(#[source] serde_json::Error),

    #[error("flushing config file: {0}")]
    FlushConfig(#[source] std::io::Error),

    #[error("Lockfile error: {0}")]
    Lockfile(#[source] Box<fuchsia_lockfile::LockfileCreateError>),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Persist error: {0}")]
    Persist(String),

    #[error("Can't set empty key")]
    EmptyKey,

    #[error("Config error: {0}")]
    Config(#[from] crate::ConfigError),

    #[error("Nested error: {0}")]
    Nested(#[from] crate::nested::NestedError),

    #[error("No mutable access to runtime level configuration")]
    NoMutableRuntime,

    #[error("No mutable access to default level configuration")]
    NoMutableDefault,
}

impl From<fuchsia_lockfile::LockfileCreateError> for StorageError {
    fn from(e: fuchsia_lockfile::LockfileCreateError) -> Self {
        Self::Lockfile(Box::new(e))
    }
}

fn format_env_variables_error(preamble: &Option<String>, values: &Vec<ConfigValue>) -> String {
    let error_list_string = values
        .iter()
        .map(|cv| {
            format!(
                "    -\"{}\" points to \"{}\", which contains variable mapping {}",
                cv.path, cv.value, cv.expansion,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let flags_string = values
        .iter()
        .map(|cv| format!("--config {}=\"<value>\"", cv.path))
        .collect::<Vec<_>>()
        .join(" ");
    let mut error_title = concat!(
        "One or more configuration values includes a variable mapping that is ignored in strict mode. ",
        "Please provide explicit values on the command line."
    )
    .to_string();
    if let Some(p) = preamble {
        error_title = format!("{error_title}\n{p}");
    }
    let mut msg = format!("{error_title}\n{error_list_string}\n\n");
    msg.push_str("These values can be overridden with the following flags:\n");
    msg.push_str("    ");
    msg.push_str(flags_string.as_str());
    msg
}

#[derive(Debug)]
pub struct ConfigValue {
    /// Dot-delimited path to the config value.
    pub path: String,
    /// The string value from the config.
    pub value: String,
    /// The variable name or expansion from the config.
    pub expansion: String,
}

#[derive(thiserror::Error, Debug)]
pub enum AssertNoEnvError {
    #[error("{}", format_env_variables_error(.0, .1))]
    EnvVariablesFound(Option<String>, Vec<ConfigValue>),

    #[error("critical unexpected error during no-env assert: {0}")]
    Unexpected(#[from] crate::api::ConfigError),
}

pub trait AssertNoEnv {
    /// Looks through the entirety of the config map to find if there are any definitions that are
    /// intended to be substituted with environment variables.
    fn assert_no_env(
        &self,
        preamble: Option<String>,
        ctx: &EnvironmentContext,
    ) -> Result<(), AssertNoEnvError>;
}

/// The type of a configuration level's mapping.
pub type ConfigMap = Map<String, Value>;

impl AssertNoEnv for ConfigMap {
    fn assert_no_env(
        &self,
        preamble: Option<String>,
        ctx: &EnvironmentContext,
    ) -> Result<(), AssertNoEnvError> {
        struct KeyValue<'a> {
            key: String,
            value: &'a serde_json::Value,
        }

        let mut values =
            self.iter().map(|(k, value)| KeyValue { key: k.into(), value }).collect::<Vec<_>>();
        let mut errors = Vec::<ConfigValue>::new();
        loop {
            let Some(kv) = values.pop() else { break };
            match &kv.value {
                Value::Object(map) => {
                    for (k, v) in map.iter() {
                        let full_path = format!("{}.{}", kv.key, k);
                        values.push(KeyValue { key: full_path, value: v });
                    }
                }
                Value::Null | Value::Bool(_) | Value::Number(_) => {}
                Value::String(s) => {
                    if let Err(crate::mapping::MappingError::StrictVariableIgnored(var)) =
                        crate::mapping::expand_macros_strict(&ctx, Value::String(s.clone()))
                    {
                        errors.push(ConfigValue {
                            path: kv.key,
                            value: s.clone(),
                            expansion: format!("${var}"),
                        });
                    }
                }
                Value::Array(arr) => {
                    for elmnt in arr.iter() {
                        values.push(KeyValue { key: kv.key.clone(), value: elmnt })
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(AssertNoEnvError::EnvVariablesFound(preamble, errors))
        }
    }
}

impl AssertNoEnv for Config {
    fn assert_no_env(
        &self,
        preamble: Option<String>,
        ctx: &EnvironmentContext,
    ) -> Result<(), AssertNoEnvError> {
        self.default.assert_no_env(preamble, ctx)
    }
}

/// An individually loaded configuration file, including the path it came from
/// if it was loaded from disk.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigFile {
    path: Option<PathBuf>,
    contents: ConfigMap,
    dirty: bool,
    flush: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub(crate) default: ConfigMap,
    global: Option<ConfigFile>,
    user: Option<ConfigFile>,
    build: Option<ConfigFile>,
    runtime: ConfigMap,
}

pub(crate) struct PriorityIterator<'a> {
    curr: Option<ConfigLevel>,
    config: &'a Config,
}

impl<'a> Iterator for PriorityIterator<'a> {
    type Item = Option<&'a ConfigMap>;

    fn next(&mut self) -> Option<Self::Item> {
        use ConfigLevel::*;
        self.curr = ConfigLevel::next(self.curr);
        match self.curr {
            Some(Runtime) => Some(Some(&self.config.runtime)),
            Some(Build) => Some(self.config.build.as_ref().map(|file| &file.contents)),
            Some(User) => Some(self.config.user.as_ref().map(|file| &file.contents)),
            Some(Global) => Some(self.config.global.as_ref().map(|file| &file.contents)),
            Some(Default) => Some(Some(&self.config.default)),
            None => None,
        }
    }
}

/// Reads a JSON formatted reader permissively, returning None if for whatever reason
/// the file couldn't be read.
///
/// If the JSON is malformed, it will just get overwritten if set is ever used.
/// (TODO: Validate above assumptions)
fn read_json<T: DeserializeOwned>(file: impl Read) -> Option<T> {
    serde_json::from_reader(file).ok()
}

fn write_json<W: Write>(
    file: Option<W>,
    value: Option<&Value>,
) -> std::result::Result<(), StorageError> {
    match (value, file) {
        (Some(v), Some(mut f)) => {
            serde_json::to_writer_pretty(&mut f, v).map_err(StorageError::WriteConfig)?;
            f.flush().map_err(StorageError::FlushConfig)
        }
        (_, _) => {
            // If either value or file are None, then return Ok(()). File being none will
            // presume the user doesn't want to save at this level.
            Ok(())
        }
    }
}

struct MaybeFlushWriter<T> {
    flush: bool,
    writer: T,
}

impl<T: Write> Write for MaybeFlushWriter<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.flush {
            log::debug!("Flushing writer");
            let ret = self.writer.flush();
            log::debug!("Flushed writer");
            ret
        } else {
            log::debug!("Skipped flushing writer (isolate detected)");
            Ok(())
        }
    }
}

/// Atomically write to the file by creating a temporary file and passing it
/// to the closure, and atomically rename it to the destination file.
fn with_writer<F>(path: Option<&Path>, f: F, flush: bool) -> std::result::Result<(), StorageError>
where
    F: FnOnce(
        Option<BufWriter<&mut MaybeFlushWriter<tempfile::NamedTempFile>>>,
    ) -> std::result::Result<(), StorageError>,
{
    if let Some(path) = path {
        let path = Path::new(path);
        let _lockfile = Lockfile::new_for(path, LockContext::current()).map_err(|e| {
            error!("Failed to create a lockfile for {path}. Check that {lockpath} doesn't exist and can be written to. Ownership information: {owner:#?}", path=path.display(), lockpath=e.lock_path.display(), owner=e.owner);
            StorageError::Lockfile(e)
        })?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let tmp = tempfile::NamedTempFile::new_in(parent)?;
        let mut writer = MaybeFlushWriter { flush, writer: tmp };
        log::debug!("Calling writer callback");
        f(Some(BufWriter::new(&mut writer)))?;
        log::debug!("Calling persist");
        writer.writer.persist(path).map_err(|e| StorageError::Persist(e.to_string()))?;
        log::debug!("Persisted");

        Ok(())
    } else {
        log::debug!("Calling writer callback with no persist");
        let ret = f(None);
        log::debug!("Called writer callback");
        ret
    }
}

impl ConfigFile {
    #[cfg(test)]
    fn from_map(path: Option<PathBuf>, contents: ConfigMap) -> Self {
        Self { path, contents, dirty: false, flush: true }
    }

    fn from_buf(path: Option<PathBuf>, buffer: impl Read, flush: bool) -> Self {
        let contents = read_json(buffer)
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(Map::default);
        Self { path, contents, dirty: false, flush }
    }

    fn from_file(path: &Path) -> std::result::Result<Self, StorageError> {
        let file = OpenOptions::new().read(true).open(path);

        match file {
            Ok(buf) => Ok(Self::from_buf(Some(path.to_owned()), BufReader::new(buf), true)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self {
                path: Some(path.to_owned()),
                contents: ConfigMap::default(),
                dirty: false,
                flush: true,
            }),
            Err(e) => Err(StorageError::Io(e)),
        }
    }

    fn from_nonflushing_file(path: &Path) -> std::result::Result<Self, StorageError> {
        let file = OpenOptions::new().read(true).open(path);

        match file {
            Ok(buf) => Ok(Self::from_buf(Some(path.to_owned()), BufReader::new(buf), false)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self {
                path: Some(path.to_owned()),
                contents: ConfigMap::default(),
                dirty: false,
                flush: false,
            }),
            Err(e) => Err(StorageError::Io(e)),
        }
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn set(&mut self, key: &str, value: Value) -> std::result::Result<bool, StorageError> {
        let key_vec: Vec<&str> = key.split('.').collect();
        let key = *key_vec.get(0).ok_or(StorageError::EmptyKey)?;
        let changed = nested_set(&mut self.contents, key, &key_vec[1..], value);
        self.dirty = self.dirty || changed;
        Ok(changed)
    }

    pub fn remove(&mut self, key: &str) -> Result<(), StorageError> {
        let key_vec: Vec<&str> = key.split('.').collect();
        let key = *key_vec.get(0).ok_or(ConfigError::KeyNotFound)?;
        self.dirty = true;
        nested_remove(&mut self.contents, key, &key_vec[1..])?;
        Ok(())
    }

    fn save(&mut self) -> Result<(), StorageError> {
        log::debug!("Saving path {:?}", self.path);

        // FIXME(81502): There is a race between the ffx CLI and the daemon service
        // in updating the config. We can lose changes if both try to change the
        // config at the same time. We can reduce the rate of races by only writing
        // to the config if the value actually changed.
        let ret = if self.is_dirty() {
            self.dirty = false;
            with_writer(
                self.path.as_deref(),
                |writer| write_json(writer, Some(&Value::Object(self.contents.clone()))),
                self.flush,
            )
        } else {
            Ok(())
        };
        log::debug!("Saved path {:?}", self.path);
        ret
    }
}

#[cfg(test)]
impl Default for ConfigFile {
    fn default() -> Self {
        Self::from_map(None, Map::default())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new(None, None, None, ConfigMap::new(), ConfigMap::new())
    }
}

impl Config {
    pub(crate) fn new(
        global: Option<ConfigFile>,
        build: Option<ConfigFile>,
        user: Option<ConfigFile>,
        runtime: ConfigMap,
        default_override: ConfigMap,
    ) -> Self {
        let mut default = match include_default!() {
            Value::Object(obj) => obj,
            _ => panic!("Statically build default configuration was not an object"),
        };
        merge_map(&mut default, &default_override);

        Self { user, build, global, runtime, default }
    }

    pub fn from_env(env: &Environment) -> Result<Self, StorageError> {
        let user_conf: Option<PathBuf> = env.get_user();
        let build_conf: Option<PathBuf> = env.get_build();
        let global_conf: Option<PathBuf> = env.get_global();
        let is_isolated = env.context().env_kind().is_isolated();
        let from_file =
            if is_isolated { ConfigFile::from_nonflushing_file } else { ConfigFile::from_file };
        let user = user_conf.as_deref().map(from_file).transpose()?;
        let build = build_conf.as_deref().map(from_file).transpose()?;
        let global = global_conf.as_deref().map(from_file).transpose()?;

        Ok(Self::new(
            global,
            build,
            user,
            env.get_runtime_args().clone(),
            env.context().get_default_overrides(),
        ))
    }

    pub(crate) fn from_paths(
        user_conf: Option<PathBuf>,
        build_conf: Option<PathBuf>,
        global_conf: Option<PathBuf>,
        runtime: ConfigMap,
        default_override: ConfigMap,
        is_isolated: bool,
    ) -> Result<Self, StorageError> {
        let from_file =
            if is_isolated { ConfigFile::from_nonflushing_file } else { ConfigFile::from_file };
        let user = user_conf.as_deref().map(from_file).transpose()?;
        let build = build_conf.as_deref().map(from_file).transpose()?;
        let global = global_conf.as_deref().map(from_file).transpose()?;

        Ok(Self::new(global, build, user, runtime, default_override))
    }

    #[cfg(test)]
    fn write<W: Write>(
        &self,
        global: Option<W>,
        build: Option<W>,
        user: Option<W>,
    ) -> Result<(), StorageError> {
        write_json(
            user,
            self.user.as_ref().map(|file| Value::Object(file.contents.clone())).as_ref(),
        )?;
        write_json(
            build,
            self.build.as_ref().map(|file| Value::Object(file.contents.clone())).as_ref(),
        )?;
        write_json(
            global,
            self.global.as_ref().map(|file| Value::Object(file.contents.clone())).as_ref(),
        )?;
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), StorageError> {
        let files = [&mut self.global, &mut self.build, &mut self.user];
        // Try to save all files and only fail out if any of them fail afterwards (with the first error). This hopefully mitigates
        // any weird partial-save issues, though there's no way to eliminate them altogether (short of filesystem
        // transactions)
        files
            .into_iter()
            .filter_map(|file| file.as_mut())
            .map(ConfigFile::save)
            .try_fold((), |_res, i| i)
    }

    pub fn get_level(&self, level: ConfigLevel) -> Option<&ConfigMap> {
        match level {
            ConfigLevel::Runtime => Some(&self.runtime),
            ConfigLevel::User => self.user.as_ref().map(|file| &file.contents),
            ConfigLevel::Build => self.build.as_ref().map(|file| &file.contents),
            ConfigLevel::Global => self.global.as_ref().map(|file| &file.contents),
            ConfigLevel::Default => Some(&self.default),
        }
    }

    pub fn get_file_path(&self, level: ConfigLevel) -> Option<&Path> {
        match level {
            ConfigLevel::User => self.user.as_ref().and_then(|file| file.path.as_deref()),
            ConfigLevel::Build => self.build.as_ref().and_then(|file| file.path.as_deref()),
            ConfigLevel::Global => self.global.as_ref().and_then(|file| file.path.as_deref()),
            _ => None,
        }
    }

    pub fn source_for_level(&self, level: ConfigLevel) -> ConfigSource {
        let file_path = self.get_file_path(level).map(|p| p.to_path_buf());
        ConfigSource::new(level).with_file_path(file_path)
    }

    pub fn get_in_level(&self, key: &str, level: ConfigLevel) -> Option<Value> {
        let key_vec: Vec<&str> = key.split('.').collect();
        nested_get(self.get_level(level), key_vec.get(0)?, &key_vec[1..]).cloned()
    }

    fn merge_object(&self, mut omap: Map<String, Value>, key: &str) -> Value {
        let key_vec: Vec<&str> = key.split('.').collect();

        for c in self.iter() {
            if let Some(Value::Object(map)) = nested_get(c, key_vec[0], &key_vec[1..]) {
                for (k, v) in map {
                    if let serde_json::map::Entry::Vacant(e) = omap.entry(k) {
                        e.insert(v.clone());
                    }
                }
            }
        }

        Value::Object(omap)
    }

    pub fn get_with_level(
        &self,
        key: &str,
        select: SelectMode,
    ) -> Option<(Option<ConfigLevel>, Value)> {
        let key_vec: Vec<&str> = key.split('.').collect();
        match select {
            SelectMode::First => {
                let mut iterator = self.iter();
                while let Some(c) = iterator.next() {
                    let level = iterator.curr.unwrap();
                    if let Some(val) = nested_get(c, *key_vec.get(0)?, &key_vec[1..]) {
                        let res = val.clone();
                        if let Value::Object(omap) = res {
                            return Some((Some(level), self.merge_object(omap, key)));
                        }
                        return Some((Some(level), res));
                    }
                }
                None
            }
            SelectMode::All => {
                let result: Vec<Value> = self
                    .iter()
                    .filter_map(|c| nested_get(c, *key_vec.get(0)?, &key_vec[1..]))
                    .cloned()
                    .collect();
                if result.is_empty() { None } else { Some((None, Value::Array(result))) }
            }
        }
    }

    pub fn get(&self, key: &str, select: SelectMode) -> Option<Value> {
        let key_vec: Vec<&str> = key.split('.').collect();
        match select {
            SelectMode::First => {
                let res = self
                    .iter()
                    .find_map(|c| nested_get(c, *key_vec.get(0)?, &key_vec[1..]))
                    .cloned();
                if let Some(Value::Object(omap)) = res {
                    // When we are querying an object, we want the semantics to
                    // match that of querying fields within an object. I.e. if
                    // we get back an object with the same fields as if queries
                    // those individual fields. This means we need to query the
                    // all the config levels, merging on all the fields that are
                    // not shadowed by a higher-level config. Note: this merging
                    // only makes sense for objects, not for arrays. Objects
                    // are treated specially in config, by virtue of the "key"
                    // syntax: "a.b.c"; there is no equivalent array syntax
                    // ("a.b[0]"), so the semantics can stay simple.
                    Some(self.merge_object(omap, key))
                } else {
                    res
                }
            }
            SelectMode::All => {
                let result: Vec<Value> = self
                    .iter()
                    .filter_map(|c| nested_get(c, *key_vec.get(0)?, &key_vec[1..]))
                    .cloned()
                    .collect();
                if result.len() > 0 { Some(Value::Array(result)) } else { None }
            }
        }
    }

    pub fn set(
        &mut self,
        key: &str,
        level: ConfigLevel,
        value: Value,
    ) -> Result<bool, StorageError> {
        let file = self.get_level_mut(level)?;
        file.set(key, value)
    }

    pub fn remove(&mut self, key: &str, level: ConfigLevel) -> Result<(), StorageError> {
        let file = self.get_level_mut(level)?;
        file.remove(key)
    }

    pub fn add(&mut self, key: &str, level: ConfigLevel, value: Value) -> Result<(), StorageError> {
        if let Some(mut current) = self.get_in_level(key, level) {
            if current.is_object() {
                return Err(crate::api::ConfigError::ValidationError(
                    crate::api::ValidationError::CannotAddToSubtree,
                )
                .into());
            } else {
                match current.as_array_mut() {
                    Some(v) => {
                        v.push(value);
                        self.set(key, level, Value::Array(v.to_vec()))?
                    }
                    None => self.set(key, level, Value::Array(vec![current, value]))?,
                }
            }
        } else {
            self.set(key, level, value)?
        };
        Ok(())
    }

    pub(crate) fn iter(&self) -> PriorityIterator<'_> {
        PriorityIterator { curr: None, config: self }
    }

    fn get_level_mut(
        &mut self,
        level: ConfigLevel,
    ) -> std::result::Result<&mut ConfigFile, StorageError> {
        match level {
            ConfigLevel::Runtime => return Err(StorageError::NoMutableRuntime),
            ConfigLevel::User => self
                .user
                .as_mut()
                .ok_or(StorageError::Config(ConfigError::UnconfiguredLevel { level })),
            ConfigLevel::Build => self
                .build
                .as_mut()
                .ok_or(StorageError::Config(ConfigError::UnconfiguredLevel { level })),
            ConfigLevel::Global => self
                .global
                .as_mut()
                .ok_or(StorageError::Config(ConfigError::UnconfiguredLevel { level })),
            ConfigLevel::Default => return Err(StorageError::NoMutableDefault),
        }
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "FFX configuration can come from several places and has an inherent priority assigned\n\
            to the different ways the configuration is gathered. A configuration key can be set\n\
            in multiple locations but the first value found is returned. The following output\n\
            shows the locations checked in descending priority order.\n"
        )?;
        let mut iterator = self.iter();
        while let Some(next) = iterator.next() {
            if let Some(level) = iterator.curr {
                match level {
                    ConfigLevel::Runtime => {
                        write!(f, "Runtime Configuration")?;
                    }
                    ConfigLevel::User => {
                        write!(f, "User Configuration")?;
                    }
                    ConfigLevel::Build => {
                        write!(f, "Build Configuration")?;
                    }
                    ConfigLevel::Global => {
                        write!(f, "Global Configuration")?;
                    }
                    ConfigLevel::Default => {
                        write!(f, "Default Configuration")?;
                    }
                };
            }
            if let Some(value) = next {
                writeln!(f, "")?;
                writeln!(f, "{}", serde_json::to_string_pretty(&value).unwrap())?;
            } else {
                writeln!(f, ": {}", "none")?;
            }
            writeln!(f, "")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::nested::RecursiveMap;
    use regex::Regex;
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::tempdir;

    const ERROR: &'static [u8] = b"0";

    const USER: &'static [u8] = br#"
        {
            "name": "User"
        }"#;

    const BUILD: &'static [u8] = br#"
        {
            "name": "Build"
        }"#;

    const GLOBAL: &'static [u8] = br#"
        {
            "name": "Global"
        }"#;

    const DEFAULT: &'static [u8] = br#"
        {
            "name": "Default"
        }"#;

    const RUNTIME: &'static [u8] = br#"
        {
            "name": "Runtime"
        }"#;

    const MAPPED: &'static [u8] = br#"
        {
            "name": "TEST_MAP"
        }"#;

    const NESTED: &'static [u8] = br#"
        {
            "name": {
               "nested": "Nested"
            }
        }"#;

    const SHALLOW: &'static [u8] = br#"
        {
            "name": {
               "nested": {
                    "shallow": "SHALLOW"
               }
            }
        }"#;

    const DEEP: &'static [u8] = br#"
        {
            "name": {
               "nested": {
                    "deep": {
                        "name": "TEST_MAP"
                    }
               }
            }
        }"#;

    const LITERAL: &'static [u8] = b"[]";

    #[test]
    fn test_persistent_build() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let persistent_config = Config::new(
            Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            Map::default(),
            Map::default(),
        );

        let value = persistent_config.get("name", SelectMode::First);
        assert!(value.is_some());
        assert_eq!(value.unwrap(), Value::String(String::from("User")));

        let mut user_file_out = String::new();
        let mut build_file_out = String::new();
        let mut global_file_out = String::new();

        unsafe {
            persistent_config.write(
                Some(BufWriter::new(global_file_out.as_mut_vec())),
                Some(BufWriter::new(build_file_out.as_mut_vec())),
                Some(BufWriter::new(user_file_out.as_mut_vec())),
            )?;
        }

        // Remove whitespace
        let mut user_file = String::from_utf8_lossy(USER).to_string();
        let mut build_file = String::from_utf8_lossy(BUILD).to_string();
        let mut global_file = String::from_utf8_lossy(GLOBAL).to_string();
        user_file.retain(|c| !c.is_whitespace());
        build_file.retain(|c| !c.is_whitespace());
        global_file.retain(|c| !c.is_whitespace());
        user_file_out.retain(|c| !c.is_whitespace());
        build_file_out.retain(|c| !c.is_whitespace());
        global_file_out.retain(|c| !c.is_whitespace());

        assert_eq!(user_file, user_file_out);
        assert_eq!(build_file, build_file_out);
        assert_eq!(global_file, global_file_out);

        Ok(())
    }

    #[test]
    fn test_priority_iterator() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: serde_json::from_slice(RUNTIME)?,
        };

        let mut test_iter = test.iter();
        assert_eq!(test_iter.next(), Some(Some(&test.runtime)));
        assert_eq!(test_iter.next(), Some(test.user.as_ref().map(|file| &file.contents)));
        assert_eq!(test_iter.next(), Some(test.build.as_ref().map(|file| &file.contents)));
        assert_eq!(test_iter.next(), Some(test.global.as_ref().map(|file| &file.contents)));
        assert_eq!(test_iter.next(), Some(Some(&test.default)));
        assert_eq!(test_iter.next(), None);
        Ok(())
    }

    #[test]
    fn test_priority_iterator_with_nones() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: None,
            global: None,
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };

        let mut test_iter = test.iter();
        assert_eq!(test_iter.next(), Some(Some(&test.runtime)));
        assert_eq!(test_iter.next(), Some(test.user.as_ref().map(|file| &file.contents)));
        assert_eq!(test_iter.next(), Some(test.build.as_ref().map(|file| &file.contents)));
        assert_eq!(test_iter.next(), Some(test.global.as_ref().map(|file| &file.contents)));
        assert_eq!(test_iter.next(), Some(Some(&test.default)));
        assert_eq!(test_iter.next(), None);
        Ok(())
    }

    #[test]
    fn test_get() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };

        let value = test.get("name", SelectMode::First);
        assert!(value.is_some());
        assert_eq!(value.unwrap(), Value::String(String::from("User")));

        let test_build = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: None,
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };

        let value_build = test_build.get("name", SelectMode::First);
        assert!(value_build.is_some());
        assert_eq!(value_build.unwrap(), Value::String(String::from("User")));

        let test_global = Config {
            user: None,
            build: None,
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };

        let value_global = test_global.get("name", SelectMode::First);
        assert!(value_global.is_some());
        assert_eq!(value_global.unwrap(), Value::String(String::from("Global")));

        let test_default = Config {
            user: None,
            build: None,
            global: None,
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };

        let value_default = test_default.get("name", SelectMode::First);
        assert!(value_default.is_some());
        assert_eq!(value_default.unwrap(), Value::String(String::from("Default")));

        let test_none = Config {
            user: None,
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };

        let value_none = test_none.get("name", SelectMode::First);
        assert!(value_none.is_none());
        Ok(())
    }

    #[test]
    fn test_set_non_map_value() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(ERROR), true)),
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        test.set("name", ConfigLevel::User, Value::String(String::from("whatever")))?;
        let value = test.get("name", SelectMode::First);
        assert_eq!(value, Some(Value::String(String::from("whatever"))));
        Ok(())
    }

    #[test]
    fn test_get_nonexistent_config() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };
        let value = test.get("field that does not exist", SelectMode::First);
        assert!(value.is_none());
        Ok(())
    }

    #[test]
    fn test_set() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };
        test.set("name", ConfigLevel::User, Value::String(String::from("build-test")))?;
        let value = test.get("name", SelectMode::First);
        assert!(value.is_some());
        assert_eq!(value.unwrap(), Value::String(String::from("build-test")));
        Ok(())
    }

    #[test]
    fn test_set_twice_does_not_change_config()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };
        assert!(test.set(
            "name1",
            ConfigLevel::Build,
            Value::String(String::from("build-test1"))
        )?);
        assert_eq!(
            test.get("name1", SelectMode::First).unwrap(),
            Value::String(String::from("build-test1"))
        );

        assert!(!test.set(
            "name1",
            ConfigLevel::Build,
            Value::String(String::from("build-test1"))
        )?);
        assert_eq!(
            test.get("name1", SelectMode::First).unwrap(),
            Value::String(String::from("build-test1"))
        );

        assert!(test.set(
            "name1",
            ConfigLevel::Build,
            Value::String(String::from("build-test2"))
        )?);
        assert_eq!(
            test.get("name1", SelectMode::First).unwrap(),
            Value::String(String::from("build-test2"))
        );

        Ok(())
    }

    #[test]
    fn test_set_build_from_none() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::default()),
            build: Some(ConfigFile::default()),
            global: Some(ConfigFile::default()),
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        let value_none = test.get("name", SelectMode::First);
        assert!(value_none.is_none());
        let error_set =
            test.set("name", ConfigLevel::Default, Value::String(String::from("default")));
        assert!(error_set.is_err(), "Should not be able to set default values at runtime");
        let value_default = test.get("name", SelectMode::First);
        assert!(
            value_default.is_none(),
            "Default value should be unset after failed attempt to set it"
        );
        test.set("name", ConfigLevel::Global, Value::String(String::from("global")))?;
        let value_global = test.get("name", SelectMode::First);
        assert!(value_global.is_some());
        assert_eq!(value_global.unwrap(), Value::String(String::from("global")));

        test.set("name", ConfigLevel::Build, Value::String(String::from("build")))?;
        let value_build = test.get("name", SelectMode::First);
        assert!(value_build.is_some());
        assert_eq!(value_build.unwrap(), Value::String(String::from("build")));

        test.set("name", ConfigLevel::User, Value::String(String::from("user")))?;
        let value_user = test.get("name", SelectMode::First);
        assert!(value_user.is_some());
        assert_eq!(value_user.unwrap(), Value::String(String::from("user")));
        Ok(())
    }

    #[test]
    fn test_remove() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };
        test.remove("name", ConfigLevel::User)?;
        let user_value = test.get("name", SelectMode::First);
        assert!(user_value.is_some());
        assert_eq!(user_value.unwrap(), Value::String(String::from("Build")));
        test.remove("name", ConfigLevel::Build)?;
        let global_value = test.get("name", SelectMode::First);
        assert!(global_value.is_some());
        assert_eq!(global_value.unwrap(), Value::String(String::from("Global")));
        test.remove("name", ConfigLevel::Global)?;
        let default_value = test.get("name", SelectMode::First);
        assert!(default_value.is_some());
        assert_eq!(default_value.unwrap(), Value::String(String::from("Default")));
        let error_removed = test.remove("name", ConfigLevel::Default);
        assert!(error_removed.is_err(), "Should not be able to remove a default value");
        let default_value = test.get("name", SelectMode::First);
        assert_eq!(
            default_value,
            Some(Value::String(String::from("Default"))),
            "value should still be default after trying to remove it (was {:?})",
            default_value
        );
        Ok(())
    }

    #[test]
    fn test_default() {
        let test = Config::new(None, None, None, Map::default(), Map::default());
        let default_value = test.get("log.enabled", SelectMode::First);
        assert_eq!(
            default_value.unwrap(),
            Value::Array(vec![Value::String("$FFX_LOG_ENABLED".to_string()), Value::Bool(true)])
        );
    }

    #[test]
    fn test_display() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: ConfigMap::default(),
        };
        let output = format!("{}", test);
        assert!(output.len() > 0);
        let user_reg = Regex::new("\"name\": \"User\"").expect("test regex");
        assert_eq!(1, user_reg.find_iter(&output).count());
        let build_reg = Regex::new("\"name\": \"Build\"").expect("test regex");
        assert_eq!(1, build_reg.find_iter(&output).count());
        let global_reg = Regex::new("\"name\": \"Global\"").expect("test regex");
        assert_eq!(1, global_reg.find_iter(&output).count());
        let default_reg = Regex::new("\"name\": \"Default\"").expect("test regex");
        assert_eq!(1, default_reg.find_iter(&output).count());
        Ok(())
    }

    fn test_map(value: Value) -> Option<Value> {
        value
            .as_str()
            .map(|s| match s {
                "TEST_MAP" => Value::String("passed".to_string()),
                _ => Value::String("failed".to_string()),
            })
            .or(Some(value))
    }

    #[test]
    fn test_mapping() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(MAPPED), true)),
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        let test_mapping = "TEST_MAP".to_string();
        let test_passed = "passed".to_string();
        let mapped_value = test.get("name", SelectMode::First).recursive_map(&test_map);
        assert_eq!(mapped_value, Some(Value::String(test_passed)));
        let identity_value = test.get("name", SelectMode::First);
        assert_eq!(identity_value, Some(Value::String(test_mapping)));
        Ok(())
    }

    #[test]
    fn test_nested_get() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: None,
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: serde_json::from_slice(NESTED)?,
        };
        let value = test.get("name.nested", SelectMode::First);
        assert_eq!(value, Some(Value::String("Nested".to_string())));
        Ok(())
    }

    #[test]
    fn test_nested_get_should_return_sub_tree()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: None,
            build: None,
            global: None,
            default: serde_json::from_slice(DEFAULT)?,
            runtime: serde_json::from_slice(NESTED)?,
        };
        let value = test.get("name", SelectMode::First);
        assert_eq!(value, Some(serde_json::from_str("{\"nested\": \"Nested\"}")?));
        Ok(())
    }

    #[test]
    fn test_nested_get_should_return_full_match()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: None,
            build: None,
            global: None,
            default: serde_json::from_slice(NESTED)?,
            runtime: serde_json::from_slice(RUNTIME)?,
        };
        let value = test.get("name.nested", SelectMode::First);
        assert_eq!(value, Some(Value::String("Nested".to_string())));
        Ok(())
    }

    #[test]
    fn test_nested_get_should_map_values_in_sub_tree()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: None,
            build: None,
            global: None,
            default: serde_json::from_slice(NESTED)?,
            runtime: serde_json::from_slice(DEEP)?,
        };
        let value = test.get("name.nested", SelectMode::First).recursive_map(&test_map);
        assert_eq!(value, Some(serde_json::from_str("{\"deep\": {\"name\": \"passed\"}}")?));
        Ok(())
    }

    #[test]
    fn test_get_should_merge_values_in_sub_tree()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: None,
            build: None,
            global: None,
            default: serde_json::from_slice(SHALLOW)?,
            runtime: serde_json::from_slice(DEEP)?,
        };
        let value: Option<Value> = test.get("name.nested", SelectMode::First);
        assert_eq!(
            value,
            Some(serde_json::from_str(r#"{"deep": {"name": "TEST_MAP"}, "shallow": "SHALLOW"}"#)?)
        );
        Ok(())
    }

    #[test]
    fn test_get_should_merge_overlapping_values_in_sub_tree()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const SHALLOW2: &'static [u8] = br#"
            {
                "name": {
                   "nested": {
                        "shallow": "SHALLOW2"
                   }
                }
            }"#;

        let test = Config {
            user: None,
            build: None,
            global: None,
            default: serde_json::from_slice(SHALLOW2)?,
            runtime: serde_json::from_slice(SHALLOW)?,
        };
        let value: Option<Value> = test.get("name.nested", SelectMode::First);
        assert_eq!(value, Some(serde_json::from_str(r#"{"shallow": "SHALLOW"}"#)?));
        Ok(())
    }

    #[test]
    fn test_get_should_merge_objects_in_sub_tree()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const OBJ1: &'static [u8] = br#"
            {
                "top": {
                   "list": ["a"]
                }
            }"#;

        const OBJ2: &'static [u8] = br#"
            {
                "top": {
                   "str": "b"
                }
            }"#;

        let test = Config {
            user: None,
            build: None,
            global: None,
            default: serde_json::from_slice(OBJ1)?,
            runtime: serde_json::from_slice(OBJ2)?,
        };
        let value: Option<Value> = test.get("top", SelectMode::First);
        assert_eq!(value, Some(serde_json::from_str(r#"{"list": ["a"], "str": "b"}"#)?));
        Ok(())
    }

    #[test]
    fn test_nested_set_from_none() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::default()),
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        test.set("name.nested", ConfigLevel::User, Value::Bool(false))?;
        let nested_value = test.get("name", SelectMode::First);
        assert_eq!(nested_value, Some(serde_json::from_str("{\"nested\": false}")?));
        Ok(())
    }

    #[test]
    fn test_nested_set_from_already_populated_tree()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(NESTED), true)),
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        test.set("name.updated", ConfigLevel::User, Value::Bool(true))?;
        let expected = json!({
           "nested": "Nested",
           "updated": true
        });
        let nested_value = test.get("name", SelectMode::First);
        assert_eq!(nested_value, Some(expected));
        Ok(())
    }

    #[test]
    fn test_nested_set_override_literals() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(LITERAL), true)),
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        test.set("name.updated", ConfigLevel::User, Value::Bool(true))?;
        let expected = json!({
           "updated": true
        });
        let nested_value = test.get("name", SelectMode::First);
        assert_eq!(nested_value, Some(expected));
        test.set("name.updated", ConfigLevel::User, serde_json::from_slice(NESTED)?)?;
        let nested_value = test.get("name.updated.name.nested", SelectMode::First);
        assert_eq!(nested_value, Some(Value::String(String::from("Nested"))));
        Ok(())
    }

    #[test]
    fn test_nested_remove_from_none() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: None,
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        let result = test.remove("name.nested", ConfigLevel::User);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_nested_remove_throws_error_if_key_not_found()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(NESTED), true)),
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        let result = test.remove("name.unknown", ConfigLevel::User);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_nested_remove_deletes_literals() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(DEEP), true)),
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        test.remove("name.nested.deep.name", ConfigLevel::User)?;
        let value = test.get("name", SelectMode::First);
        assert_eq!(value, None);
        Ok(())
    }

    #[test]
    fn test_nested_remove_deletes_subtrees() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        let mut test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(DEEP), true)),
            build: None,
            global: None,
            default: ConfigMap::default(),
            runtime: ConfigMap::default(),
        };
        test.remove("name.nested", ConfigLevel::User)?;
        let value = test.get("name", SelectMode::First);
        assert_eq!(value, None);
        Ok(())
    }

    #[test]
    fn test_additive_mode() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = Config {
            user: Some(ConfigFile::from_buf(None, BufReader::new(USER), true)),
            build: Some(ConfigFile::from_buf(None, BufReader::new(BUILD), true)),
            global: Some(ConfigFile::from_buf(None, BufReader::new(GLOBAL), true)),
            default: serde_json::from_slice(DEFAULT)?,
            runtime: serde_json::from_slice(RUNTIME)?,
        };
        let value = test.get("name", SelectMode::All);
        match value {
            Some(Value::Array(v)) => {
                assert_eq!(v.len(), 5);
                let mut v = v.into_iter();
                assert_eq!(v.next(), Some(Value::String("Runtime".to_string())));
                assert_eq!(v.next(), Some(Value::String("User".to_string())));
                assert_eq!(v.next(), Some(Value::String("Build".to_string())));
                assert_eq!(v.next(), Some(Value::String("Global".to_string())));
                assert_eq!(v.next(), Some(Value::String("Default".to_string())));
            }
            _ => {
                return Err("additive mode should return a Value::Array full of all values.".into());
            }
        }
        Ok(())
    }

    #[test]
    fn test_config_error_report() {
        // Build the following:
        //
        // {
        //   "foo": "$TEST_ENV_VAR_OF_SOME_KIND",  // expands to "whatever"
        //   "inner_map": {
        //     "bar": true,
        //     "baz": "$TEST_ENV_VAR_OF_SOME_KIND",  // expands to "whatever"
        //     "leaf": {
        //       "last": "$TEST_ENV_VAR_TWOOO",     // expands to "whomever"
        //       "last_other": ["$NONEXISTENT_VAR", "blah"],
        //     }
        //   }
        // }
        static TEST_ENV_VAR1: &'static str = "TEST_ENV_VAR_OF_SOME_KIND";
        static TEST_ENV_VAR1_VALUE: &'static str = "whatever";
        static TEST_ENV_VAR2: &'static str = "TEST_ENV_VAR_TWOOO";
        static TEST_ENV_VAR2_VALUE: &'static str = "whomever";
        let mut config_map = ConfigMap::new();
        config_map.insert("foo".to_owned(), Value::String(format!("${TEST_ENV_VAR1}")));
        let mut inner_map = ConfigMap::new();
        inner_map.insert("bar".to_owned(), Value::Bool(true));
        // Escaped sequence here.
        inner_map.insert("baz".to_owned(), Value::String(format!("${TEST_ENV_VAR1}")));
        let mut map_leaf = ConfigMap::new();
        map_leaf.insert("last".to_owned(), Value::String(format!("${TEST_ENV_VAR2}")));
        map_leaf.insert(
            "last_other".to_owned(),
            Value::Array(vec![
                Value::String("$NONEXISTENT_VAR".to_owned()),
                Value::String("blah".to_owned()),
            ]),
        );
        inner_map.insert("leaf".to_owned(), Value::Object(map_leaf));
        config_map.insert("inner_map".to_owned(), Value::Object(inner_map));

        // Now that we have the map set up, let's do the actual testing.
        let isolate_dir = tempdir().expect("tempdir");
        let mut env_vars = HashMap::new();
        env_vars.insert(TEST_ENV_VAR1.to_string(), TEST_ENV_VAR1_VALUE.to_string());
        env_vars.insert(TEST_ENV_VAR2.to_string(), TEST_ENV_VAR2_VALUE.to_string());
        let context = EnvironmentContext::isolated(
            crate::environment::ExecutableKind::Test,
            isolate_dir.path().to_owned(),
            env_vars,
            Default::default(),
            None,
            None,
            true,
        )
        .expect("env context creation");
        let result = config_map.assert_no_env(None, &context);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let AssertNoEnvError::EnvVariablesFound(None, config_values) = err else {
            panic!("wrong error type: {err:?}");
        };
        assert!(
            config_values.iter().any(|cv| cv.path.as_str() == "inner_map.leaf.last"
                && cv.value == format!("${TEST_ENV_VAR2}")
                && cv.expansion == format!("${TEST_ENV_VAR2}")),
            "config error not found in {config_values:?}"
        );
        assert!(
            config_values.iter().any(|cv| cv.path.as_str() == "inner_map.baz"
                && cv.value == format!("${TEST_ENV_VAR1}")
                && cv.expansion == format!("${TEST_ENV_VAR1}")),
            "config error not found in {config_values:?}"
        );
        assert!(
            config_values.iter().any(|cv| cv.path.as_str() == "foo"
                && cv.value == format!("${TEST_ENV_VAR1}")
                && cv.expansion == format!("${TEST_ENV_VAR1}")),
            "config error not found in {config_values:?}"
        );
        assert!(
            config_values.iter().any(|cv| cv.path.as_str() == "inner_map.leaf.last_other"
                && cv.value == "$NONEXISTENT_VAR"
                && cv.expansion == "$NONEXISTENT_VAR"),
            "config error not found in {config_values:?}"
        );
    }

    #[test]
    fn test_assert_no_env_allowed_macros_and_escaped_dollars() {
        let mut config_map = ConfigMap::new();
        config_map.insert("build".to_owned(), Value::String("$BUILD_DIR/out".to_owned()));
        config_map.insert("shared".to_owned(), Value::String("$SHARED_DATA/data".to_owned()));
        config_map.insert("ws".to_owned(), Value::String("$FIND_WORKSPACE_ROOT/ws".to_owned()));
        config_map.insert("escaped".to_owned(), Value::String("$$HOME/path".to_owned()));
        config_map.insert("literal".to_owned(), Value::String("regular string".to_owned()));

        let isolate_dir = tempdir().expect("tempdir");
        let context = EnvironmentContext::isolated(
            crate::environment::ExecutableKind::Test,
            isolate_dir.path().to_owned(),
            HashMap::new(),
            Default::default(),
            None,
            None,
            true,
        )
        .expect("env context creation");

        assert!(config_map.assert_no_env(None, &context).is_ok());
    }

    #[test]
    fn test_assert_no_env_disallowed_builtin_macros() {
        let mut config_map = ConfigMap::new();
        config_map.insert("home".to_owned(), Value::String("$HOME/.config".to_owned()));
        config_map.insert("runtime".to_owned(), Value::String("$RUNTIME/run".to_owned()));

        let isolate_dir = tempdir().expect("tempdir");
        let context = EnvironmentContext::isolated(
            crate::environment::ExecutableKind::Test,
            isolate_dir.path().to_owned(),
            HashMap::new(),
            Default::default(),
            None,
            None,
            true,
        )
        .expect("env context creation");

        let result = config_map.assert_no_env(None, &context);
        assert!(result.is_err());
        let AssertNoEnvError::EnvVariablesFound(None, config_values) = result.unwrap_err() else {
            panic!("wrong error type");
        };
        assert!(
            config_values.iter().any(|cv| cv.path == "home"
                && cv.value == "$HOME/.config"
                && cv.expansion == "$HOME")
        );
        assert!(config_values.iter().any(|cv| cv.path == "runtime"
            && cv.value == "$RUNTIME/run"
            && cv.expansion == "$RUNTIME"));
    }

    #[test]
    fn test_assert_no_env_disallowed_after_unset_allowed_macro() {
        let mut config_map = ConfigMap::new();
        config_map.insert("entry".to_owned(), Value::String("$BUILD_DIR/$HOME".to_owned()));

        let isolate_dir = tempdir().expect("tempdir");
        let context = EnvironmentContext::isolated(
            crate::environment::ExecutableKind::Test,
            isolate_dir.path().to_owned(),
            HashMap::new(),
            Default::default(),
            None,
            None,
            true,
        )
        .expect("env context creation");

        let result = config_map.assert_no_env(None, &context);
        assert!(result.is_err());
        let AssertNoEnvError::EnvVariablesFound(None, config_values) = result.unwrap_err() else {
            panic!("wrong error type");
        };
        assert!(config_values.iter().any(|cv| cv.path == "entry"
            && cv.value == "$BUILD_DIR/$HOME"
            && cv.expansion == "$HOME"));
    }
}
