// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]

use crate::errors::{Errno, error};
use crate::selinux::TaskAttrs;
use crate::{gid_t, uapi, uid_t};
use bitflags::bitflags;
use fuchsia_rcu::RcuDroppable;
use std::sync::{Arc, LazyLock};

bitflags! {
    /// Linux capability flags for thread permission checks and bounding sets.
    ///
    /// Handles only known and valid Linux capabilities (bits `0` through
    /// [`CAP_LAST_CAP`]). Any undefined capability bits in raw masks from user space
    /// are truncated during ABI conversion using [`Self::from_bits_truncate`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Capabilities: u64 {
        // keep-sorted start
        const AUDIT_CONTROL = 1 << uapi::CAP_AUDIT_CONTROL;
        const AUDIT_READ = 1 << uapi::CAP_AUDIT_READ;
        const AUDIT_WRITE = 1 << uapi::CAP_AUDIT_WRITE;
        const BLOCK_SUSPEND = 1 << uapi::CAP_BLOCK_SUSPEND;
        const BPF = 1 << uapi::CAP_BPF;
        const CHECKPOINT_RESTORE = 1 << uapi::CAP_CHECKPOINT_RESTORE;
        const CHOWN = 1 << uapi::CAP_CHOWN;
        const DAC_OVERRIDE = 1 << uapi::CAP_DAC_OVERRIDE;
        const DAC_READ_SEARCH = 1 << uapi::CAP_DAC_READ_SEARCH;
        const FOWNER = 1 << uapi::CAP_FOWNER;
        const FSETID = 1 << uapi::CAP_FSETID;
        const IPC_LOCK = 1 << uapi::CAP_IPC_LOCK;
        const IPC_OWNER = 1 << uapi::CAP_IPC_OWNER;
        const KILL = 1 << uapi::CAP_KILL;
        const LEASE = 1 << uapi::CAP_LEASE;
        const LINUX_IMMUTABLE = 1 << uapi::CAP_LINUX_IMMUTABLE;
        const MAC_ADMIN = 1 << uapi::CAP_MAC_ADMIN;
        const MAC_OVERRIDE = 1 << uapi::CAP_MAC_OVERRIDE;
        const MKNOD = 1 << uapi::CAP_MKNOD;
        const NET_ADMIN = 1 << uapi::CAP_NET_ADMIN;
        const NET_BIND_SERVICE = 1 << uapi::CAP_NET_BIND_SERVICE;
        const NET_BROADCAST = 1 << uapi::CAP_NET_BROADCAST;
        const NET_RAW = 1 << uapi::CAP_NET_RAW;
        const PERFMON = 1 << uapi::CAP_PERFMON;
        const SETFCAP = 1 << uapi::CAP_SETFCAP;
        const SETGID = 1 << uapi::CAP_SETGID;
        const SETPCAP = 1 << uapi::CAP_SETPCAP;
        const SETUID = 1 << uapi::CAP_SETUID;
        const SYSLOG = 1 << uapi::CAP_SYSLOG;
        const SYS_ADMIN = 1 << uapi::CAP_SYS_ADMIN;
        const SYS_BOOT = 1 << uapi::CAP_SYS_BOOT;
        const SYS_CHROOT = 1 << uapi::CAP_SYS_CHROOT;
        const SYS_MODULE = 1 << uapi::CAP_SYS_MODULE;
        const SYS_NICE = 1 << uapi::CAP_SYS_NICE;
        const SYS_PACCT = 1 << uapi::CAP_SYS_PACCT;
        const SYS_PTRACE = 1 << uapi::CAP_SYS_PTRACE;
        const SYS_RAWIO = 1 << uapi::CAP_SYS_RAWIO;
        const SYS_RESOURCE = 1 << uapi::CAP_SYS_RESOURCE;
        const SYS_TIME = 1 << uapi::CAP_SYS_TIME;
        const SYS_TTY_CONFIG = 1 << uapi::CAP_SYS_TTY_CONFIG;
        const WAKE_ALARM = 1 << uapi::CAP_WAKE_ALARM;
        // keep-sorted end
    }
}

// SAFETY: Capabilities is a u64 bitflags.
unsafe impl RcuDroppable for Capabilities {}

