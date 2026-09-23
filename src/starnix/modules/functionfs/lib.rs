// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use fidl::endpoints::SynchronousProxy;
use fidl_fuchsia_hardware_adb as fadb;
use fuchsia_async as fasync;
use futures_util::StreamExt;
use starnix_core::power::{create_proxy_for_wake_events_counter_zero, mark_proxy_message_handled};
use starnix_core::task::{CurrentTask, EventHandler, Kernel, WaitCanceler, WaitQueue, Waiter};
use starnix_core::vfs::pseudo::vec_directory::{VecDirectory, VecDirectoryEntry};
use starnix_core::vfs::{
    CacheMode, DirectoryEntryType, FileObject, FileObjectState, FileOps, FileSystem,
    FileSystemHandle, FileSystemOps, FileSystemOptions, FsNode, FsNodeInfo, FsNodeOps, FsStr,
    InputBuffer, OutputBuffer, fileops_impl_noop_sync, fileops_impl_seekless, fs_args,
    fs_node_impl_dir_readonly, fs_node_impl_not_dir,
};
use starnix_logging::{log_info, log_warn, track_stub};
use starnix_sync::{FunctionFsResultLock, FunctionFsStateLock, InterruptibleEvent, LockDepMutex};
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    errno, error, gid_t, ino_t, statfs, uid_t, usb_functionfs_event,
    usb_functionfs_event_type_FUNCTIONFS_BIND, usb_functionfs_event_type_FUNCTIONFS_DISABLE,
    usb_functionfs_event_type_FUNCTIONFS_ENABLE, usb_functionfs_event_type_FUNCTIONFS_UNBIND,
};
use std::collections::VecDeque;
use std::ops::Deref;
use std::sync::Arc;
use zerocopy::IntoBytes;

// The node identifiers of different nodes in FunctionFS.
const ROOT_NODE_ID: ino_t = 1;

// Control endpoint is always present in a mounted FunctionFS.
const CONTROL_ENDPOINT: &str = "ep0";
const CONTROL_ENDPOINT_NODE_ID: ino_t = 2;

const OUTPUT_ENDPOINT: &str = "ep1";
const OUTPUT_ENDPOINT_NODE_ID: ino_t = 3;

const INPUT_ENDPOINT: &str = "ep2";
const INPUT_ENDPOINT_NODE_ID: ino_t = 4;

// Magic number of the file system, different from the magic used for Descriptors and Strings.
// Set to the same value as Linux.
const FUNCTIONFS_MAGIC: u32 = 0xa647361;

const ADB_DIRECTORY: &str = "/svc/fuchsia.hardware.adb.Service";

// How long to keep Starnix awake after an ADB interaction. If no ADB reads or
// writes occur within this time period, Starnix will be allowed to suspend.
const ADB_INTERACTION_TIMEOUT: zx::Duration<zx::MonotonicTimeline> = zx::Duration::from_seconds(2);

#[derive(Default)]
struct PendingResult<T: Default> {
    event: Arc<InterruptibleEvent>,
    result: LockDepMutex<Option<Result<T, Errno>>, FunctionFsResultLock>,
}

impl<T: Default> PendingResult<T> {
    fn set_result(&self, res: Result<T, Errno>) {
        let mut result = self.result.lock();
        debug_assert!(result.is_none(), "PendingResult set more than once");

        result.replace(res);
        self.event.notify();
    }
}

struct ReadCommand {
    pending: Arc<PendingResult<Vec<u8>>>,
}

struct WriteCommand {
    data: Vec<u8>,
    pending: Arc<PendingResult<usize>>,
}

