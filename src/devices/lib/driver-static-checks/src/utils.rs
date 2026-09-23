// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use cm_rust::{ComponentDecl, FidlIntoNative, ProgramDecl};
use fidl::unpersist;
use fuchsia_archive::Utf8Reader;
use std::io::Cursor;
use {fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_data as fdata};

pub type FarReader = Utf8Reader<Cursor<Vec<u8>>>;

pub fn find_driver_components(far_reader: &mut FarReader) -> Result<Vec<(ComponentDecl, String)>> {
    let mut driver_components = Vec::new();
    let cm_paths: Vec<String> =
        far_reader.list().map(|e| e.path().to_string()).filter(|p| p.ends_with(".cm")).collect();
    for cm_path in cm_paths {
        let cm_contents = far_reader.read_file(&cm_path)?;
        let component: ComponentDecl = unpersist::<fdecl::Component>(&cm_contents)
            .context("Could not decode component manifest")?
            .fidl_into_native();

        if is_driver_component(&component) {
            driver_components.push((component, cm_path));
        }
    }
    Ok(driver_components)
}

pub fn is_driver_component(component: &ComponentDecl) -> bool {
    if let Some(ProgramDecl { runner: Some(runner), .. }) = &component.program {
        *runner == "driver"
    } else {
        false
    }
}

pub fn pluralize(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

// Color codes taken from third_party/dart-pkg/pub/io/lib/src/ansi_code.dart (same as used by ffx
// test).

/// Wraps a string in ANSI escape codes to color it green.
pub fn green(s: &str) -> String {
    format!("\x1b[32m{}\x1b[0m", s)
}

/// Wraps a string in ANSI escape codes to color it red.
pub fn red(s: &str) -> String {
    format!("\x1b[31m{}\x1b[0m", s)
}

/// Wraps a string in ANSI escape codes to color it cyan.
pub fn cyan(s: &str) -> String {
    format!("\x1b[36m{}\x1b[0m", s)
}

pub fn get_dictionary_value<'a>(
    dict: &'a fdata::Dictionary,
    key: &str,
) -> Option<&'a fdata::DictionaryValue> {
    match &dict.entries {
        Some(entries) => {
            for entry in entries {
                if entry.key == key {
                    return entry.value.as_ref().map(|val| &**val);
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pluralize() {
        assert_eq!(pluralize(0, "check"), "0 checks".to_string());
        assert_eq!(pluralize(1, "check"), "1 check".to_string());
        assert_eq!(pluralize(10, "check"), "10 checks".to_string());
    }

    #[test]
    fn test_get_dictionary_value() {
        let dict = fdata::Dictionary {
            entries: Some(vec![
                fdata::DictionaryEntry {
                    key: "runner".to_string(),
                    value: Some(Box::new(fdata::DictionaryValue::Str("driver".to_string()))),
                },
                fdata::DictionaryEntry {
                    key: "binary".to_string(),
                    value: Some(Box::new(fdata::DictionaryValue::Str(
                        "driver/compat.so".to_string(),
                    ))),
                },
            ]),
            ..Default::default()
        };
        assert_eq!(
            get_dictionary_value(&dict, "runner"),
            Some(&fdata::DictionaryValue::Str("driver".to_string()))
        );
        assert_eq!(get_dictionary_value(&dict, "device_categories"), None,);
    }
}
