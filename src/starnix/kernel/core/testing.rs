// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::mem::new_null_file;
use crate::execution::{
    create_init_child_process, create_init_process, create_system_task,
    execute_task_with_prerun_result,
};
use crate::fs::fuchsia::RemoteFs;
use crate::fs::tmpfs::TmpFs;
use crate::mm::syscalls::{do_mmap, sys_mremap};
use crate::mm::{MemoryAccessor, MemoryAccessorExt, MemoryManager, PAGE_SIZE};
use crate::security;
use crate::task::container_namespace::ContainerNamespace;
use crate::task::{
    CurrentTask, ExitStatus, Kernel, KernelFeatures, SchedulerManager, SystemLimits, Task,
    TaskBuilder,
};
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::{
    Anon, CacheMode, DirEntry, FdNumber, FileHandle, FileObject, FileOps, FileSystem,
    FileSystemHandle, FileSystemOps, FileSystemOptions, FsContext, FsNode, FsNodeFlags,
    FsNodeHandle, FsNodeInfo, FsNodeOps, FsStr, Namespace, NamespaceNode, fileops_impl_nonseekable,
    fileops_impl_noop_sync, fs_node_impl_not_dir,
};
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_async::LocalExecutor;
use selinux::SecurityServer;
use starnix_syscalls::{SyscallArg, SyscallResult};
use starnix_task_command::TaskCommand;
use starnix_types::arch::ArchWidth;
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::{Credentials, FsCred};
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{ArchSpecific, UserAddress};
use starnix_uapi::{MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE, errno, error, statfs};
use std::ffi::CString;
use std::future::Future;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::sync::{Arc, mpsc};
use zerocopy::{Immutable, IntoBytes};

/// Create a FileSystemHandle for use in testing.
///
/// Open "/pkg" and returns an FsContext rooted in that directory.
fn create_pkgfs(kernel: &Kernel) -> FileSystemHandle {
    let rights = fio::PERM_READABLE | fio::PERM_EXECUTABLE;
    let (server, client) = zx::Channel::create();
    fdio::open("/pkg", rights, server).expect("failed to open /pkg");
    RemoteFs::new_fs(
        kernel,
        client,
        FileSystemOptions { source: "/pkg".into(), ..Default::default() },
        rights,
    )
    .unwrap()
}

/// Create a Kernel object and run the given callback in the init process for that kernel.
///
/// This function is useful if you want to test code that requires a CurrentTask because
/// your callback is called with the init process as the CurrentTask.
pub fn spawn_kernel_and_run<F, R>(callback: F) -> impl Future<Output = R>
where
    F: AsyncFnOnce(&mut CurrentTask) -> R + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    spawn_kernel_and_run_internal(callback, None, TmpFs::new_fs, KernelFeatures::default())
}

/// Create and run a kernel with non-default feature settings.
pub fn spawn_kernel_with_features_and_run<F, R>(
    callback: F,
    features: KernelFeatures,
) -> impl Future<Output = R>
where
    F: AsyncFnOnce(&mut CurrentTask) -> R + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    spawn_kernel_and_run_internal(callback, None, TmpFs::new_fs, features)
}

/// Create a Kernel object and run the given synchronous callback in the init process for that kernel.
///
/// This function is useful if you want to test code that requires a CurrentTask because
/// your callback is called with the init process as the CurrentTask.
pub fn spawn_kernel_and_run_sync<F, R>(callback: F) -> impl Future<Output = R>
where
    F: FnOnce(&mut CurrentTask) -> R + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    spawn_kernel_and_run_internal_sync(
        callback,
        None,
        TmpFs::new_fs,
        KernelFeatures::default(),
        SchedulerManager::empty_for_tests(),
    )
}

/// Create a Kernel object with a custom SchedulerManager and run the given synchronous callback.
pub fn spawn_kernel_with_scheduler_and_run_sync<F, R>(
    scheduler: SchedulerManager,
    callback: F,
) -> impl Future<Output = R>
where
    F: FnOnce(&mut CurrentTask) -> R + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    spawn_kernel_and_run_internal_sync(
        callback,
        None,
        TmpFs::new_fs,
        KernelFeatures::default(),
        scheduler,
    )
}

