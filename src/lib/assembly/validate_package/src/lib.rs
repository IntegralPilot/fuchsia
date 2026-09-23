// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use assembly_validate_util::PkgNamespace;
use fuchsia_pkg::PackageManifest;
use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::path::PathBuf;
use version_history::AbiRevision;

/// Validate a package's contents.
///
/// Assumes that all component manifests will be in the `meta/` directory and have a `.cm` extension
/// within the package namespace.
pub fn validate_package(manifest: &PackageManifest) -> Result<(), PackageValidationError> {
    let blobs = manifest.blobs();

    // read meta.far contents
    let meta_far_info =
        blobs.iter().find(|b| b.path == "meta/").ok_or(PackageValidationError::MissingMetaFar)?;
    let meta_far =
        File::open(&meta_far_info.source_path).map_err(|source| PackageValidationError::Open {
            source,
            path: PathBuf::from(meta_far_info.source_path.clone()),
        })?;
    let mut reader =
        fuchsia_archive::Utf8Reader::new(meta_far).map_err(PackageValidationError::ReadArchive)?;

    // validate components in the meta/ directory
    let mut errors = BTreeMap::new();
    for path in reader.paths().into_iter().filter(|p| p.ends_with(".cm")) {
        if let Err(e) = validate_component(&path, &mut reader) {
            errors.insert(path, e);
        }
    }
    if !errors.is_empty() {
        return Err(PackageValidationError::InvalidComponents(errors));
    }

    // validate the abi_revision of the package
    let raw_abi_revision = reader
        .read_file(fuchsia_pkg::ABI_REVISION_FILE_PATH)
        .map_err(|e| PackageValidationError::MissingAbiRevisionFile(e))?;
    let abi_revision = AbiRevision::try_from(raw_abi_revision.as_slice())
        .map_err(|e| PackageValidationError::InvalidAbiRevisionFile(e))?;
    // Due to exceptional technical constraints, `bt-host` is the only example
    // of a platform component which is not built as part of the platform build,
    // and therefore cannot pass the ABI revision checks.
    //
    // This is not a generic opt-out mechanism for ABI revision validation. It's
    // specifically for platform components that are built out-of-tree, of which
    // there should only be one.
    //
    // If a specific product wants to use an out-of-tree package with an invalid
    // ABI revision, that should be specified in an allowlist in the product
    // configuration.
    //
    // TODO(https://fxbug.dev/398204128): Add a package allowlist to product
    // configuration.
    if manifest.name().as_ref() == "bt-host" {
        Ok(())
    } else {
        version_history_data::HISTORY
            .check_abi_revision_for_runtime(abi_revision)
            .map_err(PackageValidationError::UnsupportedAbiRevision)
    }
}

/// Validate an individual component within the package.
pub fn validate_component(
    manifest_path: &str,
    pkg_namespace: &mut impl PkgNamespace,
) -> anyhow::Result<()> {
    assembly_structured_config::validate_component(manifest_path, pkg_namespace)
        .context("Validating structured configuration")?;
    Ok(())
}

/// Failures that can occur when validating packages.
#[derive(Debug)]
pub enum PackageValidationError {
    Open { path: PathBuf, source: std::io::Error },
    LoadPackageManifest(anyhow::Error),
    MissingMetaFar,
    ReadArchive(fuchsia_archive::Error),
    InvalidComponents(BTreeMap<String, anyhow::Error>),
    MissingAbiRevisionFile(fuchsia_archive::Error),
    InvalidAbiRevisionFile(std::array::TryFromSliceError),
    UnsupportedAbiRevision(version_history::AbiRevisionError),
}

