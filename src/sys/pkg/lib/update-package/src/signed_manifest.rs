// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Support for parsing and generating signed manifests.

use crate::manifest::{OtaManifest, OtaManifestError, parse_ota_manifest};
use ota_manifest_proto::fuchsia::update::manifest as proto;
use prost::Message as _;
use ring::signature::{KeyPair as _, UnparsedPublicKey};
use zerocopy::byteorder::little_endian::U32;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The header of a Signed Manifest.
#[derive(Debug, PartialEq, Eq, FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct Header {
    magic: [u8; 4],
    version: U32,
    manifest_size: U32,
}

/// Magic bytes for the Signed Manifest: FuChsIA OTA Format.
pub const MAGIC: [u8; 4] = [0xfc, 0x1a, 0x07, 0xaf];

/// Version for the Signed Manifest.
pub const VERSION: u32 = 1;

/// Maximum allowed payload size for the `OtaManifest` portion (10 MiB).
pub const MAX_MANIFEST_SIZE: usize = 10 * 1024 * 1024;

/// Maximum allowed payload size for the `Signatures` portion (1 MiB).
pub const MAX_SIGNATURE_SIZE: usize = 1024 * 1024;

/// The default manifest key used to sign OTA manifests for development purposes.
pub const MANIFEST_DEV_KEY_PEM: &[u8] =
    include_bytes_from_working_dir::include_bytes_from_working_dir_env!("MANIFEST_DEV_KEY_PATH");

trait U32Ext {
    fn get_usize(&self) -> usize;
}

impl U32Ext for U32 {
    fn get_usize(&self) -> usize {
        const { assert!(usize::BITS >= u32::BITS) }
        self.get() as usize
    }
}

/// An error encountered while parsing or verifying a signed manifest.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum SignedManifestError {
    #[error("file truncated: file size {file_size} is less than required size {expected_size}")]
    Truncated { file_size: usize, expected_size: usize },

    #[error("invalid magic: {0:?}")]
    InvalidMagic([u8; 4]),

    #[error("unknown version: {0}")]
    UnknownVersion(u32),

    #[error("manifest size too large: {0} > {MAX_MANIFEST_SIZE}")]
    ManifestSizeTooLarge(usize),

    #[error("signature size too large: {0} > {MAX_SIGNATURE_SIZE}")]
    SignatureSizeTooLarge(usize),

    #[error("failed to deserialize signatures")]
    InvalidSignatures(#[source] prost::DecodeError),

    #[error("root signature verification failed")]
    RootSignatureVerificationFailed,

    #[error("manifest signature verification failed")]
    ManifestSignatureVerificationFailed,

    #[error("invalid manifest")]
    InvalidManifest(#[from] OtaManifestError),
}

/// The parsed contents of a `SignedManifest`.
pub struct RawManifest<'a> {
    /// The signed manifest version.
    pub version: u32,
    /// The unparsed OTA manifest payload.
    pub manifest_payload: &'a [u8],
    /// The signatures found in the signed manifest.
    pub signatures: proto::Signatures,
    /// The bytes that are signed: magic, version, manifest_size, and manifest_payload.
    pub signed_bytes: &'a [u8],
}

impl<'a> RawManifest<'a> {
    /// Verifies the signatures against the provided list of public keys.
    /// Returns `Ok(())` if at least one signature is valid.
    pub fn verify(
        &self,
        public_keys: &[UnparsedPublicKey<Vec<u8>>],
    ) -> Result<(), SignedManifestError> {
        if !public_keys.iter().any(|root_key| {
            root_key
                .verify(
                    &self.signatures.manifest_public_key,
                    &self.signatures.manifest_key_signature,
                )
                .is_ok()
        }) {
            return Err(SignedManifestError::RootSignatureVerificationFailed);
        }

        let manifest_public_key =
            UnparsedPublicKey::new(&ring::signature::ED25519, &self.signatures.manifest_public_key);
        match manifest_public_key.verify(self.signed_bytes, &self.signatures.manifest_signature) {
            Ok(()) => Ok(()),
            Err(ring::error::Unspecified) => {
                Err(SignedManifestError::ManifestSignatureVerificationFailed)
            }
        }
    }
}

