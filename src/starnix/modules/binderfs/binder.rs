// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_upper_case_globals)]

use crate::objects::{
    BinderObject, BinderObjectFlags, FlatBinderObject, Handle, LocalBinderObject, RefCountActions,
    SerializedBinderObject, TransactionData,
};
use crate::process::{
    ActiveTransaction, BinderProcess, BinderProcessGuard, RequestType, TransientTransactionState,
};
use crate::resource_accessor::{
    RemoteIoctl, RemoteMemoryAccessor, RemoteResourceAccessor, ResourceAccessor,
};
use crate::shared_memory::{SharedBuffer, SharedMemory, TransactionBuffers};
use crate::thread::{
    BinderThread, BinderThreadGuard, BinderThreadState, Command, QueuedCommand, RegistrationState,
    RequeueEventRegistration, TransactionError, TransactionRole, TransactionSender, WeakBinderPeer,
};
use crate::user_memory_cursor::UserMemoryCursor;
use fidl::endpoints::ClientEnd;
use starnix_core::device::DeviceOps;
use starnix_core::device::mem::new_null_file;
use starnix_core::fs::fuchsia::new_remote_file;
use starnix_core::mm::memory::MemoryObject;
use starnix_core::mm::{
    DesiredAddress, MappingName, MappingOptions, MemoryAccessor, MemoryAccessorExt, ProtectionFlags,
};

use crate::trace::{
    CATEGORY_STARNIX_BINDER, NAME_BINDER_IOCTL, NAME_HANDLE_REPLY, NAME_HANDLE_THREAD_READ,
    NAME_HANDLE_THREAD_WRITE, NAME_HANDLE_TRANSACTION, on_command_dequeued,
};
use starnix_core::security;
use starnix_core::task::{
    CurrentTask, EventHandler, Kernel, Pid, SimpleWaiter, Task, WaitCanceler, Waiter,
};
use starnix_core::vfs::buffers::{InputBuffer, OutputBuffer};
use starnix_core::vfs::{
    FdFlags, FdNumber, FileObject, FileObjectState, FileOps, FsStr, FsString, NamespaceNode,
    fileops_impl_nonseekable, fileops_impl_noop_sync,
};
use starnix_lifecycle::AtomicCounter;
use starnix_logging::{log_error, log_trace, log_warn, track_stub, with_zx_name};
use starnix_sync::{
    BinderContextManagerLevel, BinderProcsLevel, InterruptibleEvent, LockDepMutex, LockDepRwLock,
    ordered_lock_vec,
};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_types::convert::IntoFidl as _;
use starnix_types::ownership::{
    OwnedRef, Releasable, Share, TempRef, release_after, release_iter_after, release_on_error,
};
use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::{EINTR, Errno};
use starnix_uapi::math::round_up_to_increment;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    BINDER_BUFFER_FLAG_HAS_PARENT, BINDER_CURRENT_PROTOCOL_VERSION, binder_driver_command_protocol,
    binder_driver_command_protocol_BC_ACQUIRE, binder_driver_command_protocol_BC_ACQUIRE_DONE,
    binder_driver_command_protocol_BC_CLEAR_DEATH_NOTIFICATION,
    binder_driver_command_protocol_BC_CLEAR_FREEZE_NOTIFICATION,
    binder_driver_command_protocol_BC_DEAD_BINDER_DONE, binder_driver_command_protocol_BC_DECREFS,
    binder_driver_command_protocol_BC_ENTER_LOOPER, binder_driver_command_protocol_BC_FREE_BUFFER,
    binder_driver_command_protocol_BC_FREEZE_NOTIFICATION_DONE,
    binder_driver_command_protocol_BC_INCREFS, binder_driver_command_protocol_BC_INCREFS_DONE,
    binder_driver_command_protocol_BC_REGISTER_LOOPER, binder_driver_command_protocol_BC_RELEASE,
    binder_driver_command_protocol_BC_REPLY, binder_driver_command_protocol_BC_REPLY_SG,
    binder_driver_command_protocol_BC_REQUEST_DEATH_NOTIFICATION,
    binder_driver_command_protocol_BC_REQUEST_FREEZE_NOTIFICATION,
    binder_driver_command_protocol_BC_TRANSACTION,
    binder_driver_command_protocol_BC_TRANSACTION_SG, binder_freeze_info,
    binder_frozen_status_info, binder_transaction_data, binder_transaction_data_sg,
    binder_uintptr_t, binder_version, binder_write_read, errno, error, flat_binder_object, pid_t,
    transaction_flags_TF_ONE_WAY, uapi,
};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};

use std::ops::Deref;
use std::sync::Arc;
use std::vec::Vec;

use fidl_fuchsia_starnix_binder as fbinder;
use zerocopy::IntoBytes;
use zx;

#[derive(Debug, Default, Clone)]
pub struct BinderDevice(Arc<BinderDriver>);

impl Deref for BinderDevice {
    type Target = Arc<BinderDriver>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DeviceOps for BinderDevice {
    fn open(
        &self,
        current_task: &CurrentTask,
        _id: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let identifier = self.create_local_process(current_task.pid.clone());
        log_trace!("opened new BinderConnection id={}", identifier);
        Ok(Box::new(BinderConnection {
            identifier,
            device: self.clone(),
            security_state: security::binder_connection_alloc(current_task),
        }))
    }
}

/// An instance of the binder driver, associated with the process that opened the binder device.
#[derive(Debug)]
pub struct BinderConnection {
    /// The process that opened the binder device.
    pub identifier: u64,
    /// The implementation of the binder driver.
    device: BinderDevice,
    /// Security state associated this file object.
    security_state: security::BinderConnectionState,
}

impl BinderConnection {
    pub fn proc(&self, current_task: &CurrentTask) -> Result<OwnedRef<BinderProcess>, Errno> {
        let process = self.device.find_process(self.identifier)?;
        if process.key == current_task.pid.clone() {
            Ok(process)
        } else {
            process.release(current_task.kernel());
            error!(EINVAL)
        }
    }

    pub fn interrupt(&self) {
        log_trace!("interrupting BinderConnection id={}", self.identifier);
        if let Some(binder_process) = self.device.procs.read().get(&self.identifier) {
            binder_process.interrupt();
        }
    }

    fn close(&self, kernel: &Kernel) {
        log_trace!("closing BinderConnection id={}", self.identifier);
        if let Some(binder_process) = self.device.procs.write().remove(&self.identifier) {
            binder_process.close();
            binder_process.release(kernel);
        }
    }
}

impl FileOps for BinderConnection {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn close(self: Box<Self>, _file: &FileObjectState, current_task: &CurrentTask) {
        (*self).close(current_task.kernel());
    }

    fn query_events(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let binder_process = self.proc(current_task);
        release_after!(binder_process, current_task.kernel(), {
            Ok(match &binder_process {
                Ok(binder_process) => {
                    let binder_thread =
                        binder_process.lock().find_or_register_thread(&current_task.task)?;
                    release_after!(binder_thread, current_task.kernel(), {
                        let thread_state = binder_thread.lock();
                        if !thread_state.command_queue.is_empty() {
                            return FdEvents::POLLIN | FdEvents::POLLOUT;
                        } else if !thread_state.transactions.is_empty() {
                            return FdEvents::POLLOUT;
                        }
                        drop(thread_state);

                        let proc_state = binder_process.lock();
                        if !proc_state.command_queue.is_empty() {
                            FdEvents::POLLIN | FdEvents::POLLOUT
                        } else {
                            FdEvents::POLLOUT
                        }
                    })
                }
                Err(_) => FdEvents::POLLERR,
            })
        })
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        log_trace!("binder wait_async");
        let binder_process = self.proc(current_task);
        release_after!(binder_process, current_task.kernel(), {
            if let Ok(binder_process) = &binder_process {
                if let Ok(binder_thread) =
                    binder_process.lock().find_or_register_thread(&current_task.task)
                {
                    return release_after!(binder_thread, current_task.kernel(), {
                        Some(self.device.wait_async(
                            &binder_process,
                            &binder_thread,
                            waiter,
                            events,
                            handler,
                        ))
                    });
                }
            }
            handler.handle(FdEvents::POLLERR);
            Some(waiter.fake_wait())
        })
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let binder_process = self.proc(current_task)?;
        release_after!(binder_process, current_task.kernel(), {
            self.device.ioctl(
                current_task,
                &self.security_state,
                &binder_process,
                None,
                request,
                arg,
                Vec::new(),
            )
        })
    }

    fn get_memory(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _length: Option<usize>,
        _prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        error!(EINVAL)
    }

    fn mmap(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        addr: DesiredAddress,
        _memory_offset: u64,
        length: usize,
        prot_flags: ProtectionFlags,
        mapping_options: MappingOptions,
    ) -> Result<UserAddress, Errno> {
        let binder_process = self.proc(current_task)?;
        let mapping_name = MappingName::File(file.to_mapping(None)?);
        release_after!(binder_process, current_task.kernel(), {
            self.device.mmap(
                current_task,
                &binder_process,
                addr,
                length,
                prot_flags,
                mapping_options,
                mapping_name,
            )
        })
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        error!(EOPNOTSUPP)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        error!(EOPNOTSUPP)
    }

