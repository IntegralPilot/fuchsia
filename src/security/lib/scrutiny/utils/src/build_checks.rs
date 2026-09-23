// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::artifact::ArtifactReader;
use crate::io::ReadSeek;
use crate::key_value::parse_key_value;
use crate::package::{ReadContentBlobError, extract_system_image_hash_string, read_content_blob};
use anyhow::{Error, Result};
use difference::{Changeset, Difference};
use fuchsia_archive::Utf8Reader as FarReader;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::fs::read_to_string;
use std::path::Path;
use std::str::{FromStr, from_utf8};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum ValidationError {
    #[error("Invalid validation policy configuration: {error}")]
    InvalidPolicyConfiguration { error: String },
    #[error(
        "Validation failure: additional_boot_args MUST contain {expected_key}={expected_value}, but is missing key {expected_key}"
    )]
    AdditionalBootArgsMustContainsKeyMissing { expected_key: String, expected_value: String },
    #[error(
        "Validation failure: additional_boot_args MUST contain {expected_key}={expected_value}, but the value does not match. Expected: {expected_value}, Actual: {found_value}"
    )]
    AdditionalBootArgsMustContainsValueIncorrect {
        expected_key: String,
        expected_value: String,
        found_value: String,
    },
    #[error(
        "Validation failure: additional_boot_args MUST NOT contain {expected_key}={expected_value}, but does."
    )]
    AdditionalBootArgsMustNotContainsHasKeyValue { expected_key: String, expected_value: String },
    #[error("Validation error: Static package {package_name} not found.")]
    MissingStaticPackage { package_name: String },
    #[error("Validation error: Failed to read package {package_name}: {error}")]
    FailedToReadPackage { package_name: String, error: String },
    #[error(
        "Validation error: A package check was not able to run. Package: {package_name}, error: {error}"
    )]
    FailedToPerformPackageCheck { package_name: String, error: String },
    #[error(
        "Validation error: A file check was not able to run. Package: {package_name}, file: {file_path}, error: {error}"
    )]
    FailedToPerformFileCheck { package_name: String, file_path: String, error: String },
    #[error(
        "Validation error: A file check was not able to run because the file was missing. Possible paths: {file_paths} "
    )]
    FailedToFindFile { file_paths: String },
    #[error(
        "Validation error: A file check was not able to run because multiple possible files are present. Possible paths: {possible_paths:?}, files found: {files_found:?} "
    )]
    UnexpectedNumberOfFilesPresent { possible_paths: Vec<String>, files_found: Vec<String> },
    #[error(
        "Validation failure: A file that MUST be absent was found to be present. Package: {package_name}, file: {file_path}"
    )]
    UnexpectedFilePresence { package_name: String, file_path: String },
    #[error(
        "Validation failure: A file that MUST be absent or empty was found to be present with contents. Package: {package_name}, file: {file_path}"
    )]
    UnexpectedFilePresenceOrHasContents { package_name: String, file_path: String },
    #[error(
        "Validation error: Content bytes could not be converted to a string. Content source: {content_source}, error: {error}"
    )]
    FailedToParseContentsToString { content_source: String, error: String },
    #[error(
        "Validation error: Content could not be parsed as a key-value map. Content source: {content_source}, error: {error}"
    )]
    FailedToParseContentsAsKeyValueMap { content_source: String, error: String },
    #[error(
        "Validation error: Content could not be parsed as valid JSON. Content source: {content_source}, error: {error}"
    )]
    FailedToParseContentsAsJson { content_source: String, error: String },
    #[error(
        "Validation error: Cannot handle JSON key-value pair where value is an array or object. Found value: {found}, Content source: {content_source}"
    )]
    UnableToHandleJsonContent { found: String, content_source: String },
    #[error(
        "Validation error: The value type found in a JSON key-value check does not match the policy type. Found value: {found}, Found type: {found_type}, Policy value: {policy}, Content source: {content_source}"
    )]
    ContentAndPolicyJsonTypeMismatch {
        found: String,
        found_type: String,
        policy: String,
        content_source: String,
    },
    #[error(
        "Validation failure: Content MUST contain JSON key-value \"{expected_key}\":{expected_value} but does not.\nContent source: {content_source}"
    )]
    ContentMustContainsJsonKeyValueMissingOrIncorrect {
        expected_key: String,
        expected_value: String,
        content_source: String,
    },
    #[error(
        "Validation failure: Content MUST contain {expected_key}={expected_value}, but is missing key {expected_key}. Content source: {content_source}"
    )]
    ContentMustContainsKeyValueKeyMissing {
        expected_key: String,
        expected_value: String,
        content_source: String,
    },
    #[error(
        "Validation failure: Content MUST contain {expected_key}={expected_value}, but the value does not match. Expected: {expected_value}, Actual: {found_value}, Content source: {content_source}"
    )]
    ContentMustContainsKeyValueValueIncorrect {
        expected_key: String,
        expected_value: String,
        found_value: String,
        content_source: String,
    },
    #[error(
        "Validation failure: Content MUST NOT contain JSON key-value \"{expected_key}\":{expected_value}, but does. Content source: {content_source}"
    )]
    ContentMustNotContainsJsonHasKeyValue {
        expected_key: String,
        expected_value: String,
        content_source: String,
    },
    #[error(
        "Validation failure: Content MUST NOT contain {expected_key}={expected_value}, but does. Content source: {content_source}"
    )]
    ContentMustNotContainsHasKeyValue {
        expected_key: String,
        expected_value: String,
        content_source: String,
    },
    #[error(
        "Validation failure: Content MUST contain {value}, but does not. Content source: {content_source}"
    )]
    ContentMustContainValueMissing { value: String, content_source: String },
    #[error(
        "Validation failure: Content MUST NOT contain {value}, but does. Content source: {content_source}"
    )]
    ContentMustNotContainValuePresent { value: String, content_source: String },
    #[error("Validation error: Could not open golden file at {golden_path}, error: {error}")]
    FailedToOpenGoldenFile { golden_path: String, error: String },
    #[error(
        "Validation failure: Golden file mismatch. The golden file contents from {golden_path} do not match content from {content_source}. \nDiffs:\n{diffs}"
    )]
    ContentGoldenFileMismatch { golden_path: String, content_source: String, diffs: String },
}

impl ValidationError {
    /// Replaces `self` with another error that also stores the provided `package_name`.
    fn with_package_name(self, package_name: String) -> Self {
        match self {
            ValidationError::FailedToPerformFileCheck { file_path, error, .. } => {
                ValidationError::FailedToPerformFileCheck { package_name, file_path, error }
            }
            ValidationError::UnexpectedFilePresence { file_path, .. } => {
                ValidationError::UnexpectedFilePresence { package_name, file_path }
            }
            ValidationError::UnexpectedFilePresenceOrHasContents { file_path, .. } => {
                ValidationError::UnexpectedFilePresenceOrHasContents { package_name, file_path }
            }
            _ => self,
        }
    }

    /// Replaces `self` with another error that also stores the provided `content_source`.
    fn with_content_source(self, content_source: String) -> Self {
        match self {
            ValidationError::FailedToParseContentsAsJson { content_source: _, error } => {
                ValidationError::FailedToParseContentsAsJson { content_source, error }
            }
            ValidationError::UnableToHandleJsonContent { found, .. } => {
                ValidationError::UnableToHandleJsonContent { found, content_source }
            }
            ValidationError::ContentAndPolicyJsonTypeMismatch {
                found, found_type, policy, ..
            } => ValidationError::ContentAndPolicyJsonTypeMismatch {
                found,
                found_type,
                policy,
                content_source,
            },
            _ => self,
        }
    }
}

/// The type of content to expect when performing ContentChecks.
#[derive(Deserialize, Serialize)]
pub enum ContentType {
    /// JsonKeyValue currently handles bool, number, or string values from target JSON files.
    /// The value in the policy file is the expected value as a string, e.g. "true" for bool.
    JsonKeyValue(String, String),
    /// KeyValuePair expects the delimiter to be `=`, for example `key=value`.
    KeyValuePair(String, String),
    String(String),
}

/// Possible sources from which to resolve the merkle string for a package:
/// 1. The zircon.system.pkgfs.cmd value from additional_boot_args for the system image blob.
/// 2. The package listing in data/static_packages from the system image blob's data.
/// 3. The bootfs package listing within a zbi from data/bootfs_packages.
#[derive(Deserialize, Serialize)]
pub enum PackageSource {
    SystemImage,
    StaticPackages(String),
    BootfsPackages(String),
}

impl Display for PackageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageSource::SystemImage => write!(f, "system_image"),
            PackageSource::StaticPackages(pkg_name) => write!(f, "static-pkgs-index: {}", pkg_name),
            PackageSource::BootfsPackages(pkg_name) => write!(f, "bootfs-pkgs-index: {}", pkg_name),
        }
    }
}

/// Possible sources for files within a package:
/// 1. Listed in the meta/contents file of a package in the form "name=<merkle>". In this case, we
/// must resolve the merkle from the map then access the file from the blobs_dir by merkle.
/// 2. Listed as a file directly accessible in the package archive, ie meta/data/sshd-host/sshd_config in config-data.
#[derive(Deserialize, Serialize)]
pub enum FileSource {
    /// Name for the target file as a key in the meta/contents mapping from the package.
    PackageMetaContents(String),
    /// Possible paths within the package archive. If multiple files are found, validation will fail.
    /// Multiple paths are only supported to enable backwards compatibility during file migrations.
    PackageFar(Vec<String>),
    /// Name for the target file in the bootfs file collection.
    BootfsFile(String),
}

