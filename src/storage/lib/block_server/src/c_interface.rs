// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Operation, RequestId, TraceFlowId};
use crate::{IntoOrchestrator, callback_interface};
use fidl::endpoints::RequestStream;
use fidl_fuchsia_storage_block as fblock;
use fidl_fuchsia_storage_block::MAX_TRANSFER_UNBOUNDED;
use fuchsia_async as fasync;
use fuchsia_sync::{Condvar, Mutex};
use futures::stream::{AbortHandle, Abortable};
use std::borrow::{Borrow, Cow};
use std::ffi::{CStr, c_char, c_void};
use std::num::NonZero;
use std::sync::Arc;

/// cbindgen:no-export
pub type Session = callback_interface::Session<InterfaceAdapter>;

#[repr(C)]
pub struct Callbacks {
    /// An opaque context object retained by this library.  The library will pass this back into all
    /// callbacks.  The memory pointed to by `context` must last until [`block_server_delete`] is
    /// called.
    pub context: *mut c_void,
    /// Starts a thread.  The implementation must call [`block_server_thread`] on this newly created
    /// thread, providing `arg`.  Once this completes, the implementation must NOT use the block
    /// server for which the thread was started, as it could be destroyed at any time.
    /// The implementation must call [`block_server_thread_release`] after [`block_server_thread`]
    /// completes (but before [`block_server_delete`] is called).
    pub start_thread: unsafe extern "C" fn(context: *mut c_void, arg: *const c_void),
    /// Notifies the implementation of a new session.  The implementation must call
    /// [`block_server_session_run`] on a separate thread, and must call
    /// [`block_server_session_release`] after [`block_server_session_run`] (but before
    /// [`block_server_delete`] is called).
    pub on_new_session: unsafe extern "C" fn(context: *mut c_void, session: *const Session),
    /// Submits a batch of requests to be handled by the implementation.  The implementation must
    /// not retain references to `requests` after it returns.  The implementation must ensure that
    /// [`block_server_send_reply`] is called exactly once with the request ID of each entry in
    /// `requests`, regardless of its status; this call can be asynchronous but must occur before
    /// [`block_server_delete`] is called.  Note that a reply must be sent for every request before
    /// shutdown.
    pub on_requests:
        unsafe extern "C" fn(context: *mut c_void, requests: *mut Request, request_count: usize),
    /// Logs `message` to the implementation's logger.  The implementation must not retain
    /// references to `message`.
    pub log: unsafe extern "C" fn(context: *mut c_void, message: *const c_char, message_len: usize),
}

impl Callbacks {
    #[allow(dead_code)]
    fn log(&self, msg: &str) {
        let msg = msg.as_bytes();
        // SAFETY: This is safe if `context` and `log` are good.
        unsafe {
            (self.log)(self.context, msg.as_ptr() as *const c_char, msg.len());
        }
    }
}

/// cbindgen:no-export
#[allow(dead_code)]
pub struct UnownedVmo(zx::sys::zx_handle_t);

#[repr(C)]
pub struct Request {
    pub request_id: RequestId,
    pub operation: Operation,
    pub trace_flow_id: TraceFlowId,
    pub vmo: UnownedVmo,
}

unsafe impl Send for Callbacks {}
unsafe impl Sync for Callbacks {}

/// Implements [`callback_interface::Interface`] using C callbacks.
pub struct InterfaceAdapter {
    callbacks: Callbacks,
    info: super::DeviceInfo,
}

impl callback_interface::Interface for InterfaceAdapter {
    type Orchestrator = Orchestrator;

