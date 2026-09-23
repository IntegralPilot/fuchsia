// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembly_release_info::{BoardReleaseInfo, ProductReleaseInfo, ReleaseInfo};
use camino::Utf8Path;
use serde_json::Value;
use std::fs::File;
use std::io::BufReader;

/// A helper function to read a JSON file, parse it into a `serde_json::Value`,
/// and extract a specific field from it.
fn get_value_from_json_file(path: &Utf8Path, field_path: &[&str]) -> Result<Value> {
    let file = File::open(path).with_context(|| format!("Failed to open file: {}", path))?;
    let reader = BufReader::new(file);
    let json: Value = serde_json::from_reader(reader)
        .with_context(|| format!("Failed to parse JSON from file: {}", path))?;

    let mut current_value = &json;
    for field in field_path {
        current_value = current_value
            .get(field)
            .with_context(|| format!("Field '{}' not found in {}", field, path))?;
    }
    Ok(current_value.clone())
}

/// Load a Platform artifact and return the version information.
pub fn load_platform_release_info(path: &Utf8Path) -> Result<ReleaseInfo> {
    serde_json::from_value(get_value_from_json_file(
        &path.join("platform_artifacts.json"),
        &["release_info"],
    )?)
    .context("Failed to parse platform release info")
}

/// Load a Product Input Bundle artifact and return the version information.
pub fn load_pib_release_info(path: &Utf8Path) -> Result<ReleaseInfo> {
    serde_json::from_value(get_value_from_json_file(
        &path.join("product_input_bundle.json"),
        &["release_info"],
    )?)
    .context("Failed to parse PIB release info")
}

/// Load a Product artifact and return the version information.
pub fn load_product_release_info(path: &Utf8Path) -> Result<ProductReleaseInfo> {
    serde_json::from_value(get_value_from_json_file(
        &path.join("product_configuration.json"),
        &["product", "release_info"],
    )?)
    .context("Failed to parse product release info")
}

/// Return the "arch" field within this Board Configuration.
pub fn load_board_arch(path: &Utf8Path) -> Result<String> {
    serde_json::from_value(get_value_from_json_file(
        &path.join("board_configuration.json"),
        &["arch"],
    )?)
    .context("Failed to parse board arch")
}

/// Load a Board Input Bundle Set artifact and return the version information.
pub fn load_bib_set_release_info(path: &Utf8Path) -> Result<ReleaseInfo> {
    serde_json::from_value(get_value_from_json_file(
        &path.join("board_input_bundle_set.json"),
        &["release_info"],
    )?)
    .context("Failed to parse BIB Set release info")
}

/// Load a Board Config artifact and return the version information.
pub fn load_board_release_info(path: &Utf8Path) -> Result<BoardReleaseInfo> {
    serde_json::from_value(get_value_from_json_file(
        &path.join("board_configuration.json"),
        &["release_info"],
    )?)
    .context("Failed to parse board release info")
}

#[cfg(test)]
mod test {
    use super::*;
    use assembly_release_info::{BoardReleaseInfo, ProductReleaseInfo, ReleaseInfo};
    use camino::Utf8PathBuf;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn test_load_release_info() {
        let tmp = tempdir().unwrap();
        let tmp_path = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();

        // Create a dummy platform_artifacts.json
        let platform_path = tmp_path.join("platform_artifacts.json");
        let platform_release_info = ReleaseInfo {
            name: "test_platform".to_string(),
            version: "1.2.3".to_string(),
            repository: "test_repository".to_string(),
        };
        std::fs::write(
            &platform_path,
            json!({ "release_info": platform_release_info }).to_string(),
        )
        .unwrap();
        let loaded_platform = load_platform_release_info(&tmp_path).unwrap();
        assert_eq!(loaded_platform, platform_release_info);

        // Create a dummy product_configuration.json
        let product_path = tmp_path.join("product_configuration.json");
        let product_release_info = ProductReleaseInfo {
            info: ReleaseInfo {
                name: "test_product".to_string(),
                version: "4.5.6".to_string(),
                repository: "test_repository".to_string(),
            },
            pibs: vec![],
        };
        std::fs::write(
            &product_path,
            json!({ "product": { "release_info": product_release_info } }).to_string(),
        )
        .unwrap();
        let loaded_product = load_product_release_info(&tmp_path).unwrap();
        assert_eq!(loaded_product, product_release_info);

        // Create a dummy board_configuration.json
        let board_path = tmp_path.join("board_configuration.json");
        let board_release_info = BoardReleaseInfo {
            info: ReleaseInfo {
                name: "test_board".to_string(),
                version: "7.8.9".to_string(),
                repository: "test_repository".to_string(),
            },
            bib_sets: vec![],
        };
        std::fs::write(&board_path, json!({ "release_info": board_release_info }).to_string())
            .unwrap();
        let loaded_board = load_board_release_info(&tmp_path).unwrap();
        assert_eq!(loaded_board, board_release_info);
    }
}