impl Display for FileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileSource::PackageMetaContents(path) => write!(f, "meta-contents: {}", path),
            FileSource::PackageFar(paths) => {
                write!(f, "package-far: {}", paths.join(", "))
            }
            FileSource::BootfsFile(path) => write!(f, "bootfs-file: {}", path),
        }
    }
}

/// The expected state of a file when performing FileChecks.
#[derive(Deserialize, Serialize)]
pub enum FileState {
    Present,
    Absent,
    AbsentOrEmpty,
}

#[derive(Deserialize, Serialize)]
pub struct BuildCheckSpec {
    /// Checks requiring presence or absence of specific key-value pairs in additional boot args.
    pub additional_boot_args_checks: Option<ContentCheckSpec>,
    /// Checks which involve reading the contents of specific packages in the build.
    pub package_checks: Vec<PackageCheckSpec>,
    /// Checks which involve reading contents of boofs files.
    pub bootfs_file_checks: Vec<FileCheckSpec>,
}

/// Package checks operate on the content of individual packages.
/// package_source indicates how to find the merkle string for the package, which is used to fetch the
/// package from the blobs_directory's ArtifactReader.
#[derive(Deserialize, Serialize)]
pub struct PackageCheckSpec {
    /// Which set of package sources to resolve the merkle string from.
    pub source: PackageSource,
    /// Set of checks to run on files within the package.
    pub file_checks: Vec<FileCheckSpec>,
}

#[derive(Deserialize, Serialize)]
pub struct FileCheckSpec {
    /// How the file is sourced from the package contents or bootfs file collection.
    pub source: FileSource,
    /// Expected state of the file: present, absent, absent or empty.
    pub state: FileState,
    /// If the file is expected to be present, the set of checks to run on the file's contents.
    pub content_checks: Option<ContentCheckSpec>,
}

/// Defines a set of validations for content that must or must not be part of some input content.
/// There is no enforcement on mutual exclusion between must_contain and must_not_contain. If the same
/// value appears in both sets, validation will simply fail at check-time.
#[derive(Deserialize, Serialize)]
pub struct ContentCheckSpec {
    /// Set of items that must be present in the target content.
    pub must_contain: Option<Vec<ContentType>>,
    /// Set of items that must not be present in the target content.
    pub must_not_contain: Option<Vec<ContentType>>,
    /// The name of possible golden files to match. One match is needed to pass this check.
    /// The directory path they reside in is provided elsewhere.
    /// The contents of the golden file must match target content as a string.
    pub matches_golden: Option<Vec<String>>,
}

/// Validates the provided build artifacts according to the provided policy.
///
/// # Arguments
///
/// * `validation_policy` - A policy file describing checks to perform
/// * `boot_args_data` - Mapping of arg name to vector of values delimited by `+`
/// * `bootfs_files` - Mapping of file name to contents of file
/// * `static_pkgs` - Mapping of pkg name to merkle hash string
/// * `blobs_artifact_reader` - ArtifactReader backed by a build's blob set
/// * 'golden_files_dir` - Directory containing golden files for matching
pub fn validate_build_checks(
    validation_policy: BuildCheckSpec,
    boot_args_data: HashMap<String, Vec<String>>,
    bootfs_files: &HashMap<String, Vec<u8>>,
    static_pkgs: HashMap<String, String>,
    blobs_artifact_reader: &mut Box<dyn ArtifactReader>,
    golden_files_dir: &str,
) -> Result<Vec<ValidationError>, Error> {
    let mut errors_found = Vec::new();

    // If the policy specifies additional_boot_args checks, run them.
    if let Some(additional_boot_args_checks) = validation_policy.additional_boot_args_checks {
        for error in validate_additional_boot_args(additional_boot_args_checks, &boot_args_data) {
            errors_found.push(error);
        }
    }

    // If the policy specifies bootfs file checks, run them.
    for error in
        validate_bootfs_files(validation_policy.bootfs_file_checks, &bootfs_files, golden_files_dir)
    {
        errors_found.push(error);
    }

    for package_check in validation_policy.package_checks {
        // Resolve the package merkle string based on the source specified by the policy.
        let pkg_merkle_string = match package_check.source {
            PackageSource::SystemImage => extract_system_image_hash_string(&boot_args_data)?,
            PackageSource::StaticPackages(ref pkg_name) => {
                if let Some(merkle_string) = static_pkgs.get(pkg_name) {
                    merkle_string.to_string()
                } else {
                    errors_found.push(ValidationError::MissingStaticPackage {
                        package_name: pkg_name.to_string(),
                    });
                    continue;
                }
            }
            PackageSource::BootfsPackages(_) => unimplemented!(),
        };

        // Run the validations specified by the policy.
        // Specification of the concrete PackageFileValidator impl should remain internal to build_checks.
        for error in validate_package::<PackageFileChecker>(
            &package_check,
            &pkg_merkle_string,
            blobs_artifact_reader,
            golden_files_dir,
        ) {
            errors_found.push(error);
        }
    }

    Ok(errors_found)
}

fn validate_additional_boot_args(
    checks: ContentCheckSpec,
    boot_args_data: &HashMap<String, Vec<String>>,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    if let Some(must_contain_checks) = checks.must_contain {
        for check in must_contain_checks {
            match check {
                ContentType::KeyValuePair(key, value) => {
                    if !boot_args_data.contains_key(&key) {
                        errors.push(ValidationError::AdditionalBootArgsMustContainsKeyMissing {
                            expected_key: key,
                            expected_value: value,
                        });
                        continue;
                    }

                    if let Some(values_vec) = boot_args_data.get(&key) {
                        // AdditionalBootArgsCollector splits its values by the `+` delimiter.
                        // This check will only operate on the first value found in cases where
                        // multiple values are present.
                        let found_value = values_vec[0].clone();
                        if !(found_value == value) {
                            errors.push(
                                ValidationError::AdditionalBootArgsMustContainsValueIncorrect {
                                    expected_key: key,
                                    expected_value: value,
                                    found_value,
                                },
                            );
                        }
                    }
                }
                _ => {
                    errors.push(ValidationError::InvalidPolicyConfiguration {
                        error:
                            "Unexpected content type check for boot args, supports key value only."
                                .to_string(),
                    });
                }
            }
        }
    }

    if let Some(must_not_contain_checks) = checks.must_not_contain {
        for check in must_not_contain_checks {
            match check {
                ContentType::KeyValuePair(key, value) => {
                    if boot_args_data.contains_key(&key) {
                        if let Some(values_vec) = boot_args_data.get(&key) {
                            // AdditionalBootArgsCollector supports multiple `+` delimited values. This expects only 1 value for now.
                            let found_value = values_vec[0].clone();
                            if found_value == value {
                                errors.push(
                                    ValidationError::AdditionalBootArgsMustNotContainsHasKeyValue {
                                        expected_key: key,
                                        expected_value: value,
                                    },
                                );
                            }
                        }
                    }
                }
                _ => {
                    errors.push(ValidationError::InvalidPolicyConfiguration {
                        error:
                            "Unexpected content type check for boot args, supports key value only."
                                .to_string(),
                    });
                }
            }
        }
    }

    errors
}

fn validate_bootfs_files(
    bootfs_file_checks: Vec<FileCheckSpec>,
    bootfs_files: &HashMap<String, Vec<u8>>,
    golden_files_dir: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for check in bootfs_file_checks {
        match resolve_bootfs_file(&check.source, &bootfs_files) {
            Ok((file_name, file_contents)) => {
                for error in validate_file(&check, file_contents, &file_name, golden_files_dir) {
                    errors.push(error);
                }
            }
            Err(e) => {
                // This error happens if the policy specified a package file source for a bootfs file check.
                errors.push(e);
            }
        }
    }
    errors
}

/// File checker trait exists for easier testing.
trait FileChecker {
    /// check_file is responsible for both resolving and validating the file.
    fn check_file(
        check: &FileCheckSpec,
        package_far_reader: &mut FarReader<Box<dyn ReadSeek>>,
        blobs_artifact_reader: &mut Box<dyn ArtifactReader>,
        golden_files_dir: &str,
    ) -> Vec<ValidationError>
    where
        Self: Sized;
}

fn validate_package<FC: FileChecker>(
    check: &PackageCheckSpec,
    pkg_merkle_string: &String,
    blobs_artifact_reader: &mut Box<dyn ArtifactReader>,
    golden_files_dir: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Open the package as a blob from the blobs_dir reader.
    let package_blob_reader = match blobs_artifact_reader.open(&Path::new(pkg_merkle_string)) {
        Ok(reader) => reader,
        Err(e) => {
            errors.push(ValidationError::FailedToPerformPackageCheck {
                package_name: check.source.to_string(),
                error: e.to_string(),
            });
            return errors;
        }
    };

    // Interpret the blob we just opened as a Fuchsia Archive (.far).
    let mut package_far_reader = match FarReader::new(package_blob_reader) {
        Ok(far_reader) => far_reader,
        Err(e) => {
            errors.push(ValidationError::FailedToPerformPackageCheck {
                package_name: check.source.to_string(),
                error: e.to_string(),
            });
            return errors;
        }
    };

    for file_check in &check.file_checks {
        errors.extend(
            FC::check_file(
                &file_check,
                &mut package_far_reader,
                blobs_artifact_reader,
                golden_files_dir,
            )
            .iter()
            .map(|error| error.to_owned().with_package_name(check.source.to_string())),
        );
    }

    errors
}