/// Handle all of the ADB messages in an async context.
/// We receive commands from the main thread and then proxy them into the ADB channel.
/// We want to hold the wakelock until we have at least one outstanding read, because we
/// are always woken up on a new message. (If we have no outstanding reads we will not
/// receive any new messages).
///
/// At the same time we still need to handle writes and events. These are handled by always
/// clearing the proxy signal, but only clearing the kernel signal if we have an outstanding read.
async fn handle_adb(
    proxy: fadb::UsbAdbImpl_Proxy,
    message_counter: Option<zx::Counter>,
    read_commands: async_channel::Receiver<ReadCommand>,
    write_commands: async_channel::Receiver<WriteCommand>,
    state: Arc<LockDepMutex<FunctionFsState, FunctionFsStateLock>>,
) {
    /// Handle all of the events coming from the ADB device.
    ///
    /// adbd expects to receive events FUNCTIONFS_BIND, FUNCTIONFS_ENABLE, FUNCTION_DISABLE, and
    /// FUNCTIONFS_UNBIND in that order. If it receives these events out of order or does not
    /// receive some of the adb events, it may behave unexpectedly. In particular, please reference
    /// the `StartMonitor` function in `UsbFfsConnection` of `adb/daemon/usb.cpp`.
    ///
    /// This module sends a FUNCTIONFS_BIND event as soon as it is called because `handle_adb` is
    /// called after we've successfully bound to the driver. When the driver is ready to take input
    /// it will send an `OnStatusChanged{ ONLINE }` event, which is when this module sends the
    /// FUNCTIONFS_ENABLE event to indicate that adbd should start processing data.
    ///
    /// When the driver sends an `OnStatusChanged{}` event, meaning that it's not online anymore.
    /// The module will send a FUNCTIONFS_DISABLE event to stop processing data. When the stream
    /// closes, we've unbound from the driver, and the module sends a FUNCTIONFS_UNBIND event.
    async fn handle_events(
        mut stream: fadb::UsbAdbImpl_EventStream,
        message_counter: &Option<zx::Counter>,
        state: Arc<LockDepMutex<FunctionFsState, FunctionFsStateLock>>,
    ) {
        let queue_event = |event| {
            let mut state_locked = state.lock();
            state_locked
                .event_queue
                .push_back(usb_functionfs_event { type_: event as u8, ..Default::default() });
            state_locked.waiters.notify_fd_events(FdEvents::POLLIN);
        };

        queue_event(usb_functionfs_event_type_FUNCTIONFS_BIND);

        while let Some(Ok(fadb::UsbAdbImpl_Event::OnStatusChanged { status })) = stream.next().await
        {
            let is_online = status == fadb::StatusFlags::ONLINE;
            {
                let mut state_locked = state.lock();
                state_locked.is_online = is_online;
                state_locked.event_queue.push_back(usb_functionfs_event {
                    type_: if is_online {
                        usb_functionfs_event_type_FUNCTIONFS_ENABLE
                    } else {
                        usb_functionfs_event_type_FUNCTIONFS_DISABLE
                    } as u8,
                    ..Default::default()
                });
                state_locked.waiters.notify_fd_events(FdEvents::POLLIN);
                state_locked.waiters.notify_all();
            }

            // We can simply clear this after getting a response because we care about
            // reads. Allow new FIDL messages to come through and only go to sleep if
            // we have an outstanding read.
            message_counter.as_ref().map(mark_proxy_message_handled);
        }

        queue_event(usb_functionfs_event_type_FUNCTIONFS_UNBIND);
    }

    /// Consumes a stream of instants and decrements `message_counter` after
    /// each one. As long as one of the instants written to this channel is
    /// still in the future, we want to keep the container awake.
    ///
    /// NOTE: We're reusing `message_counter` in a way that's perhaps confusing:
    /// both as the number of "in flight" requests, and to track whether the ADB
    /// session seems to be idle or not. It may be clearer to have two separate
    /// counters.
    async fn handle_idle_timeouts(
        timeouts: async_channel::Receiver<zx::MonotonicInstant>,
        message_counter: &Option<zx::Counter>,
    ) {
        timeouts
            .for_each(|timeout| async move {
                use fasync::WakeupTime;
                timeout.into_timer().await;
                message_counter.as_ref().map(mark_proxy_message_handled);
            })
            .await
    }

    /// Handle the commands coming from the main thread.
    async fn handle_read_commands(
        proxy: &fadb::UsbAdbImpl_Proxy,
        timeouts_sender: async_channel::Sender<zx::MonotonicInstant>,
        commands: async_channel::Receiver<ReadCommand>,
    ) {
        let timeouts_sender = &timeouts_sender;
        commands
            .for_each(|ReadCommand { pending }| async move {
                // Queue up our receive future. We want to do this before we decrement the counter,
                // which potentially allows the container to suspend.
                let receive_future = proxy.receive();

                // Don't decrement the message counter immediately. Instead, we
                // keep the container awake for some amount of time to allow
                // Starnix to react to the message. Otherwise, the container
                // might go directly to sleep without doing anything.
                timeouts_sender
                    .send(zx::MonotonicInstant::after(ADB_INTERACTION_TIMEOUT))
                    .await
                    .expect("Should be able to send timeout");

                let response = match receive_future.await {
                    Err(err) => {
                        log_warn!("Failed to call UsbAdbImpl.Receive: {err}");
                        error!(EINVAL)
                    }
                    Ok(Err(err)) => {
                        let status = zx::Status::err_from_raw(err);
                        if matches!(
                            status,
                            zx::Status::BAD_STATE | zx::Status::CANCELED | zx::Status::PEER_CLOSED
                        ) {
                            log_info!("Receive failed due to connection shutdown: {status}");
                        } else {
                            log_warn!("Failed to receive data from adb driver: {status}");
                        }
                        // TODO(b/536021189): Fix POSIX error mapping. We should return ESHUTDOWN
                        // on endpoint disable.
                        error!(EINVAL)
                    }
                    Ok(Ok(payload)) => Ok(payload),
                };

                pending.set_result(response);
            })
            .await;
    }

    /// Handle the commands coming from the main thread.
    async fn handle_write_commands(
        proxy: &fadb::UsbAdbImpl_Proxy,
        timeouts_sender: async_channel::Sender<zx::MonotonicInstant>,
        commands: async_channel::Receiver<WriteCommand>,
    ) {
        let timeouts_sender = &timeouts_sender;
        commands
            .for_each(|WriteCommand { data, pending }| async move {
                let response = match proxy.queue_tx(&data).await {
                    Err(err) => {
                        log_warn!("Failed to call UsbAdbImpl.QueueTx: {err}");
                        error!(EINVAL)
                    }
                    Ok(Err(err)) => {
                        log_warn!("Failed to queue data to adb driver: {err}");
                        error!(EINVAL)
                    }
                    Ok(Ok(_)) => Ok(data.len()),
                };

                // Don't decrement the message counter immediately. We use the
                // ADB output as a signal that the ADB session is still
                // interactive.
                timeouts_sender
                    .send(zx::MonotonicInstant::after(ADB_INTERACTION_TIMEOUT))
                    .await
                    .expect("Should be able to send timeout");

                pending.set_result(response);
            })
            .await;
    }

    let (timeouts_sender, timeouts_receiver) = async_channel::unbounded();
    let event_future = handle_events(proxy.take_event_stream(), &message_counter, state);
    let write_commands_future =
        handle_write_commands(&proxy, timeouts_sender.clone(), write_commands);
    let read_commands_future = handle_read_commands(&proxy, timeouts_sender, read_commands);
    let timeout_future = handle_idle_timeouts(timeouts_receiver, &message_counter);
    futures::join!(event_future, write_commands_future, read_commands_future, timeout_future);
}