/// Create a Kernel object and run the given callback in the init process for that kernel.
/// The task is rooted in a `pkgfs` instance.
///
/// This function is useful if you want to test code that requires a CurrentTask because
/// your callback is called with the init process as the CurrentTask.
pub fn spawn_kernel_and_run_with_pkgfs<F, R>(callback: F) -> impl Future<Output = R>
where
    F: AsyncFnOnce(&mut CurrentTask) -> R + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    spawn_kernel_and_run_internal(callback, None, create_pkgfs, KernelFeatures::default())
}

/// Variant of `spawn_kernel_and_run()` that configures the kernel with SELinux enabled.
/// The supplied `callback` is invoked with an additional argument providing test access to the
/// SELinux security-server.
// TODO: https://fxbug.dev/335397745 - Only provide an admin/test API to the test, so that tests
// must generally exercise hooks via public entrypoints.
pub async fn spawn_kernel_with_selinux_and_run<F, R>(callback: F) -> R
where
    F: AsyncFnOnce(&mut CurrentTask, &Arc<SecurityServer>) -> R + Send + Sync + 'static,
    R: Send + Sync + 'static,
{
    let security_server = SecurityServer::new_default();
    let security_server_for_callback = security_server.clone();
    spawn_kernel_and_run_internal(
        async move |current_task| {
            security::selinuxfs_init_null(
                current_task,
                &new_null_file(current_task, OpenFlags::empty()),
            );
            callback(current_task, &security_server_for_callback).await
        },
        Some(security_server),
        TmpFs::new_fs,
        KernelFeatures::default(),
    )
    .await
}

/// Create a Kernel object, with the optional caller-supplied `security_server`, and run the given
/// callback in the init process for that kernel.
fn spawn_kernel_and_run_internal<F, FS, R>(
    callback: F,
    security_server: Option<Arc<SecurityServer>>,
    fs_factory: FS,
    features: KernelFeatures,
) -> impl Future<Output = R>
where
    R: Send + Sync + 'static,
    F: AsyncFnOnce(&mut CurrentTask) -> R + Send + Sync + 'static,
    FS: FnOnce(&Kernel) -> FileSystemHandle,
{
    spawn_kernel_and_run_internal_sync(
        move |current_task| LocalExecutor::default().run_singlethreaded(callback(current_task)),
        security_server,
        fs_factory,
        features,
        SchedulerManager::empty_for_tests(),
    )
}

/// Create a Kernel object, with the optional caller-supplied `security_server`, and run the given
/// synchronous callback in the init process for that kernel.
fn spawn_kernel_and_run_internal_sync<F, FS, R>(
    callback: F,
    security_server: Option<Arc<SecurityServer>>,
    fs_factory: FS,
    features: KernelFeatures,
    scheduler: SchedulerManager,
) -> impl Future<Output = R>
where
    R: Send + Sync + 'static,
    F: FnOnce(&mut CurrentTask) -> R + Send + Sync + 'static,
    FS: FnOnce(&Kernel) -> FileSystemHandle,
{
    let kernel = create_test_kernel(security_server, features, scheduler);
    let fs = create_test_fs_context(&kernel, fs_factory);
    let init_task = create_test_init_task(&kernel, fs);
    fasync::unblock(move || {
        let (sender, receiver) = mpsc::sync_channel(1);
        let error = execute_task_with_prerun_result(
            init_task,
            move |current_task| -> Result<(), Errno> {
                let result = callback(current_task);
                current_task.write().set_exit_status_if_not_already(ExitStatus::Exit(0));
                sender.send(result).map_err(|e| errno!(EIO, e))?;
                error!(EHWPOISON)
            },
            |_| {},
            None,
        )
        .unwrap_err();
        // EHWPOISON is expected from the pre_run task, any other error is returned.
        assert_eq!(error, errno!(EHWPOISON));
        receiver.recv().expect("recv")
    })
}

