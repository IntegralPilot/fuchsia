// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for GCE disk image packaging, serial number generation, and product bundle hashing.

use anyhow::{Context, Result};
use camino::Utf8Path;
use discovery::gce_watcher::write_file_atomically;
use flate2::Compression;
use flate2::write::GzEncoder;
use product_bundle::ProductBundle;
use rand::RngExt as _;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Default disk size for GCE full disk images (10 GiB).
pub const DEFAULT_GCE_DISK_SIZE: u64 = 10 * 1024 * 1024 * 1024;

/// Length of the hexadecimal prefix returned by [`compute_bundle_hash`].
pub const BUNDLE_HASH_HEX_LEN: usize = 12;

/// 48-bit mask for the final node identifier segment of a generated serial number.
const SERIAL_NODE_MASK: u64 = 0xffff_ffff_ffff;

/// Buffer size (64 KiB) used when streaming files into the SHA-256 hasher.
const HASH_BUFFER_SIZE: usize = 64 * 1024;

fn hash_file(path: &Path, hasher: &mut Sha256) -> Result<()> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let mut buffer = [0u8; HASH_BUFFER_SIZE];
    loop {
        let bytes_read =
            file.read(&mut buffer).with_context(|| format!("Failed to read {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(())
}

/// Generates a random serial number formatted for GCE instances (`GC-...`).
pub fn generate_serial_number() -> String {
    let mut rng = rand::rng();
    format!(
        "GC-{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        rng.random::<u32>(),
        rng.random::<u16>(),
        rng.random::<u16>(),
        rng.random::<u16>(),
        rng.random::<u64>() & SERIAL_NODE_MASK
    )
}

/// Computes a deterministic content hash of a Product Bundle to serve as a GCE image tag.
pub fn compute_bundle_hash(
    product_bundle_path: &Path,
    extra_entropy: Option<&[u8]>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    if let Some(extra) = extra_entropy {
        hasher.update(extra);
    }
    let manifest_path = product_bundle_path.join("product_bundle.json");
    if manifest_path.exists() {
        hash_file(&manifest_path, &mut hasher)?;
        let utf8_path = Utf8Path::from_path(product_bundle_path)
            .ok_or_else(|| anyhow::anyhow!("Product bundle path is not valid UTF-8"))?;
        let ProductBundle::V2(pb) = ProductBundle::try_load_from(utf8_path)
            .with_context(|| format!("Failed to load product bundle at {utf8_path}"))?;
        if let Some(system_a) = &pb.system_a {
            for image in system_a {
                let path = image.source().as_std_path();
                if path.exists() {
                    hash_file(path, &mut hasher)?;
                }
            }
        }
        for part in &pb.partitions.bootloader_partitions {
            let path = part.image.as_std_path();
            if path.exists() {
                hash_file(path, &mut hasher)?;
            }
        }
    } else {
        hasher.update(product_bundle_path.to_string_lossy().as_bytes());
    }
    let result = hasher.finalize();
    let mut hex_str = String::with_capacity(BUNDLE_HASH_HEX_LEN);
    for &b in &result[..BUNDLE_HASH_HEX_LEN / 2] {
        let _ = write!(hex_str, "{:02x}", b);
    }
    Ok(hex_str)
}

/// Packages a raw disk file into a GCE-compatible `.tar.gz` archive containing `disk.raw`.
pub fn package_gce_tar_gz(raw_disk_path: &Path, output_tar_gz: &Path) -> Result<()> {
    let mut disk_file = File::open(raw_disk_path)
        .with_context(|| format!("Failed to open raw disk file at {}", raw_disk_path.display()))?;

    write_file_atomically(output_tar_gz, |writer| -> Result<()> {
        let enc = GzEncoder::new(writer, Compression::fast());
        let mut tar = tar::Builder::new(enc);
        tar.append_file("disk.raw", &mut disk_file)
            .context("Failed to append disk.raw to tar archive")?;
        let enc = tar.into_inner().context("Failed to finalize tar archive")?;
        enc.finish().context("Failed to finish gzip encoding")?;
        Ok(())
    })
    .with_context(|| format!("Failed to write output archive at {}", output_tar_gz.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_generate_serial_number() {
        let s1 = generate_serial_number();
        let s2 = generate_serial_number();
        assert!(s1.starts_with("GC-"));
        assert!(s2.starts_with("GC-"));
        assert_ne!(s1, s2);
    }

    #[fuchsia::test]
    fn test_compute_bundle_hash_determinism() {
        let temp = tempfile::tempdir().unwrap();
        let hash1 = compute_bundle_hash(temp.path(), None).unwrap();
        let hash2 = compute_bundle_hash(temp.path(), None).unwrap();
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 12);

        let hash_with_keys =
            compute_bundle_hash(temp.path(), Some(b"ssh-ed25519 AAAAC3... test_key")).unwrap();
        assert_ne!(hash1, hash_with_keys);
        assert_eq!(hash_with_keys.len(), 12);
    }

    #[fuchsia::test]
    fn test_compute_bundle_hash_invalid_manifest_propagates_error() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("product_bundle.json"), b"not valid json").unwrap();
        let res = compute_bundle_hash(temp.path(), None);
        assert!(res.is_err());
    }

    #[fuchsia::test]
    fn test_package_gce_tar_gz() {
        let temp = tempfile::tempdir().unwrap();
        let raw_disk = temp.path().join("disk.raw");
        std::fs::write(&raw_disk, b"fake raw disk content").unwrap();

        let tar_gz = temp.path().join("disk.tar.gz");
        package_gce_tar_gz(&raw_disk, &tar_gz).unwrap();
        assert!(tar_gz.exists());

        let tar_gz_file = File::open(&tar_gz).unwrap();
        let dec = flate2::read::GzDecoder::new(tar_gz_file);
        let mut archive = tar::Archive::new(dec);
        let mut entries = archive.entries().unwrap();
        let mut entry = entries.next().unwrap().unwrap();
        assert_eq!(entry.path().unwrap(), Path::new("disk.raw"));
        let mut content = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut content).unwrap();
        assert_eq!(content, b"fake raw disk content");
    }

    #[fuchsia::test]
    fn test_package_gce_tar_gz_does_not_leave_corrupt_file_on_error() {
        let temp = tempfile::tempdir().unwrap();
        let missing_raw_disk = temp.path().join("missing.raw");
        let tar_gz = temp.path().join("disk.tar.gz");

        let res = package_gce_tar_gz(&missing_raw_disk, &tar_gz);
        assert!(res.is_err());
        assert!(!tar_gz.exists());
    }
}
