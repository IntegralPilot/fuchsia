// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

const FFX_TOOL_PATH: &str = env!("FFX_TOOL_PATH");
const PRODUCT_BUNDLE_PATH: &str = env!("PRODUCT_BUNDLE_PATH");

#[test]
fn extract_blobfs() {
    let pb_path = PathBuf::from(PRODUCT_BUNDLE_PATH);
    let blobfs_path = pb_path.join("system_a/blob.blk");
    let tmp_dir = TempDir::new().unwrap();
    let tmp_path = tmp_dir.path().to_str().unwrap();
    assert!(
        Command::new(FFX_TOOL_PATH)
            .args(vec![
                "scrutiny",
                "shell",
                &format!(
                    "tool.blobfs.extract --input {} --output {}/blobfs",
                    blobfs_path.to_str().unwrap(),
                    tmp_path
                ),
            ])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn extract_zbi() {
    let pb_path = PathBuf::from(PRODUCT_BUNDLE_PATH);
    let zbi_path = pb_path.join("system_a/fuchsia.zbi");
    let tmp_dir = TempDir::new().unwrap();
    let tmp_path = tmp_dir.path().to_str().unwrap();
    assert!(
        Command::new(FFX_TOOL_PATH)
            .args(vec![
                "scrutiny",
                "shell",
                &format!(
                    "tool.zbi.extract --input {} --output {}/zbi",
                    zbi_path.to_str().unwrap(),
                    tmp_path
                ),
            ])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn extract_recovery_zbi() {
    let pb_path = PathBuf::from(PRODUCT_BUNDLE_PATH);
    let recovery_zbi_path = pb_path.join("system_r/fuchsia.zbi");
    let tmp_dir = TempDir::new().unwrap();
    let tmp_path = tmp_dir.path().to_str().unwrap();
    assert!(
        Command::new(FFX_TOOL_PATH)
            .args(vec![
                "scrutiny",
                "shell",
                &format!(
                    "tool.zbi.extract --input {} --output {}/zbi",
                    recovery_zbi_path.to_str().unwrap(),
                    tmp_path
                ),
            ])
            .status()
            .unwrap()
            .success()
    );
}
