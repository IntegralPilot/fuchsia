// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg as fpkg;
use hmac::{KeyInit as _, Mac as _};

/// Creates and authenticates `fidl_fuchsia_pkg::ResolutionContext`s using an HMAC.
/// The contexts contain the hash of the superpackage.
#[derive(Clone, Debug)]
pub struct ContextAuthenticator {
    hmac: hmac::Hmac<sha2::Sha256>,
}

/// The size, in bytes, of the HMAC secret key.
const SECRET_LEN: usize = 64;
/// The size, in bytes, of the HMAC tag.
const TAG_LEN: usize = 32;

impl ContextAuthenticator {
    /// Create a ContextAuthenticator initialized with a random secret key.
    pub fn new() -> Self {
        let mut secret = [0; SECRET_LEN];
        let () = rand::fill(&mut secret[..]);
        Self::from_secret(secret)
    }

    fn from_secret(secret: [u8; SECRET_LEN]) -> Self {
        Self { hmac: hmac::Hmac::<sha2::Sha256>::new((&secret).into()) }
    }

    /// Create a `fidl_fuchsia_pkg::ResolutionContext`, tagged by this `ContextAuthenticator`'s
    /// secret key, capable of being authenticated by `self.authenticate(context)`.
    pub fn create(mut self, hash: &fuchsia_hash::Hash) -> fpkg::ResolutionContext {
        let () = self.hmac.update(hash.as_bytes());
        let mut bytes = self.hmac.finalize().into_bytes().to_vec();
        bytes.extend_from_slice(hash.as_bytes());
        fpkg::ResolutionContext { bytes }
    }

    /// Authenticate a `fidl_fuchsia_pkg::ResolutionContext` and return the wrapped
    /// `fuchsia_hash::Hash`.
    pub fn authenticate(
        mut self,
        context: fpkg::ResolutionContext,
    ) -> Result<fuchsia_hash::Hash, ContextAuthenticatorError> {
        let context: &[u8; TAG_LEN + fuchsia_hash::HASH_SIZE] = context
            .bytes
            .as_slice()
            .try_into()
            .map_err(|_| ContextAuthenticatorError::InvalidLength(context.bytes.len()))?;
        let (tag, hash) = context.split_at(TAG_LEN);
        let () = self.hmac.update(hash);
        let () =
            self.hmac.verify_slice(tag).map_err(ContextAuthenticatorError::AuthenticationFailed)?;
        // This will never fail, but need a way to infallibly split an array reference into two
        // array references to communicate that to the type system.
        fuchsia_hash::Hash::try_from(hash)
            .map_err(|_| ContextAuthenticatorError::InvalidLength(context.len()))
    }
}

impl Default for ContextAuthenticator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ContextAuthenticatorError {
    #[error("expected context length {} found {}", TAG_LEN + fuchsia_hash::HASH_SIZE, .0)]
    InvalidLength(usize),

    #[error("authentication failed")]
    AuthenticationFailed(#[source] hmac::digest::MacError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    fn hash() -> fuchsia_hash::Hash {
        [0; 32].into()
    }

    #[fuchsia::test]
    fn tag_len() {
        let authenticator = ContextAuthenticator::new();
        assert_eq!(authenticator.hmac.finalize().into_bytes().len(), TAG_LEN);
    }

    #[fuchsia::test]
    fn success() {
        let authenticator = ContextAuthenticator::from_secret([0u8; SECRET_LEN]);
        let context = authenticator.clone().create(&hash());
        assert_eq!(authenticator.authenticate(context).unwrap(), hash());
    }

    #[fuchsia::test]
    fn invalid_context_length() {
        let authenticator = ContextAuthenticator::from_secret([0u8; SECRET_LEN]);
        assert_matches!(
            authenticator.authenticate(fpkg::ResolutionContext { bytes: vec![] }),
            Err(ContextAuthenticatorError::InvalidLength(0))
        );
    }

    #[fuchsia::test]
    fn authentication_fails() {
        let authenticator = ContextAuthenticator::from_secret([0u8; SECRET_LEN]);
        let mut context = authenticator.clone().create(&hash());
        context.bytes[0] = !context.bytes[0];
        assert_matches!(
            authenticator.authenticate(context),
            Err(ContextAuthenticatorError::AuthenticationFailed(_))
        );
    }
}