impl Capabilities {
    pub const fn as_abi_v1(self) -> u32 {
        self.bits() as u32
    }

    pub fn from_abi_v1(bits: u32) -> Self {
        Self::from_bits_truncate(bits as u64)
    }

    pub const fn as_abi_v3(self) -> (u32, u32) {
        (self.bits() as u32, (self.bits() >> 32) as u32)
    }

    pub fn from_abi_v3(u32s: (u32, u32)) -> Self {
        let mask = u32s.0 as u64 | ((u32s.1 as u64) << 32);
        Self::from_bits_truncate(mask)
    }
}

impl std::convert::TryFrom<u64> for Capabilities {
    type Error = Errno;

    fn try_from(capability_num: u64) -> Result<Self, Self::Error> {
        match 1u64.checked_shl(capability_num as u32) {
            Some(mask) => {
                let caps = Self::from_bits_truncate(mask);
                if caps.is_empty() { error!(EINVAL) } else { Ok(caps) }
            }
            _ => error!(EINVAL),
        }
    }
}

impl std::str::FromStr for Capabilities {
    type Err = Errno;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            // keep-sorted start
            "AUDIT_CONTROL" => Self::AUDIT_CONTROL,
            "AUDIT_READ" => Self::AUDIT_READ,
            "AUDIT_WRITE" => Self::AUDIT_WRITE,
            "BLOCK_SUSPEND" => Self::BLOCK_SUSPEND,
            "BPF" => Self::BPF,
            "CHECKPOINT_RESTORE" => Self::CHECKPOINT_RESTORE,
            "CHOWN" => Self::CHOWN,
            "DAC_OVERRIDE" => Self::DAC_OVERRIDE,
            "DAC_READ_SEARCH" => Self::DAC_READ_SEARCH,
            "FOWNER" => Self::FOWNER,
            "FSETID" => Self::FSETID,
            "IPC_LOCK" => Self::IPC_LOCK,
            "IPC_OWNER" => Self::IPC_OWNER,
            "KILL" => Self::KILL,
            "LEASE" => Self::LEASE,
            "LINUX_IMMUTABLE" => Self::LINUX_IMMUTABLE,
            "MAC_ADMIN" => Self::MAC_ADMIN,
            "MAC_OVERRIDE" => Self::MAC_OVERRIDE,
            "MKNOD" => Self::MKNOD,
            "NET_ADMIN" => Self::NET_ADMIN,
            "NET_BIND_SERVICE" => Self::NET_BIND_SERVICE,
            "NET_BROADCAST" => Self::NET_BROADCAST,
            "NET_RAW" => Self::NET_RAW,
            "PERFMON" => Self::PERFMON,
            "SETFCAP" => Self::SETFCAP,
            "SETGID" => Self::SETGID,
            "SETPCAP" => Self::SETPCAP,
            "SETUID" => Self::SETUID,
            "SYSLOG" => Self::SYSLOG,
            "SYS_ADMIN" => Self::SYS_ADMIN,
            "SYS_BOOT" => Self::SYS_BOOT,
            "SYS_CHROOT" => Self::SYS_CHROOT,
            "SYS_MODULE" => Self::SYS_MODULE,
            "SYS_NICE" => Self::SYS_NICE,
            "SYS_PACCT" => Self::SYS_PACCT,
            "SYS_PTRACE" => Self::SYS_PTRACE,
            "SYS_RAWIO" => Self::SYS_RAWIO,
            "SYS_RESOURCE" => Self::SYS_RESOURCE,
            "SYS_TIME" => Self::SYS_TIME,
            "SYS_TTY_CONFIG" => Self::SYS_TTY_CONFIG,
            "WAKE_ALARM" => Self::WAKE_ALARM,
            // keep-sorted end
            _ => return error!(EINVAL),
        })
    }
}

