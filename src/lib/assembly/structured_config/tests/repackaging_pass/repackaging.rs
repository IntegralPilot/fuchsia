// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_structured_config::{Repackager, ValidationError, validate_component};
use assembly_validate_package::{PackageValidationError, validate_package};
use camino::Utf8Path;
use fuchsia_archive::Utf8Reader;
use fuchsia_pkg::{BlobInfo, PackageManifest};
use maplit::btreemap;
use std::io::Cursor;
use std::path::Path;
use tempfile::TempDir;

const PASS_WITH_CONFIG: &str = "meta/pass_with_config.cm";
const PASS_WITHOUT_CONFIG: &str = "meta/pass_without_config.cm";
const FAIL_MISSING_CONFIG: &str = "meta/fail_missing_config.cm";

fn test_package_manifest() -> (PackageManifest, TempDir) {
    // unpack the archive and create a manifest
    let archive_path = env!("TEST_PACKAGE_FAR");
    let blob_tmp = TempDir::new().unwrap();
    let blob_outdir = Utf8Path::from_path(blob_tmp.path()).unwrap();

    let manifest_tmp = TempDir::new().unwrap();
    let manifest_outdir = Utf8Path::from_path(manifest_tmp.path()).unwrap();

    let manifest = PackageManifest::from_archive(
        Path::new(&archive_path),
        blob_outdir.as_std_path(),
        manifest_outdir.as_std_path(),
    )
    .unwrap();

    (manifest, blob_tmp)
}

fn test_meta_far() -> (Utf8Reader<Cursor<Vec<u8>>>, TempDir) {
    let (package_manifest, unpacked) = test_package_manifest();
    let meta_far_source =
        package_manifest.blobs().iter().find(|b| b.path == "meta/").unwrap().source_path.clone();
    let reader = Utf8Reader::new(Cursor::new(std::fs::read(meta_far_source).unwrap())).unwrap();
    (reader, unpacked)
}

/// Makes sure that we can "turn a package valid" if it's been produced by the build without value
/// files.
#[test]
fn adding_config_makes_invalid_package_valid() {
    let (original_manifest, _unpacked_original) = test_package_manifest();

    // ensure that product validation will fail without us doing anything
    match validate_package(&original_manifest) {
        Err(PackageValidationError::InvalidComponents(..)) => (),
        other => panic!("expected validation to fail with invalid components, got {:#?}", other),
    }

    // provide config values for the previously-invalid component
    let temp = TempDir::new().unwrap();
    let tempdir = Utf8Path::from_path(temp.path()).unwrap();

    let mut repackager = Repackager::new(original_manifest.clone(), tempdir).unwrap();
    repackager
        .set_component_config(FAIL_MISSING_CONFIG, btreemap! { "foo".to_string() => true.into() })
        .unwrap();
    let new_manifest_path = repackager.build().unwrap();

    // ensure that the modified package is valid
    let new_manifest = PackageManifest::try_load_from(new_manifest_path).unwrap();
    validate_package(&new_manifest).expect("package must be valid after adding config");
}

/// Makes sure that the product assembly tooling never silently squashes an existing value file.
#[test]
fn cant_add_config_on_top_of_existing_values() {
    let (original_manifest, _unpacked_original) = test_package_manifest();
    let temp = TempDir::new().unwrap();
    let tempdir = Utf8Path::from_path(temp.path()).unwrap();
    let mut repackager = Repackager::new(original_manifest.clone(), tempdir).unwrap();
    repackager
        .set_component_config(PASS_WITH_CONFIG, btreemap! { "foo".to_string() => true.into() })
        .unwrap_err();
}

/// Checks against unintended side effects from repackaging.
#[test]
fn repackaging_with_no_config_produces_identical_manifest() {
    let (original_manifest, _unpacked_original) = test_package_manifest();

    let temp = TempDir::new().unwrap();
    let tempdir = Utf8Path::from_path(temp.path()).unwrap();
    let repackager = Repackager::new(original_manifest.clone(), tempdir).unwrap();
    let new_manifest_path = repackager.build().unwrap();

    let new_manifest = PackageManifest::try_load_from(new_manifest_path).unwrap();

    assert_eq!(original_manifest.name(), new_manifest.name(), "repackaging must re-use pkg name");
    assert_eq!(
        original_manifest.blobs().len(),
        new_manifest.blobs().len(),
        "repackaging without config must not change # of blobs"
    );

    // repackaging might change order of blobs in the manifests, sort for consistency
    let mut original_blobs: Vec<_> = original_manifest.blobs().iter().collect();
    let mut new_blobs: Vec<_> = new_manifest.blobs().iter().collect();
    original_blobs.sort_by_key(|b| &b.path);
    new_blobs.sort_by_key(|b| &b.path);

    // test blobs for equality
    // (ignoring source paths because we wrote the new blob contents into a temporary directory)
    for (original_blob, new_blob) in original_blobs.iter().zip(new_blobs.iter()) {
        let BlobInfo {
            source_path: _,
            path: original_path,
            merkle: original_merkle,
            size: original_size,
        } = original_blob;
        let BlobInfo { source_path: _, path: new_path, merkle: new_merkle, size: new_size } =
            new_blob;
        assert_eq!(original_path, new_path, "no-op repackaging should not change blob paths");
        assert_eq!(original_merkle, new_merkle, "no-op repackaging should not change blob merkles");
        assert_eq!(original_size, new_size, "no-op repackaging should not change blob sizes");
    }
}

#[test]
fn config_resolves() {
    let (mut meta_far, _unpacked_package) = test_meta_far();
    validate_component(PASS_WITH_CONFIG, &mut meta_far).unwrap();
}

#[test]
fn no_config_passes() {
    let (mut meta_far, _unpacked_package) = test_meta_far();
    validate_component(PASS_WITHOUT_CONFIG, &mut meta_far).unwrap();
}

#[test]
fn config_requires_values() {
    let (mut meta_far, _unpacked_package) = test_meta_far();
    match validate_component(FAIL_MISSING_CONFIG, &mut meta_far).unwrap_err() {
        ValidationError::ConfigValuesMissing { .. } => (),
        other => panic!("expected missing values, got {}", other),
    }
}
