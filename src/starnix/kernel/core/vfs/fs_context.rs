// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security;
use crate::task::{CurrentTask, MountsWriteToken};
use crate::vfs::{AccessCheck, ActiveNamespaceNode, Namespace, NamespaceNode};
use starnix_logging::log_trace;
use starnix_sync::LockDepRwLock;
use starnix_uapi::auth::CAP_SYS_CHROOT;
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use std::sync::Arc;

/// The mutable state for an FsContext.
///
/// This state is cloned in FsContext::fork.
#[derive(Debug, Clone)]
struct FsContextState {
    /// The namespace tree for this FsContext.
    ///
    /// This field owns the mount table for this FsContext.
    namespace: Arc<Namespace>,

    /// The root of the namespace tree for this FsContext.
    ///
    /// Operations on the file system are typically either relative to this
    /// root or to the cwd().
    root: ActiveNamespaceNode,

    /// The current working directory.
    cwd: ActiveNamespaceNode,

    // See <https://man7.org/linux/man-pages/man2/umask.2.html>
    umask: FileMode,
}

impl FsContextState {
    fn set_namespace(
        &mut self,
        new_ns: Arc<Namespace>,
        mounts_guard: &MountsWriteToken,
    ) -> Result<(), Errno> {
        log_trace!("updating namespace");
        let new_root = Namespace::translate_node(self.root.to_passive(), &new_ns, mounts_guard)
            .ok_or_else(|| errno!(EINVAL))?;
        let new_cwd = Namespace::translate_node(self.cwd.to_passive(), &new_ns, mounts_guard)
            .ok_or_else(|| errno!(EINVAL))?;

        // Only perform a mutation if the rebased nodes both exist in the target namespace.
        mounts_guard.defer_drop(std::mem::replace(&mut self.root, new_root.into_active()));
        mounts_guard.defer_drop(std::mem::replace(&mut self.cwd, new_cwd.into_active()));
        mounts_guard.defer_drop(std::mem::replace(&mut self.namespace, new_ns));
        log_trace!("namespace update succeeded");
        Ok(())
    }
}

/// The file system context associated with a task.
///
/// File system operations, such as opening a file or mounting a directory, are
/// performed using this context.
#[derive(Debug)]
pub struct FsContext {
    state: LockDepRwLock<FsContextState, starnix_sync::FsContextStateLock>,
}

impl FsContext {
    /// Create an FsContext for the given namespace.
    ///
    /// The root and cwd of the FsContext are initialized to the root of the
    /// namespace.
    pub fn new(namespace: Arc<Namespace>) -> Arc<FsContext> {
        let root = namespace.root();
        Arc::new(FsContext {
            state: FsContextState {
                namespace,
                root: root.clone().into_active(),
                cwd: root.into_active(),
                umask: FileMode::DEFAULT_UMASK,
            }
            .into(),
        })
    }

    pub fn fork(&self) -> Arc<FsContext> {
        // A child process created via fork(2) inherits its parent's umask.
        // The umask is left unchanged by execve(2).
        //
        // See <https://man7.org/linux/man-pages/man2/umask.2.html>

        Arc::new(FsContext { state: self.state.read().clone().into() })
    }

    /// Returns a reference to the current working directory.
    pub fn cwd(&self) -> NamespaceNode {
        let state = self.state.read();
        state.cwd.to_passive()
    }

    /// Returns the root.
    pub fn root(&self) -> NamespaceNode {
        let state = self.state.read();
        state.root.to_passive()
    }

    /// Change the current working directory.
    pub fn chdir(&self, current_task: &CurrentTask, name: NamespaceNode) -> Result<(), Errno> {
        name.check_access(current_task, AccessCheck::for_chdir())?;
        let mut state = self.state.write();
        state.cwd = name.into_active();
        Ok(())
    }

    /// Change the root.
    pub fn chroot(&self, current_task: &CurrentTask, name: NamespaceNode) -> Result<(), Errno> {
        name.check_access(current_task, AccessCheck::for_chroot()).map_err(|_| errno!(EACCES))?;
        security::check_task_capable(current_task, CAP_SYS_CHROOT)?;

        let mut state = self.state.write();
        state.root = name.into_active();
        Ok(())
    }

    pub fn umask(&self) -> FileMode {
        self.state.read().umask
    }

    pub fn apply_umask(&self, mode: FileMode) -> FileMode {
        let umask = self.state.read().umask;
        mode & !umask
    }

