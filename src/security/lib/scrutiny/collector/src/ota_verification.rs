// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fuchsia_merkle::Hash;
use scrutiny_collection::ota_verification::{
    BlobError, BlobErrorType, OtaError, OtaVerificationReport,
};
use scrutiny_utils::artifact::ArtifactReader;
use scrutiny_utils::http_artifact::FetchError;
use scrutiny_utils::key_value::parse_key_value;
use scrutiny_utils::package::{open_update_package, read_content_blob};
use std::path::Path;
use std::str::FromStr as _;
use update_package::parse_packages_json;

enum BlobStatus {
    Valid,
    Invalid { error_type: BlobErrorType, error_details: String },
}

/// Collects and verifies blobs for an OTA update.
pub struct OtaVerificationCollector;

impl OtaVerificationCollector {
    /// Generates an [`OtaVerificationReport`] by verifying all hashes in the OTA package graph,
    /// starting from update package Merkle root (hex string) `update_package_hash`, and fetching
    /// all blobs contained in the update via `artifact_reader`.
    pub fn collect(
        update_package_hash: &str,
        artifact_reader: &mut Box<dyn ArtifactReader>,
    ) -> Result<OtaVerificationReport> {
        let mut report = OtaVerificationReport::default();
        let mut blob_cache = std::collections::HashMap::new();

        // Verify the root hash.
        let update_hash = match Hash::from_str(update_package_hash) {
            Ok(h) => h,
            Err(e) => {
                report.errors.push(OtaError::InvalidUpdatePackageHash(format!("{:?}", e)));
                return Ok(report);
            }
        };

        Self::verify_package(
            update_hash,
            "update",
            None,
            artifact_reader,
            &mut report,
            &mut blob_cache,
        );

        // Verify the package hashes.
        // TODO(https://fxbug.dev/537430151): The open_update_package helper is misleadingly
        // named since it is not strictly limited to system update packages. This should be renamed.
        let mut far_reader = match open_update_package(update_package_hash, artifact_reader) {
            Ok(reader) => reader,
            Err(e) => {
                report.errors.push(OtaError::UpdatePackageMetaFarMissing(format!("{:?}", e)));
                return Ok(report);
            }
        };

        // TODO(https://fxbug.dev/537430151): Move this parsing logic for packages.json and
        // data/static_packages into a shared API in scrutiny_utils.
        let packages_json_contents =
            match read_content_blob(&mut far_reader, artifact_reader, "packages.json") {
                Ok(contents) => contents,
                Err(e) => {
                    report.errors.push(OtaError::PackagesJsonMissing(format!("{:?}", e)));
                    return Ok(report);
                }
            };

        let package_urls = match parse_packages_json(packages_json_contents.as_slice()) {
            Ok(urls) => urls,
            Err(e) => {
                report.errors.push(OtaError::PackagesJsonParseFailed(format!("{:?}", e)));
                return Ok(report);
            }
        };

        for pkg_url in &package_urls {
            Self::verify_package(
                pkg_url.hash(),
                pkg_url.name().as_ref(),
                Some(pkg_url.to_string()),
                artifact_reader,
                &mut report,
                &mut blob_cache,
            );
        }

        // Verify the package hashes of the static packages.
        if let Some(system_image_url) =
            // TODO(https://fxbug.dev/537449444): Replace this hardcoded string with the
            // shared "system_image" constant once it is created globally.
            package_urls.iter().find(|url| url.name().as_ref() == "system_image")
        {
            let system_image_hash = system_image_url.hash().to_string();
            let mut system_image_far =
                match open_update_package(&system_image_hash, artifact_reader) {
                    Ok(far) => far,
                    Err(e) => {
                        report.errors.push(OtaError::SystemImageMetaFarMissing(format!("{:?}", e)));
                        return Ok(report);
                    }
                };

            let static_packages_contents = match read_content_blob(
                &mut system_image_far,
                artifact_reader,
                "data/static_packages",
            ) {
                Ok(contents) => contents,
                Err(e) => {
                    report.errors.push(OtaError::StaticPackagesMissing(format!("{:?}", e)));
                    return Ok(report);
                }
            };

            let static_packages_str = match std::str::from_utf8(&static_packages_contents) {
                Ok(s) => s,
                Err(e) => {
                    report.errors.push(OtaError::StaticPackagesInvalidUtf8(format!("{:?}", e)));
                    return Ok(report);
                }
            };

            let static_packages = match parse_key_value(static_packages_str) {
                Ok(pkgs) => pkgs,
                Err(e) => {
                    report.errors.push(OtaError::StaticPackagesParseFailed(format!("{:?}", e)));
                    return Ok(report);
                }
            };

            for (pkg_name, hash_str) in static_packages {
                let hash = match Hash::from_str(&hash_str) {
                    Ok(h) => h,
                    Err(e) => {
                        report.errors.push(OtaError::InvalidBasePackageHash {
                            hash: hash_str.clone(),
                            pkg_name: pkg_name.clone(),
                            err: format!("{:?}", e),
                        });
                        continue;
                    }
                };

                Self::verify_package(
                    hash,
                    &pkg_name,
                    None,
                    artifact_reader,
                    &mut report,
                    &mut blob_cache,
                );
            }
        } else {
            report.errors.push(OtaError::SystemImageNotFound);
        }

        report.deps.extend(artifact_reader.get_deps());

        Ok(report)
    }

