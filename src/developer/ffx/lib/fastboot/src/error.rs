// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FfxFastbootError {
    #[error("Fastboot interface error: {0}")]
    Interface(#[from] ffx_fastboot_interface::fastboot_interface::FastbootError),

    #[error("Flash manifest error: {0}")]
    FlashManifest(#[from] ffx_flash_manifest::FlashManifestError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to open file {path}: {source}")]
    FileOpen { path: PathBuf, source: std::io::Error },

    #[error("Failed to get metadata for file {path}: {source}")]
    FileMetadata { path: PathBuf, source: std::io::Error },

    #[error("File read ended early: expected {expected} bytes, got {actual}")]
    FileRead { expected: u64, actual: u64 },

    #[error("Sparse image error: {0}")]
    Sparse(#[from] sparse::SparseError),

    #[error("Async task join error: {0}")]
    JoinError(#[from] tokio::task::JoinError),

    #[error(
        "Hardware mismatch! Trying to flash images built for '{}' but found '{}'",
        match attempted_products.is_empty() {
            true => expected.to_string(),
            false => format!("hw-revision {expected} / product {attempted_products:?}")
        },
        match attempted_products.is_empty() {
            true => found.to_string(),
            false => format!("hw-revision {found} / product {found_product:?}")
        }
    )]
    HardwareMismatch {
        expected: String,
        found: String,
        attempted_products: Vec<String>,
        found_product: Option<String>,
    },

    #[error("Could not verify hardware revision of target device")]
    HardwareVerificationFailure,

    #[error("Failed to canonicalize path {path}: {source}")]
    CanonicalizePath { path: PathBuf, source: std::io::Error },

    #[error("SDK context error: {0}")]
    SdkContext(#[from] ffx_config::environment::ContextError),

    #[error("Failed to open zip archive: {0}")]
    ZipArchiveOpen(#[source] zip::result::ZipError),

    #[error("Failed to read zip archive entries: {0}")]
    ZipArchiveRead(#[source] zip::result::ZipError),

    #[error("Failed to load product bundle from PBMS: {0}")]
    LoadProductBundlePbms(#[from] pbms::PbmsError),

    #[error("Failed to load product bundle: {0}")]
    ProductBundleLoad(#[source] product_bundle::ProductBundleLoadError),

    #[error(
        "Attempting to flash using a Product Bundle file with unsupported extension: {extension}"
    )]
    UnsupportedProductBundleExtension { extension: String },

    #[error(
        "Device returned a file with invalid unlock challenge length. Expected {expected} bytes but found {found} bytes."
    )]
    InvalidUnlockChallengeLength { expected: u64, found: u64 },

    #[error("Failed to decode base64 key: {0}")]
    CryptoKeyBase64Decode(#[from] base64::DecodeError),

    #[error("Failed to parse RSA private key: {0:?}")]
    CryptoKeyRejected(ring::error::KeyRejected),

    #[error("Invalid certificate length. Expected {expected} bytes but found {found} bytes.")]
    InvalidCertificateLength { expected: usize, found: u64 },

    #[error("No credentials given. Could not unlock device.")]
    NoCredentialsGiven,

    #[error("Key mismatch. Credentials given could not unlock the device.")]
    UnlockKeyMismatch,

    #[error("Could not sign unlocking keys: {0:?}")]
    CryptoSigningError(ring::error::Unspecified),

    #[error("Could not unlock device.")]
    UnlockFailed,

    #[error("Could not reboot device.")]
    ContinueBootFailed(#[source] ffx_fastboot_interface::fastboot_interface::FastbootError),

    #[error("Manifest does not contain product: {0}")]
    MissingProduct(String),

    #[error("Could not find matching partition {missing} for slot {slot}")]
    MatchingPartitionsNotFound { slot: String, missing: String },

    #[error(
        "This manifest does not support unlocking target devices. \nPlease update to a newer version of manifest and try again."
    )]
    UnlockNotSupported,

    #[error("The product requires the target to be unlocked. Please unlock target and try again.")]
    UnlockRequired,

    #[error("Did not receive reboot signal")]
    RebootSignalMissing,

    #[error("Manifest \"{path}\" is not a file.")]
    ManifestNotAFile { path: PathBuf },

    #[error("Unknown SDK type")]
    UnknownSdkType,

    #[error("Messenger error: {0}")]
    Messenger(#[from] tokio::sync::mpsc::error::SendError<crate::util::Event>),

    #[error("Integer conversion error: {0}")]
    Conversion(#[from] std::num::TryFromIntError),

    #[error("Integer parse error: {0}")]
    IntegerParse(#[from] std::num::ParseIntError),

    #[error("manifest or product_bundle must be specified")]
    ManifestOrProductBundleRequired,

    #[error(
        "Please supply the `--product-bundle` option to identify which product bundle to flash"
    )]
    ProductBundleRequired,

    #[error(
        "The flash manifest is missing the credential files to unlock this device.\nPlease unlock the target and try again."
    )]
    MissingCredentials,

    #[error("Target is already unlocked.")]
    AlreadyUnlocked,

    #[error("Only UTF-8 strings are currently supported for paths")]
    NonUtf8Path,

    #[error("Could not get file to upload: no parent directory")]
    NoParentDirectory,

    #[error("File not found in archive: {file}")]
    FileNotFoundInArchive { file: String },

    #[error("Invalid temporary file name")]
    InvalidTempFileName,

    #[error("Target is already locked.")]
    AlreadyLocked,

    #[error("Cannot lock ephemeral devices. Reboot the device to unlock.")]
    CannotLockEphemeral,

    #[error("Invalid tar archive")]
    InvalidTarArchive,

    #[error("Could not locate flash manifest in archive: {path}")]
    FlashManifestNotFoundInArchive { path: PathBuf },

    #[error("Invalid archive file name")]
    InvalidArchiveFileName,

    #[error("Upload prefix '{prefix}' is too large for the max command size ({max_len})")]
    InlineUploadOverflow { prefix: String, max_len: usize },

    // TODO(b/563409803) Add more helpful fields or discriminants to
    // FfxFastbootError::StreamingFlash
    #[error("Fastboot streaming flash error: {message}")]
    StreamingFlash { message: String },

    #[error("File path '{path}' is outside the allowed directory '{root}'")]
    PathOutsideDirectory { path: PathBuf, root: PathBuf },
}