    pub fn set_umask(&self, umask: FileMode) -> FileMode {
        let mut state = self.state.write();
        let old_umask = state.umask;

        // umask() sets the calling process's file mode creation mask
        // (umask) to mask & 0o777 (i.e., only the file permission bits of
        // mask are used), and returns the previous value of the mask.
        //
        // See <https://man7.org/linux/man-pages/man2/umask.2.html>
        state.umask = umask & FileMode::from_bits(0o777);

        old_umask
    }

    pub fn set_namespace(&self, new_ns: Arc<Namespace>) -> Result<(), Errno> {
        let mut state = self.state.write();
        let kernel = state.namespace.kernel();
        let mounts_guard = kernel.mounts_lock();
        state.set_namespace(new_ns, &mounts_guard)?;
        Ok(())
    }

    pub fn unshare_namespace(&self) {
        let mut state = self.state.write();
        let kernel = state.namespace.kernel();
        let mounts_guard = kernel.mounts_lock();

        let cloned = state.namespace.clone_namespace(&mounts_guard);
        state
            .set_namespace(cloned, &mounts_guard)
            .expect("nodes should exist in the cloned namespace");
    }

    pub fn namespace(&self) -> Arc<Namespace> {
        Arc::clone(&self.state.read().namespace)
    }

    pub fn update_root_after_pivot(&self, old_root: &NamespaceNode, new_root: &NamespaceNode) {
        let mut state = self.state.write();
        if state.root.to_passive() == *old_root {
            state.root = new_root.clone().into_active();
        }
        if state.cwd.to_passive() == *old_root {
            state.cwd = new_root.clone().into_active();
        }
    }
}

#[cfg(test)]
mod test {
    use crate::fs::tmpfs::TmpFs;
    use crate::testing::{spawn_kernel_and_run, spawn_kernel_and_run_with_pkgfs};
    use crate::vfs::{FsContext, Namespace};
    use starnix_uapi::file_mode::FileMode;
    use starnix_uapi::open_flags::OpenFlags;

    #[::fuchsia::test]
    async fn test_umask() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = FsContext::new(Namespace::new(TmpFs::new_fs(&kernel)));

            assert_eq!(FileMode::from_bits(0o22), fs.set_umask(FileMode::from_bits(0o3020)));
            assert_eq!(FileMode::from_bits(0o646), fs.apply_umask(FileMode::from_bits(0o666)));
            assert_eq!(FileMode::from_bits(0o3646), fs.apply_umask(FileMode::from_bits(0o3666)));
            assert_eq!(FileMode::from_bits(0o20), fs.set_umask(FileMode::from_bits(0o11)));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_chdir() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            assert_eq!("/", current_task.fs().cwd().path_escaping_chroot());

            let bin = current_task
                .open_file("bin".into(), OpenFlags::RDONLY)
                .expect("missing bin directory");
            current_task.fs().chdir(&current_task, bin.name.to_passive()).expect("Failed to chdir");
            assert_eq!("/bin", current_task.fs().cwd().path_escaping_chroot());

            // Now that we have changed directories to bin, we're opening a file
            // relative to that directory, which doesn't exist.
            assert!(current_task.open_file("bin".into(), OpenFlags::RDONLY).is_err());

            // However, bin still exists in the root directory.
            assert!(current_task.open_file("/bin".into(), OpenFlags::RDONLY).is_ok());

            let previous_directory = current_task
                .open_file("..".into(), OpenFlags::RDONLY)
                .expect("failed to open ..")
                .name
                .to_passive();
            current_task.fs().chdir(&current_task, previous_directory).expect("Failed to chdir");
            assert_eq!("/", current_task.fs().cwd().path_escaping_chroot());

            // Now bin exists again because we've gone back to the root.
            assert!(current_task.open_file("bin".into(), OpenFlags::RDONLY).is_ok());

            // Repeating the .. doesn't do anything because we're already at the root.
            let previous_directory = current_task
                .open_file("..".into(), OpenFlags::RDONLY)
                .expect("failed to open ..")
                .name
                .to_passive();
            current_task.fs().chdir(&current_task, previous_directory).expect("Failed to chdir");
            assert_eq!("/", current_task.fs().cwd().path_escaping_chroot());
            assert!(current_task.open_file("bin".into(), OpenFlags::RDONLY).is_ok());
        })
        .await;
    }
}