// keep-sorted start
pub const CAP_AUDIT_CONTROL: Capabilities = Capabilities::AUDIT_CONTROL;
pub const CAP_AUDIT_READ: Capabilities = Capabilities::AUDIT_READ;
pub const CAP_AUDIT_WRITE: Capabilities = Capabilities::AUDIT_WRITE;
pub const CAP_BLOCK_SUSPEND: Capabilities = Capabilities::BLOCK_SUSPEND;
pub const CAP_BPF: Capabilities = Capabilities::BPF;
pub const CAP_CHECKPOINT_RESTORE: Capabilities = Capabilities::CHECKPOINT_RESTORE;
pub const CAP_CHOWN: Capabilities = Capabilities::CHOWN;
pub const CAP_DAC_OVERRIDE: Capabilities = Capabilities::DAC_OVERRIDE;
pub const CAP_DAC_READ_SEARCH: Capabilities = Capabilities::DAC_READ_SEARCH;
pub const CAP_FOWNER: Capabilities = Capabilities::FOWNER;
pub const CAP_FSETID: Capabilities = Capabilities::FSETID;
pub const CAP_IPC_LOCK: Capabilities = Capabilities::IPC_LOCK;
pub const CAP_IPC_OWNER: Capabilities = Capabilities::IPC_OWNER;
pub const CAP_KILL: Capabilities = Capabilities::KILL;
pub const CAP_LEASE: Capabilities = Capabilities::LEASE;
pub const CAP_LINUX_IMMUTABLE: Capabilities = Capabilities::LINUX_IMMUTABLE;
pub const CAP_MAC_ADMIN: Capabilities = Capabilities::MAC_ADMIN;
pub const CAP_MAC_OVERRIDE: Capabilities = Capabilities::MAC_OVERRIDE;
pub const CAP_MKNOD: Capabilities = Capabilities::MKNOD;
pub const CAP_NET_ADMIN: Capabilities = Capabilities::NET_ADMIN;
pub const CAP_NET_BIND_SERVICE: Capabilities = Capabilities::NET_BIND_SERVICE;
pub const CAP_NET_BROADCAST: Capabilities = Capabilities::NET_BROADCAST;
pub const CAP_NET_RAW: Capabilities = Capabilities::NET_RAW;
pub const CAP_PERFMON: Capabilities = Capabilities::PERFMON;
pub const CAP_SETFCAP: Capabilities = Capabilities::SETFCAP;
pub const CAP_SETGID: Capabilities = Capabilities::SETGID;
pub const CAP_SETPCAP: Capabilities = Capabilities::SETPCAP;
pub const CAP_SETUID: Capabilities = Capabilities::SETUID;
pub const CAP_SYSLOG: Capabilities = Capabilities::SYSLOG;
pub const CAP_SYS_ADMIN: Capabilities = Capabilities::SYS_ADMIN;
pub const CAP_SYS_BOOT: Capabilities = Capabilities::SYS_BOOT;
pub const CAP_SYS_CHROOT: Capabilities = Capabilities::SYS_CHROOT;
pub const CAP_SYS_MODULE: Capabilities = Capabilities::SYS_MODULE;
pub const CAP_SYS_NICE: Capabilities = Capabilities::SYS_NICE;
pub const CAP_SYS_PACCT: Capabilities = Capabilities::SYS_PACCT;
pub const CAP_SYS_PTRACE: Capabilities = Capabilities::SYS_PTRACE;
pub const CAP_SYS_RAWIO: Capabilities = Capabilities::SYS_RAWIO;
pub const CAP_SYS_RESOURCE: Capabilities = Capabilities::SYS_RESOURCE;
pub const CAP_SYS_TIME: Capabilities = Capabilities::SYS_TIME;
pub const CAP_SYS_TTY_CONFIG: Capabilities = Capabilities::SYS_TTY_CONFIG;
pub const CAP_WAKE_ALARM: Capabilities = Capabilities::WAKE_ALARM;
// keep-sorted end
pub const CAP_LAST_CAP: u32 = uapi::CAP_LAST_CAP;

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct PtraceAccessMode: u32 {
        const READ      = 1 << 0;
        const ATTACH    = 1 << 1;
        const FSCREDS   = 1 << 2;
        const REALCREDS = 1 << 3;
        const NOAUDIT   = 1 << 4;
    }
}