fn create_test_kernel(
    security_server: Option<Arc<SecurityServer>>,
    features: KernelFeatures,
    scheduler: SchedulerManager,
) -> Arc<Kernel> {
    Kernel::new(
        b"".into(),
        features,
        SystemLimits::default(),
        ContainerNamespace::new(),
        scheduler,
        None,
        fuchsia_inspect::Node::default(),
        security::testing::kernel_state(security_server),
        /* time_adjustment_proxy=*/ None,
        /* device_tree=*/ None,
    )
    .expect("failed to create kernel")
}

fn create_test_fs_context(
    kernel: &Kernel,
    create_fs: impl FnOnce(&Kernel) -> FileSystemHandle,
) -> Arc<FsContext> {
    FsContext::new(Namespace::new(create_fs(kernel)))
}

/// Initializes a 64-bit address-space for the specified `task`.
fn create_test_mm(task: &Task) -> Result<Arc<MemoryManager>, Errno> {
    let arch_width = ArchWidth::Arch64;
    let mm =
        MemoryManager::new_for_test(task.thread_group().root_vmar.unowned(), ArchWidth::Arch64);
    let fake_executable_addr = mm.get_random_base_for_executable(arch_width, 0)?;
    mm.initialize_brk_origin(arch_width, fake_executable_addr)?;
    task.running_state()?.mm.update(Some(mm.clone()));
    Ok(mm)
}

fn create_test_init_task(kernel: &Kernel, fs: Arc<FsContext>) -> TaskBuilder {
    let init_pid = kernel.pids.lock().allocate_pid().expect("allocating init pid");
    assert_eq!(init_pid.id, 1);
    let init_task = create_init_process(
        &kernel.weak_self.upgrade().unwrap(),
        init_pid,
        TaskCommand::new(b"test-task"),
        fs.fork(),
        &[],
    )
    .expect("failed to create first task");
    create_test_mm(&init_task).expect("failed to create MM");

    let system_task =
        create_system_task(&kernel.weak_self.upgrade().unwrap(), fs).expect("create system task");
    kernel.kthreads.init(system_task).expect("failed to initialize kthreads");

    let system_task = kernel.kthreads.system_task();
    kernel.hrtimer_manager.init(&system_task).expect("init hrtimer manager worker thread");

    // Take the lock on thread group and task in the correct order to ensure any wrong ordering
    // will trigger the tracing-mutex at the right call site.
    {
        let _l1 = init_task.thread_group().read();
        let _l2 = init_task.read();
    }
    init_task
}

/// An old way of creating a task for testing
///
/// This way of creating a task has problems because the test isn't actually run with that task
/// being current, which means that functions that expect a CurrentTask to actually be mapped into
/// memory can operate incorrectly.
///
/// Please use `spawn_kernel_and_run` instead. If there isn't a variant of `spawn_kernel_and_run`
/// for this use case, please consider adding one that follows the new pattern of actually running
/// the test on the spawned task.
pub fn create_task(kernel: &Kernel, task_name: &str) -> AutoReleasableTask {
    create_task_with_security_context(kernel, task_name, &CString::new("#kernel").unwrap())
}

/// An old way of creating a task for testing, with a given security context.
///
/// See caveats on `create_task`.
pub fn create_task_with_security_context(
    kernel: &Kernel,
    task_name: &str,
    security_context: &CString,
) -> AutoReleasableTask {
    let task = create_init_child_process(
        &kernel.weak_self.upgrade().unwrap(),
        TaskCommand::new(task_name.as_bytes()),
        Credentials::with_ids(0, 0),
        Some(security_context),
    )
    .expect("failed to create second task");
    create_test_mm(&task).expect("failed to create MM");

    // Take the lock on thread group and task in the correct order to ensure any wrong ordering
    // will trigger the tracing-mutex at the right call site.
    {
        let _l1 = task.thread_group().read();
        let _l2 = task.read();
    }

    task.into()
}

