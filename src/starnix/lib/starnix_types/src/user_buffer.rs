// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::PAGE_SIZE;
use smallvec::SmallVec;
use starnix_uapi::errors::{Errno, errno, error};
use starnix_uapi::user_address::{UserAddress, UserAddress32, UserRef};
use std::sync::LazyLock;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub type UserBuffers = SmallVec<[UserBuffer; 1]>;
pub type UserBuffers32 = SmallVec<[UserBuffer32; 1]>;

/// Matches iovec_t.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, IntoBytes, KnownLayout, FromBytes, Immutable,
)]
#[repr(C)]
pub struct UserBuffer {
    pub address: UserAddress,
    pub length: usize,
}

pub static MAX_RW_COUNT: LazyLock<usize> = LazyLock::new(|| ((1 << 31) - *PAGE_SIZE) as usize);
/// Matches compat_iovec_t.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, IntoBytes, KnownLayout, FromBytes, Immutable,
)]
#[repr(C)]
#[repr(packed)]
pub struct UserBuffer32 {
    pub address: UserAddress32,
    pub length: u32,
}

impl From<UserBuffer32> for UserBuffer {
    fn from(ub32: UserBuffer32) -> Self {
        UserBuffer { address: ub32.address.into(), length: ub32.length as usize }
    }
}

impl UserBuffer {
    pub fn cap_buffers_to_max_rw_count(
        max_address: UserAddress,
        buffers: &mut UserBuffers,
    ) -> Result<usize, Errno> {
        // Linux checks all buffers for plausibility, even those past the MAX_RW_COUNT threshold.
        for buffer in buffers.iter() {
            if buffer.address > max_address
                || buffer.address.checked_add(buffer.length).ok_or_else(|| errno!(EINVAL))?
                    > max_address
            {
                return error!(EFAULT);
            }
        }
        let max_rw_count = *MAX_RW_COUNT;
        let mut total: usize = 0;
        let mut offset = 0;
        while offset < buffers.len() {
            total = total.checked_add(buffers[offset].length).ok_or_else(|| errno!(EINVAL))?;
            if total >= max_rw_count {
                buffers[offset].length -= total - max_rw_count;
                total = max_rw_count;
                buffers.truncate(offset + 1);
                break;
            }
            offset += 1;
        }
        Ok(total)
    }

    pub fn advance(&mut self, length: usize) -> Result<(), Errno> {
        self.address = self.address.checked_add(length).ok_or_else(|| errno!(EINVAL))?;
        self.length = self.length.checked_sub(length).ok_or_else(|| errno!(EINVAL))?;
        Ok(())
    }

    /// Returns whether the buffer address is 0 and its length is 0.
    pub fn is_null(&self) -> bool {
        self.address.is_null() && self.is_empty()
    }

    /// Returns whether the buffer length is 0.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Whether the given address and length is contained within this buffer.
    pub fn contains(&self, address: UserAddress, length: usize) -> bool {
        if let (Some(limit), Some(self_limit)) =
            (address.checked_add(length), self.address.checked_add(self.length))
        {
            address >= self.address && limit <= self_limit
        } else {
            false
        }
    }
}

impl<T> TryInto<UserRef<T>> for UserBuffer {
    type Error = Errno;

    /// Returns EINVAL if the buffer is too small for the type.
    fn try_into(self) -> Result<UserRef<T>, Errno> {
        if self.length < std::mem::size_of::<T>() {
            return error!(EINVAL);
        }
        Ok(UserRef::new(self.address))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use smallvec::smallvec;

    #[::fuchsia::test]
    fn test_cap_buffers_to_max_rw_count_buffer_begin_past_max_address() {
        let mut buffers =
            smallvec![UserBuffer { address: UserAddress::const_from(50), length: 10 }];
        assert_eq!(
            error!(EFAULT),
            UserBuffer::cap_buffers_to_max_rw_count(UserAddress::const_from(40), &mut buffers),
        );
    }

    #[::fuchsia::test]
    fn test_cap_buffers_to_max_rw_count_buffer_end_past_max_address() {
        let mut buffers =
            smallvec![UserBuffer { address: UserAddress::const_from(50), length: 10 }];
        assert_eq!(
            error!(EFAULT),
            UserBuffer::cap_buffers_to_max_rw_count(UserAddress::const_from(55), &mut buffers),
        );
    }

    #[::fuchsia::test]
    fn test_cap_buffers_to_max_rw_count_buffer_overflow_u64() {
        let mut buffers =
            smallvec![UserBuffer { address: UserAddress::const_from(u64::MAX - 10), length: 20 }];
        assert_eq!(
            error!(EINVAL),
            UserBuffer::cap_buffers_to_max_rw_count(
                UserAddress::const_from(u64::MAX),
                &mut buffers
            ),
        );
    }

    #[::fuchsia::test]
    fn test_cap_buffers_to_max_rw_count_shorten_buffer() {
        let mut buffers = smallvec![UserBuffer {
            address: UserAddress::const_from(0),
            length: *MAX_RW_COUNT + 10
        }];
        let total = UserBuffer::cap_buffers_to_max_rw_count(
            UserAddress::const_from(u64::MAX),
            &mut buffers,
        )
        .unwrap();
        assert_eq!(total, *MAX_RW_COUNT);
        assert_eq!(
            buffers.as_slice(),
            &[UserBuffer { address: UserAddress::const_from(0), length: *MAX_RW_COUNT }]
        );
    }

    #[::fuchsia::test]
    fn test_cap_buffers_to_max_rw_count_drop_buffer() {
        let mut buffers = smallvec![
            UserBuffer { address: UserAddress::const_from(0), length: *MAX_RW_COUNT },
            UserBuffer { address: UserAddress::const_from(1 << 33), length: 20 }
        ];
        let total = UserBuffer::cap_buffers_to_max_rw_count(
            UserAddress::const_from(u64::MAX),
            &mut buffers,
        )
        .unwrap();
        assert_eq!(total, *MAX_RW_COUNT);
        assert_eq!(
            buffers.as_slice(),
            &[UserBuffer { address: UserAddress::const_from(0), length: *MAX_RW_COUNT }]
        );
    }

    #[::fuchsia::test]
    fn test_cap_buffers_to_max_rw_count_drop_and_shorten_buffer() {
        let mut buffers = smallvec![
            UserBuffer { address: UserAddress::const_from(0), length: *MAX_RW_COUNT - 10 },
            UserBuffer { address: UserAddress::const_from(1 << 33), length: 20 },
            UserBuffer { address: UserAddress::const_from(2 << 33), length: 20 }
        ];
        let total = UserBuffer::cap_buffers_to_max_rw_count(
            UserAddress::const_from(u64::MAX),
            &mut buffers,
        )
        .unwrap();
        assert_eq!(total, *MAX_RW_COUNT);
        assert_eq!(
            buffers.as_slice(),
            &[
                UserBuffer { address: UserAddress::const_from(0), length: *MAX_RW_COUNT - 10 },
                UserBuffer { address: UserAddress::const_from(1 << 33), length: 10 },
            ]
        );
    }
}
