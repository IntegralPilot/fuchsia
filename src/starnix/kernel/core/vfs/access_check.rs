// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security::PermissionFlags;
use starnix_uapi::open_flags::OpenFlags;

/// Context or reason for an access permission check.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CheckAccessReason {
    /// Syscall `access(2)` or `faccessat(2)`.
    Access,
    /// Syscall `chdir(2)`.
    Chdir,
    /// Syscall `chroot(2)`.
    Chroot,
    /// Syscall `execve(2)`, `execveat(2)`, script interpreter, or ELF dynamic linker execution.
    Exec,
    /// Timestamp update (`utimensat(2)`).
    ChangeTimestamps { now: bool },
    /// Internal VFS permission check.
    InternalPermissionChecks,
}

/// Configuration for access permission checks performed on filesystem nodes.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct AccessCheck {
    permission_flags: PermissionFlags,
    reason: CheckAccessReason,
}

impl AccessCheck {
    /// Internal VFS permission check for the specified access rights.
    pub fn for_internal(permissions: impl Into<PermissionFlags>) -> Self {
        Self {
            permission_flags: permissions.into(),
            reason: CheckAccessReason::InternalPermissionChecks,
        }
    }

    /// Check for execute (search) permission when changing working directory.
    pub fn for_chdir() -> Self {
        Self { permission_flags: PermissionFlags::EXEC, reason: CheckAccessReason::Chdir }
    }

    /// Check for execute (search) permission when changing root directory.
    pub fn for_chroot() -> Self {
        Self { permission_flags: PermissionFlags::EXEC, reason: CheckAccessReason::Chroot }
    }

    /// Check for specified access rights for `access(2)` / `faccessat(2)`.
    pub fn for_access(permissions: impl Into<PermissionFlags>) -> Self {
        Self { permission_flags: permissions.into(), reason: CheckAccessReason::Access }
    }

    /// Permission flags to check.
    pub fn permission_flags(&self) -> PermissionFlags {
        self.permission_flags
    }

    /// Reason for the access check.
    pub fn reason(&self) -> CheckAccessReason {
        self.reason
    }
}

/// Configuration for opening a file, bundling open flags with concrete permission checks.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct OpenAccessCheck {
    open_flags: OpenFlags,
    access_check: AccessCheck,
}

impl From<OpenFlags> for OpenAccessCheck {
    fn from(open_flags: OpenFlags) -> Self {
        Self {
            open_flags,
            access_check: AccessCheck {
                permission_flags: open_flags.into(),
                reason: CheckAccessReason::InternalPermissionChecks,
            },
        }
    }
}

impl OpenAccessCheck {
    /// Creates an open check with explicit open flags and access check.
    pub fn new(open_flags: OpenFlags, access_check: AccessCheck) -> Self {
        Self { open_flags, access_check }
    }

    /// Skips permission checks when opening a file with the specified flags.
    pub fn skip(open_flags: OpenFlags) -> Self {
        Self {
            open_flags,
            access_check: AccessCheck {
                permission_flags: PermissionFlags::empty(),
                reason: CheckAccessReason::InternalPermissionChecks,
            },
        }
    }

    /// Configures read-only flags with an execute permission check for `execve(2)` / `execveat(2)`.
    pub fn for_exec() -> Self {
        Self {
            open_flags: OpenFlags::RDONLY,
            access_check: AccessCheck {
                permission_flags: PermissionFlags::EXEC,
                reason: CheckAccessReason::Exec,
            },
        }
    }

    /// Open flags for the file.
    pub fn open_flags(&self) -> OpenFlags {
        self.open_flags
    }

    /// Permission checks to perform when opening the file.
    pub fn access_check(&self) -> AccessCheck {
        self.access_check
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use starnix_uapi::file_mode::Access;
    use starnix_uapi::open_flags::OpenFlags;

    #[::fuchsia::test]
    fn test_access_check() {
        let for_internal_access = AccessCheck::for_internal(Access::READ);
        assert_eq!(for_internal_access.permission_flags(), PermissionFlags::READ);
        assert_eq!(for_internal_access.reason(), CheckAccessReason::InternalPermissionChecks);

        let for_internal_perms = AccessCheck::for_internal(PermissionFlags::EXEC);
        assert_eq!(for_internal_perms.permission_flags(), PermissionFlags::EXEC);
        assert_eq!(for_internal_perms.reason(), CheckAccessReason::InternalPermissionChecks);

        let for_chdir = AccessCheck::for_chdir();
        assert_eq!(for_chdir.permission_flags(), PermissionFlags::EXEC);
        assert_eq!(for_chdir.reason(), CheckAccessReason::Chdir);

        let for_chroot = AccessCheck::for_chroot();
        assert_eq!(for_chroot.permission_flags(), PermissionFlags::EXEC);
        assert_eq!(for_chroot.reason(), CheckAccessReason::Chroot);

        let for_access = AccessCheck::for_access(Access::READ | Access::WRITE);
        assert_eq!(for_access.permission_flags(), PermissionFlags::READ | PermissionFlags::WRITE);
        assert_eq!(for_access.reason(), CheckAccessReason::Access);
    }

    #[::fuchsia::test]
    fn test_open_access_check() {
        let from_flags = OpenAccessCheck::from(OpenFlags::RDWR);
        assert_eq!(from_flags.open_flags(), OpenFlags::RDWR);
        assert_eq!(
            from_flags.access_check().permission_flags(),
            PermissionFlags::READ | PermissionFlags::WRITE
        );
        assert_eq!(from_flags.access_check().reason(), CheckAccessReason::InternalPermissionChecks);

        let skip_check = OpenAccessCheck::skip(OpenFlags::RDWR | OpenFlags::CREAT);
        assert_eq!(skip_check.open_flags(), OpenFlags::RDWR | OpenFlags::CREAT);
        assert_eq!(skip_check.access_check().permission_flags(), PermissionFlags::empty());
        assert_eq!(skip_check.access_check().reason(), CheckAccessReason::InternalPermissionChecks);

        let for_exec = OpenAccessCheck::for_exec();
        assert_eq!(for_exec.open_flags(), OpenFlags::RDONLY);
        assert_eq!(for_exec.access_check().permission_flags(), PermissionFlags::EXEC);
        assert_eq!(for_exec.access_check().reason(), CheckAccessReason::Exec);

        let custom =
            OpenAccessCheck::new(OpenFlags::RDONLY, AccessCheck::for_internal(Access::READ));
        assert_eq!(custom.open_flags(), OpenFlags::RDONLY);
        assert_eq!(custom.access_check().permission_flags(), PermissionFlags::READ);
    }
}