pub const PTRACE_MODE_READ: PtraceAccessMode = PtraceAccessMode::READ;
pub const PTRACE_MODE_ATTACH: PtraceAccessMode = PtraceAccessMode::ATTACH;
pub const PTRACE_MODE_FSCREDS: PtraceAccessMode = PtraceAccessMode::FSCREDS;
pub const PTRACE_MODE_REALCREDS: PtraceAccessMode = PtraceAccessMode::REALCREDS;
pub const PTRACE_MODE_READ_FSCREDS: PtraceAccessMode = PtraceAccessMode::from_bits_truncate(
    PtraceAccessMode::READ.bits() | PtraceAccessMode::FSCREDS.bits(),
);
pub const PTRACE_MODE_READ_REALCREDS: PtraceAccessMode = PtraceAccessMode::from_bits_truncate(
    PtraceAccessMode::READ.bits() | PtraceAccessMode::REALCREDS.bits(),
);
pub const PTRACE_MODE_ATTACH_FSCREDS: PtraceAccessMode = PtraceAccessMode::from_bits_truncate(
    PtraceAccessMode::ATTACH.bits() | PtraceAccessMode::FSCREDS.bits(),
);
pub const PTRACE_MODE_ATTACH_REALCREDS: PtraceAccessMode = PtraceAccessMode::from_bits_truncate(
    PtraceAccessMode::ATTACH.bits() | PtraceAccessMode::REALCREDS.bits(),
);
pub const PTRACE_MODE_NOAUDIT: PtraceAccessMode = PtraceAccessMode::NOAUDIT;

#[derive(Debug, Clone, RcuDroppable)]
pub struct Credentials {
    pub uid: uid_t,
    pub gid: gid_t,
    pub euid: uid_t,
    pub egid: gid_t,
    pub saved_uid: uid_t,
    pub saved_gid: gid_t,
    pub groups: Vec<gid_t>,

    /// See https://man7.org/linux/man-pages/man2/setfsuid.2.html
    pub fsuid: uid_t,

    /// See https://man7.org/linux/man-pages/man2/setfsgid.2.html
    pub fsgid: gid_t,

    /// From https://man7.org/linux/man-pages/man7/capabilities.7.html
    ///
    /// > This is a limiting superset for the effective capabilities that the thread may assume. It
    /// > is also a limiting superset for the capabilities that may be added to the inheritable set
    /// > by a thread that does not have the CAP_SETPCAP capability in its effective set.
    ///
    /// > If a thread drops a capability from its permitted set, it can never reacquire that
    /// > capability (unless it execve(2)s either a set-user-ID-root program, or a program whose
    /// > associated file capabilities grant that capability).
    pub cap_permitted: Capabilities,

    /// From https://man7.org/linux/man-pages/man7/capabilities.7.html
    ///
    /// > This is the set of capabilities used by the kernel to perform permission checks for the
    /// > thread.
    pub cap_effective: Capabilities,

    /// From https://man7.org/linux/man-pages/man7/capabilities.7.html
    ///
    /// > This is a set of capabilities preserved across an execve(2).  Inheritable capabilities
    /// > remain inheritable when executing any program, and inheritable capabilities are added to
    /// > the permitted set when executing a program that has the corresponding bits set in the file
    /// > inheritable set.
    ///
    /// > Because inheritable capabilities are not generally preserved across execve(2) when running
    /// > as a non-root user, applications that wish to run helper programs with elevated
    /// > capabilities should consider using ambient capabilities, described below.
    pub cap_inheritable: Capabilities,

    /// From https://man7.org/linux/man-pages/man7/capabilities.7.html
    ///
    /// > The capability bounding set is a mechanism that can be used to limit the capabilities that
    /// > are gained during execve(2).
    ///
    /// > Since Linux 2.6.25, this is a per-thread capability set. In older kernels, the capability
    /// > bounding set was a system wide attribute shared by all threads on the system.
    pub cap_bounding: Capabilities,

    /// From https://man7.org/linux/man-pages/man7/capabilities.7.html
    ///
    /// > This is a set of capabilities that are preserved across an execve(2) of a program that is
    /// > not privileged.  The ambient capability set obeys the invariant that no capability can
    /// > ever be ambient if it is not both permitted and inheritable.
    ///
    /// > Executing a program that changes UID or GID due to the set-user-ID or set-group-ID bits
    /// > or executing a program that has any file capabilities set will clear the ambient set.
    pub cap_ambient: Capabilities,