    /// Verifies a package's meta FAR and all of its referenced content blobs listed in `meta/contents`.
    ///
    /// Fetches `package_hash` via `reader`, parses its package manifest, and recursively verifies
    /// all child blobs. Results and errors are recorded in `report`, while `blob_cache` deduplicates
    /// verification across packages.
    fn verify_package(
        package_hash: Hash,
        package_name: &str,
        package_url: Option<String>,
        reader: &mut Box<dyn ArtifactReader>,
        report: &mut OtaVerificationReport,
        blob_cache: &mut std::collections::HashMap<Hash, BlobStatus>,
    ) {
        if !Self::verify_blob(
            package_hash,
            package_name,
            package_hash,
            &package_url,
            reader,
            report,
            blob_cache,
        ) {
            return;
        }

        let mut pkg_far = match open_update_package(&package_hash.to_string(), reader) {
            Ok(far) => far,
            Err(e) => {
                report.errors.push(OtaError::PackageMetaFarMissing {
                    pkg_name: package_name.to_string(),
                    err: format!("{:?}", e),
                });
                return;
            }
        };

        let meta_contents_bytes =
            match pkg_far.read_file(scrutiny_utils::package::META_CONTENTS_PATH) {
                Ok(bytes) => bytes,
                Err(e) => {
                    report.errors.push(OtaError::PackageMetaContentsMissing {
                        pkg_name: package_name.to_string(),
                        err: format!("{:?}", e),
                    });
                    return;
                }
            };

        let meta_contents_str = match std::str::from_utf8(&meta_contents_bytes) {
            Ok(s) => s,
            Err(e) => {
                report.errors.push(OtaError::PackageMetaContentsInvalidUtf8 {
                    pkg_name: package_name.to_string(),
                    err: format!("{:?}", e),
                });
                return;
            }
        };

        let paths_to_merkles = match parse_key_value(meta_contents_str) {
            Ok(map) => map,
            Err(e) => {
                report.errors.push(OtaError::PackageMetaContentsParseFailed {
                    pkg_name: package_name.to_string(),
                    err: format!("{:?}", e),
                });
                return;
            }
        };

        for (_, blob_hash_str) in paths_to_merkles {
            let blob_hash = match Hash::from_str(&blob_hash_str) {
                Ok(h) => h,
                Err(e) => {
                    report.errors.push(OtaError::InvalidBlobHash {
                        hash: blob_hash_str.clone(),
                        pkg_name: package_name.to_string(),
                        err: format!("{:?}", e),
                    });
                    continue;
                }
            };
            Self::verify_blob(
                blob_hash,
                package_name,
                package_hash,
                &package_url,
                reader,
                report,
                blob_cache,
            );
        }
    }