/// Given a `FileSource` (`BootfsFile`), return the contents from the bootfs_file map.
fn resolve_bootfs_file(
    source: &FileSource,
    bootfs_files: &HashMap<String, Vec<u8>>,
) -> Result<(String, Option<Vec<u8>>), ValidationError> {
    match source {
        FileSource::BootfsFile(file_name) => {
            // File not found is represented by `None` for `bytes`.
            let bytes = match bootfs_files.get(file_name) {
                Some(bytes) => Some(bytes.clone()),
                None => None,
            };
            Ok((file_name.to_string(), bytes))
        }
        _ => Err(ValidationError::InvalidPolicyConfiguration {
            error: "Policy specified non-bootfs file source for a bootfs file check".to_string(),
        }),
    }
}

/// Given a `FileSource` (`MetaContents` or `PackageFar`), find and read a file.
/// Returns (file path found, optional bytes read) or `ValidationError`.
fn resolve_package_file(
    source: &FileSource,
    package_far_reader: &mut FarReader<Box<dyn ReadSeek>>,
    blobs_artifact_reader: &mut Box<dyn ArtifactReader>,
) -> Result<(String, Option<Vec<u8>>), Error> {
    // First, find the file and read its contents if it is present.
    // File absence is represented by `file_contents_bytes` = `None`.
    match source {
        FileSource::PackageMetaContents(path) => {
            // Read `meta/contents` to find merkle, then read the corresponding blob's bytes.
            match read_content_blob(package_far_reader, blobs_artifact_reader, &path) {
                Ok(bytes) => {
                    return Ok((path.to_string(), Some(bytes)));
                }
                Err(ReadContentBlobError::MetaContentsDoesNotContainFile { .. }) => {
                    return Ok((String::new(), None));
                }
                Err(e) => {
                    // For `FileSource::MetaContents` checks, if a file is listed in `meta/contents`
                    // but NOT found in blobs, it is considered to be an error.
                    return Err(e.into());
                }
            }
        }
        FileSource::PackageFar(possible_paths) => {
            // Find the file within possible paths that is present in the package.
            let files_in_package = package_far_reader
                .list()
                .map(|entry| entry.path().to_string())
                .collect::<HashSet<String>>();
            let mut files_found = Vec::new();
            for path in possible_paths {
                if files_in_package.contains(path) {
                    files_found.push(path.to_string());
                }
            }

            if files_found.len() > 1 {
                return Err(ValidationError::UnexpectedNumberOfFilesPresent {
                    possible_paths: possible_paths.to_vec(),
                    files_found,
                }
                .into());
            }

            if files_found.len() == 1 {
                let file_path_found = files_found[0].clone();
                let bytes = package_far_reader.read_file(&file_path_found)?;
                return Ok((file_path_found, Some(bytes)));
            }

            Ok((String::new(), None))
        }
        _ => Err(ValidationError::InvalidPolicyConfiguration {
            error: "Policy specified non-package file source for a package file check".to_string(),
        }
        .into()),
    }
}

/// Checks the state of the file (present, absent, absent or empty) before calling
/// `validate_file_contents`, if content checks are specified.
fn validate_file(
    check: &FileCheckSpec,
    content_bytes: Option<Vec<u8>>,
    content_source: &str,
    golden_files_dir: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Check that the state of the file (present or absent) matches policy expectations.
    match check.state {
        FileState::Present => {
            let bytes = match content_bytes {
                Some(bytes) => bytes,
                None => {
                    errors.push(ValidationError::FailedToFindFile {
                        file_paths: check.source.to_string(),
                    });
                    return errors;
                }
            };

            // If we have content checks beyond just the file being there, run them.
            if let Some(content_checks) = &check.content_checks {
                for error_found in
                    validate_file_contents(content_checks, bytes, content_source, golden_files_dir)
                {
                    errors.push(error_found);
                }
            }
        }
        FileState::Absent => {
            // To pass this check, file_contents_bytes must be None, indicating that a file was not found.
            if content_bytes.is_some() {
                errors.push(ValidationError::UnexpectedFilePresence {
                    // Package name is not known here and needs to be supplied by error handler.
                    package_name: String::new(),
                    file_path: content_source.to_string(),
                });
            }
        }
        FileState::AbsentOrEmpty => {
            // To pass this check, file_contents_bytes must be either None or an empty byte vector.
            if let Some(bytes) = content_bytes {
                if bytes.len() > 0 {
                    errors.push(ValidationError::UnexpectedFilePresenceOrHasContents {
                        // Package name is not known here and needs to be supplied by error handler.
                        package_name: String::new(),
                        file_path: content_source.to_string(),
                    });
                }
            }
        }
    }
    errors
}

/// `content_source` is the file path from where the bytes were read.
/// This method doesn't open or read files, so the file path is provided for error traceability.
fn validate_file_contents(
    checks: &ContentCheckSpec,
    content_bytes: Vec<u8>,
    content_source: &str,
    golden_files_dir: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Currently, content checks expect contents representable as a string.
    // The string content may be further processed into a key-value map.
    let content_str = match from_utf8(&content_bytes) {
        Ok(content_str) => content_str,
        Err(e) => {
            errors.push(ValidationError::FailedToParseContentsToString {
                content_source: content_source.to_string(),
                error: e.to_string(),
            });
            return errors;
        }
    };

    errors.extend(file_contents_must_contain(checks, content_str, content_source));
    errors.extend(file_contents_must_not_contain(checks, content_str, content_source));
    errors.extend(file_contents_match_golden(
        checks,
        content_str,
        content_source,
        golden_files_dir,
    ));

    errors
}

fn file_contents_must_contain(
    checks: &ContentCheckSpec,
    content_str: &str,
    content_source: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    if let Some(must_contain) = &checks.must_contain {
        for check in must_contain {
            match check {
                ContentType::JsonKeyValue(key, value) => {
                    match json_contents_contain_key_value_pair(key, value, content_str) {
                        Ok(contains) => {
                            if !contains {
                                errors.push(
                                    ValidationError::ContentMustContainsJsonKeyValueMissingOrIncorrect {
                                        expected_key: key.to_string(),
                                        expected_value: value.to_string(),
                                        content_source: content_source.to_string(),
                                    },
                                );
                                continue;
                            }
                        }
                        Err(e) => {
                            errors.push(e.with_content_source(content_source.to_string()));
                            continue;
                        }
                    }
                }
                ContentType::KeyValuePair(key, value) => {
                    let mapping = match parse_key_value(content_str) {
                        Ok(map) => map,
                        Err(e) => {
                            errors.push(ValidationError::FailedToParseContentsAsKeyValueMap {
                                content_source: content_source.to_string(),
                                error: e.to_string(),
                            });
                            continue;
                        }
                    };

                    if !mapping.contains_key(key) {
                        errors.push(ValidationError::ContentMustContainsKeyValueKeyMissing {
                            expected_key: key.to_string(),
                            expected_value: value.to_string(),
                            content_source: content_source.to_string(),
                        });
                        continue;
                    }

                    if let Some(found) = mapping.get(key) {
                        if found != value {
                            errors.push(
                                ValidationError::ContentMustContainsKeyValueValueIncorrect {
                                    expected_key: key.to_string(),
                                    expected_value: value.to_string(),
                                    found_value: found.to_string(),
                                    content_source: content_source.to_string(),
                                },
                            );
                        }
                    }
                }
                ContentType::String(value) => {
                    if !content_str.contains(value) {
                        errors.push(ValidationError::ContentMustContainValueMissing {
                            value: value.to_string(),
                            content_source: content_source.to_string(),
                        });
                    }
                }
            }
        }
    }
    errors
}

fn file_contents_must_not_contain(
    checks: &ContentCheckSpec,
    content_str: &str,
    content_source: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if let Some(must_not_contain) = &checks.must_not_contain {
        for check in must_not_contain {
            match check {
                ContentType::JsonKeyValue(key, value) => {
                    match json_contents_contain_key_value_pair(key, value, content_str) {
                        Ok(contains) => {
                            if contains {
                                errors.push(
                                    ValidationError::ContentMustNotContainsJsonHasKeyValue {
                                        expected_key: key.to_string(),
                                        expected_value: value.to_string(),
                                        content_source: content_source.to_string(),
                                    },
                                );
                                continue;
                            }
                        }
                        Err(e) => {
                            errors.push(e.with_content_source(content_source.to_string()));
                            continue;
                        }
                    }
                }
                ContentType::KeyValuePair(key, value) => {
                    let mapping = match parse_key_value(content_str) {
                        Ok(map) => map,
                        Err(e) => {
                            errors.push(ValidationError::FailedToParseContentsAsKeyValueMap {
                                content_source: content_source.to_string(),
                                error: e.to_string(),
                            });
                            continue;
                        }
                    };

                    if mapping.contains_key(key) {
                        if let Some(found) = mapping.get(key) {
                            if found == value {
                                errors.push(ValidationError::ContentMustNotContainsHasKeyValue {
                                    expected_key: key.to_string(),
                                    expected_value: value.to_string(),
                                    content_source: content_source.to_string(),
                                });
                            }
                        }
                    }
                }
                ContentType::String(value) => {
                    if content_str.contains(value) {
                        errors.push(ValidationError::ContentMustNotContainValuePresent {
                            value: value.to_string(),
                            content_source: content_source.to_string(),
                        });
                    }
                }
            }
        }
    }
    errors
}