/// Parse a `SignedManifest` without verifying its signatures or parsing its payload.
///
/// Returns the `RawManifest` on success.
pub fn parse_raw(bytes: &[u8]) -> Result<RawManifest<'_>, SignedManifestError> {
    let (header, rest) =
        Header::read_from_prefix(bytes).map_err(|_| SignedManifestError::Truncated {
            file_size: bytes.len(),
            expected_size: std::mem::size_of::<Header>(),
        })?;

    if header.magic != MAGIC {
        return Err(SignedManifestError::InvalidMagic(header.magic));
    }

    let version = header.version.get();
    if version != VERSION {
        return Err(SignedManifestError::UnknownVersion(version));
    }

    let manifest_size = header.manifest_size.get_usize();
    if manifest_size > MAX_MANIFEST_SIZE {
        return Err(SignedManifestError::ManifestSizeTooLarge(manifest_size));
    }

    let (manifest_payload, after_manifest) =
        rest.split_at_checked(manifest_size).ok_or_else(|| SignedManifestError::Truncated {
            file_size: bytes.len(),
            expected_size: std::mem::size_of::<Header>() + manifest_size,
        })?;

    let (signature_size_val, signature_bytes) =
        U32::read_from_prefix(after_manifest).map_err(|_| SignedManifestError::Truncated {
            file_size: bytes.len(),
            expected_size: std::mem::size_of::<Header>()
                + manifest_size
                + std::mem::size_of::<U32>(),
        })?;

    let signature_size = signature_size_val.get_usize();
    if signature_size > MAX_SIGNATURE_SIZE {
        return Err(SignedManifestError::SignatureSizeTooLarge(signature_size));
    }

    let (signature_payload, _) =
        signature_bytes.split_at_checked(signature_size).ok_or_else(|| {
            SignedManifestError::Truncated {
                file_size: bytes.len(),
                expected_size: std::mem::size_of::<Header>()
                    + manifest_size
                    + std::mem::size_of::<U32>()
                    + signature_size,
            }
        })?;

    let signatures_msg = proto::Signatures::decode(signature_payload)
        .map_err(SignedManifestError::InvalidSignatures)?;

    // The signed portion comprises the magic, version, manifest_size, and manifest bytes.
    let signed_bytes = &bytes[..std::mem::size_of::<Header>() + manifest_size];

    Ok(RawManifest { version, manifest_payload, signatures: signatures_msg, signed_bytes })
}

/// Parse and verify a `SignedManifest`.
///
/// Returns the parsed `OtaManifest` on success.
pub fn parse_and_verify(
    bytes: &[u8],
    public_keys: &[UnparsedPublicKey<Vec<u8>>],
) -> Result<OtaManifest, SignedManifestError> {
    let raw = parse_raw(bytes)?;
    let () = raw.verify(public_keys)?;
    Ok(parse_ota_manifest(raw.manifest_payload)?)
}