    fn get_info(&self) -> Cow<'_, super::DeviceInfo> {
        Cow::Borrowed(&self.info)
    }

    fn spawn_session(&self, session: Arc<Session>) {
        unsafe {
            (self.callbacks.on_new_session)(self.callbacks.context, Arc::into_raw(session));
        }
    }

    fn on_requests(&self, requests: &[callback_interface::Request]) {
        let mut c_requests = Vec::with_capacity(requests.len());
        for req in requests {
            c_requests.push(Request {
                request_id: req.request_id,
                operation: req.operation.clone(),
                trace_flow_id: req.trace_flow_id,
                // We are handing out unowned references to the VMO here.  This is safe because the
                // VMO bin holds references to any closed VMOs until all preceding operations have
                // finished.
                vmo: UnownedVmo(
                    req.vmo.as_ref().map(|v| v.raw_handle()).unwrap_or(zx::sys::ZX_HANDLE_INVALID),
                ),
            });
        }
        unsafe {
            (self.callbacks.on_requests)(
                self.callbacks.context,
                c_requests.as_mut_ptr(),
                c_requests.len(),
            )
        }
    }
}

#[repr(C)]
pub struct PartitionInfo {
    pub device_flags: u32,
    pub start_block: u64,
    pub block_count: u64,
    pub block_size: u32,
    pub type_guid: [u8; 16],
    pub instance_guid: [u8; 16],
    pub name: *const c_char,
    pub flags: u64,
    pub max_transfer_size: u32,
}

/// cbindgen:no-export
#[allow(non_camel_case_types)]
type zx_handle_t = zx::sys::zx_handle_t;

/// cbindgen:no-export
#[allow(non_camel_case_types)]
type zx_status_t = zx::sys::zx_status_t;

impl PartitionInfo {
    /// # Safety
    ///
    /// [`self.name`] must point to valid, null-terminated C-string, or be a nullptr.
    unsafe fn to_rust(&self) -> super::DeviceInfo {
        super::DeviceInfo::Partition(super::PartitionInfo {
            device_flags: fblock::DeviceFlag::from_bits_truncate(self.device_flags),
            start_block_offset: Some(self.start_block),
            block_count: self.block_count,
            type_guid: self.type_guid,
            instance_guid: self.instance_guid,
            name: if self.name.is_null() {
                "".to_string()
            } else {
                String::from_utf8_lossy(unsafe { CStr::from_ptr(self.name).to_bytes() }).to_string()
            },
            flags: Some(self.flags),
            max_transfer_blocks: if self.max_transfer_size != MAX_TRANSFER_UNBOUNDED {
                NonZero::new(self.max_transfer_size / self.block_size)
            } else {
                None
            },
        })
    }
}

struct ExecutorMailbox(Mutex<Mail>, Condvar);

impl ExecutorMailbox {
    fn post(&self, mail: Mail) -> Mail {
        let old = std::mem::replace(&mut *self.0.lock(), mail);
        self.1.notify_all();
        old
    }

    fn new() -> Self {
        Self(Mutex::default(), Condvar::new())
    }
}

type ShutdownCallback = unsafe extern "C" fn(*mut c_void);

#[derive(Default)]
enum Mail {
    #[default]
    None,
    Initialized(fasync::ScopeHandle, AbortHandle),
    AsyncShutdown(*const BlockServer, ShutdownCallback, *mut c_void),
    ThreadFinished(*const BlockServer, ShutdownCallback, *mut c_void),
    Finished,
}

// SAFETY: `Mail::AsyncShutdown` is thread-safe.
unsafe impl Send for Mail {}

pub struct Orchestrator {
    session_manager: callback_interface::SessionManager<InterfaceAdapter>,
    mbox: ExecutorMailbox,
}

impl IntoOrchestrator for Arc<Orchestrator> {
    type SM = callback_interface::SessionManager<InterfaceAdapter>;

    fn into_orchestrator(self) -> Arc<Orchestrator> {
        self
    }
}

impl Borrow<callback_interface::SessionManager<InterfaceAdapter>> for Orchestrator {
    fn borrow(&self) -> &callback_interface::SessionManager<InterfaceAdapter> {
        &self.session_manager
    }
}

pub struct BlockServer {
    server: super::BlockServer<callback_interface::SessionManager<InterfaceAdapter>>,
    scope: fasync::ScopeHandle,
    abort_handle: AbortHandle,
    orchestrator: Arc<Orchestrator>,
}