    /// Verifies a single blob against its expected Merkle `hash`.
    ///
    /// Opens the blob via `reader` and computes its Merkle root digest. Returns `true` if valid.
    /// If missing or corrupted, appends a [`BlobError`] associated with `parent_name` to `report`
    /// and returns `false`. Deduplicates checks using `blob_cache`.
    fn verify_blob(
        hash: Hash,
        parent_name: &str,
        parent_hash: Hash,
        parent_url: &Option<String>,
        reader: &mut Box<dyn ArtifactReader>,
        report: &mut OtaVerificationReport,
        blob_cache: &mut std::collections::HashMap<Hash, BlobStatus>,
    ) -> bool {
        if let Some(status) = blob_cache.get(&hash) {
            match status {
                BlobStatus::Valid => return true,
                BlobStatus::Invalid { error_type, error_details } => {
                    report.failed_blobs.push(BlobError {
                        hash,
                        parent_package_name: Some(parent_name.to_string()),
                        parent_package_hash: Some(parent_hash),
                        parent_package_url: parent_url.clone(),
                        error_type: error_type.clone(),
                        error_details: error_details.clone(),
                    });
                    return false;
                }
            }
        }

        let hash_str = hash.to_string();
        let pkg_path = Path::new(&hash_str);

        let verification_result = match reader.open(pkg_path) {
            Ok(mut r) => match fuchsia_merkle::root_from_reader(&mut r) {
                Ok(computed_hash) => {
                    if computed_hash != hash {
                        Err((
                            BlobErrorType::Corrupted,
                            format!("Hash mismatch: expected {}, got {}", hash, computed_hash),
                        ))
                    } else {
                        Ok(())
                    }
                }
                Err(e) => Err((
                    BlobErrorType::Other,
                    format!("Failed to hash package {}: {:?}", parent_name, e),
                )),
            },
            Err(e) => {
                let (err_type, err_details) =
                    if let Some(fetch_err) = e.root_cause().downcast_ref::<FetchError>() {
                        match fetch_err {
                            FetchError::NotFound => (BlobErrorType::Missing, format!("{:#}", e)),
                            _ => (BlobErrorType::FetchError, format!("{:#}", e)),
                        }
                    } else if let Some(io_err) = e.root_cause().downcast_ref::<std::io::Error>() {
                        if io_err.kind() == std::io::ErrorKind::NotFound {
                            (BlobErrorType::Missing, format!("{:#}", e))
                        } else {
                            (BlobErrorType::Other, format!("{:#}", e))
                        }
                    } else {
                        let err_str = e.to_string();
                        if err_str.contains("contains no artifact definition") {
                            (BlobErrorType::Missing, format!("{:#}", e))
                        } else if err_str.contains("Failed to decompress")
                            || err_str.contains("Hash mismatch")
                        {
                            (BlobErrorType::Corrupted, format!("{:#}", e))
                        } else {
                            (BlobErrorType::Other, format!("{:#}", e))
                        }
                    };
                Err((err_type, err_details))
            }
        };

        match verification_result {
            Ok(()) => {
                blob_cache.insert(hash, BlobStatus::Valid);
                true
            }
            Err((err_type, details)) => {
                report.failed_blobs.push(BlobError {
                    hash,
                    parent_package_name: Some(parent_name.to_string()),
                    parent_package_hash: Some(parent_hash),
                    parent_package_url: parent_url.clone(),
                    error_type: err_type.clone(),
                    error_details: details.clone(),
                });

                blob_cache.insert(
                    hash,
                    BlobStatus::Invalid { error_type: err_type, error_details: details },
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_archive::write as far_write;
    use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
    use fuchsia_url::{PackageName, RepositoryUrl};
    use scrutiny_testing::artifact::MockArtifactReader;
    use scrutiny_utils::package::META_CONTENTS_PATH;
    use std::collections::BTreeMap;
    use std::io::{BufWriter, Read};
    use update_package::serialize_packages_json;

    // Helper to create a FAR archive
    fn create_far(meta_contents_bytes: &[u8]) -> Vec<u8> {
        let mut far = BufWriter::new(Vec::new());
        let reader: Box<dyn Read> = Box::new(meta_contents_bytes);
        let map: BTreeMap<&str, (u64, Box<dyn Read>)> = maplit::btreemap! {
            META_CONTENTS_PATH => (meta_contents_bytes.len() as u64, reader),
        };
        far_write(&mut far, map).unwrap();
        far.into_inner().unwrap()
    }

    struct MockSetup {
        update_hash: String,
        reader: Box<dyn ArtifactReader>,
        base_pkg_hash: String,
        other_pkg_hash: String,
        inner_blob_hash: String,
    }

    fn build_mock_setup(
        corrupt_base: bool,
        missing_base: bool,
        missing_other: bool,
        omit_system_image: bool,
        missing_inner: bool,
    ) -> MockSetup {
        let mut mock_reader = MockArtifactReader::new();

        let base_pkg_contents = create_far(b"");
        let base_pkg_hash = fuchsia_merkle::root_from_slice(&base_pkg_contents);
        if !missing_base {
            if corrupt_base {
                mock_reader.append_artifact(&base_pkg_hash.to_string(), b"corrupted".to_vec());
            } else {
                mock_reader.append_artifact(&base_pkg_hash.to_string(), base_pkg_contents.clone());
            }
        }

        let static_pkgs_str = format!("base_pkg/0={}\n", base_pkg_hash);
        let static_pkgs_bytes = static_pkgs_str.as_bytes();
        let static_pkgs_hash = fuchsia_merkle::root_from_slice(static_pkgs_bytes);
        mock_reader.append_artifact(&static_pkgs_hash.to_string(), static_pkgs_bytes.to_vec());

        let meta_contents_str = format!("data/static_packages={}\n", static_pkgs_hash);
        let system_image_far_contents = create_far(meta_contents_str.as_bytes());
        let system_image_hash = fuchsia_merkle::root_from_slice(&system_image_far_contents);
        mock_reader.append_artifact(&system_image_hash.to_string(), system_image_far_contents);

        let inner_blob_bytes = b"inner_blob_data";
        let inner_blob_hash = fuchsia_merkle::root_from_slice(inner_blob_bytes);
        if !missing_inner {
            mock_reader.append_artifact(&inner_blob_hash.to_string(), inner_blob_bytes.to_vec());
        }

        let other_pkg_meta_contents = format!("bin/app={}\n", inner_blob_hash);
        let other_pkg_contents = create_far(other_pkg_meta_contents.as_bytes());
        let other_pkg_hash = fuchsia_merkle::root_from_slice(&other_pkg_contents);
        if !missing_other {
            mock_reader.append_artifact(&other_pkg_hash.to_string(), other_pkg_contents.clone());
        }

        let repo_url = RepositoryUrl::parse_host("fuchsia.com".to_string()).unwrap();
        let mut pkg_urls = vec![PinnedAbsolutePackageUrl::new(
            repo_url.clone(),
            PackageName::from_str("other_pkg").unwrap(),
            None,
            other_pkg_hash,
        )];
        if !omit_system_image {
            pkg_urls.push(PinnedAbsolutePackageUrl::new(
                repo_url,
                PackageName::from_str("system_image").unwrap(),
                None,
                system_image_hash,
            ));
        }

        let packages_json_contents = serialize_packages_json(&pkg_urls).unwrap();
        let packages_json_hash = fuchsia_merkle::root_from_slice(&packages_json_contents);
        mock_reader.append_artifact(&packages_json_hash.to_string(), packages_json_contents);

        let update_meta_contents_str = format!("packages.json={}\n", packages_json_hash);
        let update_far_contents = create_far(update_meta_contents_str.as_bytes());
        let update_hash = fuchsia_merkle::root_from_slice(&update_far_contents);
        mock_reader.append_artifact(&update_hash.to_string(), update_far_contents);

        MockSetup {
            update_hash: update_hash.to_string(),
            reader: Box::new(mock_reader),
            base_pkg_hash: base_pkg_hash.to_string(),
            other_pkg_hash: other_pkg_hash.to_string(),
            inner_blob_hash: inner_blob_hash.to_string(),
        }
    }

    #[test]
    fn test_success() {
        let mut setup = build_mock_setup(false, false, false, false, false);
        let report =
            OtaVerificationCollector::collect(&setup.update_hash, &mut setup.reader).unwrap();
        assert!(report.errors.is_empty(), "Expected no errors, got: {:?}", report.errors);
        assert!(report.failed_blobs.is_empty(), "Expected no failed blobs");
    }

    #[test]
    fn test_missing_base_package() {
        let mut setup = build_mock_setup(false, true, false, false, false);
        let report =
            OtaVerificationCollector::collect(&setup.update_hash, &mut setup.reader).unwrap();
        assert_eq!(report.failed_blobs.len(), 1);
        assert_eq!(report.failed_blobs[0].error_type, BlobErrorType::Missing);
        assert_eq!(report.failed_blobs[0].hash.to_string(), setup.base_pkg_hash);
        assert_eq!(report.failed_blobs[0].parent_package_name, Some("base_pkg/0".to_string()));
    }

    #[test]
    fn test_corrupted_base_package() {
        let mut setup = build_mock_setup(true, false, false, false, false);
        let report =
            OtaVerificationCollector::collect(&setup.update_hash, &mut setup.reader).unwrap();
        assert_eq!(report.failed_blobs.len(), 1);
        assert_eq!(report.failed_blobs[0].error_type, BlobErrorType::Corrupted);
        assert_eq!(report.failed_blobs[0].hash.to_string(), setup.base_pkg_hash);
        assert_eq!(report.failed_blobs[0].parent_package_name, Some("base_pkg/0".to_string()));
    }

    #[test]
    fn test_missing_update_package_blob() {
        let mut setup = build_mock_setup(false, false, true, false, false);
        let report =
            OtaVerificationCollector::collect(&setup.update_hash, &mut setup.reader).unwrap();
        assert_eq!(report.failed_blobs.len(), 1);
        assert_eq!(report.failed_blobs[0].error_type, BlobErrorType::Missing);
        assert_eq!(report.failed_blobs[0].hash.to_string(), setup.other_pkg_hash);
        assert_eq!(report.failed_blobs[0].parent_package_name, Some("other_pkg".to_string()));
    }

    #[test]
    fn test_missing_system_image() {
        let mut setup = build_mock_setup(false, false, false, true, false);
        let report =
            OtaVerificationCollector::collect(&setup.update_hash, &mut setup.reader).unwrap();
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0], OtaError::SystemImageNotFound);
    }

    #[test]
    fn test_missing_inner_blob() {
        let mut setup = build_mock_setup(false, false, false, false, true);
        let report =
            OtaVerificationCollector::collect(&setup.update_hash, &mut setup.reader).unwrap();
        assert_eq!(report.failed_blobs.len(), 1);
        assert_eq!(report.failed_blobs[0].error_type, BlobErrorType::Missing);
        assert_eq!(report.failed_blobs[0].hash.to_string(), setup.inner_blob_hash);
        assert_eq!(report.failed_blobs[0].parent_package_name, Some("other_pkg".to_string()));
    }

    #[test]
    fn test_invalid_update_hash() {
        let mut mock_reader: Box<dyn ArtifactReader> = Box::new(MockArtifactReader::new());
        let report =
            OtaVerificationCollector::collect("this_is_not_a_valid_hash", &mut mock_reader)
                .unwrap();

        assert_eq!(report.errors.len(), 1);
        match &report.errors[0] {
            OtaError::InvalidUpdatePackageHash(_) => {}
            _ => panic!("Expected InvalidUpdatePackageHash, got {:?}", report.errors[0]),
        }
    }

    #[test]
    fn test_missing_update_package_meta_far() {
        let mut mock_reader: Box<dyn ArtifactReader> = Box::new(MockArtifactReader::new());
        let valid_hash = "0".repeat(64);
        let report = OtaVerificationCollector::collect(&valid_hash, &mut mock_reader).unwrap();
        assert_eq!(report.failed_blobs.len(), 1);
        assert_eq!(report.failed_blobs[0].error_type, BlobErrorType::Missing);
        assert_eq!(report.errors.len(), 1);
        match &report.errors[0] {
            OtaError::UpdatePackageMetaFarMissing(_) => {}
            _ => panic!("Expected UpdatePackageMetaFarMissing, got {:?}", report.errors[0]),
        }
    }

    struct MockHttpFetcher {
        responses: std::collections::HashMap<
            String,
            Result<Vec<u8>, scrutiny_utils::http_artifact::FetchError>,
        >,
    }

    impl MockHttpFetcher {
        fn new(
            responses: std::collections::HashMap<
                String,
                Result<Vec<u8>, scrutiny_utils::http_artifact::FetchError>,
            >,
        ) -> Self {
            Self { responses }
        }
    }

    impl scrutiny_utils::http_artifact::HttpFetcher for MockHttpFetcher {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, scrutiny_utils::http_artifact::FetchError> {
            match self.responses.get(url) {
                Some(Ok(bytes)) => Ok(bytes.clone()),
                Some(Err(status)) => Err(status.clone()),
                None => Err(scrutiny_utils::http_artifact::FetchError::NotFound),
            }
        }
    }

    #[test]
    fn test_end_to_end_with_http_reader() {
        use scrutiny_utils::http_artifact::HttpArtifactReader;

        let mut responses = std::collections::HashMap::new();

        let base_pkg_contents = create_far(b"");
        let base_pkg_hash = fuchsia_merkle::root_from_slice(&base_pkg_contents);
        let base_pkg_blob =
            delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, &base_pkg_contents);
        responses.insert(format!("http://localhost/blobs/1/{}", base_pkg_hash), Ok(base_pkg_blob));

        let static_pkgs_str = format!("base_pkg/0={}\n", base_pkg_hash);
        let static_pkgs_bytes = static_pkgs_str.as_bytes();
        let static_pkgs_hash = fuchsia_merkle::root_from_slice(static_pkgs_bytes);
        let static_pkgs_blob =
            delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, &static_pkgs_bytes);
        responses
            .insert(format!("http://localhost/blobs/1/{}", static_pkgs_hash), Ok(static_pkgs_blob));

        let meta_contents_str = format!("data/static_packages={}\n", static_pkgs_hash);
        let system_image_far_contents = create_far(meta_contents_str.as_bytes());
        let system_image_hash = fuchsia_merkle::root_from_slice(&system_image_far_contents);
        let system_image_blob = delivery_blob::generate(
            delivery_blob::DeliveryBlobType::Type1,
            &system_image_far_contents,
        );
        responses.insert(
            format!("http://localhost/blobs/1/{}", system_image_hash),
            Ok(system_image_blob),
        );

        let repo_url = RepositoryUrl::parse_host("fuchsia.com".to_string()).unwrap();
        let pkg_urls = vec![PinnedAbsolutePackageUrl::new(
            repo_url,
            PackageName::from_str("system_image").unwrap(),
            None,
            system_image_hash,
        )];

        let packages_json_contents = serialize_packages_json(&pkg_urls).unwrap();
        let packages_json_hash = fuchsia_merkle::root_from_slice(&packages_json_contents);
        let packages_json_blob = delivery_blob::generate(
            delivery_blob::DeliveryBlobType::Type1,
            &packages_json_contents,
        );
        responses.insert(
            format!("http://localhost/blobs/1/{}", packages_json_hash),
            Ok(packages_json_blob),
        );

        let update_meta_contents_str = format!("packages.json={}\n", packages_json_hash);
        let update_far_contents = create_far(update_meta_contents_str.as_bytes());
        let update_hash = fuchsia_merkle::root_from_slice(&update_far_contents);
        let update_blob =
            delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, &update_far_contents);
        responses.insert(format!("http://localhost/blobs/1/{}", update_hash), Ok(update_blob));

        let fetcher = MockHttpFetcher::new(responses);
        let mut reader: Box<dyn ArtifactReader> = Box::new(
            HttpArtifactReader::new(fetcher, "http://localhost/blobs/".to_string(), None).unwrap(),
        );

        let report =
            OtaVerificationCollector::collect(&update_hash.to_string(), &mut reader).unwrap();
        assert!(report.errors.is_empty(), "Expected no errors, got: {:?}", report.errors);
        assert!(report.failed_blobs.is_empty(), "Expected no failed blobs");
    }
}