/// Maps a region of mery at least `len` bytes long with `PROT_READ | PROT_WRITE`,
/// `MAP_ANONYMOUS | MAP_PRIVATE`, returning the mapped address.
pub fn map_memory_anywhere(current_task: &CurrentTask, len: u64) -> UserAddress {
    map_memory(current_task, UserAddress::NULL, len)
}

/// Maps a region of memory large enough for the object with `PROT_READ | PROT_WRITE`,
/// `MAP_ANONYMOUS | MAP_PRIVATE` and writes the object to it, returning the mapped address.
///
/// Useful for syscall in-pointer parameters.
pub fn map_object_anywhere<T>(current_task: &CurrentTask, object: &T) -> UserAddress
where
    T: IntoBytes + Immutable,
{
    let addr = map_memory_anywhere(current_task, std::mem::size_of::<T>() as u64);
    current_task.write_object(addr.into(), object).expect("could not write object");
    addr
}

/// Maps `length` at `address` with `PROT_READ | PROT_WRITE`, `MAP_ANONYMOUS | MAP_PRIVATE`.
///
/// Returns the address returned by `sys_mmap`.
pub fn map_memory(current_task: &CurrentTask, address: UserAddress, length: u64) -> UserAddress {
    map_memory_with_flags(current_task, address, length, MAP_ANONYMOUS | MAP_PRIVATE)
}

/// Maps `length` at `address` with `PROT_READ | PROT_WRITE` and the specified flags.
///
/// Returns the address returned by `sys_mmap`.
pub fn map_memory_with_flags(
    current_task: &CurrentTask,
    address: UserAddress,
    length: u64,
    flags: u32,
) -> UserAddress {
    do_mmap(
        current_task,
        address,
        length as usize,
        PROT_READ | PROT_WRITE,
        flags,
        FdNumber::from_raw(-1),
        0,
    )
    .expect("Could not map memory")
}

/// Convenience wrapper around [`sys_mremap`] which extracts the returned [`UserAddress`] from
/// the generic [`SyscallResult`].
pub fn remap_memory(
    current_task: &CurrentTask,
    old_addr: UserAddress,
    old_length: u64,
    new_length: u64,
    flags: u32,
    new_addr: UserAddress,
) -> Result<UserAddress, Errno> {
    sys_mremap(current_task, old_addr, old_length as usize, new_length as usize, flags, new_addr)
}

/// Fills one page in the `current_task`'s address space starting at `addr` with the ASCII character
/// `data`. Panics if the write failed.
///
/// This method uses the `#[track_caller]` attribute, which will display the caller's file and line
/// number in the event of a panic. This makes it easier to find test regressions.
#[track_caller]
pub fn fill_page(current_task: &CurrentTask, addr: UserAddress, data: char) {
    let data = [data as u8].repeat(*PAGE_SIZE as usize);
    if let Err(err) = current_task.write_memory(addr, &data) {
        panic!("write page: failed to fill page @ {addr:?} with {data:?}: {err:?}");
    }
}

/// Checks that the page in `current_task`'s address space starting at `addr` is readable.
/// Panics if the read failed, or the page was not filled with the ASCII character `data`.
///
/// This method uses the `#[track_caller]` attribute, which will display the caller's file and line
/// number in the event of a panic. This makes it easier to find test regressions.
#[track_caller]
pub fn check_page_eq(current_task: &CurrentTask, addr: UserAddress, data: char) {
    let buf = match current_task.read_memory_to_vec(addr, *PAGE_SIZE as usize) {
        Ok(b) => b,
        Err(err) => panic!("read page: failed to read page @ {addr:?}: {err:?}"),
    };
    assert!(
        buf.into_iter().all(|c| c == data as u8),
        "unexpected payload: page @ {addr:?} should be filled with {data:?}"
    );
}