    /// From https://man7.org/linux/man-pages/man7/capabilities.7.html
    ///
    /// > Starting with kernel 2.6.26, and with a kernel in which file capabilities are enabled,
    /// > Linux implements a set of per-thread securebits flags that can be used to disable special
    /// > handling of capabilities for UID 0 (root).
    ///
    /// > The securebits flags can be modified and retrieved using the prctl(2)
    /// > PR_SET_SECUREBITS and PR_GET_SECUREBITS operations.  The CAP_SETPCAP capability is
    /// > required to modify the flags.
    pub securebits: SecureBits,

    /// The SELinux security state of the task.
    pub security_state: TaskAttrs,
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct SecureBits: u32 {
        const KEEP_CAPS = 1 << uapi::SECURE_KEEP_CAPS;
        const KEEP_CAPS_LOCKED = 1 <<  uapi::SECURE_KEEP_CAPS_LOCKED;
        const NO_SETUID_FIXUP = 1 << uapi::SECURE_NO_SETUID_FIXUP;
        const NO_SETUID_FIXUP_LOCKED = 1 << uapi::SECURE_NO_SETUID_FIXUP_LOCKED;
        const NOROOT = 1 << uapi::SECURE_NOROOT;
        const NOROOT_LOCKED = 1 << uapi::SECURE_NOROOT_LOCKED;
        const NO_CAP_AMBIENT_RAISE = 1 << uapi::SECURE_NO_CAP_AMBIENT_RAISE;
        const NO_CAP_AMBIENT_RAISE_LOCKED = 1 << uapi::SECURE_NO_CAP_AMBIENT_RAISE_LOCKED;
    }
}

// SAFETY: SecureBits is a u32 bitflags.
unsafe impl RcuDroppable for SecureBits {}

static ROOT_CREDENTIALS: LazyLock<Arc<Credentials>> =
    LazyLock::new(|| Arc::new(Credentials::with_ids(0, 0)));

impl Credentials {
    /// Creates a set of credentials with all possible permissions and capabilities.
    pub fn root() -> Arc<Self> {
        ROOT_CREDENTIALS.clone()
    }

    /// Creates a set of credentials with the given uid and gid. If the uid is 0, the credentials
    /// will grant superuser access.
    pub fn with_ids(uid: uid_t, gid: gid_t) -> Credentials {
        let caps = if uid == 0 { Capabilities::all() } else { Capabilities::empty() };
        Credentials {
            uid,
            gid,
            euid: uid,
            egid: gid,
            saved_uid: uid,
            saved_gid: gid,
            groups: vec![],
            fsuid: uid,
            fsgid: gid,
            cap_permitted: caps,
            cap_effective: caps,
            cap_inheritable: Capabilities::empty(),
            cap_bounding: Capabilities::all(),
            cap_ambient: Capabilities::empty(),
            securebits: SecureBits::empty(),
            security_state: TaskAttrs::for_kernel(),
        }
    }

    pub fn is_in_group(&self, gid: gid_t) -> bool {
        self.egid == gid || self.groups.contains(&gid)
    }

    /// Updates the `securebits` field, taking into account *`_LOCKED` bits.
    pub fn set_securebits(&mut self, securebits: SecureBits) -> Result<(), Errno> {
        // If a lock bit is set then neither it nor the corresponding `SECBIT_*` can be changed.
        let mut locked_bits = SecureBits::empty();
        if self.securebits.contains(SecureBits::NOROOT_LOCKED) {
            locked_bits |= SecureBits::NOROOT | SecureBits::NOROOT_LOCKED;
        }
        if self.securebits.contains(SecureBits::KEEP_CAPS_LOCKED) {
            locked_bits |= SecureBits::KEEP_CAPS | SecureBits::KEEP_CAPS_LOCKED;
        }
        if self.securebits.contains(SecureBits::NO_SETUID_FIXUP_LOCKED) {
            locked_bits |= SecureBits::NO_SETUID_FIXUP | SecureBits::NO_SETUID_FIXUP_LOCKED;
        }
        if self.securebits.contains(SecureBits::NO_CAP_AMBIENT_RAISE_LOCKED) {
            locked_bits |=
                SecureBits::NO_CAP_AMBIENT_RAISE | SecureBits::NO_CAP_AMBIENT_RAISE_LOCKED;
        }

        if securebits & locked_bits != self.securebits & locked_bits {
            return error!(EPERM);
        }

        self.securebits = securebits;
        Ok(())
    }