    fn flush(&self, _file: &FileObject, current_task: &CurrentTask) {
        // Errors are not meaningful on flush.
        let Ok(binder_process) = self.proc(current_task) else { return };
        release_after!(binder_process, current_task.kernel(), {
            binder_process.kick_all_threads()
        });
    }
}

/// A connection to a binder driver from a remote process.
#[derive(Debug)]
pub struct RemoteBinderConnection {
    binder_connection: BinderConnection,
}

impl RemoteBinderConnection {
    pub fn map_external_vmo(
        &self,
        current_task: &CurrentTask,
        vmo: fidl::Vmo,
        mapped_address: u64,
    ) -> Result<(), Errno> {
        let binder_process = self.binder_connection.proc(current_task)?;
        release_after!(binder_process, current_task.kernel(), {
            binder_process.map_external_vmo(vmo, mapped_address)
        })
    }

    pub fn ioctl(
        &self,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
        ioctl_reads: Vec<fbinder::IoctlReadWrite>,
        files: Vec<fbinder::FileHandle>,
        vmo: zx::Vmo,
    ) -> Result<Vec<fbinder::IoctlReadWrite>, Errno> {
        let binder_process = self.binder_connection.proc(current_task)?;
        release_after!(binder_process, current_task.kernel(), {
            let remote_ioctl =
                RemoteIoctl { ioctl_reads, ioctl_writes: Cell::new(Vec::new()), vmo };
            self.binder_connection.device.ioctl(
                current_task,
                &self.binder_connection.security_state,
                &binder_process,
                Some(&remote_ioctl),
                request,
                arg,
                files,
            )?;
            Ok(remote_ioctl.ioctl_writes.take())
        })
    }

    pub fn interrupt(&self) {
        self.binder_connection.interrupt();
    }

    pub fn close(&self, kernel: &Kernel) {
        self.binder_connection.close(kernel);
    }
}

/// Holds the context for a binder operation, including information about the sending process and
/// thread.
pub struct OperationContext<'a> {
    pub current_task: &'a CurrentTask,
    pub connection_security_state: &'a security::BinderConnectionState,
    pub binder_proc: &'a BinderProcess,
    pub binder_thread: &'a BinderThread,
    pub memory_accessor: &'a dyn MemoryAccessor,
}

impl<'a> OperationContext<'a> {
    fn resource_accessor(&self) -> &dyn ResourceAccessor {
        self.binder_proc.get_resource_accessor(self.current_task)
    }

    /// Acquires both the [`BinderProcess::state`] and [`BinderThread::state`] locks in the correct
    /// order. This ensures that deadlocks are avoided by always locking Process before Thread.
    fn lock(&self) -> (BinderProcessGuard<'_>, BinderThreadGuard<'_>) {
        let proc_guard = self.binder_proc.lock();
        let thread_guard = self.binder_thread.lock();
        (proc_guard, thread_guard)
    }
}

/// Android's binder kernel driver implementation.
#[derive(Debug)]
pub struct BinderDriver {
    /// The context manager, the object represented by the zero handle.
    pub context_manager: LockDepMutex<Option<Arc<BinderObject>>, BinderContextManagerLevel>,

    /// euid of the first process that successfully registered as context manager.
    /// Matches Linux `binder_context_mgr_uid`: once set, only the same euid may
    /// re-register after the previous manager exits. `u32::MAX` means unset.
    pub context_manager_uid: AtomicU32,

    /// Manages the internal state of each process interacting with the binder driver.
    ///
    /// The Driver owns the BinderProcess. There can be at most one connection to the binder driver
    /// per process. When the last file descriptor to the binder in the process is closed, the
    /// value is removed from the map.
    pub procs: LockDepRwLock<BTreeMap<u64, OwnedRef<BinderProcess>>, BinderProcsLevel>,

    /// The identifier to use for the next created `BinderProcess`.
    next_identifier: AtomicCounter<u64>,
}

impl Releasable for BinderDriver {
    type Context<'a> = &'a CurrentTask;

    fn release<'a>(mut self, context: &'a CurrentTask) {
        let current_task = context;
        for binder_process in std::mem::take(self.procs.get_mut()).into_values() {
            binder_process.release(current_task.kernel());
        }
    }
}

impl Default for BinderDriver {
    #[allow(clippy::let_and_return)]
    fn default() -> Self {
        Self {
            context_manager: Default::default(),
            context_manager_uid: AtomicU32::new(u32::MAX),
            procs: Default::default(),
            next_identifier: Default::default(),
        }
    }
}

impl BinderDriver {
    pub fn find_process(&self, identifier: u64) -> Result<OwnedRef<BinderProcess>, Errno> {
        self.procs.read().get(&identifier).map(OwnedRef::share).ok_or_else(|| errno!(ENOENT))
    }

    /// Finds all binder processes that associate with the given `pid`.
    fn find_processes_by_pid(&self, pid: pid_t) -> Vec<OwnedRef<BinderProcess>> {
        self.procs
            .read()
            .iter()
            .filter_map(|(_k, v)| if v.key.id == pid { Some(OwnedRef::share(v)) } else { None })
            .collect::<Vec<_>>()
    }

    /// Creates and register the binder process state to represent a local process with `key`.
    fn create_local_process(&self, key: Pid) -> u64 {
        self.create_process(key, None)
    }

    /// Creates and register the binder process state to represent a remote process with `key`.
    fn create_remote_process(&self, key: Pid, resource_accessor: RemoteResourceAccessor) -> u64 {
        self.create_process(key, Some(Arc::new(resource_accessor)))
    }

    /// Creates and register the binder process state to represent a process with `key`.
    fn create_process(
        &self,
        key: Pid,
        resource_accessor: Option<Arc<RemoteResourceAccessor>>,
    ) -> u64 {
        let identifier = self.next_identifier.next();
        let binder_process = BinderProcess::new(identifier, key, resource_accessor);
        assert!(
            self.procs.write().insert(identifier, binder_process).is_none(),
            "process with same identifier created"
        );
        identifier
    }

    /// Creates the binder process and thread state to represent a process with `key` and one main
    /// thread.
    #[cfg(test)]
    /// Return a `RemoteBinderConnection` that can be used to driver a remote connection to the
    /// binder device represented by this driver.
    pub fn create_process_and_thread(
        &self,
        key: Pid,
        task: &Task,
    ) -> (OwnedRef<BinderProcess>, OwnedRef<BinderThread>) {
        let identifier = self.create_local_process(key.clone());
        let binder_process = self.find_process(identifier).expect("find_process");
        let binder_thread =
            binder_process.lock().find_or_register_thread(task).expect("find_or_register_thread");
        (binder_process, binder_thread)
    }

    /// Return a `RemoteBinderConnection` that can be used to driver a remote connection to the
    /// binder device represented by this driver.
    pub fn open_remote(
        this: &BinderDevice,
        current_task: &CurrentTask,
        process_accessor: ClientEnd<fbinder::ProcessAccessorMarker>,
        process: zx::Process,
    ) -> Arc<RemoteBinderConnection> {
        let process_accessor =
            fbinder::ProcessAccessorSynchronousProxy::new(process_accessor.into_channel());
        let identifier = this.create_remote_process(
            current_task.pid.clone(),
            RemoteResourceAccessor {
                process_accessor,
                process,
                remote_creds: current_task.current_creds().clone(),
            },
        );
        Arc::new(RemoteBinderConnection {
            binder_connection: BinderConnection {
                identifier,
                device: BinderDevice(Arc::clone(this)),
                security_state: security::binder_connection_alloc(current_task),
            },
        })
    }

    pub fn get_context_manager(
        &self,
        current_task: &CurrentTask,
    ) -> Result<(Arc<BinderObject>, TempRef<'_, BinderProcess>), Errno> {
        let mut context_manager = self.context_manager.lock();
        if let Some(context_manager_object) = context_manager.as_ref().cloned() {
            match context_manager_object.owner.upgrade().map(TempRef::into_static) {
                Some(owner) => {
                    return Ok((context_manager_object, owner));
                }
                None => {
                    // The context manager was registered, but its owner process has died. Returning
                    // ESRCH dispatches BR_DEAD_REPLY to the client.
                    *context_manager = None;
                    return error!(ESRCH);
                }
            }
        }

        log_trace!(
            "Task {} tried to get context manager but one is not registered or dead. \
            Avoid the race condition by waiting until the context manager is ready.",
            current_task.tid
        );
        error!(ENOENT)
    }

