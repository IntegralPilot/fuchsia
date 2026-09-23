// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Provides a safe truncated byte slice view of a VMO via a `Deref<Target=[u8]>` implementation.
#[derive(Debug)]
pub struct TruncatedImmutableMapping {
    mapping: crate::ImmutableMapping,
    len: usize,
}

impl TruncatedImmutableMapping {
    /// Maps the `vmo` with [`crate::ImmutableMapping::create_from_vmo`].
    ///
    /// The slice returned by the `Deref` implementation will be truncated to `len` instead of the
    /// size of `vmo`.
    ///
    /// Errors if `len` is longer than the size of `vmo`.
    pub fn create_from_vmo(
        vmo: &zx::Vmo,
        immediately_page: bool,
        len: usize,
    ) -> Result<Self, Error> {
        let mapping = crate::ImmutableMapping::create_from_vmo(vmo, immediately_page)?;
        if mapping.len() < len {
            return Err(Error::VmoSmallerThanLen { vmo_len: mapping.len(), truncated_len: len });
        }
        Ok(Self { mapping, len })
    }
}

impl std::ops::Deref for TruncatedImmutableMapping {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        &self.mapping[..self.len]
    }
}

/// Error type for `TruncatedImmutableMapping::create_from_vmo`.
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to create immutable mapping")]
    CreateImmutableMapping(#[from] crate::ImmutableMappingError),

    #[error("the VMO is smaller than the requested len, {vmo_len} < {truncated_len}")]
    VmoSmallerThanLen { vmo_len: usize, truncated_len: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_len() {
        let vmo = zx::Vmo::create(1).unwrap();
        let too_long = vmo.get_size().unwrap() + 1;
        std::assert_matches!(
            TruncatedImmutableMapping::create_from_vmo(&vmo, false, too_long.try_into().unwrap()),
            Err(Error::VmoSmallerThanLen { .. })
        );
    }

    #[test]
    fn respects_len() {
        let vmo = zx::Vmo::create(30).unwrap();
        let () = vmo.write(b"the-content", 0).unwrap();

        let mapping = TruncatedImmutableMapping::create_from_vmo(&vmo, false, 3).unwrap();

        assert_eq!(&mapping[..], b"the");
    }
}
