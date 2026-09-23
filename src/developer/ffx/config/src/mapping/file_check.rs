// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde_json::Value;
use std::path::PathBuf;

/// Filters for config values that map to files that are reachable. Returns None
/// for strings that don't correspond to files discoverable by [`PathBuf::exists`],
/// but maps to the same value for anything else.
pub(crate) fn file_check(value: Value) -> Option<Value> {
    match &value {
        Value::String(s) if PathBuf::from(s).exists() => Some(value),
        Value::String(ne) => {
            // filter out strings that don't correspond to existing files.
            log::debug!("Filtering out config value for file \"{ne}\" as it does not exist");
            None
        }
        _ => Some(value), // but let any other type through.
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests
#[cfg(test)]
mod test {
    use super::*;

    use serde_json::json;
    use tempfile::NamedTempFile;

    #[test]
    fn test_file_mapper() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let file = NamedTempFile::new()?;
        if let Some(path) = file.path().to_str() {
            let test = Value::String(path.to_string());
            assert_eq!(file_check(test), Some(Value::String(path.to_string())));
            Ok(())
        } else {
            return Err("Unable to get temp file path".into());
        }
    }

    #[test]
    fn test_file_mapper_returns_none() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let test = json!("/fake_path/should_not_exist");
        assert_eq!(file_check(test), None);
        Ok(())
    }
}
