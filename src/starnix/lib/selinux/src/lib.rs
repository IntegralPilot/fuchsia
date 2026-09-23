// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(variant_size_differences)]

pub mod local_cache;
pub mod permission_check;
pub mod policy;
pub mod security_server;

mod new_policy;

pub use access_vector_cache::{AccessQueryArgs, DEFAULT_SHARED_SIZE, QueryCacheCapacity};
pub use concurrent_access_cache::{AccessCacheStorage, ConcurrentAccessCache};
pub use security_server::{PolicySeqNo, SecurityServer};

mod access_vector_cache;
mod cache_stats;
mod concurrent_access_cache;
mod concurrent_cache;
mod exceptions_config;
mod kernel_permissions;
mod sid_table;
mod sync;

/// Allow callers to use the kernel class & permission definitions.
pub use kernel_permissions::*;

/// Numeric class Ids are provided to the userspace AVC surfaces (e.g. "create", "access", etc).
pub use policy::ClassId;

pub use starnix_uapi::selinux::{InitialSid, ReferenceInitialSid, SecurityId, TaskAttrs};

use new_policy::FsUseType;

/// Identifies a specific class by its policy-defined Id, or as a kernel object class enum Id.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ObjectClass {
    /// Refers to a well-known SELinux kernel object class (e.g. "process", "file", "capability").
    Kernel(KernelClass),
    /// Refers to a policy-defined class by its policy-defined numeric Id. This is most commonly
    /// used when handling queries from userspace, which refer to classes by-Id.
    ClassId(ClassId),
}

impl From<ClassId> for ObjectClass {
    fn from(id: ClassId) -> Self {
        Self::ClassId(id)
    }
}

impl<T: Into<KernelClass>> From<T> for ObjectClass {
    fn from(class: T) -> Self {
        Self::Kernel(class.into())
    }
}

/// A borrowed byte slice that contains no `NUL` characters by truncating the input slice at the
/// first `NUL` (if any) upon construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NullessByteStr<'a>(&'a [u8]);

impl<'a> NullessByteStr<'a> {
    /// Returns a non-null-terminated representation of the security context string.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl<'a, S: AsRef<[u8]> + ?Sized> From<&'a S> for NullessByteStr<'a> {
    /// Any `AsRef<[u8]>` can be processed into a [`NullessByteStr`]. The [`NullessByteStr`] will
    /// retain everything up to (but not including) a null character, or else the complete byte
    /// string.
    fn from(s: &'a S) -> Self {
        let value = s.as_ref();
        match value.iter().position(|c| *c == 0) {
            Some(end) => Self(&value[..end]),
            None => Self(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileSystemMountSids {
    pub context: Option<SecurityId>,
    pub fs_context: Option<SecurityId>,
    pub def_context: Option<SecurityId>,
    pub root_context: Option<SecurityId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileSystemLabel {
    pub sid: SecurityId,
    pub scheme: FileSystemLabelingScheme,
    // Sids obtained by parsing the mount options of the FileSystem.
    pub mount_sids: FileSystemMountSids,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FileSystemLabelingScheme {
    /// This filesystem was mounted with "context=".
    Mountpoint { sid: SecurityId },
    /// This filesystem has an "fs_use_xattr", "fs_use_task", or "fs_use_trans" entry in the
    /// policy. If the `fs_use_type` is "fs_use_xattr" then the `default_sid` specifies the SID
    /// with which to label `FsNode`s of files that do not have the "security.selinux" xattr.
    FsUse { fs_use_type: FsUseType, default_sid: SecurityId },
    /// This filesystem has one or more "genfscon" statements associated with it in the policy.
    /// If `supports_seclabel` is true then nodes in the filesystem may be dynamically relabeled.
    GenFsCon { supports_seclabel: bool },
}

/// SELinux security context-related filesystem mount options. These options are documented in the
/// `context=context, fscontext=context, defcontext=context, and rootcontext=context` section of
/// the `mount(8)` manpage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileSystemMountOptions {
    /// Specifies the effective security context to use for all nodes in the filesystem, and the
    /// filesystem itself. If the filesystem already contains security attributes then these are
    /// ignored. May not be combined with any of the other options.
    pub context: Option<Vec<u8>>,
    /// Specifies an effective security context to use for un-labeled nodes in the filesystem,
    /// rather than falling-back to the policy-defined "file" context.
    pub def_context: Option<Vec<u8>>,
    /// The value of the `fscontext=[security-context]` mount option. This option is used to
    /// label the filesystem (superblock) itself.
    pub fs_context: Option<Vec<u8>>,
    /// The value of the `rootcontext=[security-context]` mount option. This option is used to
    /// (re)label the inode located at the filesystem mountpoint.
    pub root_context: Option<Vec<u8>>,
}

/// Status information parameter for the [`SeLinuxStatusPublisher`] interface.
pub struct SeLinuxStatus {
    /// SELinux-wide enforcing vs. permissive mode  bit.
    pub is_enforcing: bool,
    /// Number of times the policy has been changed since SELinux started.
    pub change_count: u32,
    /// Bit indicating whether operations unknown SELinux abstractions will be denied.
    pub deny_unknown: bool,
}

/// Interface for security server to interact with selinuxfs status file.
pub trait SeLinuxStatusPublisher: Send + Sync {
    /// Sets the value part of the associated selinuxfs status file.
    fn set_status(&mut self, policy_status: SeLinuxStatus);
}

pub use new_policy::PolicyCap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_class_permissions() {
        let test_class_id = ClassId::for_test(20);
        assert_eq!(ObjectClass::ClassId(test_class_id), test_class_id.into());
        for variant in ProcessPermission::PERMISSIONS {
            assert_eq!(KernelClass::Process, variant.class());
            assert_eq!("process", variant.class().name());
            assert_eq!(ObjectClass::Kernel(KernelClass::Process), variant.class().into());
        }
    }

    #[test]
    fn nulless_byte_str_equivalence() {
        let unterminated: NullessByteStr<'_> = b"u:object_r:test_valid_t:s0".into();
        let nul_terminated: NullessByteStr<'_> = b"u:object_r:test_valid_t:s0\0".into();
        let nul_containing: NullessByteStr<'_> =
            b"u:object_r:test_valid_t:s0\0IGNORE THIS\0!\0".into();

        for context in [nul_terminated, nul_containing] {
            assert_eq!(unterminated, context);
            assert_eq!(unterminated.as_bytes(), context.as_bytes());
        }
    }
}