fn file_contents_match_golden(
    checks: &ContentCheckSpec,
    content_str: &str,
    content_source: &str,
    golden_files_dir: &str,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let mut match_found = false;
    if let Some(golden_file_names) = &checks.matches_golden {
        // Only one golden file must match to be successful.
        for golden_file_name in golden_file_names {
            let golden_path = Path::new(golden_files_dir).join(golden_file_name);
            match read_to_string(&golden_path) {
                Ok(golden_contents) => {
                    // Diffs are calculated and reported relative to the golden file.
                    let Changeset { diffs, distance, .. } =
                        Changeset::new(&golden_contents, content_str, "\n");
                    if distance == 0 {
                        match_found = true;
                    } else if distance > 0 {
                        let mut reported_diffs = String::new();
                        for diff in diffs {
                            match diff {
                                Difference::Same(_) => {}
                                Difference::Add(ref line) => {
                                    reported_diffs.push_str(&format!("+{}\n", line));
                                }
                                Difference::Rem(ref line) => {
                                    reported_diffs.push_str(&format!("-{}\n", line));
                                }
                            }
                        }
                        errors.push(ValidationError::ContentGoldenFileMismatch {
                            golden_path: golden_path.to_string_lossy().to_string(),
                            content_source: content_source.to_string(),
                            diffs: reported_diffs,
                        });
                    }
                }
                Err(e) => {
                    errors.push(ValidationError::FailedToOpenGoldenFile {
                        golden_path: golden_path.to_string_lossy().to_string(),
                        error: e.to_string(),
                    });
                }
            }
        }
    }

    if match_found {
        return Vec::new();
    }
    errors
}

fn json_contents_contain_key_value_pair(
    key: &str,
    value: &str,
    content_str: &str,
) -> Result<bool, ValidationError> {
    let mapping: serde_json::Value = match serde_json::from_str(content_str) {
        Ok(map) => map,
        Err(e) => {
            return Err(ValidationError::FailedToParseContentsAsJson {
                content_source: String::new(),
                error: e.to_string(),
            });
        }
    };

    match &mapping[key] {
        serde_json::Value::Null => {
            // Assumption: found null values are treated as absent from the found mapping.
            // This logic will need to be updated if we actually need to check for
            // presence of null values, e.g. {"key": null}.
            Ok(false)
        }
        serde_json::Value::Bool(found_value) => {
            // Policy specifies boolean values as string. Parse and compare here.
            match value {
                "true" => Ok(*found_value),
                "false" => Ok(!found_value),
                _ => Err(ValidationError::ContentAndPolicyJsonTypeMismatch {
                    found: found_value.to_string(),
                    found_type: "Bool".to_string(),
                    policy: value.to_string(),
                    content_source: String::new(),
                }),
            }
        }
        serde_json::Value::Number(found_value) => {
            // Per serde_json implementation and docs, Number may be i64, u64, or f64.
            if found_value.is_i64() {
                match i64::from_str(value) {
                    Ok(policy_val) => return Ok(policy_val == found_value.as_i64().unwrap()),
                    Err(_) => {
                        return Err(ValidationError::ContentAndPolicyJsonTypeMismatch {
                            found: found_value.to_string(),
                            found_type: "Number i64".to_string(),
                            policy: value.to_string(),
                            content_source: String::new(),
                        });
                    }
                }
            }
            if found_value.is_u64() {
                match u64::from_str(value) {
                    Ok(policy_val) => return Ok(policy_val == found_value.as_u64().unwrap()),
                    Err(_) => {
                        return Err(ValidationError::ContentAndPolicyJsonTypeMismatch {
                            found: found_value.to_string(),
                            found_type: "Number u64".to_string(),
                            policy: value.to_string(),
                            content_source: String::new(),
                        });
                    }
                }
            }
            if found_value.is_f64() {
                match f64::from_str(value) {
                    Ok(policy_val) => return Ok(policy_val == found_value.as_f64().unwrap()),
                    Err(_) => {
                        return Err(ValidationError::ContentAndPolicyJsonTypeMismatch {
                            found: found_value.to_string(),
                            found_type: "Number f64".to_string(),
                            policy: value.to_string(),
                            content_source: String::new(),
                        });
                    }
                }
            }
            // Reaching this error likely means a bug in serde_json.
            Err(ValidationError::UnableToHandleJsonContent {
                found: found_value.to_string(),
                content_source: String::new(),
            })
        }
        serde_json::Value::String(found_value) => Ok(found_value == value),
        val => Err(ValidationError::UnableToHandleJsonContent {
            found: val.to_string(),
            content_source: String::new(),
        }),
    }
}

struct PackageFileChecker;