    pub fn ioctl(
        &self,
        current_task: &CurrentTask,
        connection_security_state: &security::BinderConnectionState,
        binder_proc: &BinderProcess,
        remote_ioctl: Option<&RemoteIoctl>,
        request: u32,
        arg: SyscallArg,
        mut files: Vec<fbinder::FileHandle>,
    ) -> Result<SyscallResult, Errno> {
        fuchsia_trace::duration!(CATEGORY_STARNIX_BINDER, NAME_BINDER_IOCTL, "request" => request);
        let user_arg = UserAddress::from(arg);
        let remote_memory_accessor =
            match (binder_proc.remote_resource_accessor.as_ref(), remote_ioctl) {
                (Some(remote_resource_accessor), Some(remote_ioctl)) => {
                    Some(&RemoteMemoryAccessor {
                        remote_resource_accessor: remote_resource_accessor.clone(),
                        remote_ioctl,
                    })
                }
                _ => None,
            };
        match request {
            uapi::BINDER_FREEZE => {
                if user_arg.is_null() {
                    return error!(EINVAL);
                }

                let user_ref = UserRef::<binder_freeze_info>::new(user_arg);
                let binder_freeze_info { pid, enable, timeout_ms } = binder_proc
                    .get_memory_accessor(current_task, remote_memory_accessor)
                    .read_object(user_ref)?;
                let freezing = match enable {
                    0 => false,
                    1 => true,
                    _ => return error!(EINVAL),
                };

                let target_binder_procs = self.find_processes_by_pid(pid as pid_t);
                if target_binder_procs.is_empty() {
                    return error!(EINVAL);
                }

                let mut pending_notifications = Vec::with_capacity(target_binder_procs.len());

                let res = release_iter_after!(target_binder_procs, current_task.kernel(), {
                    let freeze_locks =
                        target_binder_procs.iter().map(|p| &p.freeze_state).collect::<Vec<_>>();
                    // Do not allow freezing process with more than 16 binders as lockdep
                    // doesn't support it.
                    if freeze_locks.len() > 16 {
                        return error!(ENOTSUP);
                    }
                    let mut target_binder_procs_freeze_locked = ordered_lock_vec(&freeze_locks);

                    if !freezing {
                        target_binder_procs_freeze_locked.iter_mut().for_each(|bp| {
                            pending_notifications.extend(bp.thaw());
                        });
                        return Ok(SUCCESS);
                    }

                    let state_locks =
                        target_binder_procs.iter().map(|p| &p.state).collect::<Vec<_>>();
                    let target_binder_procs_state_locked = ordered_lock_vec(&state_locks);

                    // Clone threads in the proc to lock them all until freeze is done.
                    let threads: Vec<OwnedRef<BinderThread>> = target_binder_procs_state_locked
                        .iter()
                        .map(|p| p.thread_pool.threads.values().map(|t| OwnedRef::share(t)))
                        .flatten()
                        .collect();
                    release_iter_after!(threads, current_task.kernel(), {
                        let threads_locks = threads.iter().map(|t| &t.state).collect::<Vec<_>>();
                        let threads_locked = ordered_lock_vec(&threads_locks);

                        // Avoid freezing the target procs if there is any pending transaction
                        if target_binder_procs_state_locked
                            .iter()
                            .any(|binder_process| binder_process.has_pending_transactions())
                            || threads_locked
                                .iter()
                                .any(|binder_thread| binder_thread.has_pending_transactions())
                        {
                            if timeout_ms > 0 {
                                track_stub!(
                                    TODO("https://fxbug.dev/391657004"),
                                    "BINDER_FREEZE timeout"
                                );
                            }
                            return error!(EAGAIN);
                        }

                        target_binder_procs_freeze_locked.iter_mut().for_each(|bp| {
                            pending_notifications.extend(bp.freeze());
                        });
                        Ok(SUCCESS)
                    })
                });

                if res.is_ok() {
                    for (proc, cmd) in pending_notifications {
                        proc.enqueue_command(cmd.into());
                        proc.release(current_task.kernel());
                    }
                } else {
                    for (proc, _) in pending_notifications {
                        proc.release(current_task.kernel());
                    }
                }
                return res;
            }
            uapi::BINDER_GET_FROZEN_INFO => {
                if user_arg.is_null() {
                    return error!(EINVAL);
                }

                let user_ref = UserRef::<binder_frozen_status_info>::new(user_arg);
                let memory_accessor =
                    binder_proc.get_memory_accessor(current_task, remote_memory_accessor);
                let binder_frozen_status_info { pid, .. } =
                    memory_accessor.read_object(user_ref)?;
                let target_binder_procs = self.find_processes_by_pid(pid as pid_t);
                if target_binder_procs.is_empty() {
                    return error!(EINVAL);
                }
                let mut has_sync_recv = false;
                let mut has_async_recv = false;
                release_iter_after!(target_binder_procs, current_task.kernel(), {
                    target_binder_procs.iter().for_each(|binder_proc| {
                        let freeze_state = binder_proc.freeze_state.lock();
                        has_sync_recv |= freeze_state.freeze_status.has_sync_recv;
                        has_async_recv |= freeze_state.freeze_status.has_async_recv;
                    });
                });
                memory_accessor.write_object(
                    user_ref,
                    &binder_frozen_status_info {
                        pid,
                        // TODO(https://fxbug.dev/391657004): After timeout is supported, use
                        // the second right bit as the indicator whether it has any pending
                        // transactions.
                        sync_recv: has_sync_recv as u32,
                        async_recv: has_async_recv as u32,
                    },
                )?;
                return Ok(SUCCESS);
            }
            _ => {}
        }
        let binder_thread = binder_proc.lock().find_or_register_thread(&current_task.task)?;
        release_after!(binder_thread, current_task.kernel(), {
            match request {
                uapi::BINDER_VERSION => {
                    // A thread is requesting the version of this binder driver.
                    if user_arg.is_null() {
                        return error!(EINVAL);
                    }
                    let response =
                        binder_version { protocol_version: BINDER_CURRENT_PROTOCOL_VERSION as i32 };
                    log_trace!("binder version is {:?}", response);
                    binder_proc
                        .get_memory_accessor(current_task, remote_memory_accessor)
                        .write_object(UserRef::new(user_arg), &response)?;
                    Ok(SUCCESS)
                }
                uapi::BINDER_SET_CONTEXT_MGR | uapi::BINDER_SET_CONTEXT_MGR_EXT => {
                    // A process is registering itself as the context manager.
                    // Match Linux binder_ioctl_set_ctx_mgr: reject if a live manager is
                    // already set (EBUSY), and lock the manager role to the first setter's
                    // euid (EPERM on mismatch).
                    security::binder_set_context_mgr(current_task)?;
                    let flags = if request == uapi::BINDER_SET_CONTEXT_MGR_EXT {
                        if user_arg.is_null() {
                            return error!(EINVAL);
                        }
                        let user_ref = UserRef::<flat_binder_object>::new(user_arg);
                        let flat_binder_object = binder_proc
                            .get_memory_accessor(current_task, remote_memory_accessor)
                            .read_object(user_ref)?;
                        BinderObjectFlags::parse(flat_binder_object.flags)?
                    } else {
                        BinderObjectFlags::empty()
                    };

                    log_trace!("binder setting context manager with flags {:x}", flags);

                    let euid = current_task.current_creds().euid as u32;
                    // Lock the manager uid to the first successful setter (Linux behavior).
                    match self.context_manager_uid.compare_exchange(
                        u32::MAX,
                        euid,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {}
                        Err(existing) if existing == euid => {}
                        Err(_) => return error!(EPERM),
                    }

                    let mut context_manager = self.context_manager.lock();
                    if let Some(existing) = context_manager.as_ref() {
                        if existing.owner.upgrade().is_some() {
                            return error!(EBUSY);
                        }
                        // Drop a dead manager marker so a same-euid process can re-register.
                        *context_manager = None;
                    }

                    *context_manager =
                        Some(BinderObject::new_context_manager_marker(binder_proc, flags));
                    Ok(SUCCESS)
                }
                uapi::BINDER_WRITE_READ => {
                    // A thread is requesting to exchange data with the binder driver.
                    if user_arg.is_null() {
                        return error!(EINVAL);
                    }

                    let memory_accessor =
                        binder_proc.get_memory_accessor(current_task, remote_memory_accessor);
                    let user_ref = UserRef::<binder_write_read>::new(user_arg);
                    let mut input = memory_accessor.read_object(user_ref)?;

                    log_trace!("binder write/read request start {:?}", input);
                    let mut has_consumed_write = false;
                    let result = (|| {
                        let context = OperationContext {
                            current_task,
                            connection_security_state: &connection_security_state,
                            binder_proc,
                            binder_thread: &binder_thread,
                            memory_accessor,
                        };
                        if input.write_size > input.write_consumed {
                            // The calling thread wants to write some data to the binder driver.
                            let mut cursor = UserMemoryCursor::new(
                                memory_accessor,
                                UserAddress::from(input.write_buffer),
                                input.write_size,
                            )?;

                            // Skip already-consumed commands.
                            cursor.advance(input.write_consumed)?;

                            // Handle all the data the calling thread sent, which may include
                            // multiple commands.
                            while cursor.bytes_read() < input.write_size as usize {
                                self.handle_thread_write(&context, &mut files, &mut cursor)?;
                                has_consumed_write = true;
                                input.write_consumed = cursor.bytes_read() as u64;
                            }
                        }

                        if input.read_size > input.read_consumed {
                            // The calling thread wants to read some data from the binder driver,
                            // blocking if there is nothing immediately available.
                            let mut read_buffer = UserBuffer {
                                address: UserAddress::from(input.read_buffer),
                                length: input.read_size as usize,
                            };
                            read_buffer.advance(input.read_consumed as usize)?;
                            let read_result = match self.handle_thread_read(&context, &read_buffer)
                            {
                                // If the wait was interrupted and some command has been consumed,
                                // return a success.
                                Err(err) if err == EINTR && has_consumed_write => Ok(0),
                                r => r,
                            };
                            input.read_consumed += read_result? as u64;
                        }

                        log_trace!("binder write/read request end {:?}", input);
                        Ok(SUCCESS)
                    })();

                    // Write back to the calling thread how much data was read/written, even when
                    // returning an error.
                    memory_accessor.write_object(user_ref, &input)?;
                    result
                }
                uapi::BINDER_SET_MAX_THREADS => {
                    if user_arg.is_null() {
                        return error!(EINVAL);
                    }

                    let user_ref = UserRef::<u32>::new(user_arg);
                    let new_max_threads = binder_proc
                        .get_memory_accessor(current_task, remote_memory_accessor)
                        .read_object(user_ref)? as usize;
                    log_trace!("setting max binder threads to {}", new_max_threads);
                    binder_proc.lock().max_thread_count = new_max_threads;
                    Ok(SUCCESS)
                }
                uapi::BINDER_ENABLE_ONEWAY_SPAM_DETECTION => {
                    track_stub!(
                        TODO("https://fxbug.dev/322874289"),
                        "binder ENABLE_ONEWAY_SPAM_DETECTION"
                    );
                    Ok(SUCCESS)
                }
                uapi::BINDER_THREAD_EXIT => {
                    log_trace!("binder thread {} exiting", binder_thread.tid);
                    binder_proc.lock().unregister_thread(current_task, binder_thread.tid);
                    Ok(SUCCESS)
                }
                uapi::BINDER_GET_NODE_DEBUG_INFO => {
                    track_stub!(TODO("https://fxbug.dev/322874232"), "binder GET_NODE_DEBUG_INFO");
                    error!(EOPNOTSUPP)
                }
                uapi::BINDER_GET_NODE_INFO_FOR_REF => {
                    track_stub!(
                        TODO("https://fxbug.dev/322874148"),
                        "binder GET_NODE_INFO_FOR_REF"
                    );
                    error!(EOPNOTSUPP)
                }

                _ => {
                    track_stub!(
                        TODO("https://fxbug.dev/322874384"),
                        "binder unknown ioctl",
                        request
                    );
                    log_error!("binder received unknown ioctl request 0x{:08x}", request);
                    error!(EINVAL)
                }
            }
        })
    }