pub struct FunctionFs;
impl FunctionFs {
    pub fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        if options.source != "adb" {
            track_stub!(TODO("https://fxbug.dev/329699340"), "FunctionFS supports other uses");
            return error!(ENODEV);
        }

        // ADB daemon assumes that ADB works over USB if FunctionFS is able to mount.
        // Check that the ADB directory capability is provided to the kernel, and fail to mount
        // if it is not.
        if let Err(e) = std::fs::read_dir(ADB_DIRECTORY) {
            log_warn!(
                "Attempted to mount FunctionFS for adb, but could not read {ADB_DIRECTORY}: {e}"
            );
            return error!(ENODEV);
        }

        let uid = if let Some(uid) = options.params.get(b"uid") {
            fs_args::parse::<uid_t>(uid.as_ref())?
        } else {
            0
        };
        let gid = if let Some(gid) = options.params.get(b"gid") {
            fs_args::parse::<gid_t>(gid.as_ref())?
        } else {
            0
        };

        let fs = FileSystem::new(current_task.kernel(), CacheMode::Uncached, FunctionFs, options)?;

        let creds = FsCred { uid, gid };
        let info = FsNodeInfo::new(mode!(IFDIR, 0o777), creds);
        fs.create_root_with_info(ROOT_NODE_ID, FunctionFsRootDir::default(), info);
        Ok(fs)
    }
}

impl FileSystemOps for FunctionFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(FUNCTIONFS_MAGIC))
    }

    fn name(&self) -> &'static FsStr {
        b"functionfs".into()
    }
}

#[derive(Default)]
struct FunctionFsState {
    // Keeps track of the number of FileObject's created for the control endpoint.
    // When all FileObjects are closed, the filesystem resets to its initial state.
    // See https://docs.kernel.org/usb/functionfs.html.
    num_control_file_objects: usize,