/// Helper function to generate valid `SignedManifest` bytes, using a provided manifest key
/// signature or signing the manifest public key with `manifest_key`.
pub fn generate(
    manifest: OtaManifest,
    manifest_key: &ring::signature::Ed25519KeyPair,
    key_signature: Option<&[u8]>,
) -> Result<Vec<u8>, SignedManifestError> {
    let manifest_bytes = manifest.serialize();
    if manifest_bytes.len() > MAX_MANIFEST_SIZE {
        return Err(SignedManifestError::ManifestSizeTooLarge(manifest_bytes.len()));
    }
    let manifest_size = manifest_bytes.len() as u32;

    let header =
        Header { magic: MAGIC, version: U32::new(VERSION), manifest_size: U32::new(manifest_size) };

    let mut signed_bytes = Vec::with_capacity(std::mem::size_of::<Header>() + manifest_bytes.len());
    signed_bytes.extend_from_slice(header.as_bytes());
    signed_bytes.extend_from_slice(&manifest_bytes);

    let manifest_signature = manifest_key.sign(&signed_bytes).as_ref().to_vec();
    let manifest_public_key = manifest_key.public_key().as_ref().to_vec();
    let manifest_key_signature = match key_signature {
        Some(sig) => sig.to_vec(),
        None => manifest_key.sign(&manifest_public_key).as_ref().to_vec(),
    };

    let signatures_msg =
        proto::Signatures { manifest_signature, manifest_public_key, manifest_key_signature };
    let signatures_bytes = signatures_msg.encode_to_vec();
    if signatures_bytes.len() > MAX_SIGNATURE_SIZE {
        return Err(SignedManifestError::SignatureSizeTooLarge(signatures_bytes.len()));
    }
    let signatures_size = signatures_bytes.len() as u32;

    let mut out = signed_bytes;
    out.extend_from_slice(U32::new(signatures_size).as_bytes());
    out.extend_from_slice(&signatures_bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    fn make_ota_manifest() -> OtaManifest {
        OtaManifest {
            product_bundle_version: "1.2.3.4".parse().unwrap(),
            board: "test-board".to_string(),
            epoch: 1,
            mode: crate::update_mode::UpdateMode::Normal,
            blob_base_url: "http://example.com".to_string(),
            images: vec![],
            blobs: vec![],
        }
    }

    fn make_keypair() -> ring::signature::Ed25519KeyPair {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn make_public_key(keypair: &ring::signature::Ed25519KeyPair) -> UnparsedPublicKey<Vec<u8>> {
        UnparsedPublicKey::new(&ring::signature::ED25519, keypair.public_key().as_ref().to_vec())
    }

    #[test]
    fn test_parse_and_verify_success() {
        let manifest_key = make_keypair();
        let manifest = make_ota_manifest();
        let bytes = generate(manifest.clone(), &manifest_key, None).unwrap();

        let trusted_keys = vec![make_public_key(&manifest_key)];
        let parsed = parse_and_verify(&bytes, &trusted_keys).unwrap();
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn test_parse_and_verify_wrong_magic() {
        let manifest_key = make_keypair();
        let manifest = make_ota_manifest();
        let mut bytes = generate(manifest, &manifest_key, None).unwrap();

        bytes[0] ^= 0xff;

        let trusted_keys = vec![make_public_key(&manifest_key)];
        let err = parse_and_verify(&bytes, &trusted_keys).unwrap_err();
        assert_matches!(err, SignedManifestError::InvalidMagic(_));
    }

    #[test]
    fn test_parse_and_verify_wrong_version() {
        let manifest_key = make_keypair();
        let manifest = make_ota_manifest();
        let mut bytes = generate(manifest, &manifest_key, None).unwrap();

        // Version starts at byte offset 4, length 4, little endian.
        bytes[4] ^= 0x01;

        let trusted_keys = vec![make_public_key(&manifest_key)];
        let err = parse_and_verify(&bytes, &trusted_keys).unwrap_err();
        assert_matches!(err, SignedManifestError::UnknownVersion(_));
    }

    #[test]
    fn test_parse_and_verify_truncated_header() {
        let trusted_keys = vec![];
        let bytes = vec![0; std::mem::size_of::<Header>() - 1];
        let err = parse_and_verify(&bytes, &trusted_keys).unwrap_err();
        assert_matches!(err, SignedManifestError::Truncated { .. });
    }

    #[test]
    fn test_parse_and_verify_truncated_signature() {
        let manifest_key = make_keypair();
        let manifest = make_ota_manifest();
        let mut bytes = generate(manifest, &manifest_key, None).unwrap();

        // Truncate the signature payload
        bytes.truncate(bytes.len() - 1);

        let trusted_keys = vec![make_public_key(&manifest_key)];
        let err = parse_and_verify(&bytes, &trusted_keys).unwrap_err();
        assert_matches!(err, SignedManifestError::Truncated { .. });
    }

    #[test]
    fn test_parse_and_verify_bad_root_signature() {
        let manifest_key = make_keypair();
        let root_key = make_keypair();
        let manifest = make_ota_manifest();
        let bad_sig = vec![0u8; 64];
        let bytes = generate(manifest, &manifest_key, Some(&bad_sig)).unwrap();

        let trusted_keys = vec![make_public_key(&root_key)];

        let err = parse_and_verify(&bytes, &trusted_keys).unwrap_err();
        assert_matches!(err, SignedManifestError::RootSignatureVerificationFailed);
    }

    #[test]
    fn test_parse_and_verify_bad_manifest_signature() {
        let manifest_key = make_keypair();
        let manifest = make_ota_manifest();
        let mut bytes = generate(manifest, &manifest_key, None).unwrap();

        // Corrupt one byte of the manifest payload (which starts after the header)
        let payload_start = std::mem::size_of::<Header>();
        bytes[payload_start] ^= 0xFF;

        let trusted_keys = vec![make_public_key(&manifest_key)];

        let err = parse_and_verify(&bytes, &trusted_keys).unwrap_err();
        assert_matches!(err, SignedManifestError::ManifestSignatureVerificationFailed);
    }

    #[test]
    fn test_generate_with_key_signature() {
        let manifest_key = make_keypair();
        let root_key = make_keypair();
        let manifest = make_ota_manifest();

        let manifest_public_key = manifest_key.public_key().as_ref();
        let manifest_key_sig = root_key.sign(manifest_public_key).as_ref().to_vec();

        let bytes = generate(manifest.clone(), &manifest_key, Some(&manifest_key_sig)).unwrap();

        let trusted_keys = vec![make_public_key(&root_key)];
        let parsed = parse_and_verify(&bytes, &trusted_keys).unwrap();
        assert_eq!(parsed, manifest);
    }
}