    /// Consumes one command from the userspace binder_write_read buffer and handles it.
    /// This method will never block.
    fn handle_thread_write(
        &self,
        context: &OperationContext<'_>,
        files: &mut Vec<fbinder::FileHandle>,
        cursor: &mut UserMemoryCursor,
    ) -> Result<(), Errno> {
        let command = cursor.read_object::<binder_driver_command_protocol>()?;
        fuchsia_trace::duration!(
            CATEGORY_STARNIX_BINDER,
            NAME_HANDLE_THREAD_WRITE,
            "command" => command
        );
        let result = match command {
            binder_driver_command_protocol_BC_ENTER_LOOPER => {
                let (mut proc_state, mut thread_state) = context.lock();
                thread_state.handle_looper_registration(&mut proc_state, RegistrationState::Main)
            }
            binder_driver_command_protocol_BC_REGISTER_LOOPER => {
                let (mut proc_state, mut thread_state) = context.lock();
                thread_state
                    .handle_looper_registration(&mut proc_state, RegistrationState::Auxilliary)
            }
            binder_driver_command_protocol_BC_INCREFS
            | binder_driver_command_protocol_BC_ACQUIRE
            | binder_driver_command_protocol_BC_DECREFS
            | binder_driver_command_protocol_BC_RELEASE => {
                let handle = cursor.read_object::<u32>()?.into();
                context.binder_proc.handle_refcount_operation(command, handle)
            }
            binder_driver_command_protocol_BC_INCREFS_DONE
            | binder_driver_command_protocol_BC_ACQUIRE_DONE => {
                let object = LocalBinderObject {
                    weak_ref_addr: UserAddress::from(cursor.read_object::<binder_uintptr_t>()?),
                    strong_ref_addr: UserAddress::from(cursor.read_object::<binder_uintptr_t>()?),
                };
                context.binder_proc.handle_refcount_operation_done(command, object)
            }
            binder_driver_command_protocol_BC_FREE_BUFFER => {
                let buffer_ptr = UserAddress::from(cursor.read_object::<binder_uintptr_t>()?);
                context.binder_proc.handle_free_buffer(buffer_ptr)
            }
            binder_driver_command_protocol_BC_REQUEST_DEATH_NOTIFICATION => {
                let handle = cursor.read_object::<u32>()?.into();
                let cookie = cursor.read_object::<binder_uintptr_t>()?;
                context.binder_proc.handle_request_death_notification(handle, cookie)
            }
            binder_driver_command_protocol_BC_CLEAR_DEATH_NOTIFICATION => {
                let handle = cursor.read_object::<u32>()?.into();
                let cookie = cursor.read_object::<binder_uintptr_t>()?;
                context.binder_proc.handle_clear_death_notification(handle, cookie)
            }
            binder_driver_command_protocol_BC_DEAD_BINDER_DONE => {
                let _cookie = cursor.read_object::<binder_uintptr_t>()?;
                Ok(())
            }
            binder_driver_command_protocol_BC_TRANSACTION => {
                let data = cursor.read_object::<binder_transaction_data>()?;
                self.handle_transaction(
                    context,
                    files,
                    binder_transaction_data_sg { transaction_data: data, buffers_size: 0 },
                )
                .or_else(|err| err.dispatch(context.binder_thread))
            }
            binder_driver_command_protocol_BC_REPLY => {
                let data = cursor.read_object::<binder_transaction_data>()?;
                self.handle_reply(
                    context,
                    files,
                    binder_transaction_data_sg { transaction_data: data, buffers_size: 0 },
                )
                .or_else(|err| err.dispatch(context.binder_thread))
            }
            binder_driver_command_protocol_BC_TRANSACTION_SG => {
                let data = cursor.read_object::<binder_transaction_data_sg>()?;
                self.handle_transaction(context, files, data)
                    .or_else(|err| err.dispatch(context.binder_thread))
            }
            binder_driver_command_protocol_BC_REPLY_SG => {
                let data = cursor.read_object::<binder_transaction_data_sg>()?;
                self.handle_reply(context, files, data)
                    .or_else(|err| err.dispatch(context.binder_thread))
            }
            binder_driver_command_protocol_BC_REQUEST_FREEZE_NOTIFICATION => {
                let handle = cursor.read_object::<u32>()?.into();
                let cookie = cursor.read_object::<binder_uintptr_t>()?;
                context.binder_proc.handle_request_freeze_notification(handle, cookie)
            }
            binder_driver_command_protocol_BC_FREEZE_NOTIFICATION_DONE => {
                let cookie = cursor.read_object::<binder_uintptr_t>()?;
                context.binder_proc.handle_freeze_notification_done(cookie);
                Ok(())
            }
            binder_driver_command_protocol_BC_CLEAR_FREEZE_NOTIFICATION => {
                let handle = cursor.read_object::<u32>()?.into();
                let cookie = cursor.read_object::<binder_uintptr_t>()?;
                context.binder_proc.handle_clear_freeze_notification(handle, cookie)
            }
            _ => {
                log_error!("binder received unknown RW command: {:#08x}", command);
                error!(EINVAL)
            }
        };

        if let Err(err) = &result {
            // Downgrade to TRACE level as the driver is stable and standard userspace IPC
            // command failures (e.g. expected ENOENT/EAGAIN races during teardown/freeze)
            // must not flood the Fuchsia system log at ERROR level (fixes VIM3 logspam b/409499623).
            log_trace!("binder command {:#x} failed: {:?}", command, err);
        }
        result
    }