impl FileChecker for PackageFileChecker {
    fn check_file(
        check: &FileCheckSpec,
        package_far_reader: &mut FarReader<Box<dyn ReadSeek>>,
        blobs_artifact_reader: &mut Box<dyn ArtifactReader>,
        golden_files_dir: &str,
    ) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        // First, find the file and read its contents if it is present.
        // File absence is represented by file_contents_bytes = None.
        let (file_path_found, file_contents_bytes) =
            match resolve_package_file(&check.source, package_far_reader, blobs_artifact_reader) {
                Ok((path, bytes)) => (path, bytes),
                Err(e) => {
                    errors.push(ValidationError::FailedToPerformFileCheck {
                        // Package name is not known here and needs to be supplied by error handler.
                        package_name: String::new(),
                        file_path: check.source.to_string(),
                        error: e.to_string(),
                    });
                    return errors;
                }
            };
        for error in validate_file(check, file_contents_bytes, &file_path_found, golden_files_dir) {
            errors.push(error);
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::META_CONTENTS_PATH;
    use anyhow::anyhow;
    use fuchsia_archive::write as far_write;
    use maplit::hashmap;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::{BufWriter, Cursor, Read, Write};
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    struct TestArtifactReader {
        artifacts: HashMap<PathBuf, Vec<u8>>,
    }

    impl TestArtifactReader {
        fn new(artifacts: HashMap<PathBuf, Vec<u8>>) -> Self {
            Self { artifacts }
        }
    }

    impl ArtifactReader for TestArtifactReader {
        fn open(&mut self, path: &Path) -> Result<Box<dyn ReadSeek>> {
            if let Some(bytes) = self.artifacts.get(path) {
                return Ok(Box::new(Cursor::new(bytes.clone())));
            }
            Err(anyhow!("No artifact found for path: {:?}", path))
        }

        fn read_bytes(&mut self, path: &Path) -> Result<Vec<u8>> {
            if let Some(bytes) = self.artifacts.get(path) {
                return Ok(bytes.clone());
            }
            Err(anyhow!("No artifact found for path: {:?}", path))
        }

        fn get_deps(&self) -> HashSet<PathBuf> {
            panic!("not implemented");
        }
    }

    struct TestErrorFreeFileChecker;

    impl FileChecker for TestErrorFreeFileChecker {
        fn check_file(
            _check: &FileCheckSpec,
            _package_far_reader: &mut FarReader<Box<dyn ReadSeek>>,
            _blobs_artifact_reader: &mut Box<dyn ArtifactReader>,
            _golden_files_dir: &str,
        ) -> Vec<ValidationError>
        where
            Self: Sized,
        {
            Vec::new()
        }
    }

    fn create_package_far(contents: HashMap<&str, &[u8]>) -> Vec<u8> {
        let mut contents_map: BTreeMap<&str, (u64, Box<dyn Read>)> = BTreeMap::new();
        for (path, bytes) in contents {
            let bytes_reader: Box<dyn Read> = Box::new(bytes);
            contents_map.insert(path, (bytes.len() as u64, bytes_reader));
        }
        let mut package_far = BufWriter::new(Vec::new());
        far_write(&mut package_far, contents_map).unwrap();
        package_far.into_inner().unwrap()
    }

    // Test against a basic policy which has all of the elements included.
    #[test]
    fn test_validate_build_checks_success() {
        let expected_key = "test_key";
        let expected_value = "test_value";
        let policy = BuildCheckSpec {
            additional_boot_args_checks: Some(ContentCheckSpec {
                must_contain: Some(vec![ContentType::KeyValuePair(
                    expected_key.to_string(),
                    expected_value.to_string(),
                )]),
                must_not_contain: Some(vec![ContentType::KeyValuePair(
                    "some_other_key".to_string(),
                    "and_value".to_string(),
                )]),
                matches_golden: None,
            }),
            package_checks: Vec::new(),
            bootfs_file_checks: Vec::new(),
        };
        let boot_args_data = hashmap! {
            expected_key.to_string() => vec![expected_value.to_string()]
        };

        let mut artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));
        let errors = validate_build_checks(
            policy,
            boot_args_data,
            &HashMap::new(),
            HashMap::new(),
            &mut artifact_reader,
            "",
        )
        .unwrap();

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_build_checks_tolerates_absent_boot_args_policy() {
        let expected_key = "test_key";
        let expected_value = "test_value";
        let policy = BuildCheckSpec {
            additional_boot_args_checks: None,
            package_checks: Vec::new(),
            bootfs_file_checks: Vec::new(),
        };
        let boot_args_data = hashmap! {
            expected_key.to_string() => vec![expected_value.to_string()]
        };

        let mut artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));
        let errors = validate_build_checks(
            policy,
            boot_args_data,
            &HashMap::new(),
            HashMap::new(),
            &mut artifact_reader,
            "",
        )
        .unwrap();

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_boot_args_must_contain_success() {
        let expected_key = "test_key";
        let expected_value = "test_value";
        let policy = ContentCheckSpec {
            must_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            must_not_contain: None,
            matches_golden: None,
        };
        let input_data = hashmap! {
            expected_key.to_string() => vec![expected_value.to_string()]
        };

        let validation_errors = validate_additional_boot_args(policy, &input_data);

        assert_eq!(validation_errors.len(), 0);
    }

    #[test]
    fn test_boot_args_must_contain_failure() {
        let expected_key = "test_key";
        let expected_value = "test_value";
        let policy = ContentCheckSpec {
            must_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            must_not_contain: None,
            matches_golden: None,
        };
        let input_data = hashmap! {
            "some_other_key".to_string() => vec!["some_other_value".to_string()]
        };

        let validation_errors = validate_additional_boot_args(policy, &input_data);

        assert!(validation_errors.len() == 1);
        match &validation_errors[0] {
            // Check that we report the value we were looking for, but did not find.
            ValidationError::AdditionalBootArgsMustContainsKeyMissing {
                expected_key,
                expected_value,
            } => {
                assert_eq!(*expected_key, "test_key".to_string());
                assert_eq!(*expected_value, "test_value".to_string());
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_boot_args_must_contain_invalid_policy_configuration() {
        // Boot args checks only accept KeyValue pair as the content type.
        let policy = ContentCheckSpec {
            must_contain: Some(vec![ContentType::String("test".to_string())]),
            must_not_contain: None,
            matches_golden: None,
        };
        let input_data = HashMap::new();

        let validation_errors = validate_additional_boot_args(policy, &input_data);

        assert_eq!(validation_errors.len(), 1);
        // Check error type.
        match &validation_errors[0] {
            ValidationError::InvalidPolicyConfiguration { error: _ } => {}
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_boot_args_must_not_contain_success() {
        let expected_key = "test_key";
        let expected_value = "test_value";
        // The policy sets the expectations.
        let policy = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            matches_golden: None,
        };
        // The input data here conforms to the policy.
        let input_data = hashmap! {
            "some_other_key".to_string() => vec!["some_other_value".to_string()]
        };

        let validation_errors = validate_additional_boot_args(policy, &input_data);

        assert_eq!(validation_errors.len(), 0);
    }

    #[test]
    fn test_boot_args_must_not_contain_failure() {
        let expected_key = "test_key";
        let expected_value = "test_value";
        // The policy sets the expectations.
        let policy = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            matches_golden: None,
        };
        // The input data here does not conform to the policy.
        let input_data = hashmap! {
            expected_key.to_string() => vec![expected_value.to_string()]
        };

        let validation_errors = validate_additional_boot_args(policy, &input_data);

        assert_eq!(validation_errors.len(), 1);
        match &validation_errors[0] {
            // Check that we report the value we were expecting to be absent, but was present.
            ValidationError::AdditionalBootArgsMustNotContainsHasKeyValue {
                expected_key,
                expected_value,
            } => {
                assert_eq!(*expected_key, "test_key");
                assert_eq!(*expected_value, "test_value");
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_boot_args_must_not_contain_invalid_policy_configuration() {
        // Boot args checks only accept KeyValue pair as the content type.
        let policy = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::String("test".to_string())]),
            matches_golden: None,
        };
        let input_data = HashMap::new();

        let validation_errors = validate_additional_boot_args(policy, &input_data);

        assert_eq!(validation_errors.len(), 1);
        // Check error type.
        match &validation_errors[0] {
            ValidationError::InvalidPolicyConfiguration { error: _ } => {}
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_bootfs_file_check_success() {
        let content_source = "bootfs_file_name";
        let expected_string = "sample_contents";

        let bootfs_files = hashmap![
            content_source.to_string() => expected_string.as_bytes().to_vec()
        ];
        let bootfs_file_checks = vec![FileCheckSpec {
            source: FileSource::BootfsFile(content_source.to_string()),
            state: FileState::Present,
            content_checks: Some(ContentCheckSpec {
                must_not_contain: None,
                must_contain: Some(vec![ContentType::String(expected_string.to_string())]),
                matches_golden: None,
            }),
        }];

        let errors = validate_bootfs_files(bootfs_file_checks, &bootfs_files, "");

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_bootfs_file_check_failure() {
        let content_source = "bootfs_file_name";
        let expected_string = "sample_contents";

        let bootfs_files = hashmap![
            content_source.to_string() => expected_string.as_bytes().to_vec()
        ];
        let bootfs_file_checks = vec![FileCheckSpec {
            source: FileSource::BootfsFile(content_source.to_string()),
            state: FileState::Present,
            content_checks: Some(ContentCheckSpec {
                must_not_contain: Some(vec![ContentType::String(expected_string.to_string())]),
                must_contain: None,
                matches_golden: None,
            }),
        }];

        let errors = validate_bootfs_files(bootfs_file_checks, &bootfs_files, "");

        assert_eq!(errors.len(), 1);

        match &errors[0] {
            ValidationError::ContentMustNotContainValuePresent {
                value,
                content_source: reported_source,
            } => {
                assert_eq!(expected_string, value);
                assert_eq!(content_source, reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_bootfs_file_check_error() {
        let content_source = "bootfs_file_name";
        let expected_string = "sample_contents";

        let bootfs_files = hashmap![
            content_source.to_string() => expected_string.as_bytes().to_vec()
        ];
        let bootfs_file_checks = vec![FileCheckSpec {
            source: FileSource::PackageMetaContents(content_source.to_string()),
            state: FileState::Present,
            content_checks: Some(ContentCheckSpec {
                must_not_contain: None,
                must_contain: None,
                matches_golden: None,
            }),
        }];

        let errors = validate_bootfs_files(bootfs_file_checks, &bootfs_files, "");

        assert_eq!(errors.len(), 1);

        match &errors[0] {
            ValidationError::InvalidPolicyConfiguration { .. } => {}
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_package_check_fails_to_open_blob() {
        // Create mocks with nothing in them and verify validate_package returns an error.
        let check =
            PackageCheckSpec { source: PackageSource::SystemImage, file_checks: Vec::new() };
        let pkg_merkle_string = "unused_merkle".to_string();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let validation_errors = validate_package::<TestErrorFreeFileChecker>(
            &check,
            &pkg_merkle_string,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(validation_errors.len(), 1);
        match &validation_errors[0] {
            ValidationError::FailedToPerformPackageCheck { package_name: _, error: _ } => {}
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_package_check_fails_to_read_blob_as_far() {
        // Create mocks with a package in the ArtifactReader, but it's not a .far. Verify error result.
        let check =
            PackageCheckSpec { source: PackageSource::SystemImage, file_checks: Vec::new() };
        let pkg_merkle_string = "test_pkg_merkle".to_string();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> = Box::new(TestArtifactReader::new(
            hashmap![
                PathBuf::from_str(&pkg_merkle_string).unwrap() => "some non-far contents".as_bytes().to_vec()
            ],
        ));

        let validation_errors = validate_package::<TestErrorFreeFileChecker>(
            &check,
            &pkg_merkle_string,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(validation_errors.len(), 1);
        match &validation_errors[0] {
            ValidationError::FailedToPerformPackageCheck { package_name: _, error: _ } => {}
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_package_check_file_validation_error() {
        // Create mocks with a valid package and .far, but file validation fails.
        // file_checks is not used since the file validation functionality is mocked, but
        // the vec must contain at least 1 element to execute the validation code path.
        let check = PackageCheckSpec {
            source: PackageSource::SystemImage,
            file_checks: vec![FileCheckSpec {
                source: FileSource::PackageMetaContents("sample/path".to_string()),
                state: FileState::Present,
                content_checks: None,
            }],
        };
        let pkg_merkle_string = "test_pkg_merkle".to_string();
        let pkg_far_contents =
            hashmap![ META_CONTENTS_PATH => "some/meta/contents/entry".as_bytes()];
        let pkg_far_bytes = create_package_far(pkg_far_contents);
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(hashmap![
                PathBuf::from_str(&pkg_merkle_string).unwrap() => pkg_far_bytes
            ]));
        // The mock file validator will return 4 errors to exercise the error handling code.
        // These 4 errors will not supply a package name to match the FileValidator's real impl.
        struct TestFileCheckerWithErrors;

        impl FileChecker for TestFileCheckerWithErrors {
            fn check_file(
                check: &FileCheckSpec,
                _package_far_reader: &mut FarReader<Box<dyn ReadSeek>>,
                _blobs_artifact_reader: &mut Box<dyn ArtifactReader>,
                _golden_files_dir: &str,
            ) -> Vec<ValidationError>
            where
                Self: Sized,
            {
                vec![
                    ValidationError::FailedToPerformFileCheck {
                        package_name: String::new(),
                        file_path: check.source.to_string(),
                        error: "some error message".to_string(),
                    },
                    ValidationError::UnexpectedFilePresence {
                        package_name: String::new(),
                        file_path: check.source.to_string(),
                    },
                    ValidationError::UnexpectedFilePresenceOrHasContents {
                        package_name: String::new(),
                        file_path: check.source.to_string(),
                    },
                    ValidationError::FailedToFindFile { file_paths: check.source.to_string() },
                ]
            }
        }

        let validation_errors = validate_package::<TestFileCheckerWithErrors>(
            &check,
            &pkg_merkle_string,
            &mut blobs_artifact_reader,
            "",
        );

        // Check that the errors returned by validate_package are the ones we constructed for the mock FileValidator.
        assert_eq!(validation_errors.len(), 4);
        match &validation_errors[0] {
            ValidationError::FailedToPerformFileCheck { package_name, file_path, error } => {
                // The validate_package method will inject the package name in the error returned.
                assert_eq!(*package_name, check.source.to_string());
                assert_eq!(*file_path, check.file_checks[0].source.to_string());
                assert_eq!(*error, "some error message".to_string());
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
        match &validation_errors[1] {
            ValidationError::UnexpectedFilePresence { package_name, file_path } => {
                // The validate_package method will inject the package name in the error returned.
                assert_eq!(*package_name, check.source.to_string());
                assert_eq!(*file_path, check.file_checks[0].source.to_string());
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
        match &validation_errors[2] {
            ValidationError::UnexpectedFilePresenceOrHasContents { package_name, file_path } => {
                // The validate_package method will inject the package name in the error returned.
                assert_eq!(*package_name, check.source.to_string());
                assert_eq!(*file_path, check.file_checks[0].source.to_string());
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
        match &validation_errors[3] {
            // validate_package should not modify the error messaging for this error.
            ValidationError::FailedToFindFile { file_paths } => {
                assert_eq!(*file_paths, check.file_checks[0].source.to_string());
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_package_check_success() {
        // Set up mocks to allow validate_package to run a file check without error.
        let check = PackageCheckSpec {
            source: PackageSource::SystemImage,
            file_checks: vec![FileCheckSpec {
                source: FileSource::PackageMetaContents("sample/path".to_string()),
                state: FileState::Present,
                content_checks: None,
            }],
        };
        let pkg_merkle_string = "test_pkg_merkle".to_string();
        let pkg_far_contents =
            hashmap![ META_CONTENTS_PATH => "some/meta/contents/entry".as_bytes()];
        let pkg_far_bytes = create_package_far(pkg_far_contents);
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(hashmap![
                PathBuf::from_str(&pkg_merkle_string).unwrap() => pkg_far_bytes
            ]));

        let validation_errors = validate_package::<TestErrorFreeFileChecker>(
            &check,
            &pkg_merkle_string,
            &mut blobs_artifact_reader,
            "",
        );

        assert!(validation_errors.is_empty());
    }

    #[test]
    fn test_package_check_empty_file_checks() {
        // Set up mocks to allow validate_package to run with no file checks.
        let check =
            PackageCheckSpec { source: PackageSource::SystemImage, file_checks: Vec::new() };
        let pkg_merkle_string = "test_pkg_merkle".to_string();
        let pkg_far_contents =
            hashmap![ META_CONTENTS_PATH => "some/meta/contents/entry".as_bytes()];
        let pkg_far_bytes = create_package_far(pkg_far_contents);
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(hashmap![
                PathBuf::from_str(&pkg_merkle_string).unwrap() => pkg_far_bytes
            ]));

        let validation_errors = validate_package::<TestErrorFreeFileChecker>(
            &check,
            &pkg_merkle_string,
            &mut blobs_artifact_reader,
            "",
        );

        assert!(validation_errors.is_empty());
    }

    #[test]
    fn test_resolve_file_meta_contents_finds_bytes() {
        // Set up a package with meta/contents containing a key-value pair for a file.
        // Set up blobs artifact reader to have the file present with bytes.
        // Verify resolve_file uses meta/contents info to find and read the blob's bytes.
        let file_name = "some/file";
        let file_merkle_string = "merkle";
        let file_contents_bytes = "some file contents".as_bytes();
        let source: FileSource = FileSource::PackageMetaContents(file_name.to_string());
        let meta_contents_file_contents = format!("{}={}", file_name, file_merkle_string);
        let pkg_far_contents =
            hashmap![ META_CONTENTS_PATH => meta_contents_file_contents.as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(hashmap![
                PathBuf::from_str(&file_merkle_string).unwrap() => file_contents_bytes.to_vec()
            ]));

        let (file_found, bytes_found) =
            resolve_package_file(&source, &mut pkg_far_reader, &mut blobs_artifact_reader).unwrap();

        assert_eq!(file_found, file_name.to_string());
        assert_eq!(bytes_found.unwrap(), file_contents_bytes.to_vec());
    }

    #[test]
    fn test_resolve_file_meta_contents_missing_returns_none() {
        // Set up a package with meta/contents that does not contain the contents we're looking for.
        // Set up blobs artifact reader to be empty.
        // Verify resolve_file returns None for bytes found, indicating missing file.
        let file_name = "some/file";
        let source = FileSource::PackageMetaContents(file_name.to_string());
        let pkg_far_contents =
            hashmap![ META_CONTENTS_PATH => "random/other/file=othermerkle".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let (file_found, bytes_found) =
            resolve_package_file(&source, &mut pkg_far_reader, &mut blobs_artifact_reader).unwrap();

        assert!(file_found.is_empty());
        assert!(bytes_found.is_none());
    }

    #[test]
    fn test_resolve_file_meta_contents_error() {
        // Set up a package with meta/contents that isn't parseable as key-value pairs.
        // This is one of several ways to trigger the error flow we want to exercise.
        let file_name = "some/file";
        let source = FileSource::PackageMetaContents(file_name.to_string());
        let pkg_far_contents =
            hashmap![ META_CONTENTS_PATH => "something that is not a key value pair".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let res = resolve_package_file(&source, &mut pkg_far_reader, &mut blobs_artifact_reader);

        assert!(res.is_err());
    }

    #[test]
    fn test_resolve_file_package_far_finds_bytes() {
        // Set up a package containing a file we want to find directly.
        // The blobs_artifact_reader does not participate in this flow and can be empty.
        let file_name = "some/file";
        let file_contents_bytes = "some file contents".as_bytes();
        let source = FileSource::PackageFar(vec![file_name.to_string()]);
        let pkg_far_contents = hashmap![ file_name => file_contents_bytes];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let (file_found, bytes_found) =
            resolve_package_file(&source, &mut pkg_far_reader, &mut blobs_artifact_reader).unwrap();

        assert_eq!(file_found, file_name.to_string());
        assert_eq!(bytes_found.unwrap(), file_contents_bytes.to_vec());
    }

    #[test]
    fn test_resolve_file_package_far_missing_returns_none() {
        // Set up a package for a file we want to find, but it does not contain it.
        // The blobs_artifact_reader does not participate in this flow and can be empty.
        let file_name = "some/file";
        let source = FileSource::PackageFar(vec![file_name.to_string()]);
        let pkg_far_contents = hashmap![ "some/other/file" => "misc contents".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let (file_found, bytes_found) =
            resolve_package_file(&source, &mut pkg_far_reader, &mut blobs_artifact_reader).unwrap();

        assert!(file_found.is_empty());
        assert!(bytes_found.is_none());
    }

    #[test]
    fn test_resolve_file_package_far_multiple_files_error() {
        // Set up a policy specifying multiple possible paths for a file.
        // Set up a package containing files for multiple of the possible paths. This should error.
        // The blobs_artifact_reader does not participate in this flow and can be empty.
        let file_name_one = "some/file";
        let file_name_two = "some/other/file";
        let source =
            FileSource::PackageFar(vec![file_name_one.to_string(), file_name_two.to_string()]);
        let pkg_far_contents = hashmap![ file_name_one => "misc contents".as_bytes(), file_name_two => "some other misc contents".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let res = resolve_package_file(&source, &mut pkg_far_reader, &mut blobs_artifact_reader);

        assert!(res.is_err());
    }

    #[test]
    fn test_validate_file_contents_not_string_readable() {
        let checks =
            ContentCheckSpec { must_contain: None, must_not_contain: None, matches_golden: None };
        // Invalid utf8 bytes from the from_utf8 documentation.
        let content_bytes = vec![0, 159, 146, 150];
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::FailedToParseContentsToString {
                content_source: reported,
                error: _,
            } => {
                // Check that the error reports the content source.
                assert_eq!(content_source, *reported)
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        };
    }

    #[test]
    fn test_validate_file_contents_invalid_json_error() {
        let expected_json_kvp =
            ContentType::JsonKeyValue("some_config_value".to_string(), "true".to_string());
        let checks = ContentCheckSpec {
            must_contain: Some(vec![expected_json_kvp]),
            must_not_contain: None,
            matches_golden: None,
        };
        let content_string = "some text that is not valid json";
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::FailedToParseContentsAsJson { content_source: reported, error: _ } => {
                // Check that the error reports the content source.
                assert_eq!(content_source, *reported)
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        };
    }

    #[test]
    fn test_validate_file_contents_must_contain_json_kvp_success() {
        let expected_json_kvp =
            ContentType::JsonKeyValue("some_config_value".to_string(), "true".to_string());
        let checks = ContentCheckSpec {
            must_contain: Some(vec![expected_json_kvp]),
            must_not_contain: None,
            matches_golden: None,
        };

        // A policy specifying "true" will match both the string and bool forms of json content.
        for content_string in [
            json!({"some_config_value": true}).to_string(),
            json!({"some_config_value": "true"}).to_string(),
        ]
        .into_iter()
        {
            let content_bytes = content_string.as_bytes().to_vec();
            let content_source = "content_source".to_string();

            let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

            assert_eq!(errors.len(), 0);
        }
    }

    #[test]
    fn test_validate_file_contents_must_contain_json_kvp_failure() {
        let expected_key = "some_config_value".to_string();
        let expected_value = "true".to_string();
        let expected_json_kvp =
            ContentType::JsonKeyValue(expected_key.clone(), expected_value.clone());
        let checks = ContentCheckSpec {
            must_contain: Some(vec![expected_json_kvp]),
            must_not_contain: None,
            matches_golden: None,
        };
        let content_string = json!({
            "some_config_value": false
        })
        .to_string();
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ContentMustContainsJsonKeyValueMissingOrIncorrect {
                expected_key: reported_key,
                expected_value: reported_value,
                content_source: reported,
            } => {
                assert_eq!(expected_key.to_string(), *reported_key);
                assert_eq!(expected_value.to_string(), *reported_value);
                assert_eq!(content_source, *reported)
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        };
    }

    #[test]
    fn test_validate_file_contents_must_contain_kvp_success() {
        let expected_key = "key";
        let expected_value = "value";
        let checks = ContentCheckSpec {
            must_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            must_not_contain: None,
            matches_golden: None,
        };
        let content_string = format!("{}={}", expected_key, expected_value);
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_contents_must_contain_kvp_failure() {
        let expected_key = "key";
        let expected_value = "value";
        let checks = ContentCheckSpec {
            must_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            must_not_contain: None,
            matches_golden: None,
        };
        // This should trigger the KeyMissing error.
        let content_string = format!("{}={}", "not_expected_key", "not_expected_value");
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ContentMustContainsKeyValueKeyMissing {
                expected_key: reported_key,
                expected_value: reported_value,
                content_source: reported_source,
            } => {
                assert_eq!(expected_key.to_string(), *reported_key);
                assert_eq!(expected_value.to_string(), *reported_value);
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_contents_must_contain_kvp_error() {
        let expected_key = "key";
        let expected_value = "value";
        let checks = ContentCheckSpec {
            must_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            must_not_contain: None,
            matches_golden: None,
        };
        // This should trigger the failure to parse error.
        let content_bytes = "something not a key value pair".as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::FailedToParseContentsAsKeyValueMap {
                content_source: reported_source,
                error: _,
            } => {
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_contents_must_contain_string_success() {
        let expected_string = "must be present";
        let checks = ContentCheckSpec {
            must_contain: Some(vec![ContentType::String(expected_string.to_string())]),
            must_not_contain: None,
            matches_golden: None,
        };
        let content_string = format!("some other text, {}, more text", expected_string);
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_contents_must_contain_string_failure() {
        let expected_string = "must be present";
        let checks = ContentCheckSpec {
            must_contain: Some(vec![ContentType::String(expected_string.to_string())]),
            must_not_contain: None,
            matches_golden: None,
        };
        let content_bytes =
            "some other text, not the magic string though, more text".as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ContentMustContainValueMissing {
                value,
                content_source: reported_source,
            } => {
                assert_eq!(expected_string, *value);
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_contents_must_not_contain_json_kvp_success() {
        let expected_key = "key";
        let expected_value = "value";
        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::JsonKeyValue(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            matches_golden: None,
        };
        let content_string = json!({
            "some_other_key": "some_other_value"
        })
        .to_string();
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_contents_must_not_contain_json_kvp_failure() {
        let expected_key = "key";
        let expected_value = "value";
        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::JsonKeyValue(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            matches_golden: None,
        };
        let content_string = json!({
            expected_key.to_string(): expected_value.to_string()
        })
        .to_string();

        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ContentMustNotContainsJsonHasKeyValue {
                expected_key: reported_key,
                expected_value: reported_value,
                content_source: reported,
            } => {
                assert_eq!(expected_key.to_string(), *reported_key);
                assert_eq!(expected_value.to_string(), *reported_value);
                assert_eq!(content_source, *reported)
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        };
    }

    #[test]
    fn test_validate_file_contents_must_not_contain_kvp_success() {
        let expected_key = "key";
        let expected_value = "value";
        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            matches_golden: None,
        };
        let content_string = format!("{}={}", "some_other_key", "some_other_value");
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_contents_must_not_contain_kvp_failure() {
        let expected_key = "key";
        let expected_value = "value";
        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            matches_golden: None,
        };
        let content_string = format!("{}={}", expected_key, expected_value);
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ContentMustNotContainsHasKeyValue {
                expected_key: reported_key,
                expected_value: reported_value,
                content_source: reported_source,
            } => {
                assert_eq!(expected_key.to_string(), *reported_key);
                assert_eq!(expected_value.to_string(), *reported_value);
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_contents_must_not_contain_kvp_error() {
        let expected_key = "key";
        let expected_value = "value";
        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::KeyValuePair(
                expected_key.to_string(),
                expected_value.to_string(),
            )]),
            matches_golden: None,
        };
        // This should trigger the failure to parse error.
        let content_bytes = "something not a key value pair".as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::FailedToParseContentsAsKeyValueMap {
                content_source: reported_source,
                error: _,
            } => {
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_contents_must_not_contain_string_success() {
        let expected_string = "must not be present";
        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::String(expected_string.to_string())]),
            matches_golden: None,
        };
        let content_bytes =
            "some other text, not the expected string, more text".as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_contents_must_not_contain_string_failure() {
        let expected_string = "must not be present";
        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: Some(vec![ContentType::String(expected_string.to_string())]),
            matches_golden: None,
        };
        let content_string = format!("some other text, {}, more text", expected_string);
        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(&checks, content_bytes, &content_source, "");

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ContentMustNotContainValuePresent {
                value,
                content_source: reported_source,
            } => {
                assert_eq!(expected_string, *value);
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_matches_golden_success() {
        let content_string = "some content
        another line of stuff
        third line";
        // Set up the golden file to have the same contents as expected contents.
        let mut golden_file = NamedTempFile::new().unwrap();
        golden_file.write_all(content_string.as_bytes()).unwrap();

        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: None,
            matches_golden: Some(vec![golden_file.path().display().to_string()]),
        };

        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(
            &checks,
            content_bytes,
            &content_source,
            std::env::temp_dir().to_str().unwrap(),
        );

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_matches_multiple_golden_success() {
        let content_string = "some content
        another line of stuff
        third line";

        // Set up a golden file to have the different than expected contents.
        let failing_golden_content = "some unexpected content";
        let mut failing_golden_file = NamedTempFile::new().unwrap();
        failing_golden_file.write_all(failing_golden_content.as_bytes()).unwrap();

        // Set up a golden file to have the same contents as expected contents.
        let mut golden_file = NamedTempFile::new().unwrap();
        golden_file.write_all(content_string.as_bytes()).unwrap();

        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: None,
            matches_golden: Some(vec![
                failing_golden_file.path().display().to_string(),
                golden_file.path().display().to_string(),
            ]),
        };

        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(
            &checks,
            content_bytes,
            &content_source,
            std::env::temp_dir().to_str().unwrap(),
        );

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_matches_golden_failure() {
        let content_string = "some content
        another line of stuff
        third line\n";

        // Set up the golden file to have expected contents plus some extra.
        let extra_golden_content = "extra expected content";
        let mut golden_file = NamedTempFile::new().unwrap();
        golden_file.write_all(content_string.as_bytes()).unwrap();
        golden_file.write_all(extra_golden_content.as_bytes()).unwrap();

        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: None,
            matches_golden: Some(vec![golden_file.path().display().to_string()]),
        };

        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(
            &checks,
            content_bytes,
            &content_source,
            std::env::temp_dir().to_str().unwrap(),
        );

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ContentGoldenFileMismatch {
                golden_path,
                diffs,
                content_source: reported_source,
            } => {
                assert_eq!(golden_file.path().display().to_string(), *golden_path);
                assert!(diffs.contains(extra_golden_content));
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_matches_multiple_golden_failure() {
        let content_string = "some content
        another line of stuff
        third line\n";

        // Set up the golden file to have expected contents plus some extra.
        let extra_golden_content = "extra expected content";
        let mut golden_file = NamedTempFile::new().unwrap();
        golden_file.write_all(content_string.as_bytes()).unwrap();
        golden_file.write_all(extra_golden_content.as_bytes()).unwrap();

        // Set up a second golden file with entirely unexpected contents.
        let second_golden_content = "random unrelated text";
        let mut second_golden_file = NamedTempFile::new().unwrap();
        second_golden_file.write_all(second_golden_content.as_bytes()).unwrap();

        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: None,
            matches_golden: Some(vec![
                golden_file.path().display().to_string(),
                second_golden_file.path().display().to_string(),
            ]),
        };

        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(
            &checks,
            content_bytes,
            &content_source,
            std::env::temp_dir().to_str().unwrap(),
        );

        assert_eq!(errors.len(), 2);
        match &errors[0] {
            ValidationError::ContentGoldenFileMismatch {
                golden_path,
                diffs,
                content_source: reported_source,
            } => {
                assert_eq!(golden_file.path().display().to_string(), *golden_path);
                assert!(diffs.contains(extra_golden_content));
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
        match &errors[1] {
            ValidationError::ContentGoldenFileMismatch {
                golden_path,
                diffs,
                content_source: reported_source,
            } => {
                assert_eq!(second_golden_file.path().display().to_string(), *golden_path);
                assert!(diffs.contains(second_golden_content));
                assert_eq!(content_source, *reported_source);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_matches_golden_error_loading_golden() {
        let content_string = "some content
        another line of stuff
        third line";

        let golden_file_name = "non_existent_file_path";
        let checks = ContentCheckSpec {
            must_contain: None,
            must_not_contain: None,
            matches_golden: Some(vec![golden_file_name.to_string()]),
        };

        let content_bytes = content_string.as_bytes().to_vec();
        let content_source = "content_source".to_string();

        let errors = validate_file_contents(
            &checks,
            content_bytes,
            &content_source,
            std::env::temp_dir().to_str().unwrap(),
        );

        assert_eq!(errors.len(), 1);
        // Verify the golden file directory and file name were combined.
        let path_searched =
            std::env::temp_dir().join(golden_file_name).to_string_lossy().to_string();
        match &errors[0] {
            ValidationError::FailedToOpenGoldenFile { golden_path, error: _ } => {
                assert_eq!(&path_searched, golden_path);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_json_contents_contains_parse_error() {
        let key = "key";
        let value = "value";
        let content_str = "non valid json";

        let res = json_contents_contain_key_value_pair(key, value, content_str);

        match res {
            Ok(_) => assert!(
                false,
                "Unexpectedly did not return error from attempting to parse invalid json"
            ),
            Err(e) => match e {
                ValidationError::FailedToParseContentsAsJson { content_source: _, error: _ } => {}
                e => assert!(false, "Unexpected error from failure or error case test: {}", e),
            },
        }
    }

    #[test]
    fn test_json_contents_contains_handles_u64_number_success() {
        let key = "key";
        // Must be able to parse value as u64.
        let value = "15";
        let content_str = json!({
            "key": 15
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str)
            .expect("failed to check json containing number value");

        assert!(res)
    }

    #[test]
    fn test_json_contents_contains_handles_u64_number_failure() {
        let key = "key";
        // Must be able to parse value as u64.
        let value = "32";
        let content_str = json!({
            "key": 15
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str)
            .expect("failed to check json containing number value");

        assert!(!res)
    }

    #[test]
    fn test_json_contents_contains_handles_i64_number_success() {
        let key = "key";
        // Must be able to parse value as i64. This means negative values in serde_json's definition.
        let value = "-15";
        let content_str = json!({
            "key": -15
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str)
            .expect("failed to check json containing number value");

        assert!(res)
    }

    #[test]
    fn test_json_contents_contains_handles_i64_number_failure() {
        let key = "key";
        // Must be able to parse value as i64. This means negative values in serde_json's definition.
        let value = "-32";
        let content_str = json!({
            "key": -15
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str)
            .expect("failed to check json containing number value");

        assert!(!res)
    }

    #[test]
    fn test_json_contents_contains_handles_f64_number_success() {
        let key = "key";
        // Must be able to parse value as f64.
        let value = "1.5";
        let content_str = json!({
            "key": 1.5
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str)
            .expect("failed to check json containing number value");

        assert!(res)
    }

    #[test]
    fn test_json_contents_contains_handles_f64_number_failure() {
        let key = "key";
        // Must be able to parse value as f64.
        let value = "3.245";
        let content_str = json!({
            "key": 1.5
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str)
            .expect("failed to check json containing number value");

        assert!(!res)
    }

    #[test]
    fn test_json_contents_contains_handles_number_type_mismatch() {
        // Policy has float, content has u64.
        let key = "key";
        let value = "3.245";
        let content_str = json!({
            "key": 15
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str);

        match res {
            Ok(_) => assert!(
                false,
                "Unexpectedly did not return error from attempting to compare different json types"
            ),
            Err(e) => match e {
                ValidationError::ContentAndPolicyJsonTypeMismatch { .. } => {}
                e => assert!(false, "Unexpected error from failure or error case test: {}", e),
            },
        }
    }

    #[test]
    fn test_json_contents_contains_handles_bool_type_mismatch() {
        // Policy expects something other than bool, content has bool.
        let key = "key";
        let value = "3.245";
        let content_str = json!({
            "key": true
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str);

        match res {
            Ok(_) => assert!(
                false,
                "Unexpectedly did not return error from attempting to compare different json types"
            ),
            Err(e) => match e {
                ValidationError::ContentAndPolicyJsonTypeMismatch { .. } => {}
                e => assert!(false, "Unexpected error from failure or error case test: {}", e),
            },
        }
    }

    #[test]
    fn test_json_contents_contains_does_not_handle_array() {
        let key = "key";
        let value = "value";
        let content_str = json!({
            "key": ["array", "of", "values"]
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str);

        match res {
            Ok(_) => assert!(
                false,
                "Unexpectedly did not return error from attempting to compare different json types"
            ),
            Err(e) => match e {
                ValidationError::UnableToHandleJsonContent { .. } => {}
                e => assert!(false, "Unexpected error from failure or error case test: {}", e),
            },
        }
    }

    #[test]
    fn test_json_contents_contains_does_not_handle_object() {
        let key = "key";
        let value = "value";
        let content_str = json!({
            "key": {
                "some_other_object_key": "value"
            }
        })
        .to_string();

        let res = json_contents_contain_key_value_pair(key, value, &content_str);

        match res {
            Ok(_) => assert!(
                false,
                "Unexpectedly did not return error from attempting to compare different json types"
            ),
            Err(e) => match e {
                ValidationError::UnableToHandleJsonContent { .. } => {}
                e => assert!(false, "Unexpected error from failure or error case test: {}", e),
            },
        }
    }

    #[test]
    fn test_validate_file_handles_resolve_error() {
        // Set up a package with meta/contents that isn't parseable as key-value pairs.
        // This is one of several ways to trigger the error flow we want to exercise.
        // This is similar to the test scoped to resolve_file, except is for validate_file.
        let file_name = "some/file";
        let source = FileSource::PackageMetaContents(file_name.to_string());
        let file_check = FileCheckSpec { source, state: FileState::Present, content_checks: None };
        let pkg_far_contents =
            hashmap![ META_CONTENTS_PATH => "something that is not a key value pair".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let errors = PackageFileChecker::check_file(
            &file_check,
            &mut pkg_far_reader,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::FailedToPerformFileCheck { package_name, file_path, error: _ } => {
                // validate_file does not know package_name, which is injected by the caller.
                assert!(package_name.is_empty());
                // The Display trait impl for the source adds indication that it is from `meta-contents`.
                assert_eq!(&format!("meta-contents: {}", file_name), file_path);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_absent_success() {
        // Set up policy to target a file directly in package far rather than meta contents.
        // This simplifies the test by allowing the blob reader to be empty.
        let file_name = "some/file";
        let source = FileSource::PackageFar(vec![file_name.to_string()]);
        let file_check = FileCheckSpec { source, state: FileState::Absent, content_checks: None };
        let pkg_far_contents = hashmap![ "not/the/file" => "contents".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let errors = PackageFileChecker::check_file(
            &file_check,
            &mut pkg_far_reader,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_absent_failure() {
        // Set up policy to target a file directly in package far rather than meta contents.
        // This simplifies the test by allowing the blob reader to be empty.
        let file_name: &str = "some/file";
        let source = FileSource::PackageFar(vec![file_name.to_string()]);
        let file_check = FileCheckSpec { source, state: FileState::Absent, content_checks: None };
        let pkg_far_contents = hashmap![ file_name => "contents".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let errors = PackageFileChecker::check_file(
            &file_check,
            &mut pkg_far_reader,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::UnexpectedFilePresence { package_name, file_path } => {
                assert!(package_name.is_empty());
                assert_eq!(file_name, file_path);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_absent_or_empty_success() {
        // Set up policy to target a file directly in package far rather than meta contents.
        // This simplifies the test by allowing the blob reader to be empty.
        let file_name = "some/file";
        let source = FileSource::PackageFar(vec![file_name.to_string()]);
        let file_check =
            FileCheckSpec { source, state: FileState::AbsentOrEmpty, content_checks: None };
        let pkg_far_contents = hashmap![ "not/the/file" => "".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let errors = PackageFileChecker::check_file(
            &file_check,
            &mut pkg_far_reader,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_absent_or_empty_failure() {
        // Set up policy to target a file directly in package far rather than meta contents.
        // This simplifies the test by allowing the blob reader to be empty.
        let file_name: &str = "some/file";
        let source = FileSource::PackageFar(vec![file_name.to_string()]);
        let file_check =
            FileCheckSpec { source, state: FileState::AbsentOrEmpty, content_checks: None };
        let pkg_far_contents = hashmap![ file_name => "contents".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let errors = PackageFileChecker::check_file(
            &file_check,
            &mut pkg_far_reader,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::UnexpectedFilePresenceOrHasContents { package_name, file_path } => {
                assert!(package_name.is_empty());
                assert_eq!(file_name, file_path);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }

    #[test]
    fn test_validate_file_present_success() {
        // Set up policy to target a file directly in package far rather than meta contents.
        // This simplifies the test by allowing the blob reader to be empty.
        let file_name = "some/file";
        let source = FileSource::PackageFar(vec![file_name.to_string()]);
        let file_check = FileCheckSpec { source, state: FileState::Present, content_checks: None };
        let pkg_far_contents = hashmap![ file_name => "contents".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let errors = PackageFileChecker::check_file(
            &file_check,
            &mut pkg_far_reader,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn test_validate_file_present_failure() {
        // Set up policy to target a file directly in package far rather than meta contents.
        // This simplifies the test by allowing the blob reader to be empty.
        let file_name = "some/file";
        let source = FileSource::PackageFar(vec![file_name.to_string()]);
        let file_check = FileCheckSpec { source, state: FileState::Present, content_checks: None };
        let pkg_far_contents = hashmap![ "some/other/file" => "contents".as_bytes()];
        let pkg_far = create_package_far(pkg_far_contents);
        let pkg_far_box: Box<dyn ReadSeek> = Box::new(Cursor::new(pkg_far));
        let mut pkg_far_reader: FarReader<Box<dyn ReadSeek>> = FarReader::new(pkg_far_box).unwrap();
        let mut blobs_artifact_reader: Box<dyn ArtifactReader> =
            Box::new(TestArtifactReader::new(HashMap::new()));

        let errors = PackageFileChecker::check_file(
            &file_check,
            &mut pkg_far_reader,
            &mut blobs_artifact_reader,
            "",
        );

        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::FailedToFindFile { file_paths } => {
                // The Display trait impl for the source adds indication that it is from `package-far`.
                assert_eq!(&format!("package-far: {}", file_name), file_paths);
            }
            e => assert!(false, "Unexpected error from failure or error case test: {}", e),
        }
    }
}
