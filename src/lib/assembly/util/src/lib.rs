// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Utility methods and traits used throughout assembly.
mod fast_copy;
mod insert_unique;
mod named_map;
mod paths;

pub use fast_copy::{copy_dir, fast_copy};
pub use insert_unique::{
    BTreeMapDuplicateKeyError, DuplicateKeyError, InsertAllUniqueExt, InsertUniqueExt, MapEntry,
};
pub use named_map::{Key as NamedMapKey, NamedMap};
pub use paths::{PathTypeMarker, TypedPathBuf, shorten_path};

use anyhow::{Context as _, Result, bail};
use camino::Utf8PathBuf;
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

/// Read a config file (or really any JSON/JSON5 file) into a instance of type
/// T, with a useful error context if it fails.
pub fn read_config<T>(path: impl AsRef<Path>) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let mut file = File::open(path.as_ref())
        .context(format!("Unable to open file: {}", path.as_ref().display()))?;
    from_reader(&mut file).context(format!("Unable to read file: {}", path.as_ref().display()))
}

/// Serializes the given object to a JSON file.
pub fn write_json_file<T: ?Sized>(json_path: impl AsRef<Path>, value: &T) -> Result<()>
where
    T: Serialize,
{
    let json_path = json_path.as_ref();
    if let Some(parent) = json_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let file = File::create(json_path)
        .with_context(|| format!("cannot create {}", json_path.display()))?;
    serde_json::to_writer_pretty(&file, &value)
        .with_context(|| format!("cannot serialize {}", json_path.display()))
}

/// Deserialize an instance of type T from an IO stream of JSON or JSON5
pub fn from_reader<R, T>(reader: &mut R) -> Result<T>
where
    R: Read,
    T: serde::de::DeserializeOwned,
{
    let mut data = String::default();
    reader.read_to_string(&mut data).context("Cannot read the config")?;

    // First parse the json5 to a `serde_json::Value`, which handles the syntax
    // differences between json5 and json.
    let value: serde_json::Value =
        serde_json5::from_str(&data).context("Cannot parse the json5 config")?;

    // Dump the Value into a JSON string.
    let json = serde_json::to_string_pretty(&value)?;

    // Re-parse using serde_json, which will throw errors when encountering
    // maps when deserializing unit-type enum variants (serde_json5 doesn't do
    // this).
    // TODO: Remove this series of transformations after the following issue
    // is fixed: https://github.com/google/serde_json5/issues/10
    serde_json::from_str(&json).context("cannot parse the config using serde_json")
}

/// Helper fn to insert into an empty Option, or return an Error.
pub fn set_option_once_or<T, E>(
    opt: &mut Option<T>,
    value: impl Into<Option<T>>,
    e: E,
) -> Result<(), E> {
    set_option_once_or_else(opt, value, || e)
}

/// Helper fn to insert into an empty Option, or return an Error created by a
/// closure.
pub fn set_option_once_or_else<T, E, F: FnOnce() -> E>(
    opt: &mut Option<T>,
    value: impl Into<Option<T>>,
    f: F,
) -> Result<(), E> {
    let value = value.into();
    if value.is_none() {
        Ok(())
    } else {
        if opt.is_some() {
            Err(f())
        } else {
            *opt = value;
            Ok(())
        }
    }
}

// A helper function to get the compiled regex, describing a valid string
// that can be used when communicating with the upstream versioning service.
// The regex is initialized only on the first call.
fn valid_version_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9./_-]{0,63}$").unwrap())
}

/// Return an error if the given string does not satisfy the string constraints.
pub fn validate_release_info_string(candidate: String) -> Result<String> {
    if valid_version_regex().is_match(&candidate) {
        Ok(candidate)
    } else {
        bail!(
            "Invalid version string: \"{candidate}\". The string must be 1-63 characters long, start with an alphanumeric character, and only contain alphanumeric characters, '.', '_', '-', or '/'.",
        )
    }
}

/// Return the "version" string if it is provided.
/// If not, return the contents of the file located at the path "version_file".
/// If neither argument is provided, return the string "unversioned".
pub fn get_release_version(
    version: &Option<String>,
    version_file: &Option<Utf8PathBuf>,
) -> Result<String> {
    get_string_or_file_content(
        version,
        version_file,
        "unversioned",
        "version and version_file cannot both be supplied",
    )
}