    /// A binder thread is starting a transaction on a remote binder object.
    pub fn handle_transaction(
        &self,
        context: &OperationContext<'_>,
        files: &mut Vec<fbinder::FileHandle>,
        data: binder_transaction_data_sg,
    ) -> Result<(), TransactionError> {
        fuchsia_trace::duration!(
            CATEGORY_STARNIX_BINDER,
            NAME_HANDLE_TRANSACTION,
            "code" => data.transaction_data.code,
            "data_size" => data.transaction_data.data_size as u64,
            "offsets_size" => data.transaction_data.offsets_size as u64
        );
        // SAFETY: Transactions can only refer to handles.
        let handle = unsafe { data.transaction_data.target.handle }.into();

        let (object, target_proc, mut guard) = match handle {
            Handle::ContextManager => {
                let (object, owner) = self.get_context_manager(context.current_task)?;
                (object, Some(owner), None)
            }
            Handle::Object { index } => {
                let binder_proc = context.binder_proc.lock();
                let (object, guard) =
                    binder_proc.handles.get(index).ok_or(TransactionError::Failure)?;
                let owner = object.owner.upgrade().map(TempRef::into_static);
                (object, owner, Some(guard))
            }
        };

        let mut actions = RefCountActions::default();
        release_after!(actions, (), {
            release_after!(guard, &mut actions, {
                let target_proc = target_proc.ok_or(TransactionError::Dead)?;
                let oneway = data.transaction_data.flags & transaction_flags_TF_ONE_WAY != 0;
                // Track freeze status if the target proc is frozen
                let is_target_frozen = {
                    let mut freeze_state = target_proc.freeze_state.lock();
                    if freeze_state.freeze_status.frozen {
                        freeze_state.freeze_status.has_sync_recv |= !oneway;
                        freeze_state.freeze_status.has_async_recv |= oneway;
                    }
                    freeze_state.freeze_status.frozen
                };
                // If the target proc is frozen in the sync transaction, reply with the Frozen error
                if is_target_frozen && !oneway {
                    return Err(TransactionError::Frozen);
                }
                let target_task = target_proc.get_task().ok_or(TransactionError::Dead)?;

                security::binder_transaction(
                    context.current_task,
                    &target_task,
                    context.connection_security_state,
                )?;

                let security_context: Option<FsString> =
                    if object.flags.contains(BinderObjectFlags::TXN_SECURITY_CTX) {
                        let mut security_context = FsString::from(
                            security::binder_get_context(
                                context.current_task,
                                context.connection_security_state,
                            )
                            .unwrap_or_default(),
                        );
                        security_context.push(b'\0');
                        Some(security_context)
                    } else {
                        None
                    };

                // Copy the transaction data to the target process.
                let (buffers, mut transaction_state) = self.copy_transaction_buffers(
                    context,
                    files,
                    &target_task,
                    target_proc.get_resource_accessor(target_task.deref()),
                    &target_proc,
                    &data,
                    security_context.as_ref().map(|c: &FsString| c.as_ref()),
                )?;

                let transaction = TransactionData {
                    peer_pid: if oneway { 0 } else { context.binder_proc.key.id },
                    peer_tid: context.binder_thread.tid,
                    peer_euid: context.current_task.current_creds().euid,
                    object: {
                        if handle.is_handle_0() {
                            // This handle (0) always refers to the context manager, which is always
                            // "remote", even for the context manager itself.
                            FlatBinderObject::Remote { handle }
                        } else {
                            FlatBinderObject::Local { object: object.local }
                        }
                    },
                    code: data.transaction_data.code,
                    flags: data.transaction_data.flags,

                    buffers: buffers.clone(),
                };

                if let Some(guard) = guard.take() {
                    transaction_state.push_guard(guard);
                }

                let trace_id = fuchsia_trace::Id::new();

                if oneway {
                    // The caller is not expecting a reply.
                    context.binder_thread.lock().enqueue_command(
                        if is_target_frozen {
                            Command::PendingFrozen
                        } else {
                            Command::OnewayTransactionComplete
                        }
                        .into(),
                    );

                    // Register the transaction buffer.
                    target_proc.lock().active_transactions.insert(
                        buffers.data.address,
                        ActiveTransaction {
                            request_type: RequestType::Oneway { object: object.clone() },
                            state: transaction_state.into_state(),
                        }
                        .into(),
                    );

                    // Oneway transactions are enqueued on the binder object and processed one at a time.
                    // This guarantees that oneway transactions are processed in the order they are
                    // submitted, and one at a time.
                    let mut object_state = object.lock();
                    if object_state.handling_oneway_transaction {
                        // Currently, a oneway transaction is being handled. Queue this one so that it is
                        // scheduled when the buffer from the in-progress transaction is freed.
                        object_state.oneway_transactions.push_back((transaction, trace_id));
                        return Ok(());
                    }

                    // No oneway transactions are being handled, which means that no buffer will be
                    // freed, kicking off scheduling from the oneway queue. Instead, we must schedule
                    // the transaction regularly, but mark the object as handling a oneway transaction.
                    object_state.handling_oneway_transaction = true;

                    drop(object_state);
                    target_proc.enqueue_command(QueuedCommand::new(
                        Command::OnewayTransaction(transaction),
                        trace_id,
                    ));
                } else {
                    let target_thread = match match context.binder_thread.lock().transactions.last()
                    {
                        Some(TransactionRole::Receiver { peer, .. }) => peer.upgrade(),
                        _ => None,
                    } {
                        Some((proc, thread)) if proc.key == target_proc.key => Some(thread),
                        _ => None,
                    };

                    let transaction_sender = TransactionSender {
                        target_proc: target_proc.identifier,
                        target_thread: target_thread.as_ref().map(|t| t.tid),
                        is_alive: true,
                        target_thread_handle: target_thread.as_ref().map(|t| t.thread.clone()),
                        trace_id,
                    };

                    // Make the sender thread part of the transaction so it doesn't get scheduled to handle
                    // any other transactions.
                    let expected_len = {
                        let mut thread_state = context.binder_thread.lock();
                        thread_state.transactions.push(TransactionRole::Sender(transaction_sender));
                        thread_state.transactions.len()
                    };

                    // Register the transaction buffer.
                    target_proc.lock().active_transactions.insert(
                        buffers.data.address,
                        ActiveTransaction {
                            request_type: RequestType::RequestResponse,
                            state: transaction_state.into_state(),
                        }
                        .into(),
                    );

                    let queued = QueuedCommand::new(
                        Command::Transaction {
                            sender: WeakBinderPeer::new(context.binder_proc, context.binder_thread),
                            data: transaction,
                        },
                        trace_id,
                    );

                    if let Some(target_thread) = target_thread {
                        target_thread.lock().enqueue_command(queued);
                    } else {
                        // If we don't have an already known target thread, select one if
                        // available.
                        if let Some(thread) = target_proc.enqueue_command(queued) {
                            // If we were able to schedule on a thread's queue (rather than on a
                            // process' queue), we can update the sender record with that
                            // information.
                            let mut binder_thread_state = context.binder_thread.lock();
                            // If in the time between us dropping the thread lock and reacquiring
                            // it, if target has died, generate_dead_replies could have popped the
                            // transaction from our stack.
                            if binder_thread_state.transactions.len() == expected_len {
                                if let Some(TransactionRole::Sender(TransactionSender {
                                    target_proc: id,
                                    is_alive: true,
                                    target_thread_handle,
                                    ..
                                })) = binder_thread_state.transactions.last_mut()
                                    && *id == target_proc.identifier
                                {
                                    *target_thread_handle = Some(thread.thread.clone());
                                }
                            }
                        }
                    }
                };
                Ok(())
            })
        })
    }

    /// A binder thread is sending a reply to a transaction.
    pub fn handle_reply(
        &self,
        context: &OperationContext<'_>,
        files: &mut Vec<fbinder::FileHandle>,
        data: binder_transaction_data_sg,
    ) -> Result<(), TransactionError> {
        fuchsia_trace::duration!(
            CATEGORY_STARNIX_BINDER,
            NAME_HANDLE_REPLY,
            "code" => data.transaction_data.code,
            "data_size" => data.transaction_data.data_size as u64,
            "offsets_size" => data.transaction_data.offsets_size as u64
        );
        // Find the process and thread that initiated the transaction. This reply is for them.
        let (target_proc, target_thread, trace_id) =
            context.binder_thread.lock().pop_transaction_caller()?;
        let mut send_reply = || -> Result<(), TransactionError> {
            let target_task = target_proc.get_task().ok_or(TransactionError::Dead)?;

            // Copy the transaction data to the target process.
            let (buffers, transaction_state) = self.copy_transaction_buffers(
                context,
                files,
                &target_task,
                target_proc.get_resource_accessor(target_task.deref()),
                &target_proc,
                &data,
                None,
            )?;

            // Register the transaction buffer.
            target_proc.lock().active_transactions.insert(
                buffers.data.address,
                ActiveTransaction {
                    request_type: RequestType::RequestResponse,
                    state: transaction_state.into_state(),
                }
                .into(),
            );

            // Atomically enqueue the reply on the target thread and the
            // transaction complete command on the local thread.
            {
                let (mut target_thread, mut binder_thread) =
                    BinderThread::ordered_lock(&target_thread, context.binder_thread);
                target_thread.enqueue_command(QueuedCommand::new(
                    Command::Reply(TransactionData {
                        peer_pid: context.binder_proc.key.id,
                        peer_tid: context.binder_thread.tid,
                        peer_euid: context.current_task.current_creds().euid,

                        object: FlatBinderObject::Remote { handle: Handle::ContextManager },
                        code: data.transaction_data.code,
                        flags: data.transaction_data.flags,

                        buffers,
                    }),
                    trace_id,
                ));

                binder_thread.enqueue_command(Command::TransactionComplete.into());
            }

            Ok(())
        };
        if let Err(e) = send_reply() {
            // Sending to the target process failed, notify of the transaction failure.
            let _ = e.dispatch_with_trace_id(&target_thread, trace_id);
            return Err(e);
        }
        Ok(())
    }

