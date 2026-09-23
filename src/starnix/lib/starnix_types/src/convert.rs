// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use fidl_fuchsia_starnix_binder as fbinder;
use starnix_uapi::open_flags::OpenFlags;
use std::ops::Bound;

pub trait FromFidl<T>: Sized {
    fn from_fidl(value: T) -> Self;
}

pub trait IntoFidl<T>: Sized {
    // Required method
    fn into_fidl(self) -> T;
}

impl<T, U> IntoFidl<U> for T
where
    U: FromFidl<T>,
{
    fn into_fidl(self) -> U {
        U::from_fidl(self)
    }
}

impl FromFidl<fio::OpenFlags> for OpenFlags {
    fn from_fidl(fio_flags: fio::OpenFlags) -> Self {
        let mut result = if fio_flags.contains(fio::OpenFlags::RIGHT_WRITABLE) {
            if fio_flags.contains(fio::OpenFlags::RIGHT_READABLE) {
                OpenFlags::RDWR
            } else {
                OpenFlags::WRONLY
            }
        } else {
            OpenFlags::RDONLY
        };
        if fio_flags.contains(fio::OpenFlags::CREATE) {
            result |= OpenFlags::CREAT;
        }
        if fio_flags.contains(fio::OpenFlags::CREATE_IF_ABSENT) {
            result |= OpenFlags::EXCL;
        }
        if fio_flags.contains(fio::OpenFlags::TRUNCATE) {
            result |= OpenFlags::TRUNC;
        }
        if fio_flags.contains(fio::OpenFlags::APPEND) {
            result |= OpenFlags::APPEND;
        }
        if fio_flags.contains(fio::OpenFlags::DIRECTORY) {
            result |= OpenFlags::DIRECTORY;
        }
        result
    }
}

impl FromFidl<OpenFlags> for fio::OpenFlags {
    fn from_fidl(flags: OpenFlags) -> fio::OpenFlags {
        let mut result = fio::OpenFlags::empty();
        if flags.can_read() {
            result |= fio::OpenFlags::RIGHT_READABLE | fio::OpenFlags::POSIX_WRITABLE;
        }
        if flags.can_write() {
            result |= fio::OpenFlags::RIGHT_WRITABLE;
        }
        if flags.contains(OpenFlags::CREAT) {
            result |= fio::OpenFlags::CREATE;
        }
        if flags.contains(OpenFlags::EXCL) {
            result |= fio::OpenFlags::CREATE_IF_ABSENT;
        }
        if flags.contains(OpenFlags::TRUNC) {
            result |= fio::OpenFlags::TRUNCATE;
        }
        if flags.contains(OpenFlags::APPEND) {
            result |= fio::OpenFlags::APPEND;
        }
        if flags.contains(OpenFlags::DIRECTORY) {
            result |= fio::OpenFlags::DIRECTORY;
        }
        result
    }
}

impl FromFidl<OpenFlags> for fbinder::FileFlags {
    fn from_fidl(flags: OpenFlags) -> Self {
        let mut result = Self::empty();
        if flags.can_read() {
            result |= Self::RIGHT_READABLE;
        }
        if flags.can_write() {
            result |= Self::RIGHT_WRITABLE;
        }
        if flags.contains(OpenFlags::DIRECTORY) {
            result |= Self::DIRECTORY;
        }

        result
    }
}

impl FromFidl<fbinder::FileFlags> for OpenFlags {
    fn from_fidl(flags: fbinder::FileFlags) -> Self {
        let readable = flags.contains(fbinder::FileFlags::RIGHT_READABLE);
        let writable = flags.contains(fbinder::FileFlags::RIGHT_WRITABLE);
        let mut result = Self::empty();
        if readable && writable {
            result = Self::RDWR;
        } else if writable {
            result = Self::WRONLY;
        } else if readable {
            result = Self::RDONLY;
        }
        if flags.contains(fbinder::FileFlags::DIRECTORY) {
            result |= Self::DIRECTORY;
        }
        result
    }
}

/// Extension trait for `std::ops::Bound` to allow fallible mapping of its contents.
///
/// This is useful in situations where range bounds must be converted into a different type
/// prior to range-queries (e.g. converting a `Bound<usize>` received from an API but needing
/// to index into an internal structure requiring `Bound<std::num::NonZeroU16>`), where
/// the conversion operation itself is fallible (e.g. `try_from`).
pub trait BoundExt<T> {
    fn try_map<U, E, F>(self, f: F) -> Result<Bound<U>, E>
    where
        F: FnOnce(T) -> Result<U, E>;
}

impl<T> BoundExt<T> for Bound<T> {
    fn try_map<U, E, F>(self, f: F) -> Result<Bound<U>, E>
    where
        F: FnOnce(T) -> Result<U, E>,
    {
        match self {
            Bound::Unbounded => Ok(Bound::Unbounded),
            Bound::Included(x) => Ok(Bound::Included(f(x)?)),
            Bound::Excluded(x) => Ok(Bound::Excluded(f(x)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starnix_uapi::uapi;

    fn assert_flags_equals(flags: OpenFlags, fio_flags: fio::OpenFlags) {
        assert_eq!(flags, fio_flags.into_fidl());
        assert_eq!(fio_flags, flags.into_fidl());
    }

    #[::fuchsia::test]
    fn test_access() {
        let read_only = OpenFlags::from_bits_truncate(uapi::O_RDONLY);
        assert!(read_only.can_read());
        assert!(!read_only.can_write());

        let write_only = OpenFlags::from_bits_truncate(uapi::O_WRONLY);
        assert!(!write_only.can_read());
        assert!(write_only.can_write());

        let read_write = OpenFlags::from_bits_truncate(uapi::O_RDWR);
        assert!(read_write.can_read());
        assert!(read_write.can_write());
    }

    #[::fuchsia::test]
    fn test_conversion() {
        assert_flags_equals(
            OpenFlags::RDONLY,
            fio::OpenFlags::RIGHT_READABLE | fio::OpenFlags::POSIX_WRITABLE,
        );
        assert_flags_equals(OpenFlags::WRONLY, fio::OpenFlags::RIGHT_WRITABLE);
        assert_flags_equals(
            OpenFlags::RDWR,
            fio::OpenFlags::RIGHT_READABLE
                | fio::OpenFlags::RIGHT_WRITABLE
                | fio::OpenFlags::POSIX_WRITABLE,
        );
    }

    #[::fuchsia::test]
    fn test_bound_try_map() {
        let f = |x: usize| -> Result<std::num::NonZeroU16, &'static str> {
            let val = u16::try_from(x).map_err(|_| "out of range")?;
            std::num::NonZeroU16::new(val).ok_or("is zero")
        };

        assert_eq!(
            Bound::<usize>::Unbounded.try_map(f),
            Ok(Bound::<std::num::NonZeroU16>::Unbounded)
        );
        assert_eq!(
            Bound::Included(5usize).try_map(f),
            Ok(Bound::Included(std::num::NonZeroU16::new(5).unwrap()))
        );
        assert_eq!(
            Bound::Excluded(3usize).try_map(f),
            Ok(Bound::Excluded(std::num::NonZeroU16::new(3).unwrap()))
        );

        assert_eq!(Bound::Included(usize::MAX).try_map(f), Err("out of range"));
        assert_eq!(Bound::Included(0usize).try_map(f), Err("is zero"));
    }
}
