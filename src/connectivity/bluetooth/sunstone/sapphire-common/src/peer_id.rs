// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::num::NonZero;

/// A unique identifier for a remote peer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId(NonZero<u64>);

impl PeerId {
    /// Creates a new `PeerId` with the given value.
    pub const fn new(value: u64) -> Option<Self> {
        if let Some(nz) = NonZero::new(value) { Some(Self(nz)) } else { None }
    }

    /// Returns the raw 64-bit value of this peer ID.
    pub const fn value(self) -> u64 {
        self.0.get()
    }
}

/// Error returned when trying to create a `PeerId` from an invalid value.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InvalidPeerId;

impl TryFrom<u64> for PeerId {
    type Error = InvalidPeerId;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidPeerId)
    }
}

impl From<NonZero<u64>> for PeerId {
    fn from(value: NonZero<u64>) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_id() {
        let peer_id = PeerId::new(1).unwrap();
        assert_eq!(peer_id.value(), 1);
        assert!(PeerId::new(0).is_none());
    }

    #[test]
    fn test_peer_id_try_from_u64() {
        let peer_id = PeerId::try_from(1u64);
        assert_eq!(peer_id, Ok(PeerId::new(1).unwrap()));

        let peer_id_err = PeerId::try_from(0u64);
        assert_eq!(peer_id_err, Err(InvalidPeerId));
    }

    #[test]
    fn test_peer_id_from_nonzero_u64() {
        let value = core::num::NonZero::new(1u64).unwrap();
        let peer_id = PeerId::from(value);
        assert_eq!(peer_id.value(), 1);
    }
}