/// Creates a new block server.  Returns nullptr on failure (e.g. if the thread to run the block
/// server failed to start).
///
/// # Safety
///
/// All callbacks in `callbacks` must be safe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_new(
    partition_info: &PartitionInfo,
    callbacks: Callbacks,
) -> *mut BlockServer {
    let start_thread = callbacks.start_thread;
    let context = callbacks.context;
    let block_size = partition_info.block_size;

    let session_manager = callback_interface::SessionManager::new(
        Arc::new(InterfaceAdapter { callbacks, info: unsafe { partition_info.to_rust() } }),
        block_size,
    );

    let orchestrator = Arc::new(Orchestrator { session_manager, mbox: ExecutorMailbox::new() });

    unsafe {
        (start_thread)(context, Arc::into_raw(orchestrator.clone()) as *const c_void);
    }

    let mbox = &orchestrator.mbox;
    let mail = {
        let mut mail = mbox.0.lock();
        mbox.1.wait_while(&mut mail, |mail| matches!(mail, Mail::None));
        std::mem::replace(&mut *mail, Mail::None)
    };

    match mail {
        Mail::Initialized(scope, abort_handle) => Box::into_raw(Box::new(BlockServer {
            server: super::BlockServer::new(block_size, orchestrator.clone()),
            scope,
            abort_handle,
            orchestrator: orchestrator.clone(),
        })),
        Mail::Finished => std::ptr::null_mut(),
        _ => unreachable!(),
    }
}

/// Runs the main loop to handle FIDL requests for the block server.  Blocks until the server is
/// shutting down.
///
/// After this returns, the caller *must* call [`block_server_thread_release`] on the same thread.
///
/// # Safety
///
/// `arg` must be the value passed to the `start_thread` callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_thread(arg: *const c_void) {
    let orchestrator = unsafe { &*(arg as *const Orchestrator) };

    let mut executor = fasync::LocalExecutor::default();
    let scope = fasync::Scope::new();

    // Create a future which will run until `abort_handle` is aborted, so that the scope will run
    // that long as well.
    let (abort_handle, registration) = AbortHandle::new_pair();
    let root_task = scope.spawn(async move {
        let _ = Abortable::new(std::future::pending::<()>(), registration).await;
    });
    orchestrator.mbox.post(Mail::Initialized(scope.clone(), abort_handle));

    // Block until the abort handle is fired.  This is the main entry point, tasks are spawned on
    // this executor.
    let _ = executor.run_singlethreaded(root_task);

    // At this point, the abort handle was fired, which happens when shutdown begins.
    {
        let mut mbox = orchestrator.mbox.0.lock();
        let mail = std::mem::take(&mut *mbox);
        if let Mail::AsyncShutdown(block_server, callback, arg) = mail {
            *mbox = Mail::ThreadFinished(block_server, callback, arg);
            orchestrator.mbox.1.notify_all();
        } else {
            *mbox = mail;
        }
    }

    // Synchronously cancel the scope which is processing FIDL requests.
    let _ = executor.run_singlethreaded(scope.cancel());

    // No more sessions can be created.  Before we drop the `BlockServer` instance we must make
    // sure there are no sessions running because otherwise there could be outstanding responses
    // that would result in `block_server_send_reply` being called.
    orchestrator.session_manager.terminate();
}

/// Called to release the thread.  This *must* always be called on the thread spawned by
/// [`Callbacks::start_thread`], regardless of whether [`block_server_thread`] is called or not.
///
/// # Safety
///
/// `arg` must be the value passed to the `start_thread` callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_thread_release(arg: *const c_void) {
    // SAFETY: This balances the `into_raw` in `block_server_new`.
    let orchestrator = unsafe { Arc::from_raw(arg as *const Orchestrator) };

    let mail = orchestrator.mbox.post(Mail::Finished);
    match mail {
        Mail::None | Mail::Finished => {}
        Mail::ThreadFinished(block_server, callback, arg) => {
            // SAFETY: No other threads are running now, so it should be safe to drop the
            // `BlockServer` instance.
            let _ = unsafe { Box::from_raw(block_server as *mut BlockServer) };

            // SAFETY: Whoever supplied the callback must guarantee it's safe.
            unsafe {
                callback(arg);
            }
        }
        _ => panic!("block_server_thread_release called while thread is still running"),
    }
}

