// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use camino::{Utf8Path, Utf8PathBuf};
use include_bytes_from_working_dir::include_bytes_from_working_dir_env;
use std::fs;
use tempfile::tempdir;

use vbmeta::{ChainPartition, Salt, VBMeta};

const KEY_PATH: &str = env!("AVB_KEY");
const METADATA_PATH: &str = env!("AVB_METADATA");
const PUBLIC_KEY_PATH: &str = env!("PUBLIC_KEY");
const IMAGE_PATH: &str = env!("IMAGE");
const SALT: &str = env!("SALT");
const EXPECTED_VBMETA: &[u8] = include_bytes_from_working_dir_env!("EXPECTED_VBMETA");
const EXPECTED_VBMETA_CHAIN: &[u8] = include_bytes_from_working_dir_env!("EXPECTED_VBMETA_CHAIN");

#[test]
fn avbtool_comparison() {
    let tmp = tempdir().unwrap();
    let outdir = Utf8Path::from_path(tmp.path()).unwrap();

    let salt = Salt::decode_hex(SALT).unwrap();
    let generated_path = VBMeta::builder("vbmeta", KEY_PATH)
        .key_metadata(METADATA_PATH)
        .salt(salt)
        .hash_descriptor("zircon", IMAGE_PATH)
        .construct(outdir)
        .unwrap();
    let generated_bytes = fs::read(&generated_path).unwrap();

    assert_eq!(generated_bytes, EXPECTED_VBMETA);
}

#[test]
fn avbtool_chain_partition_comparison() {
    let tmp = tempdir().unwrap();
    let outdir = Utf8Path::from_path(tmp.path()).unwrap();

    let generated_path = VBMeta::builder("vbmeta_chain", KEY_PATH)
        .key_metadata(METADATA_PATH)
        .chain_partition(ChainPartition {
            rollback_index_location: 1,
            partition_name: "system".to_string(),
            public_key_path: Utf8PathBuf::from(PUBLIC_KEY_PATH),
        })
        .construct(outdir)
        .unwrap();
    let generated_bytes = fs::read(&generated_path).unwrap();

    assert_eq!(generated_bytes, EXPECTED_VBMETA_CHAIN);
}