impl fmt::Display for PackageValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use PackageValidationError::*;
        match self {
            Open { path, source } => write!(f, "Unable to open `{}`: {}.", path.display(), source),
            LoadPackageManifest(source) => {
                write!(f, "Unable to decode JSON for package manifest: {}.", source)
            }
            MissingMetaFar => write!(f, "The package seems to be missing a meta/ directory."),
            ReadArchive(source) => write!(f, "Unable to read the package's meta.far: {}.", source),
            InvalidComponents(components) => {
                for (name, error) in components {
                    write!(f, "\n└── {}: {}", name, error)?;
                    let mut source = error.source();
                    while let Some(s) = source {
                        write!(f, "\n    └── {}", s)?;
                        source = s.source();
                    }
                }
                Ok(())
            }
            MissingAbiRevisionFile(cause) => {
                write!(
                    f,
                    "The package seems to be missing an abi revision file:  {}",
                    fuchsia_pkg::ABI_REVISION_FILE_PATH
                )?;
                write!(f, "\n└── {}", cause)?;
                Ok(())
            }
            InvalidAbiRevisionFile(cause) => {
                write!(f, "The package abi revision file was not valid")?;
                write!(f, "\n└── {cause}")?;
                Ok(())
            }
            UnsupportedAbiRevision(version_history::AbiRevisionError::TooNew {
                abi_revision,
                supported_versions,
            }) => write!(
                f,
                "Package targets an unknown target ABI revision: 0x{}.

Your Fuchsia SDK is probably too old to support it. Either update your
SDK to a newer version, or target one of the following supported API levels
when building this package:{}",
                abi_revision, supported_versions
            ),
            UnsupportedAbiRevision(version_history::AbiRevisionError::PlatformMismatch {
                abi_revision,
                package_date,
                package_commit_hash,
                platform_date,
                platform_commit_hash,
            }) => {
                write!(
                    f,
                    "Package targets an unsupported platform ABI revision: 0x{}

Assembly inputs must come from *exactly* the same Fuchsia version as the SDK.
",
                    abi_revision
                )?;
                if package_date == platform_date {
                    write!(
                        f,
                        "
Your assembly inputs and SDK appear to be both from the same day ({}), but
from different `integration.git` repos (Git revision prefix {:#X} vs {:#X}).
",
                        package_date, package_commit_hash, platform_commit_hash
                    )?;
                } else {
                    write!(
                        f,
                        "
Your assembly inputs are from: {}
Your Fuchsia SDK is from: {}
",
                        package_date, platform_date
                    )?;
                }

                write!(
                    f,
                    "
You probably updated your SDK without updating your assembly inputs, or vice
versa. Ensure your SDK and assembly inputs come from the same release and try
again."
                )
            }
            UnsupportedAbiRevision(version_history::AbiRevisionError::UnstableMismatch {
                abi_revision,
                package_sdk_date,
                package_sdk_commit_hash,
                platform_date,
                platform_commit_hash,
                supported_versions,
            }) => {
                write!(
                    f,
                    "Package targets an unsupported unstable ABI revision: 0x{}.

Packages that target unstable API levels like HEAD and NEXT must be built and
assembled by *exactly* the same version of the SDK.
",
                    abi_revision
                )?;

                if package_sdk_date == platform_date {
                    write!(
                        f,
                        "
The SDK that built the package comes from the same day as this SDK ({}), but
they appear to come from different `integration.git` repos (Git revision prefix
{:#X} vs {:#X}).
",
                        package_sdk_date, package_sdk_commit_hash, platform_commit_hash
                    )?;
                } else {
                    write!(
                        f,
                        "
The SDK that built the package is from: {}
Your Fuchsia SDK is from: {}
",
                        package_sdk_date, platform_date
                    )?;
                }
                write!(
                    f,
                    "
Ensure that the build and assembly steps use exactly the same SDK and try
again, or rebuild the package targeting one of the following stable API
levels:{}",
                    supported_versions
                )
            }
            UnsupportedAbiRevision(version_history::AbiRevisionError::Retired {
                version,
                supported_versions,
            }) => write!(
                f,
                "Package targets retired API level {} (0x{}).

API level {} has been retired and is no longer supported. The package's owner
must update it to target one of the following API levels:{}",
                version.api_level, version.abi_revision, version.api_level, supported_versions
            ),
            UnsupportedAbiRevision(version_history::AbiRevisionError::Invalid) => write!(
                f,
                "The package was marked with the 'Invalid' ABI revision:
0x{}. This is unusual and probably represents a bug.",
                AbiRevision::INVALID
            ),
            UnsupportedAbiRevision(version_history::AbiRevisionError::Malformed {
                abi_revision,
            }) => write!(
                f,
                "The package was marked with an unrecognized 'special'
ABI revision 0x{}.

This is unusual. The ABI revision may be malformed, or your Fuchsia SDK may be
too old to know what this means. Consider updating your SDK to a newer
version.",
                abi_revision
            ),
        }
    }
}