    // Whether the FunctionFS has input/output endpoints, which are /ep2 and /ep1
    // respectively. /ep0 is the control endpoint and is always available.
    has_input_output_endpoints: bool,

    // Whether the FunctionFS is currently online (host connected).
    is_online: bool,

    adb_read_channel: Option<async_channel::Sender<ReadCommand>>,
    adb_write_channel: Option<async_channel::Sender<WriteCommand>>,

    // FIDL binding to the adb driver, for start and stop calls.
    device_proxy: Option<fadb::DeviceSynchronousProxy>,

    // FunctionFs events that indicate the connection state, to be read through
    // the control endpoint.
    event_queue: VecDeque<usb_functionfs_event>,

    waiters: WaitQueue,
}

pub enum AdbProxyMode {
    /// Don't proxy events at all.
    None,

    /// Have the Starnix runner proxy events such that the container
    /// will wake up if events are received while the container is
    /// suspended.
    WakeContainer,
}

fn connect_to_device(
    proxy: AdbProxyMode,
) -> Result<
    (fadb::DeviceSynchronousProxy, fadb::UsbAdbImpl_SynchronousProxy, Option<zx::Counter>),
    Errno,
> {
    let mut dir = std::fs::read_dir(ADB_DIRECTORY).map_err(|_| errno!(EINVAL))?;

    let Some(Ok(entry)) = dir.next() else {
        return error!(EBUSY);
    };
    let path =
        entry.path().join("adb").into_os_string().into_string().map_err(|_| errno!(EINVAL))?;

    let (client_channel, server_channel) = zx::Channel::create();
    fdio::service_connect(&path, server_channel).map_err(|_| errno!(EINVAL))?;
    let device_proxy = fadb::DeviceSynchronousProxy::new(client_channel);

    let (adb_proxy, server_end) = fidl::endpoints::create_sync_proxy::<fadb::UsbAdbImpl_Marker>();
    let (adb_proxy, message_counter) = match proxy {
        AdbProxyMode::None => (adb_proxy, None),
        AdbProxyMode::WakeContainer => {
            let (adb_proxy, message_counter) = create_proxy_for_wake_events_counter_zero(
                adb_proxy.into_channel(),
                "adb".to_string(),
            );
            let adb_proxy = fadb::UsbAdbImpl_SynchronousProxy::from_channel(adb_proxy);
            (adb_proxy, Some(message_counter))
        }
    };

    device_proxy
        .start_adb(server_end, zx::MonotonicInstant::INFINITE)
        .map_err(|_| errno!(EINVAL))?
        .map_err(|_| errno!(EINVAL))?;
    return Ok((device_proxy, adb_proxy, message_counter));
}

#[derive(Default)]
struct FunctionFsRootDir {
    state: Arc<LockDepMutex<FunctionFsState, FunctionFsStateLock>>,
}

impl FunctionFsRootDir {
    fn create_endpoints(&self, kernel: &Kernel) -> Result<(), Errno> {
        let mut state = self.state.lock();

        // create_endpoints can be called multiple times as descriptors are written
        // to the control endpoint.
        if state.has_input_output_endpoints {
            return Ok(());
        }
        let (device_proxy, adb_proxy, message_counter) =
            connect_to_device(AdbProxyMode::WakeContainer)?;
        state.device_proxy = Some(device_proxy);

        let (read_command_sender, read_command_receiver) = async_channel::unbounded();
        state.adb_read_channel = Some(read_command_sender);

        let (write_command_sender, write_command_receiver) = async_channel::unbounded();
        state.adb_write_channel = Some(write_command_sender);

        state.event_queue.clear();

        let state_copy = Arc::clone(&self.state);
        // Spawn our future that will handle all of the ADB messages.
        // Spawn our future that will handle all of the ADB messages.
        kernel.kthreads.spawn_future(
            move || async move {
                let adb_proxy = fadb::UsbAdbImpl_Proxy::new(fidl::AsyncChannel::from_channel(
                    adb_proxy.into_channel(),
                ));
                handle_adb(
                    adb_proxy,
                    message_counter,
                    read_command_receiver,
                    write_command_receiver,
                    state_copy,
                )
                .await
            },
            "functionfs_adb_worker",
        );

        state.has_input_output_endpoints = true;
        Ok(())
    }

    fn from_fs(fs: &FileSystem) -> &Self {
        fs.root()
            .node
            .downcast_ops::<FunctionFsRootDir>()
            .expect("failed to downcast functionfs root dir")
    }

