// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module provides types and hook APIs supporting Linux Security Modules
//! functionality in Starnix.  LSM provides a generic set of hooks, and opaque
//! types, used to decouple the rest of the kernel from the details of any
//! specific security enforcement subsystem (e.g. SELinux, POSIX.1e, etc).
//!
//! Although this module is hard-wired to the SELinux implementation, callers
//! should treat the types as opaque; hook implementations necessarily have access
//! to kernel structures, but not the other way around.

use selinux::{SecurityId, SecurityServer};
use std::sync::Arc;

/// SELinux implementations called by the LSM hooks.
mod selinux_hooks;
pub use selinux_hooks::audit::Auditable;

/// Common capabilities hook implementations called by the LSM hooks.
mod common_cap;

/// YAMA hook implementations used to restirct ptrace access.
pub mod yama;

/// Linux Security Modules hooks for use within the Starnix kernel.
mod hooks;
pub use hooks::*;

/// Audit logging to be used from different kernel components
mod audit;
pub use audit::*;

/// Opaque structure encapsulating security subsystem state for the whole system.
pub struct KernelState {
    state: Option<selinux_hooks::KernelState>,
}

impl KernelState {
    pub fn access_denial_count(&self) -> u64 {
        self.state.as_ref().map_or(0u64, |state| state.access_denial_count())
    }
}

/// The opaque type used by [`crate::vfs::FsNodeInfo`] to store security state.
#[derive(Debug, Default)]
pub struct FsNodeState(selinux_hooks::FsNodeState);

/// Opaque structure holding security state for a [`binderfs::BinderConnection`].
#[derive(Debug)]
pub struct BinderConnectionState {
    state: selinux_hooks::BinderConnectionState,
}

/// Opaque structure holding security state for a [`crate::vfs::socket::Socket`].
#[derive(Debug, Default)]
pub struct SocketState {
    state: selinux_hooks::SocketState,
}

/// Opaque structure holding security state for a [`crate::vfs::FileObject`].
#[derive(Debug)]
pub struct FileObjectState {
    state: selinux_hooks::FileObjectState,
}

/// Opaque structure holding security state for a [`crate::vfs::FileSystem`].
#[derive(Debug)]
pub struct FileSystemState {
    state: selinux_hooks::FileSystemState,
}

/// Opaque structure holding security state for a bpf [`ebpf_api::maps::Map`].
#[derive(Debug)]
pub struct BpfMapState {
    state: selinux_hooks::BpfMapState,
}

/// Opaque structure holding security state for a bpf [`crate::bpf::program::Program`].
#[derive(Debug)]
pub struct BpfProgState {
    state: selinux_hooks::BpfProgState,
}

/// Opaque structure holding security state for a PerfEventFileState.
#[derive(Debug)]
pub struct PerfEventState {
    state: selinux_hooks::PerfEventState,
}

/// Opaque structure holding security state for a current task.
#[derive(Default, Debug)]
pub struct CurrentTaskState {
    pub state: selinux_hooks::CurrentTaskState,
}