/// Return the "repo" string if it is provided.
/// If not, return the contents of the file located at the path "repo_file".
/// If neither argument is provided, return the string "unknown".
pub fn get_release_repository(
    repo: &Option<String>,
    repo_file: &Option<Utf8PathBuf>,
) -> Result<String> {
    get_string_or_file_content(
        repo,
        repo_file,
        "unknown",
        "repo and repo_file cannot both be supplied",
    )
}

fn get_string_or_file_content(
    field: &Option<String>,
    field_file: &Option<Utf8PathBuf>,
    undefined_message: &str,
    both_defined_message: &str,
) -> Result<String> {
    let s = match (field, field_file) {
        (None, None) => undefined_message.to_string(),
        (Some(_), Some(_)) => bail!(both_defined_message.to_string()),
        (Some(field), _) => field.to_string(),
        (None, Some(field_file)) => {
            let s = fs::read_to_string(field_file)?;
            s.trim().to_string()
        }
    };
    if &s == "" {
        return Ok(undefined_message.to_string());
    }
    Ok(s)
}

/// Sanitize a string for use in assembly artifacts. Meaning:
///   > All uppercase letters are converted to lowercase.
///   > All instances of characters that are not lowercase alphanumeric,
///     '.', '_', or '-' are converted to '_'.
pub fn sanitize_for_mos_apis(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-' {
                c
            } else if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use camino::Utf8Path;
    use serde::Deserialize;
    use serde_json::json;
    use std::io::{Cursor, Write};
    use tempfile::{NamedTempFile, TempDir};

    #[derive(Debug, Deserialize, PartialEq)]
    struct MyStruct {
        key1: String,
    }

    #[test]
    fn test_set_option_once() {
        let mut opt = None;

        // should be able to set None on None.
        assert!(
            set_option_once_or(&mut opt, None, anyhow!("an error")).is_ok(),
            "Setting None on None failed"
        );

        // should be able to set Value on None.
        assert!(
            set_option_once_or(&mut opt, Some("some value"), anyhow!("an error")).is_ok(),
            "initial set value failed"
        );
        assert_eq!(opt, Some("some value"));

        // setting None on Some should be a no-op.
        assert!(
            set_option_once_or(&mut opt, None, anyhow!("an error")).is_ok(),
            "Setting None on Some failed"
        );
        assert_eq!(opt, Some("some value"), "Setting None on Some was not a no-op");

        // setting Some on Some should fail.
        assert!(
            set_option_once_or(&mut opt, "other value", anyhow!("an error")).is_err(),
            "Setting Some on Some did not fail"
        );
        assert_eq!(
            opt,
            Some("some value"),
            "Setting Some(other) on Some(value) changed the value with an error"
        );
    }

    #[test]
    fn reader_valid_json5() {
        let json5: String = r#"{key1: "value1",}"#.to_string();
        let mut cursor = Cursor::new(json5);
        let value: MyStruct = from_reader(&mut cursor).unwrap();
        assert_eq!(value.key1, "value1");
    }

    #[test]
    fn reader_invalid_json5() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct MyStruct {}
        let json5: String = r#"{key1: "value1",}"#.to_string();
        let mut cursor = Cursor::new(json5);
        let value: Result<MyStruct> = from_reader(&mut cursor);
        assert!(value.is_err());
    }

    #[test]
    fn test_read_config() {
        let json = json!({
            "key1": "value1",
        });
        let file = tempfile::NamedTempFile::new().unwrap();
        serde_json::ser::to_writer(&file, &json).unwrap();

        let value: MyStruct = read_config(file.path()).unwrap();
        let expected: MyStruct = serde_json::from_value(json).unwrap();
        assert_eq!(expected, value);
    }

    #[test]
    fn test_write_json_file() {
        let expected = json!({
            "key1": "value1",
        });
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("config.json");
        write_json_file(&path, &expected).unwrap();

        let actual: serde_json::Value = read_config(path).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn test_default() {
        let version: Option<String> = None;
        let version_file: Option<Utf8PathBuf> = None;
        let version = get_string_or_file_content(
            &version,
            &version_file,
            "unversioned",
            "version and version_file cannot both be supplied",
        )
        .unwrap();
        assert_eq!("unversioned".to_string(), version);
    }

    #[test]
    fn test_version_string() {
        let version: Option<String> = Some("version_string".to_string());
        let version_file: Option<Utf8PathBuf> = None;
        let version = get_string_or_file_content(
            &version,
            &version_file,
            "unversioned",
            "version and version_file cannot both be supplied",
        )
        .unwrap();
        assert_eq!("version_string".to_string(), version);
    }

    #[test]
    fn test_empty_version_string() {
        let version: Option<String> = Some("".to_string());
        let version_file: Option<Utf8PathBuf> = None;
        let version = get_string_or_file_content(
            &version,
            &version_file,
            "unversioned",
            "version and version_file cannot both be supplied",
        )
        .unwrap();
        assert_eq!("unversioned".to_string(), version);
    }

    #[test]
    fn test_version_file() {
        let mut file = NamedTempFile::new().unwrap();
        write!(&mut file, "version_file").unwrap();

        let version: Option<String> = None;
        let version_file: Option<Utf8PathBuf> =
            Some(Utf8Path::from_path(file.path()).unwrap().into());
        let version = get_string_or_file_content(
            &version,
            &version_file,
            "unversioned",
            "version and version_file cannot both be supplied",
        )
        .unwrap();
        assert_eq!("version_file".to_string(), version);
    }

    #[test]
    fn test_empty_version_file() {
        let mut file = NamedTempFile::new().unwrap();
        write!(&mut file, "").unwrap();

        let version: Option<String> = None;
        let version_file: Option<Utf8PathBuf> =
            Some(Utf8Path::from_path(file.path()).unwrap().into());
        let version = get_string_or_file_content(
            &version,
            &version_file,
            "unversioned",
            "version and version_file cannot both be supplied",
        )
        .unwrap();
        assert_eq!("unversioned".to_string(), version);
    }

    #[test]
    fn test_error_for_both_versions() {
        let mut file = NamedTempFile::new().unwrap();
        write!(&mut file, "version_file").unwrap();

        let version: Option<String> = Some("version_string".to_string());
        let version_file: Option<Utf8PathBuf> =
            Some(Utf8Path::from_path(file.path()).unwrap().into());
        assert!(
            get_string_or_file_content(
                &version,
                &version_file,
                "unversioned",
                "version and version_file cannot both be supplied",
            )
            .is_err()
        );
    }

    #[test]
    fn test_version_file_missing() {
        let version: Option<String> = None;
        let version_file: Option<Utf8PathBuf> = Some(Utf8PathBuf::new());
        assert!(
            get_string_or_file_content(
                &version,
                &version_file,
                "unversioned",
                "version and version_file cannot both be supplied",
            )
            .is_err()
        );
    }

    #[test]
    fn test_validate_string() {
        let s = "0123579abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-_".to_string();
        assert!(validate_release_info_string(s).is_ok());
    }

    #[test]
    fn test_validate_string_forward_slash() {
        let s = "a/b".to_string();
        assert!(validate_release_info_string(s).is_ok());
    }

    #[test]
    fn test_validate_string_too_long() {
        let s = "01234567890123456789012345678901234567890123456789012345678901234".to_string();
        assert_eq!(s.len(), 65);
        assert!(validate_release_info_string(s).is_err());
    }

    #[test]
    fn test_validate_string_invalid_first_char() {
        let s = ".a".to_string();
        assert!(validate_release_info_string(s).is_err());
    }

    #[test]
    fn test_validate_string_invalid_char() {
        let s = "a?b".to_string();
        assert!(validate_release_info_string(s).is_err());
    }

    #[test]
    fn test_sanitize_for_mos_apis() {
        assert_eq!(sanitize_for_mos_apis("MAIN/1234"), "main_1234");
        assert_eq!(sanitize_for_mos_apis("Product-Name"), "product-name");
        assert_eq!(sanitize_for_mos_apis("1.2.3"), "1.2.3");
        assert_eq!(sanitize_for_mos_apis("repo@path"), "repo_path");
    }
}