    fn from_file(file: &FileObject) -> &Self {
        Self::from_fs(&file.fs)
    }

    fn on_control_opened(&self) {
        let mut state = self.state.lock();
        state.num_control_file_objects += 1;
    }

    fn on_control_closed(&self) {
        let mut state = self.state.lock();
        state.num_control_file_objects -= 1;
        if state.num_control_file_objects == 0 {
            // When all control endpoints are closed, the filesystem resets to its initial state.
            if let Some(device_proxy) = state.device_proxy.as_ref() {
                let _ = device_proxy
                    .stop_adb(zx::MonotonicInstant::INFINITE)
                    .map_err(|_| errno!(EINVAL));
            }

            state.has_input_output_endpoints = false;
            state.is_online = false;
            state.adb_read_channel = None;
            state.adb_write_channel = None;
        }
    }

    fn wait_until_online(
        &self,
        current_task: &CurrentTask,
        file: &FileObject,
    ) -> Result<(), Errno> {
        loop {
            let waiter = {
                let state = self.state.lock();
                if state.is_online {
                    return Ok(());
                }
                if file.flags().contains(OpenFlags::NONBLOCK) {
                    return error!(EAGAIN);
                }
                let waiter = Waiter::new();
                state.waiters.wait_async(&waiter);
                waiter
            };
            waiter.wait(current_task)?;
        }
    }

    fn available(&self) -> usize {
        let state = self.state.lock();
        state.event_queue.len()
    }

    fn write(
        &self,
        current_task: &CurrentTask,
        file: &FileObject,
        bytes: &[u8],
    ) -> Result<usize, Errno> {
        self.wait_until_online(current_task, file)?;

        let data = Vec::from(bytes);
        let pending = Arc::<PendingResult<usize>>::default();
        let guard = pending.event.begin_wait();

        if let Some(channel) = self.state.lock().adb_write_channel.as_ref() {
            channel
                .send_blocking(WriteCommand { data, pending: pending.clone() })
                .map_err(|_| errno!(EINVAL))?;
        } else {
            return error!(ENODEV);
        }

        current_task.block_until(guard, zx::MonotonicInstant::INFINITE)?;

        let mut result = pending.result.lock();
        result.take().ok_or_else(|| errno!(EINTR))?
    }

    fn read(&self, current_task: &CurrentTask, file: &FileObject) -> Result<Vec<u8>, Errno> {
        self.wait_until_online(current_task, file)?;

        let pending = Arc::<PendingResult<Vec<u8>>>::default();
        let guard = pending.event.begin_wait();
        if let Some(channel) = self.state.lock().adb_read_channel.as_ref() {
            channel
                .send_blocking(ReadCommand { pending: pending.clone() })
                .map_err(|_| errno!(EINVAL))?;
        } else {
            return error!(ENODEV);
        }

        current_task.block_until(guard, zx::MonotonicInstant::INFINITE)?;

        let mut result = pending.result.lock();
        result.take().ok_or_else(|| errno!(EINTR))?
    }
}

impl FsNodeOps for FunctionFsRootDir {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let mut entries = vec![];
        entries.push(VecDirectoryEntry {
            entry_type: DirectoryEntryType::REG,
            name: CONTROL_ENDPOINT.into(),
            inode: Some(CONTROL_ENDPOINT_NODE_ID),
        });

        let state = self.state.lock();
        if state.has_input_output_endpoints {
            entries.push(VecDirectoryEntry {
                entry_type: DirectoryEntryType::REG,
                name: INPUT_ENDPOINT.into(),
                inode: Some(INPUT_ENDPOINT_NODE_ID),
            });
            entries.push(VecDirectoryEntry {
                entry_type: DirectoryEntryType::REG,
                name: OUTPUT_ENDPOINT.into(),
                inode: Some(OUTPUT_ENDPOINT_NODE_ID),
            });
        }

