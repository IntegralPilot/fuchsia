// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Definition of the [`Fd`] wrapper for file descriptors in the shell AST and runtime.

/// A file descriptor number in the shell AST and runtime.
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    Default,
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
)]
#[repr(transparent)]
pub struct Fd(pub i32);

impl Fd {
    /// Standard input (FD 0).
    pub const STDIN: Self = Self(0);
    /// Standard output (FD 1).
    pub const STDOUT: Self = Self(1);
    /// Standard error (FD 2).
    pub const STDERR: Self = Self(2);

    /// Creates a new AST file descriptor wrapper.
    pub const fn new(fd: i32) -> Self {
        Self(fd)
    }

    /// Returns the underlying integer file descriptor number.
    pub const fn raw(&self) -> i32 {
        self.0
    }
}

impl std::fmt::Display for Fd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<i32> for Fd {
    fn from(fd: i32) -> Self {
        Self(fd)
    }
}

impl From<Fd> for i32 {
    fn from(fd: Fd) -> i32 {
        fd.0
    }
}

impl std::os::fd::AsRawFd for Fd {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0
    }
}

impl std::os::fd::FromRawFd for Fd {
    unsafe fn from_raw_fd(fd: std::os::fd::RawFd) -> Self {
        Self(fd)
    }
}

impl std::os::fd::IntoRawFd for Fd {
    fn into_raw_fd(self) -> std::os::fd::RawFd {
        self.0
    }
}

impl From<std::os::fd::BorrowedFd<'_>> for Fd {
    fn from(fd: std::os::fd::BorrowedFd<'_>) -> Self {
        use std::os::fd::AsRawFd;
        Self(fd.as_raw_fd())
    }
}

impl From<&std::os::fd::OwnedFd> for Fd {
    fn from(fd: &std::os::fd::OwnedFd) -> Self {
        use std::os::fd::AsRawFd;
        Self(fd.as_raw_fd())
    }
}

impl From<std::os::fd::OwnedFd> for Fd {
    fn from(fd: std::os::fd::OwnedFd) -> Self {
        use std::os::fd::AsRawFd;
        Self(fd.as_raw_fd())
    }
}