    pub fn as_fscred(&self) -> FsCred {
        FsCred { uid: self.fsuid, gid: self.fsgid }
    }

    pub fn euid_as_fscred(&self) -> FsCred {
        FsCred { uid: self.euid, gid: self.egid }
    }

    pub fn uid_as_fscred(&self) -> FsCred {
        FsCred { uid: self.uid, gid: self.gid }
    }

    /// Adjusts the capability sets (permitted, effective, and ambient) of these credentials
    /// to reflect changes in user IDs (UID, EUID, Saved UID, or FSUID) from a previous state.
    ///
    /// This method compares the current state of these credentials against the `prev`
    /// credentials to implement the Linux security model rules for UID transitions (as described
    /// in `capabilities(7)` under "Effect of user ID changes on capabilities"). It is typically
    /// called when preparing a new set of `Credentials` during `setuid()` family syscalls or
    /// during `exec` after UID/GID bits have been applied.
    pub fn update_capabilities(&mut self, prev: &Credentials) {
        // https://man7.org/linux/man-pages/man7/capabilities.7.html
        // If one or more of the real, effective, or saved set user IDs
        // was previously 0, and as a result of the UID changes all of
        // these IDs have a nonzero value, then all capabilities are
        // cleared from the permitted, effective, and ambient capability
        // sets.
        //
        // SECBIT_KEEP_CAPS: Setting this flag allows a thread that has one or more 0
        // UIDs to retain capabilities in its permitted set when it
        // switches all of its UIDs to nonzero values.
        // The setting of the SECBIT_KEEP_CAPS flag is ignored if the
        // SECBIT_NO_SETUID_FIXUP flag is set.  (The latter flag
        // provides a superset of the effect of the former flag.)
        // SECBIT_NO_SETUID_FIXUP: Setting  this  flag  stops  the  kernel from adjusting
        // the process's permitted, effective, and ambient capability sets when the thread's
        // effective and filesystem UIDs are switched between zero and nonzero values.
        if self.securebits.contains(SecureBits::NO_SETUID_FIXUP) {
            return;
        }
        let was_any_zero = prev.uid == 0 || prev.euid == 0 || prev.saved_uid == 0;
        let is_all_nonzero = self.uid != 0 && self.euid != 0 && self.saved_uid != 0;

        if was_any_zero && is_all_nonzero {
            if !self.securebits.contains(SecureBits::KEEP_CAPS) {
                self.cap_permitted = Capabilities::empty();
                self.cap_effective = Capabilities::empty();
            }
            self.cap_ambient = Capabilities::empty();
        }
        // If the effective user ID is changed from 0 to nonzero, then
        // all capabilities are cleared from the effective set.
        if prev.euid == 0 && self.euid != 0 {
            self.cap_effective = Capabilities::empty();
        } else if prev.euid != 0 && self.euid == 0 {
            // If the effective user ID is changed from nonzero to 0, then
            // the permitted set is copied to the effective set.
            self.cap_effective = self.cap_permitted;
        }

        // If the filesystem user ID is changed from 0 to nonzero (see
        // setfsuid(2)), then the following capabilities are cleared from
        // the effective set: CAP_CHOWN, CAP_DAC_OVERRIDE,
        // CAP_DAC_READ_SEARCH, CAP_FOWNER, CAP_FSETID,
        // CAP_LINUX_IMMUTABLE (since Linux 2.6.30), CAP_MAC_OVERRIDE,
        // and CAP_MKNOD (since Linux 2.6.30).
        let fs_capabilities = CAP_CHOWN
            | CAP_DAC_OVERRIDE
            | CAP_DAC_READ_SEARCH
            | CAP_FOWNER
            | CAP_FSETID
            | CAP_LINUX_IMMUTABLE
            | CAP_MAC_OVERRIDE
            | CAP_MKNOD;
        if prev.fsuid == 0 && self.fsuid != 0 {
            self.cap_effective &= !fs_capabilities;
        } else if prev.fsuid != 0 && self.fsuid == 0 {
            // If the filesystem UID is changed from nonzero to 0, then any
            // of these capabilities that are enabled in the permitted set
            // are enabled in the effective set.
            self.cap_effective |= self.cap_permitted & fs_capabilities;
        }
    }
}

