// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_rcu::RcuDroppable;
use std::num::NonZeroU32;
use zerocopy::{Immutable, IntoBytes};

/// Identifies a Security Context.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, IntoBytes, Immutable, RcuDroppable,
)]
pub struct SecurityId(pub NonZeroU32);

/// Initial Security Identifier (SID) values defined by the SELinux Reference Policy.
/// Where the SELinux Reference Policy retains definitions for some deprecated initial SIDs, this
/// enum omits deprecated entries for clarity.
#[repr(u64)]
pub enum ReferenceInitialSid {
    Kernel = 1,
    Security = 2,
    Unlabeled = 3,
    _Fs = 4,
    File = 5,
    Init = 7,
    _Port = 9,
    _Netif = 10,
    _Netmsg = 11,
    _Node = 12,
    _Sysctl = 17,
    Devnull = 27,

    /// Lowest Security Identifier value guaranteed not to be used by this
    /// implementation to refer to an initial Security Context.
    FirstUnused,
}

#[macro_export]
macro_rules! initial_sid_enum {
    ($(#[$meta:meta])* $name:ident {
        $($(#[$variant_meta:meta])* $variant:ident ($variant_name: literal)),*,
    }) => {
        $(#[$meta])*
        pub enum $name {
            $($(#[$variant_meta])* $variant = ReferenceInitialSid::$variant as isize),*
        }

        impl $name {
            pub fn all_variants() -> &'static [Self] {
                &[
                    $($name::$variant),*
                ]
            }

            pub fn name(&self) -> &'static str {
                match self {
                    $($name::$variant => $variant_name),*
                }
            }
        }
    }
}

initial_sid_enum! {
/// Initial Security Identifier (SID) values actually used by this implementation.
/// These must be present in the policy, for it to be valid.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
    InitialSid {
        // keep-sorted start
        Devnull("devnull"),
        File("file"),
        Init("init"),
        Kernel("kernel"),
        Security("security"),
        Unlabeled("unlabeled"),
        // keep-sorted end
    }
}

impl From<InitialSid> for SecurityId {
    fn from(initial_sid: InitialSid) -> Self {
        // Initial SIDs are used by the kernel as placeholder `SecurityId` values for objects
        // created prior to the SELinux policy being loaded, and are resolved to the policy-defined
        // Security Context when referenced after policy load.
        Self(NonZeroU32::new(initial_sid as u32).unwrap())
    }
}

/// The SELinux security structure for `ThreadGroup`.
#[derive(Clone, Debug, PartialEq, RcuDroppable)]
pub struct TaskAttrs {
    /// Current SID for the task.
    pub current_sid: SecurityId,

    /// SID for the task upon the next execve call.
    pub exec_sid: Option<SecurityId>,

    /// SID for files created by the task.
    pub fscreate_sid: Option<SecurityId>,

    /// SID for kernel-managed keys created by the task.
    pub keycreate_sid: Option<SecurityId>,

    /// SID prior to the last execve.
    pub previous_sid: SecurityId,

    /// SID for sockets created by the task.
    pub sockcreate_sid: Option<SecurityId>,

    /// Indicates that the task with these credentials is performing an internal operation where
    /// access checks must be skipped.
    pub internal_operation: bool,
}

impl TaskAttrs {
    /// Returns initial state for kernel tasks.
    pub fn for_kernel() -> Self {
        Self::for_transition(InitialSid::Kernel.into(), InitialSid::Kernel.into())
    }

    /// Returns placeholder state for use when SELinux is not enabled.
    pub fn for_selinux_disabled() -> Self {
        Self::for_transition(InitialSid::Unlabeled.into(), InitialSid::Unlabeled.into())
    }

    /// Used to create new security state when transitioning a task to a new SID.
    pub fn for_transition(new_sid: SecurityId, previous_sid: SecurityId) -> Self {
        Self {
            current_sid: new_sid,
            previous_sid,
            exec_sid: None,
            fscreate_sid: None,
            keycreate_sid: None,
            sockcreate_sid: None,
            internal_operation: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_alloc_for_kernel() {
        let for_kernel = TaskAttrs::for_kernel();
        assert_eq!(for_kernel.current_sid, InitialSid::Kernel.into());
        assert_eq!(for_kernel.previous_sid, for_kernel.current_sid);
        assert_eq!(for_kernel.exec_sid, None);
        assert_eq!(for_kernel.fscreate_sid, None);
        assert_eq!(for_kernel.keycreate_sid, None);
        assert_eq!(for_kernel.sockcreate_sid, None);
    }
}
