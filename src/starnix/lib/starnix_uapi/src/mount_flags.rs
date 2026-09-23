// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uapi;
use atomic_bitflags::atomic_bitflags;

atomic_bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct MountFlags: u32 {
        // per-mountpoint flags
        const RDONLY = uapi::MS_RDONLY;
        const NOEXEC = uapi::MS_NOEXEC;
        const NOSUID = uapi::MS_NOSUID;
        const NODEV = uapi::MS_NODEV;
        const NOATIME = uapi::MS_NOATIME;
        const NODIRATIME = uapi::MS_NODIRATIME;
        const RELATIME = uapi::MS_RELATIME;
        const STRICTATIME = uapi::MS_STRICTATIME;

        // per-superblock flags
        const SILENT = uapi::MS_SILENT;
        const LAZYTIME = uapi::MS_LAZYTIME;
        const SYNCHRONOUS = uapi::MS_SYNCHRONOUS;
        const DIRSYNC = uapi::MS_DIRSYNC;
        const MANDLOCK = uapi::MS_MANDLOCK;

        // mount() control flags
        const REMOUNT = uapi::MS_REMOUNT;
        const BIND = uapi::MS_BIND;
        const MOVE = uapi::MS_MOVE;
        const REC = uapi::MS_REC;
        const DOWNSTREAM = uapi::MS_SLAVE;
        const SHARED = uapi::MS_SHARED;
        const PRIVATE = uapi::MS_PRIVATE;

        /// Flags that change be changed with REMOUNT.
        ///
        /// MS_DIRSYNC and MS_SILENT cannot be changed with REMOUNT.
        const CHANGEABLE_WITH_REMOUNT = MountpointFlags::all().bits() |
            Self::MANDLOCK.bits() | Self::LAZYTIME.bits() | Self::SYNCHRONOUS.bits();
    }
}

atomic_bitflags! {
    /// Subset of `MountFlags` that allow the behaviours of different mountpoints to the same
    /// underlying `FileSystem` to be independently configured.
    /// Note that
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct MountpointFlags: u32 {
        // Flags stored for each mountpoint to configure its behaviour.
        const RDONLY = MountFlags::RDONLY.bits();
        const NOEXEC = MountFlags::NOEXEC.bits();
        const NOSUID = MountFlags::NOSUID.bits();
        const NODEV = MountFlags::NODEV.bits();
        const NOATIME = MountFlags::NOATIME.bits();
        const NODIRATIME = MountFlags::NODIRATIME.bits();
        const RELATIME = MountFlags::RELATIME.bits();

        const STORED_ON_MOUNT = Self::RDONLY.bits() | Self::NOEXEC.bits() | Self::NOSUID.bits() |
            Self::NODEV.bits() | Self::NOATIME.bits() | Self::NODIRATIME.bits() | Self::RELATIME.bits();

        // Flags affecting the behaviour of a single operation on a mountpoint.
        const STRICTATIME = MountFlags::STRICTATIME.bits();
        const REC = MountFlags::REC.bits();

        /// Flags used to control how file access times are managed. Note that STRICTATIME is only
        /// used by callers to specify that all other bits should be cleared; it is never stored.
        const ATIME_MODE_FLAGS = Self::NOATIME.bits() | Self::RELATIME.bits() |Self::STRICTATIME.bits();
        const ATIME_FLAGS = Self::ATIME_MODE_FLAGS.bits() | Self::NODIRATIME.bits();
    }
}

impl MountpointFlags {
    /// Ensures that `self` has an access-time flag set, copying the flag value from `existing` if
    /// no flag is currently set.
    /// This is used both to apply the kernel default `MS_RELATIME` if no other access-time flag is
    /// passed to `mount()`, and to preserve existing access-time flags when re-mounting, unless the
    /// `MS_STRICTATIME` flag is explicitly passed.
    pub fn default_atime_from(&mut self, existing: MountpointFlags) {
        if !self.intersects(Self::ATIME_FLAGS) {
            // If no `ATIME_FLAGS` are set at all, preserve the `existing` flags.
            *self |= existing & Self::ATIME_FLAGS;
        } else if !self.intersects(Self::ATIME_MODE_FLAGS) {
            // If the caller did not set any `ATIME_MODE_FLAGS` then default to the `RELATIME` mode.
            *self |= Self::RELATIME;
        }
    }
}

impl From<MountpointFlags> for MountFlags {
    fn from(flags: MountpointFlags) -> Self {
        // MountpointFlags is defined using only bits that are valid for MountFlags.
        Self::from_bits_retain(flags.bits())
    }
}

atomic_bitflags! {
    /// Subset of `MountFlags` that affect `FileSystem` behaviour.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct FileSystemFlags: u32 {
        const RDONLY = MountFlags::RDONLY.bits();
        const DIRSYNC = MountFlags::DIRSYNC.bits();
        const LAZYTIME = MountFlags::LAZYTIME.bits();
        const MANDLOCK = MountFlags::MANDLOCK.bits();
        const SILENT = MountFlags::SILENT.bits();
        const SYNCHRONOUS = MountFlags::SYNCHRONOUS.bits();
    }
}

impl From<FileSystemFlags> for MountFlags {
    fn from(flags: FileSystemFlags) -> Self {
        // FileSystemFlags is defined using only bits that are valid for MountFlags.
        Self::from_bits_retain(flags.bits())
    }
}

impl MountFlags {
    pub fn mountpoint_flags(&self) -> MountpointFlags {
        MountpointFlags::from_bits_truncate(self.bits())
    }

    pub fn file_system_flags(&self) -> FileSystemFlags {
        FileSystemFlags::from_bits_truncate(self.bits())
    }
}

impl std::fmt::Display for MountpointFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        MountFlags::from(*self).fmt(f)
    }
}

impl std::fmt::Display for FileSystemFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        MountFlags::from(*self).fmt(f)
    }
}

/// Display trait implementation for the subset of flags that are stored with mounts or superblocks.
impl std::fmt::Display for MountFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `RDONLY` is stored both with the mount and the superblock flags.
        write!(f, "{}", if self.contains(Self::RDONLY) { "ro" } else { "rw" })?;

        // Stored only in the mount flags.
        if self.contains(Self::NOEXEC) {
            write!(f, ",noexec")?;
        }
        if self.contains(Self::NOSUID) {
            write!(f, ",nosuid")?;
        }
        if self.contains(Self::NODEV) {
            write!(f, ",nodev")?
        }
        if self.contains(Self::NOATIME) {
            write!(f, ",noatime")?;
        }
        if self.contains(Self::NODIRATIME) {
            write!(f, ",nodiratime")?;
        }
        if self.contains(Self::RELATIME) {
            write!(f, ",relatime")?;
        }

        // Stored only on the superblock.
        if self.contains(Self::DIRSYNC) {
            write!(f, ",dirsync")?;
        }
        if self.contains(Self::LAZYTIME) {
            write!(f, ",lazytime")?;
        }
        if self.contains(Self::MANDLOCK) {
            write!(f, ",mand")?;
        }
        // `SILENT` is not reported in proc-mounts nor proc-pid-mountinfo.
        if self.contains(Self::SYNCHRONOUS) {
            write!(f, ",sync")?;
        }

        Ok(())
    }
}