        Ok(VecDirectory::new_file(entries))
    }

    fn lookup(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<starnix_core::vfs::FsNodeHandle, Errno> {
        let name = std::str::from_utf8(name).map_err(|_| errno!(ENOENT))?;
        let cred = node.info().cred();
        match name {
            CONTROL_ENDPOINT => Ok(node.fs().create_node(
                CONTROL_ENDPOINT_NODE_ID,
                FunctionFsControlEndpoint,
                FsNodeInfo::new(mode!(IFREG, 0o600), cred),
            )),
            OUTPUT_ENDPOINT => Ok(node.fs().create_node(
                OUTPUT_ENDPOINT_NODE_ID,
                FunctionFsOutputEndpoint,
                FsNodeInfo::new(mode!(IFREG, 0o600), cred),
            )),
            INPUT_ENDPOINT => Ok(node.fs().create_node(
                INPUT_ENDPOINT_NODE_ID,
                FunctionFsInputEndpoint,
                FsNodeInfo::new(mode!(IFREG, 0o600), cred),
            )),
            _ => error!(ENOENT),
        }
    }
}

// FunctionFS Control Endpoint is both readable and writable.
// Clients should write USB descriptors to the endpoint to setup the USB connection.
// Clients can read `usb_functionfs_event`s to know about the USB connection state.
struct FunctionFsControlEndpoint;
impl FsNodeOps for FunctionFsControlEndpoint {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let fs = node.fs();
        let rootdir = fs
            .root()
            .node
            .downcast_ops::<FunctionFsRootDir>()
            .expect("failed to downcast functionfs root dir");
        rootdir.on_control_opened();
        Ok(Box::new(FunctionFsControlEndpoint))
    }
}

impl FileOps for FunctionFsControlEndpoint {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn close(self: Box<Self>, file: &FileObjectState, _current_task: &CurrentTask) {
        let rootdir = FunctionFsRootDir::from_fs(&file.fs);
        rootdir.on_control_closed();
    }

    fn read(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        // The control endpoint does not currently implement blocking read.
        // ADB would only read from this endpoint after polling it.
        track_stub!(
            TODO("https://fxbug.dev/329699340"),
            "FunctionFS blocking read on control endpoint"
        );

        let rootdir = FunctionFsRootDir::from_file(file);

        let mut state = rootdir.state.lock();
        if !state.event_queue.is_empty() {
            if data.available() < std::mem::size_of::<usb_functionfs_event>() {
                return error!(EINVAL);
            }
        } else {
            return error!(EAGAIN);
        }
        let front = state.event_queue.pop_front().expect("pop from non-empty event queue");
        data.write(front.as_bytes())
    }

    fn write(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        // The ADB driver creates and passes its own descriptors to the host system over the wire,
        // and so, Starnix does not need to parse the descriptors that Android sends.
        // Here we directly attempt to connect to the driver via FIDL, and create endpoints for data transfer.
        track_stub!(TODO("https://fxbug.dev/329699340"), "FunctionFS should parse descriptors");

        let rootdir = FunctionFsRootDir::from_file(file);
        rootdir.create_endpoints(current_task.kernel().deref())?;

        Ok(data.drain())
    }

    fn wait_async(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        let rootdir = FunctionFsRootDir::from_file(file);
        let state = rootdir.state.lock();
        Some(state.waiters.wait_async_fd_events(waiter, events, handler))
    }

    fn query_events(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let rootdir = FunctionFsRootDir::from_file(file);
        if rootdir.available() > 0 { Ok(FdEvents::POLLIN) } else { Ok(FdEvents::empty()) }
    }
}

// FunctionFSInputEndpoint is device to host communication, a.k.a. the "IN" USB direction.
// This endpoint is writable, and not readable.
struct FunctionFsInputEndpoint;
impl FsNodeOps for FunctionFsInputEndpoint {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(FunctionFsInputEndpoint))
    }
}

impl FileOps for FunctionFsInputEndpoint {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        error!(EINVAL)
    }

    fn write(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let bytes = data.read_all()?;
        let rootdir = FunctionFsRootDir::from_file(file);
        rootdir.write(current_task, file, &bytes)
    }
}

// FunctionFSOutputEndpoint is host to device communication, a.k.a. the "OUT" USB direction.
// This endpoint is readable, and not writable.
struct FunctionFsOutputEndpoint;
impl FsNodeOps for FunctionFsOutputEndpoint {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(FunctionFsOutputFileObject))
    }
}

struct FunctionFsOutputFileObject;

impl FileOps for FunctionFsOutputFileObject {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let rootdir = FunctionFsRootDir::from_file(file);
        let payload = rootdir.read(current_task, file)?;
        if payload.len() > data.available() {
            // This means the data will only be partially written, with the rest discarded.
            // Instead of attempting this, we'll instead return error to the client.
            return error!(EINVAL);
        }

        data.write(&payload)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(EINVAL)
    }
}