    /// Select which command queue to read from, preferring the thread-local one.
    /// If a transaction is pending, deadlocks can happen if reading from the process queue.
    fn get_active_command(
        thread_state: &mut BinderThreadState,
        proc_state: &mut crate::process::BinderProcessState,
        worker_thread: &zx::Thread,
    ) -> Option<QueuedCommand> {
        if !thread_state.command_queue.is_empty() || !thread_state.transactions.is_empty() {
            thread_state.command_queue.pop_front()
        } else {
            let item = proc_state.command_queue.pop_front();
            if let Some(queued) = &item {
                on_command_dequeued(&queued.command, queued.trace_id);
                if let Command::Transaction { sender, .. } = &queued.command {
                    if let Some((_proc, sender_thread)) = sender.upgrade() {
                        if let Some(event) = &*sender_thread.requeue_event.lock() {
                            let _ = event.assign_new_owner(worker_thread);
                        }
                    }
                }
            }
            item
        }
    }

    /// Dequeues a command from the thread's commands' queue, or blocks until commands are available.
    pub fn handle_thread_read(
        &self,
        context: &OperationContext<'_>,
        read_buffer: &UserBuffer,
    ) -> Result<usize, Errno> {
        fuchsia_trace::duration!(CATEGORY_STARNIX_BINDER, NAME_HANDLE_THREAD_READ);
        loop {
            {
                let mut binder_proc_state = context.binder_proc.lock();

                if binder_proc_state.should_request_thread(context.binder_thread) {
                    let bytes_written = Command::SpawnLooper
                        .write_to_memory(context.memory_accessor, read_buffer)?;
                    binder_proc_state.did_request_thread();
                    return Ok(bytes_written);
                }
            }

            let (mut proc_state, mut thread_state) = context.lock();

            if thread_state.request_kick {
                thread_state.request_kick = false;
                return Ok(0);
            }

            let command_with_trace = Self::get_active_command(
                &mut thread_state,
                &mut proc_state,
                &context.binder_thread.thread,
            )
            .or_else(|| {
                // If there is no pending command, but the current transaction is marked as dead,
                // pop the transaction and dispatch a `DeadReply`.
                match thread_state.transactions.last() {
                    Some(TransactionRole::Sender(TransactionSender {
                        is_alive: false,
                        trace_id,
                        ..
                    })) => {
                        let trace_id = *trace_id;
                        if let Some(TransactionRole::Sender(_)) = thread_state.transactions.pop() {
                            on_command_dequeued(&Command::DeadReply, trace_id);
                            Some(QueuedCommand::new(Command::DeadReply, trace_id))
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            });

            // If we have sent a request and are about to wait for a response, the thread we've
            // sent to should inherit our priority while we wait.
            let target_thread = thread_state
                .transactions
                .iter()
                .rev()
                .find_map(|t| match t {
                    // Only look for the most recently sent transaction, even if it doesn't have a
                    // target_thread set. Earlier transactions (if any) won't unblock this thread
                    // any sooner by inheriting increased priority.
                    TransactionRole::Sender(sender) => Some(sender),
                    _ => None,
                })
                .and_then(|s| s.target_thread_handle.clone());

            if let Some(QueuedCommand { command, trace_id }) = command_with_trace {
                // Attempt to write the command to the thread's buffer.
                let bytes_written =
                    command.write_to_memory(context.memory_accessor, read_buffer)?;
                let has_pending_proc_commands = match command {
                    Command::Transaction { sender, .. } => {
                        // The transaction is synchronous and we're expected to give a reply, so
                        // push the transaction onto the transaction stack.
                        let tx = TransactionRole::Receiver { peer: sender, trace_id };
                        thread_state.transactions.push(tx);
                        false
                    }
                    Command::Reply(..) => {
                        // The sender got a reply, pop the sender entry from the transaction stack.
                        let transaction =
                            thread_state.transactions.pop().expect("transaction stack underflow!");
                        // Command::Reply is sent to the receiver side. So the popped transaction
                        // must be a Sender role.
                        assert!(
                            matches!(transaction, TransactionRole::Sender(_)),
                            "Active Transaction: {:?}, Pending Transactions {:?}, Command: {:?}, Pending Commands: {:?}",
                            transaction,
                            thread_state.transactions,
                            command,
                            thread_state.command_queue,
                        );
                        !proc_state.command_queue.is_empty()
                    }
                    Command::FrozenBinder(info) => {
                        proc_state.in_flight_freeze_notifications.insert(info.cookie);
                        false
                    }
                    Command::TransactionComplete
                    | Command::OnewayTransaction(..)
                    | Command::OnewayTransactionComplete
                    | Command::AcquireRef(..)
                    | Command::ReleaseRef(..)
                    | Command::IncRef(..)
                    | Command::DecRef(..)
                    | Command::Error(..)
                    | Command::FailedReply
                    | Command::DeadReply
                    | Command::DeadBinder(..)
                    | Command::FrozenReply
                    | Command::PendingFrozen
                    | Command::ClearDeathNotificationDone(..)
                    | Command::SpawnLooper
                    | Command::ClearFreezeNotificationDone(..) => false,
                };

                drop(thread_state);
                drop(proc_state);

                if has_pending_proc_commands {
                    context.binder_proc.wake_process_and_available_thread();
                }

                return Ok(bytes_written);
            }

            // No commands readily available to read. Wait for work. The thread will wait on both
            // the thread queue and the process queue, and loop back to check whether some work is
            // available.
            let event = InterruptibleEvent::new();
            let _requeue_registration = if target_thread.is_none() {
                Some(RequeueEventRegistration::new(context.binder_thread, event.clone()))
            } else {
                None
            };
            let (mut waiter, guard) = SimpleWaiter::new(&event);
            thread_state.command_queue.wait_async_simple(&mut waiter);

            // Ensure the file descriptor has not been closed or interrupted, after registering
            // for the waiters but before waiting.
            if proc_state.closed {
                return error!(EBADF);
            }

            if proc_state.interrupted {
                proc_state.interrupted = false;
                return error!(EINTR);
            }

            // Drop locks before sleeping. Order matters: thread_state first, then proc_state
            // because dropping thread_state registers it in available_threads. If we drop
            // proc_state first, Process could check available_threads while thread_state
            // is still held and falsely assume there are no threads available, missing
            // the queued command!
            drop(thread_state);
            drop(proc_state);

            // Put this thread to sleep. If we know the thread we are sending to, use that to
            // inherit priority.
            context.current_task.block_with_optional_owner_until(
                guard,
                target_thread.as_deref(),
                zx::MonotonicInstant::INFINITE,
            )?;
        }
    }

    /// Copies transaction buffers from the source process' address space to a new buffer in the
    /// target process' shared binder VMO.
    /// Returns the transaction buffers in the target process, as well as the transaction state.
    ///
    /// If `security_context` is present, it must be null terminated.
    pub fn copy_transaction_buffers<'a>(
        &self,
        source: &OperationContext<'_>,
        source_files: &mut Vec<fbinder::FileHandle>,
        target_task: &Task,
        target_resource_accessor: &'a dyn ResourceAccessor,
        target_proc: &BinderProcess,
        data: &binder_transaction_data_sg,
        security_context: Option<&FsStr>,
    ) -> Result<(TransactionBuffers, TransientTransactionState<'a>), TransactionError> {
        // Get the shared memory of the target process.
        let mut shared_memory_lock = target_proc.shared_memory.lock();
        let shared_memory = shared_memory_lock.as_mut().ok_or_else(|| errno!(ENOMEM))?;

        // Allocate a buffer from the target process' shared memory.
        let mut allocations = shared_memory.allocate_buffers(
            data.transaction_data.data_size as usize,
            data.transaction_data.offsets_size as usize,
            data.buffers_size as usize,
            round_up_to_increment(
                security_context.map(|s| s.len()).unwrap_or(0),
                std::mem::size_of::<binder_uintptr_t>(),
            )?,
        )?;

        // Copy the security context content.
        if let Some(data) = security_context {
            let security_buffer = allocations.security_context_buffer.as_mut().unwrap();
            security_buffer.as_mut_bytes()[..data.len()].copy_from_slice(data);
        }

        // SAFETY: `binder_transaction_data` was read from a userspace VMO, which means that all
        // bytes are defined, making union access safe (even if the value is garbage).
        let userspace_addrs = unsafe { data.transaction_data.data.ptr };

        // Copy the data straight into the target's buffer.
        if data.transaction_data.data_size > 0 {
            source.memory_accessor.read_memory_to_slice(
                UserAddress::from(userspace_addrs.buffer),
                allocations.data_buffer.as_mut_bytes(),
            )?;
        }
        if data.transaction_data.offsets_size > 0 {
            source.memory_accessor.read_objects_to_slice(
                UserRef::new(UserAddress::from(userspace_addrs.offsets)),
                allocations.offsets_buffer.as_mut_bytes(),
            )?;
        }

        // Translate any handles/fds from the source process' handle table to the target process'
        // handle table.
        let transient_transaction_state = self.translate_objects(
            source,
            source_files,
            target_task,
            target_resource_accessor,
            target_proc,
            allocations.offsets_buffer.as_bytes(),
            allocations.data_buffer.as_mut_bytes(),
            &mut allocations.scatter_gather_buffer,
        )?;

        Ok((allocations.into(), transient_transaction_state))
    }

    /// Translates file descriptors from the sending process to the receiving process.
    fn translate_files<'a>(
        source: &OperationContext<'_>,
        source_files: &mut Vec<fbinder::FileHandle>,
        target_task: &Task,
        target_resource_accessor: &'a dyn ResourceAccessor,
        fds: Vec<FdNumber>,
        add_action: &mut dyn FnMut(FdNumber),
    ) -> Result<Vec<FdNumber>, Errno> {
        // Create a map of FD to index into `source_files`. This is used to check if the already
        // file exists in `source_files`, allowing us to avoid fetching it via the
        // `source_resource_accessor`.
        let source_map = source_files
            .iter()
            .enumerate()
            .map(|(i, file)| (file.fd.unwrap(), i))
            .collect::<HashMap<_, _>>();
        // Create a vector of FDs to fetch via the `source_resource_accessor`.
        let fds_to_get = fds
            .iter()
            .filter(|fd| !source_map.contains_key(&fd.raw()))
            .copied()
            .collect::<Vec<_>>();
        let mut get_files = if fds_to_get.is_empty() {
            Vec::new()
        } else {
            source.resource_accessor().get_files_with_flags(source.current_task, fds_to_get)?
        };
        let mut drain = get_files.drain(0..);
        // Merge `source_files` and `get_files` together.
        let mut target_files = Vec::with_capacity(fds.len());
        for fd in fds {
            let file = if let Some(pos) = source_map.get(&fd.raw()) {
                let source_file = std::mem::replace(&mut source_files[*pos], Default::default());
                let flags = source_file.flags.ok_or_else(|| errno!(ENOENT))?.into_fidl();
                let new_file = if let Some(handle) = source_file.handle {
                    new_remote_file(source.current_task, handle, flags)?
                } else if let Some(_bag) = source_file.bag {
                    // TODO(https://fxbug.dev/481167098): Support composite file descriptors.
                    log_warn!("FileHandle::bag is not supported");
                    return error!(EBADFD);
                } else {
                    new_null_file(source.current_task, flags)
                };
                (new_file, FdFlags::empty())
            } else if let Some(file) = drain.next() {
                file
            } else {
                return error!(ENOENT);
            };

            security::binder_transfer_file(source.current_task, target_task, &(file.0))?;

            target_files.push(file);
        }
        // Finally add the files to the `target_resource_accessor`.
        target_resource_accessor.add_files_with_flags(source.current_task, target_files, add_action)
    }

    /// Translates binder object handles/FDs from the sending process to the receiver process,
    /// patching the transaction data as needed.
    ///
    /// When a binder object is sent from one process to another, it must be added to the receiving
    /// process' handle table. Conversely, a handle being sent to the process that owns the
    /// underlying binder object should receive the actual pointers to the object.
    ///
    /// When a binder buffer object is sent from one process to another, the buffer described by the
    /// buffer object must be copied into the receiver's address space.
    ///
    /// When a binder file descriptor object is sent from one process to another, the file
    /// descriptor must be `dup`-ed into the receiver's FD table.
    ///
    /// Returns [`TransientTransactionState`], which contains the handles in the target process'
    /// handle table for which temporary strong references were acquired, along with duped FDs. This
    /// object takes care of releasing these resources when dropped, due to an error or a
    /// `BC_FREE_BUFFER` command.
    pub fn translate_objects<'a>(
        &self,
        source: &OperationContext<'_>,
        source_files: &mut Vec<fbinder::FileHandle>,
        target_task: &Task,
        target_resource_accessor: &'a dyn ResourceAccessor,
        target_proc: &BinderProcess,
        offsets: &[binder_uintptr_t],
        transaction_data: &mut [u8],
        sg_buffer: &mut SharedBuffer<'_, u8>,
    ) -> Result<TransientTransactionState<'a>, TransactionError> {
        if offsets.is_empty() {
            return Ok(TransientTransactionState::new(target_resource_accessor, target_proc));
        }