/// # Safety
///
/// `block_server` must be valid and either `block_server_delete` or `block_server_delete_async` may
/// only be called once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_delete(block_server: *const BlockServer) {
    {
        // SAFETY: The caller asserts that `block_server` is valid.
        let server = unsafe { &*block_server };

        // NOTE: The order here is important.  We must terminate the server's main thread first
        // before terminating sessions to avoid races that can happen when a session has been just
        // created.

        // Start by terminating the main server thread.
        server.abort_handle.abort();
        {
            let mbox = &server.orchestrator.mbox;
            let mut mail = mbox.0.lock();
            mbox.1.wait_while(&mut mail, |mbox| !matches!(mbox, Mail::Finished));
        }

        // Now that is done, no more sessions can be created, so now we can terminate all sessions.
        Borrow::<callback_interface::SessionManager<InterfaceAdapter>>::borrow(
            server.orchestrator.as_ref(),
        )
        .terminate();
    }

    // SAFETY: No other threads are running, so we can drop the `BlockServer` instance.
    let _ = unsafe { Box::from_raw(block_server as *mut BlockServer) };
}

/// # Safety
///
/// `block_server` must be valid and either `block_server_delete` or `block_server_delete_async` may
/// only be called once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_delete_async(
    block_server: *const BlockServer,
    callback: ShutdownCallback,
    arg: *mut c_void,
) {
    let abort_handle = {
        // SAFETY: The caller asserts that `block_server` is valid.
        let server = unsafe { &*block_server };

        // We must post to the mailbox before we call abort to ensure that the callback is correctly
        // called.
        assert!(!matches!(
            server.orchestrator.mbox.post(Mail::AsyncShutdown(block_server, callback, arg)),
            Mail::Finished
        ));

        server.abort_handle.clone()
    };

    // As soon as we call `abort`, we must assume the `BlockServer` instance has been dropped.
    abort_handle.abort();
}

/// Serves the Volume protocol for this server.  `handle` is consumed.
///
/// # Safety
///
/// `block_server` and `handle` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_serve(block_server: *const BlockServer, handle: zx_handle_t) {
    // SAFETY: The caller ensures that `block_server` and `handle` are valid.
    let (block_server, handle) = unsafe { (&*block_server, zx::NullableHandle::from_raw(handle)) };
    block_server.scope.spawn(async move {
        let _ = block_server
            .server
            .handle_requests(fblock::BlockRequestStream::from_channel(
                fasync::Channel::from_channel(handle.into()),
            ))
            .await;
    });
}

/// Serves the Mapper protocol for this server.  `handle` is consumed.
///
/// # Safety
///
/// `block_server` and `handle` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_serve_mapper(
    block_server: *const BlockServer,
    handle: zx_handle_t,
) {
    // SAFETY: The caller ensures that `block_server` and `handle` are valid.
    let (block_server, handle) = unsafe { (&*block_server, zx::NullableHandle::from_raw(handle)) };
    block_server.scope.spawn(async move {
        let _ = block_server
            .server
            .handle_mapper_requests(fblock::MapperRequestStream::from_channel(
                fasync::Channel::from_channel(handle.into()),
            ))
            .await;
    });
}

/// # Safety
///
/// `session` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_session_run(session: &Session) {
    let session = unsafe { Arc::from_raw(session) };
    session.run();
    let _ = Arc::into_raw(session);
}

/// # Safety
///
/// `session` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_session_release(session: &Session) {
    session.terminate_async();
    unsafe { Arc::from_raw(session) };
}

/// # Safety
///
/// `block_server` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn block_server_send_reply(
    block_server: &BlockServer,
    request_id: RequestId,
    status: zx_status_t,
) {
    block_server.orchestrator.session_manager.complete_request(request_id, zx::Status::ok(status));
}
