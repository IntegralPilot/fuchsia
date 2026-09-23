// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::superblock::BLOCK_SIZE;
use anyhow::Error;
use fidl_fuchsia_io as fio;
use fsverity_merkle::FsVerityDescriptor as DecodedDescriptor;

/// Found in the last block of fsverity data.
pub struct FsVerityDescriptor<'a> {
    pub file_size: u64,
    pub algorithm: fio::HashAlgorithm,
    descriptor: DecodedDescriptor<'a>,
}

impl<'a> FsVerityDescriptor<'a> {
    /// Parses out the descriptor from bytes.
    pub fn from_bytes(data: &'a [u8]) -> Result<Self, Error> {
        let descriptor = DecodedDescriptor::new(data, BLOCK_SIZE)?;

        Ok(Self {
            file_size: descriptor.file_size() as u64,
            algorithm: descriptor.digest_algorithm(),
            descriptor,
        })
    }

    pub fn root(&self) -> &[u8] {
        self.descriptor.root_digest()
    }

    pub fn salt(&self) -> &[u8] {
        self.descriptor.salt()
    }

    /// Create a fuchsia.io VerificationOptions to match this descriptor.
    pub fn fio_verification_options(&self) -> fio::VerificationOptions {
        fio::VerificationOptions {
            hash_algorithm: Some(self.algorithm),
            salt: Some(self.descriptor.salt().to_vec()),
            ..Default::default()
        }
    }
}
