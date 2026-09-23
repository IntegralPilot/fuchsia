// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A FutexAddress is a more limited form of UserAddress. FutexAddress values must be aligned
// to a 4 byte boundary and must be within the restricted address space range.

use starnix_uapi::errors::{Errno, error};
use starnix_uapi::restricted_aspace::RESTRICTED_ASPACE_RANGE;
use starnix_uapi::user_address::UserAddress;
use std::fmt;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};
use zx::sys::zx_vaddr_t;

#[derive(
    Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd, IntoBytes, KnownLayout, FromBytes, Immutable,
)]
#[repr(transparent)]
pub struct FutexAddress(zx_vaddr_t);

impl FutexAddress {
    pub fn ptr(&self) -> zx_vaddr_t {
        self.0
    }
}

impl TryFrom<usize> for FutexAddress {
    type Error = Errno;

    fn try_from(value: usize) -> Result<Self, Errno> {
        // Futex addresses must be aligned to a 4 byte boundary.
        if value % 4 != 0 {
            return error!(EINVAL);
        }
        // Futex addresses cannot be outside of the restricted address space range.
        if !RESTRICTED_ASPACE_RANGE.contains(&value) {
            return error!(EFAULT);
        }
        Ok(FutexAddress(value))
    }
}

impl TryFrom<UserAddress> for FutexAddress {
    type Error = Errno;

    fn try_from(value: UserAddress) -> Result<Self, Errno> {
        value.ptr().try_into()
    }
}

impl Into<UserAddress> for FutexAddress {
    fn into(self) -> UserAddress {
        UserAddress::const_from(self.ptr() as u64)
    }
}

impl fmt::Display for FutexAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

impl fmt::Debug for FutexAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FutexAddress").field(&format_args!("{:#x}", self.0)).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starnix_uapi::restricted_aspace::RESTRICTED_ASPACE_HIGHEST_ADDRESS;

    #[::fuchsia::test]
    fn test_misaligned_address() {
        let result = FutexAddress::try_from(3);
        assert!(result.is_err());
    }

    #[::fuchsia::test]
    fn test_normal_mode_address() {
        let result = FutexAddress::try_from(RESTRICTED_ASPACE_HIGHEST_ADDRESS);
        assert!(result.is_err());
    }

    #[::fuchsia::test]
    fn test_maximal_address() {
        let result = FutexAddress::try_from(usize::MAX);
        assert!(result.is_err());
    }

    #[::fuchsia::test]
    fn test_regular_restricted_address() {
        // This address is a valid restricted mode address on every architecture.
        let result = FutexAddress::try_from(2 * 1 << 20);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().ptr(), 2 * 1 << 20);
    }
}