/// Checks that the page in `current_task`'s address space starting at `addr` is readable.
/// Panics if the read failed, or the page *was* filled with the ASCII character `data`.
///
/// This method uses the `#[track_caller]` attribute, which will display the caller's file and line
/// number in the event of a panic. This makes it easier to find test regressions.
#[track_caller]
pub fn check_page_ne(current_task: &CurrentTask, addr: UserAddress, data: char) {
    let buf = match current_task.read_memory_to_vec(addr, *PAGE_SIZE as usize) {
        Ok(b) => b,
        Err(err) => panic!("read page: failed to read page @ {addr:?}: {err:?}"),
    };
    assert!(
        !buf.into_iter().all(|c| c == data as u8),
        "unexpected payload: page @ {addr:?} should not be filled with {data:?}"
    );
}

/// Checks that the page in `current_task`'s address space starting at `addr` is unmapped.
/// Panics if the read succeeds, or if an error other than `EFAULT` occurs.
///
/// This method uses the `#[track_caller]` attribute, which will display the caller's file and line
/// number in the event of a panic. This makes it easier to find test regressions.
#[track_caller]
pub fn check_unmapped(current_task: &CurrentTask, addr: UserAddress) {
    match current_task.read_memory_to_vec(addr, *PAGE_SIZE as usize) {
        Ok(_) => panic!("read page: page @ {addr:?} should be unmapped"),
        Err(err) if err == starnix_uapi::errors::EFAULT => {}
        Err(err) => {
            panic!("read page: expected EFAULT reading page @ {addr:?} but got {err:?} instead")
        }
    }
}

/// An FsNodeOps implementation that panics if you try to open it. Useful as a stand-in for testing
/// APIs that require a FsNodeOps implementation but don't actually use it.
pub struct PanickingFsNode;

impl FsNodeOps for PanickingFsNode {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        panic!("should not be called")
    }
}

/// An implementation of [`FileOps`] that panics on any read, write, or ioctl operation.
pub struct PanickingFile;

impl PanickingFile {
    /// Creates a [`FileObject`] whose implementation panics on reads, writes, and ioctls.
    pub fn new_file(current_task: &CurrentTask) -> FileHandle {
        anon_test_file(current_task, Box::new(PanickingFile), OpenFlags::RDWR)
    }
}

impl FileOps for PanickingFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        panic!("write called on TestFile")
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        panic!("read called on TestFile")
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _request: u32,
        _arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        panic!("ioctl called on TestFile")
    }
}

/// Returns a new anonymous test file with the specified `ops` and `flags`.
pub fn anon_test_file(
    current_task: &CurrentTask,
    ops: Box<dyn FileOps>,
    flags: OpenFlags,
) -> FileHandle {
    // TODO: https://fxbug.dev/404739824 - Confirm whether to handle this as a "private" node.
    Anon::new_private_file(current_task, ops, flags, "[fuchsia:test_file]")
}

/// Helper to write out data to a task's memory sequentially.
pub struct UserMemoryWriter<'a> {
    // The task's memory manager.
    mm: &'a Task,
    // The address to which to write the next bit of data.
    current_addr: UserAddress,
}

impl<'a> UserMemoryWriter<'a> {
    /// Constructs a new `UserMemoryWriter` to write to `task`'s memory at `addr`.
    pub fn new(task: &'a Task, addr: UserAddress) -> Self {
        Self { mm: task, current_addr: addr }
    }

    /// Writes all of `data` to the current address in the task's address space, incrementing the
    /// current address by the size of `data`. Returns the address at which the data starts.
    /// Panics on failure.
    pub fn write(&mut self, data: &[u8]) -> UserAddress {
        let bytes_written = self.mm.write_memory(self.current_addr, data).unwrap();
        assert_eq!(bytes_written, data.len());
        let start_addr = self.current_addr;
        self.current_addr = (self.current_addr + bytes_written).unwrap();
        start_addr
    }

    /// Writes `object` to the current address in the task's address space, incrementing the
    /// current address by the size of `object`. Returns the address at which the data starts.
    /// Panics on failure.
    pub fn write_object<T: IntoBytes + Immutable>(&mut self, object: &T) -> UserAddress {
        self.write(object.as_bytes())
    }