        let mut transaction_state =
            TransientTransactionState::new(target_resource_accessor, target_proc);
        release_on_error!(transaction_state, (), {
            let mut sg_remaining_buffer = sg_buffer.user_buffer();
            let mut sg_buffer_offset = 0;
            let mut files = Vec::with_capacity(offsets.len());
            for (offset_idx, object_offset) in offsets.iter().map(|o| *o as usize).enumerate() {
                // Bounds-check the offset.
                if object_offset >= transaction_data.len() {
                    return error!(EINVAL)?;
                }
                let serialized_object =
                    SerializedBinderObject::from_bytes(&transaction_data[object_offset..])?;
                let translated_object = match serialized_object {
                    SerializedBinderObject::Handle { handle, flags, cookie } => {
                        security::binder_transfer_binder(source.current_task, target_task)?;

                        match handle {
                            Handle::ContextManager => {
                                // The special handle 0 does not need to be translated. It is universal.
                                serialized_object
                            }
                            Handle::Object { index } => {
                                // 1. Find the object and add a guard on it in the
                                //    transaction to ensures the receiving process keep
                                //    it alive until the transactions is finished
                                let (proxy, guard) = source
                                    .binder_proc
                                    .lock()
                                    .handles
                                    .get(index)
                                    .ok_or(TransactionError::Failure)?;
                                if proxy.owner.as_ptr() == target_proc.weak_self.as_ptr() {
                                    // The binder object belongs to the receiving process.

                                    transaction_state.push_guard(guard);

                                    // 2. Convert the binder object from a handle to a local object.
                                    SerializedBinderObject::Object { local: proxy.local, flags }
                                } else {
                                    // The binder object does not belong to the receiving
                                    // process.

                                    // Insert the handle in the handle table of the receiving process
                                    // and add a strong reference to it to ensure it survives for the
                                    // lifetime of the transaction.
                                    let mut actions = RefCountActions::default();
                                    let new_handle = target_proc
                                        .lock()
                                        .insert_for_transaction(guard, &mut actions);
                                    actions.release(());
                                    // Tie this handle's strong reference to be held as long as this
                                    // buffer.
                                    transaction_state.push_handle(new_handle);
                                    SerializedBinderObject::Handle {
                                        handle: new_handle,
                                        flags,
                                        cookie,
                                    }
                                }
                            }
                        }
                    }
                    SerializedBinderObject::Object { local, flags } => {
                        security::binder_transfer_binder(source.current_task, target_task)?;

                        let mut actions = RefCountActions::default();
                        release_after!(actions, (), {
                            // We are passing a binder object across process boundaries. We need
                            // to translate this address to some handle.

                            // Register this binder object if it hasn't already been registered.
                            let guard = source.binder_proc.lock().find_or_register_object(
                                source.binder_thread,
                                local,
                                flags,
                            );
                            // Create a handle in the receiving process that references the binder object
                            // in the sender's process.
                            let handle =
                                target_proc.lock().insert_for_transaction(guard, &mut actions);
                            // Tie this handle's strong reference to be held as long as this buffer.
                            transaction_state.push_handle(handle);

                            // Translate the serialized object into a handle.
                            SerializedBinderObject::Handle { handle, flags, cookie: 0 }
                        })
                    }
                    SerializedBinderObject::File { fd, cookie } => {
                        files.push(TransientFile { object_offset, fd, cookie });
                        continue;
                    }
                    SerializedBinderObject::Buffer {
                        buffer,
                        length,
                        flags,
                        parent,
                        parent_offset,
                    } => {
                        // Copy the memory pointed to by this buffer object into the receiver.
                        if length > sg_remaining_buffer.length {
                            return error!(EINVAL)?;
                        }
                        source.memory_accessor.read_memory_to_slice(
                            buffer,
                            &mut sg_buffer.as_mut_bytes()
                                [sg_buffer_offset..sg_buffer_offset + length],
                        )?;

                        let translated_buffer_address = sg_remaining_buffer.address;

                        // If the buffer has a parent, it means that the parent buffer has a pointer to
                        // this buffer. This pointer will need to be translated to the receiver's
                        // address space.
                        if flags & BINDER_BUFFER_FLAG_HAS_PARENT != 0 {
                            // The parent buffer must come earlier in the object list and already be
                            // copied into the receiver's address space. Otherwise we would be fixing
                            // up memory in the sender's address space, which is marked const in the
                            // userspace runtime.
                            if parent >= offset_idx {
                                return error!(EINVAL)?;
                            }

                            // Find the parent buffer payload. There is a pointer in the buffer
                            // that points to this object.
                            let parent_buffer_payload = find_parent_buffer(
                                transaction_data,
                                sg_buffer,
                                offsets[parent] as usize,
                            )?;

                            // Bounds-check that the offset is within the buffer.
                            if parent_offset >= parent_buffer_payload.len() {
                                return error!(EINVAL)?;
                            }

                            // Patch the pointer with the translated address.
                            translated_buffer_address
                                .write_to_prefix(&mut parent_buffer_payload[parent_offset..])
                                .map_err(|_| errno!(EINVAL))?;
                        }

                        // Update the scatter-gather buffer to account for the buffer we just wrote.
                        // We pad the length of this buffer so that the next buffer starts at an aligned
                        // offset.
                        let padded_length =
                            round_up_to_increment(length, std::mem::size_of::<binder_uintptr_t>())?;
                        sg_remaining_buffer = UserBuffer {
                            address: (sg_remaining_buffer.address + padded_length)?,
                            length: sg_remaining_buffer.length - padded_length,
                        };
                        sg_buffer_offset += padded_length;

                        // Patch this buffer with the translated address.
                        SerializedBinderObject::Buffer {
                            buffer: translated_buffer_address,
                            length,
                            flags,
                            parent,
                            parent_offset,
                        }
                    }
                    SerializedBinderObject::FileArray { num_fds, parent, parent_offset } => {
                        // The parent buffer must come earlier in the object list and already be
                        // copied into the receiver's address space. Otherwise we would be fixing
                        // up memory in the sender's address space, which is marked const in the
                        // userspace runtime.
                        if parent >= offset_idx {
                            return error!(EINVAL)?;
                        }

                        // Find the parent buffer payload. The file descriptor array is in here.
                        let parent_buffer_payload = find_parent_buffer(
                            transaction_data,
                            sg_buffer,
                            offsets[parent] as usize,
                        )?;

                        // Bounds-check that the offset is within the buffer.
                        if parent_offset >= parent_buffer_payload.len() {
                            return error!(EINVAL)?;
                        }

                        // Verify alignment and size before reading the data as a [u32].
                        let (layout, _) =
                            zerocopy::Ref::<&mut [u8], [u32]>::from_prefix_with_elems(
                                &mut parent_buffer_payload[parent_offset..],
                                num_fds,
                            )
                            .map_err(|_| errno!(EINVAL))?;
                        let fd_array = zerocopy::Ref::into_mut(layout);

                        // Dup each file descriptor and re-write the value of the new FD.
                        let new_fds = Self::translate_files(
                            source,
                            source_files,
                            target_task,
                            target_resource_accessor,
                            fd_array.iter().map(|fd| FdNumber::from_raw(*fd as i32)).collect(),
                            // Close this FD if the transaction ends either by success or failure.
                            &mut |fd| transaction_state.push_owned_fd(fd),
                        )?;
                        for (fd, new_fd) in std::iter::zip(fd_array, new_fds) {
                            *fd = new_fd.raw() as u32;
                        }

                        SerializedBinderObject::FileArray { num_fds, parent, parent_offset }
                    }
                };

                translated_object.write_to(&mut transaction_data[object_offset..])?;
            }

            let new_fds = Self::translate_files(
                source,
                source_files,
                target_task,
                target_resource_accessor,
                files.iter().map(|TransientFile { fd, .. }| *fd).collect(),
                // Close this FD if the transaction fails.
                &mut |fd| transaction_state.push_transient_fd(fd),
            )?;
            for (TransientFile { object_offset, cookie, .. }, new_fd) in
                std::iter::zip(files, new_fds)
            {
                SerializedBinderObject::File { fd: new_fd, cookie }
                    .write_to(&mut transaction_data[object_offset..])?;
            }

            Ok(())
        });
        Ok(transaction_state)
    }

    pub fn mmap(
        &self,
        current_task: &CurrentTask,
        binder_proc: &BinderProcess,
        addr: DesiredAddress,
        length: usize,
        prot_flags: ProtectionFlags,
        mapping_options: MappingOptions,
        mapping_name: MappingName,
    ) -> Result<UserAddress, Errno> {
        // Do not support mapping shared memory more than once.
        let mut shared_memory = binder_proc.shared_memory.lock();
        if shared_memory.is_some() {
            return error!(EINVAL);
        }

        // Reject writable mappings (Linux binder_mmap returns -EPERM).
        if prot_flags.contains(ProtectionFlags::WRITE) {
            return error!(EPERM);
        }

        // Create a VMO that will be shared between the driver and the client process.
        let vmo = with_zx_name(
            zx::Vmo::create(length as u64).map_err(|_| errno!(ENOMEM))?,
            b"starnix:device_binder",
        );
        let memory = MemoryObject::from(vmo);

        // Strip WRITE from the userspace handle so mprotect(PROT_WRITE) fails
        // at zx_vmar_protect, matching Linux's VM_MAYWRITE clearing.
        let user_memory = Arc::new(
            memory
                .duplicate_handle(zx::Rights::VMO_DEFAULT - zx::Rights::WRITE)
                .map_err(|_| errno!(ENOMEM))?,
        );

        // Map the VMO into the binder process' address space.
        let mm = current_task.mm()?;
        let user_address =
            mm.map_memory(addr, user_memory, 0, length, prot_flags, mapping_options, mapping_name)?;

        // Map the VMO into the driver's address space.
        match SharedMemory::map(&memory, user_address, length) {
            Ok(mem) => {
                *shared_memory = Some(mem);
                Ok(user_address)
            }
            Err(err) => {
                // Try to cleanup by unmapping from userspace, but ignore any errors. We
                // can't really recover from them.
                let _ = mm.unmap(user_address, length);
                Err(err)
            }
        }
    }

    fn wait_async(
        &self,
        binder_proc: &BinderProcess,
        binder_thread: &BinderThread,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        // THREADING: Always acquire the [`BinderThread::state`] lock before the
        // [`BinderProcess::command_queue`] lock or else it may lead to deadlock.
        let thread_state = binder_thread.lock();

        let handler = match handler {
            EventHandler::None | EventHandler::HandleOnce(_) => handler,
            EventHandler::Enqueue { .. } | EventHandler::Epoll(_) => {
                EventHandler::HandleOnce(Arc::new(Some(handler).into()))
            }
        };

        let w1 = thread_state.command_queue.wait_async_fd_events(waiter, events, handler.clone());
        let w2 = binder_proc.process_waiters.wait_async_fd_events(waiter, events, handler);
        WaitCanceler::merge(w1, w2)
    }
}