/// The owner and group of a file. Used as a parameter for functions that create files.
#[derive(Debug, Clone, Copy)]
pub struct FsCred {
    pub uid: uid_t,
    pub gid: gid_t,
}

impl FsCred {
    pub const fn root() -> Self {
        Self { uid: 0, gid: 0 }
    }
}

impl From<Credentials> for FsCred {
    fn from(c: Credentials) -> Self {
        c.as_fscred()
    }
}

#[derive(Debug, Default, Clone)]
pub struct UserAndOrGroupId {
    pub uid: Option<uid_t>,
    pub gid: Option<gid_t>,
}

impl UserAndOrGroupId {
    pub fn is_none(&self) -> bool {
        self.uid.is_none() && self.gid.is_none()
    }

    pub fn is_some(&self) -> bool {
        !self.is_none()
    }

    pub fn clear(&mut self) {
        self.uid = None;
        self.gid = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::fuchsia::test]
    fn test_empty() {
        assert_eq!(Capabilities::empty().bits(), 0);
    }

    #[::fuchsia::test]
    fn test_union() {
        let expected = CAP_BLOCK_SUSPEND | CAP_AUDIT_READ;
        assert_eq!(CAP_BLOCK_SUSPEND.union(CAP_AUDIT_READ), expected);
        assert_eq!(CAP_BLOCK_SUSPEND.union(CAP_BLOCK_SUSPEND), CAP_BLOCK_SUSPEND);
    }

    #[::fuchsia::test]
    fn test_difference() {
        let base = CAP_BPF | CAP_AUDIT_WRITE;
        let expected = CAP_BPF;
        assert_eq!(base.difference(CAP_AUDIT_WRITE), expected);
        assert_eq!(base.difference(CAP_AUDIT_WRITE | CAP_BPF), Capabilities::empty());
    }

    #[::fuchsia::test]
    fn test_contains() {
        let base = CAP_BPF | CAP_AUDIT_WRITE;
        assert!(base.contains(CAP_AUDIT_WRITE));
        assert!(base.contains(CAP_BPF));
        assert!(base.contains(CAP_AUDIT_WRITE | CAP_BPF));

        assert!(!base.contains(CAP_AUDIT_CONTROL));
        assert!(!base.contains(CAP_AUDIT_WRITE | CAP_BPF | CAP_AUDIT_CONTROL));
    }

    #[::fuchsia::test]
    fn test_insert() {
        let mut capabilities = CAP_BLOCK_SUSPEND;
        capabilities.insert(CAP_BLOCK_SUSPEND);
        assert_eq!(capabilities, CAP_BLOCK_SUSPEND);

        capabilities.insert(CAP_AUDIT_READ);
        let expected = CAP_BLOCK_SUSPEND | CAP_AUDIT_READ;
        assert_eq!(capabilities, expected);
    }

    #[::fuchsia::test]
    fn test_remove() {
        let mut capabilities = CAP_BLOCK_SUSPEND;
        capabilities.remove(CAP_BLOCK_SUSPEND);
        assert_eq!(capabilities, Capabilities::empty());

        let mut capabilities = CAP_BLOCK_SUSPEND | CAP_AUDIT_READ;
        capabilities.remove(CAP_AUDIT_READ);
        assert_eq!(capabilities, CAP_BLOCK_SUSPEND);
    }

    #[::fuchsia::test]
    fn test_try_from() {
        let capabilities = CAP_BLOCK_SUSPEND;
        assert_eq!(Capabilities::try_from(uapi::CAP_BLOCK_SUSPEND as u64), Ok(capabilities));

        assert_eq!(Capabilities::try_from(200000), error!(EINVAL));
    }
}
