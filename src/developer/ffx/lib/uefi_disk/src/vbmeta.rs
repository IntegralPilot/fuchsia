// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for signing VBMeta descriptors for custom ZBIs.

pub use assembled_system::vbmeta::FUCHSIA_HASH_DESCRIPTOR_NAME;
use camino::Utf8Path;
use fho::{Result, user_error};
use std::path::Path;
use vbmeta::VBMeta;

/// Creates a signed VBMeta image file at `dest` for the given key, key metadata, and ZBI file.
pub fn generate_vbmeta(
    key_path: &Path,
    metadata_path: &Path,
    zbi_path: &Path,
    dest: &Path,
) -> Result<()> {
    let dest_utf8 =
        Utf8Path::from_path(dest).ok_or_else(|| user_error!("Non-UTF8 destination path"))?;
    if dest_utf8.extension() != Some("vbmeta") {
        return Err(user_error!("Destination path must have .vbmeta extension"));
    }
    let outdir = dest_utf8.parent().ok_or_else(|| user_error!("Invalid destination path"))?;
    let file_stem =
        dest_utf8.file_stem().ok_or_else(|| user_error!("Invalid destination file stem"))?;

    let key_path_utf8 =
        Utf8Path::from_path(key_path).ok_or_else(|| user_error!("Non-UTF8 key path"))?;
    let metadata_path_utf8 =
        Utf8Path::from_path(metadata_path).ok_or_else(|| user_error!("Non-UTF8 metadata path"))?;
    let zbi_path_utf8 =
        Utf8Path::from_path(zbi_path).ok_or_else(|| user_error!("Non-UTF8 ZBI path"))?;

    let output_path = VBMeta::builder(file_stem, key_path_utf8)
        .key_metadata(metadata_path_utf8)
        .hash_descriptor(FUCHSIA_HASH_DESCRIPTOR_NAME, zbi_path_utf8)
        .construct(outdir)
        .map_err(|e| user_error!("Failed to generate VBMeta image: {e}"))?;
    assert_eq!(output_path, dest_utf8);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VBMETA_TEST_KEY: &str = include_str!(
        "../../../../../../third_party/android/platform/external/avb/test/data/testkey_atx_psk.pem"
    );
    const VBMETA_TEST_KEY_METADATA: &[u8] = include_bytes!(
        "../../../../../../third_party/android/platform/external/avb/test/data/atx_metadata.bin"
    );

    #[fuchsia::test]
    fn test_generate_vbmeta() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("atx_psk.pem");
        let metadata_path = dir.path().join("avb_atx_metadata.bin");
        let zbi_path = dir.path().join("test.zbi");
        let dest = dir.path().join("zircon.vbmeta");

        std::fs::write(&key_path, VBMETA_TEST_KEY).unwrap();
        std::fs::write(&metadata_path, VBMETA_TEST_KEY_METADATA).unwrap();
        std::fs::write(&zbi_path, b"fake zbi contents").unwrap();

        generate_vbmeta(&key_path, &metadata_path, &zbi_path, &dest).unwrap();
        assert!(dest.exists());
    }

    #[fuchsia::test]
    fn test_generate_vbmeta_invalid_dest() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("atx_psk.pem");
        let metadata_path = dir.path().join("avb_atx_metadata.bin");
        let zbi_path = dir.path().join("test.zbi");

        assert!(generate_vbmeta(&key_path, &metadata_path, &zbi_path, Path::new("/")).is_err());
        assert!(
            generate_vbmeta(&key_path, &metadata_path, &zbi_path, &dir.path().join("zircon.zbi"))
                .is_err()
        );
    }
}