/// Finds a buffer object's payload in the transaction. The buffer object describing the payload is
/// deserialized from `transaction_data` at `buffer_object_offset`. The actual payload is located in
/// `sg_buffer`. The buffer object must have already been validated and its payload copied to
/// `sg_buffer`. This is true for parent objects, as they are required to be processed before being
/// referenced by child objects.
fn find_parent_buffer<'a>(
    transaction_data: &[u8],
    sg_buffer: &mut SharedBuffer<'a, u8>,
    buffer_object_offset: usize,
) -> Result<&'a mut [u8], Errno> {
    // The buffer object has already been validated, since the requirement is that parent objects
    // are processed before their children. In addition, the payload has been written by us, so it
    // should be guaranteed to be valid. Still, it is possible for userspace to mutate this memory
    // while we are processing it, so we still perform checked arithmetic to avoid panics in
    // starnix.

    // Verify that the offset is within the transaction data.
    if buffer_object_offset >= transaction_data.len() {
        return error!(EINVAL);
    }

    // Deserialize the parent object buffer and extract the relevant data.
    let (buffer_payload_addr, buffer_payload_length) =
        match SerializedBinderObject::from_bytes(&transaction_data[buffer_object_offset..])? {
            SerializedBinderObject::Buffer { buffer, length, .. } => (buffer, length),
            _ => return error!(EINVAL)?,
        };

    // Calculate the start and end of the buffer payload in the scatter gather buffer.
    // The buffer payload will have been copied to the scatter gather buffer, so recover the
    // offset from its userspace address.
    if buffer_payload_addr < sg_buffer.user_buffer().address {
        // This should never happen unless userspace is messing with us, since we wrote this address
        // during translation.
        return error!(EINVAL);
    }
    let buffer_payload_start = buffer_payload_addr - sg_buffer.user_buffer().address;
    let buffer_payload_end =
        buffer_payload_start.checked_add(buffer_payload_length).ok_or_else(|| errno!(EINVAL))?;

    // Return a slice that represents the parent buffer.
    Ok(&mut sg_buffer.as_mut_bytes()[buffer_payload_start..buffer_payload_end])
}

/// Represents a file descriptor during a binder transaction.
struct TransientFile {
    /// Offset of `BINDER_TYPE_FD` object within transaction data.
    object_offset: usize,
    /// A `BINDER_TYPE_FD` object. A file descriptor.
    fd: FdNumber,
    cookie: binder_uintptr_t,
}