    /// Returns the current address at which data will be next written.
    pub fn current_address(&self) -> UserAddress {
        self.current_addr
    }
}

#[derive(Debug)]
pub struct AutoReleasableTask(Option<CurrentTask>);

impl AutoReleasableTask {
    fn as_ref(this: &Self) -> &CurrentTask {
        this.0.as_ref().unwrap()
    }

    fn as_mut(this: &mut Self) -> &mut CurrentTask {
        this.0.as_mut().unwrap()
    }
}

impl From<CurrentTask> for AutoReleasableTask {
    fn from(task: CurrentTask) -> Self {
        Self(Some(task))
    }
}

impl From<TaskBuilder> for AutoReleasableTask {
    fn from(builder: TaskBuilder) -> Self {
        CurrentTask::from(builder).into()
    }
}

impl Drop for AutoReleasableTask {
    fn drop(&mut self) {
        self.0.take().unwrap().release(());
    }
}

impl std::ops::Deref for AutoReleasableTask {
    type Target = CurrentTask;

    fn deref(&self) -> &Self::Target {
        AutoReleasableTask::as_ref(self)
    }
}

impl std::ops::DerefMut for AutoReleasableTask {
    fn deref_mut(&mut self) -> &mut Self::Target {
        AutoReleasableTask::as_mut(self)
    }
}

impl std::borrow::Borrow<CurrentTask> for AutoReleasableTask {
    fn borrow(&self) -> &CurrentTask {
        AutoReleasableTask::as_ref(self)
    }
}

impl std::convert::AsRef<CurrentTask> for AutoReleasableTask {
    fn as_ref(&self) -> &CurrentTask {
        AutoReleasableTask::as_ref(self)
    }
}

impl ArchSpecific for AutoReleasableTask {
    fn is_arch32(&self) -> bool {
        self.deref().is_arch32()
    }
}

impl MemoryAccessor for AutoReleasableTask {
    fn read_memory<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        (**self).read_memory(addr, bytes)
    }
    fn read_memory_partial_until_null_byte<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        (**self).read_memory_partial_until_null_byte(addr, bytes)
    }
    fn read_memory_partial<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        (**self).read_memory_partial(addr, bytes)
    }
    fn write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
        (**self).write_memory(addr, bytes)
    }
    fn write_memory_partial(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
        (**self).write_memory_partial(addr, bytes)
    }
    fn zero(&self, addr: UserAddress, length: usize) -> Result<usize, Errno> {
        (**self).zero(addr, length)
    }
}

struct TestFs;
impl FileSystemOps for TestFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(0))
    }
    fn name(&self) -> &'static FsStr {
        "test".into()
    }
}

pub fn create_testfs(kernel: &Kernel) -> FileSystemHandle {
    FileSystem::new(&kernel, CacheMode::Uncached, TestFs, Default::default())
        .expect("testfs constructed with valid options")
}

pub fn create_testfs_with_root(kernel: &Kernel, ops: impl FsNodeOps) -> FileSystemHandle {
    let test_fs = create_testfs(kernel);
    let root_ino = test_fs.allocate_ino();
    test_fs.create_root(root_ino, ops);
    test_fs
}

pub fn create_fs_node_for_testing(fs: &FileSystemHandle, ops: impl FsNodeOps) -> FsNodeHandle {
    let ino = fs.allocate_ino();
    let info = FsNodeInfo::new(mode!(IFDIR, 0o777), FsCred::root());
    FsNode::new_uncached(ino, ops, fs, info, FsNodeFlags::empty())
}

pub fn create_namespace_node_for_testing(
    fs: &FileSystemHandle,
    ops: impl FsNodeOps,
) -> NamespaceNode {
    let node = create_fs_node_for_testing(fs, ops);
    NamespaceNode::new_anonymous(DirEntry::new_unrooted(node))
}
