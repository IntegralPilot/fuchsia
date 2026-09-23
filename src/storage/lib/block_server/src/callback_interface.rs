// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::verifier::Verifier;
use crate::{
    ActiveRequests, DecodedRequest, DeviceInfo, HandleRequestResult, IntoOrchestrator, OffsetMap,
    Operation, RequestId, SessionHelper, TraceFlowId, WriteFlags,
};
use anyhow::Error;
use block_protocol::{BlockFifoRequest, BlockFifoResponse};
use fidl_fuchsia_storage_block as fblock;
use fuchsia_sync::{Condvar, Mutex};
use futures::TryStreamExt as _;
use futures::stream::{AbortHandle, Abortable};
use mapping::reader::BlockService;
use std::borrow::{Borrow, Cow};
use std::collections::{HashMap, VecDeque};
use std::mem::MaybeUninit;
use std::sync::{Arc, OnceLock, Weak};

pub mod block_service;
pub use block_service::DefaultCallbackBlockService;

/// An in-flight request.
#[derive(Clone, Debug)]
pub struct Request {
    /// The ID that a request is associated with, for later completion in
    /// [`SessionManager::complete_request`].  Note that this is not necessarily a unique
    /// identifier, and multiple Requests may have the same request_id (e.g. due to request
    /// splitting).  The library internally reference-counts requests which use this ID.
    pub request_id: RequestId,
    pub operation: Operation,
    pub trace_flow_id: TraceFlowId,
    /// `vmo` is always Some for Operation::Read or Operation::Write, and None otherwise.
    pub vmo: Option<Arc<zx::Vmo>>,
}

pub trait Interface: Send + Sync + Unpin + 'static {
    type Orchestrator: Borrow<SessionManager<Self>> + Send + Sync;

    /// Called to get block/partition information.
    fn get_info(&self) -> Cow<'_, DeviceInfo>;

    /// Start running a new session.  The interface must ensure that [`Session::run`] is called
    /// on a different thread.
    fn spawn_session(&self, session: Arc<Session<Self>>);

    /// Starts a batch of requests.  The implementation may block if there are too many in-flight
    /// requests, providing pushback.  The interface is responsible for eventually calling
    /// [`SessionManager::complete_request`] for each request (even during server shutdown).
    ///
    /// Implementations are responsible for checking that request block ranges fall within valid
    /// device/partition bounds, and completing with `zx::Status::OUT_OF_RANGE` if out of bounds.
    fn on_requests(&self, requests: &[Request]);

    /// Returns the BlockService implementation for this interface.
    ///
    /// By default, returns the fallback [`DefaultCallbackBlockService`].
    fn into_block_service(
        self: Arc<Self>,
        orchestrator: &Arc<Self::Orchestrator>,
    ) -> Arc<dyn BlockService> {
        Arc::new(DefaultCallbackBlockService::<Self>::new(orchestrator))
    }

    /// Called when a new mapper session is opened.
    /// Returns the [`Verifier`] for page delivery.
    fn on_open_mapper_session(
        &self,
        _mapping_vmo: &zx::Vmo,
        delivery_queue: zx::Vmo,
    ) -> Result<Verifier, zx::Status> {
        Ok(Verifier::new(delivery_queue))
    }
}

/// A bit flag set on [`RequestId`] values for internal requests issued by the server (such as
/// [`DefaultCallbackBlockService`] reads) rather than external client sessions. This allows
/// [`SessionManager::complete_request`] to distinguish internal requests and route completions
/// to internal callback handlers instead of client session FIFOs.
const INTERNAL_REQUEST_FLAG: usize = 1 << 63;

#[derive(Default)]
struct InflightRequests {
    /// Total number of in-flight requests submitted to `Interface::on_requests`.
    count: usize,
    /// Completion callbacks for internal requests (e.g. from `DefaultCallbackBlockService`).
    callbacks: HashMap<RequestId, Box<dyn FnOnce(Result<(), zx::Status>) + Send>>,
    /// Monotonically increasing counter for allocating internal request IDs.
    next_internal_id: usize,
}

struct SessionManagerInner<I: Interface + ?Sized> {
    open_sessions: HashMap<usize, Weak<Session<I>>>,
}

// The signals used on the session's FIFO.

/// Signalled on the session's FIFO to wake up the FIFO loop.
const FIFO_WAKE_SIGNAL: zx::Signals = zx::Signals::USER_0;

/// Signalled on the session's FIFO to terminate the FIFO loop.
const SHUTDOWN_SIGNAL: zx::Signals = zx::Signals::USER_1;

pub struct SessionManager<I: Interface + ?Sized> {
    interface: Arc<I>,
    block_size: u32,
    // These represent active *client* requests, which correspond to one or more in-flight requests.
    active_requests: ActiveRequests<Arc<Session<I>>>,
    inflight_requests: Mutex<InflightRequests>,
    no_inflight_requests_condvar: Condvar,
    inner: Mutex<SessionManagerInner<I>>,
    no_open_sessions_condvar: Condvar,
    block_service: OnceLock<Arc<dyn BlockService>>,
}

impl<I: Interface + ?Sized> super::SessionManager for SessionManager<I> {
    const SUPPORTS_DECOMPRESSION: bool = false;

    type Orchestrator = I::Orchestrator;
    type Session = Arc<Session<I>>;

    fn session_eq(a: &Arc<Session<I>>, b: &Arc<Session<I>>) -> bool {
        Arc::ptr_eq(a, b)
    }

    async fn on_attach_vmo(
        _orchestrator: Arc<Self::Orchestrator>,
        _vmo: &Arc<zx::Vmo>,
    ) -> Result<(), zx::Status> {
        Ok(())
    }

    /// Called when a new session request stream is opened. Constructs a `SessionHelper` with the
    /// client-provided `offset_map` and device constraints, spawns the session runner via
    /// `I::spawn_session`, and processes FIFO requests until the session channel is closed or
    /// aborted.
    async fn open_session(
        orchestrator: Arc<Self::Orchestrator>,
        mut stream: fblock::SessionRequestStream,
        offset_map: OffsetMap,
        block_size: u32,
    ) -> Result<(), Error> {
        let sm: &SessionManager<I> = orchestrator.as_ref().borrow();
        let max_blocks = sm.get_info().max_transfer_blocks();
        let (helper, fifo) =
            SessionHelper::new(orchestrator.clone(), offset_map, max_blocks, block_size)?;
        let (abort_handle, registration) = AbortHandle::new_pair();
        let session = Arc::new(Session {
            helper,
            fifo,
            queue: Mutex::default(),
            abort_handle,
            close_callback: Mutex::new(None),
        });
        let sm = orchestrator.as_ref().borrow();
        sm.inner
            .lock()
            .open_sessions
            .insert(Arc::as_ptr(&session) as usize, Arc::downgrade(&session));

        sm.interface.spawn_session(session.clone());

        let result = Abortable::new(
            async {
                while let Some(request) = stream.try_next().await? {
                    match session.helper.handle_request(request).await? {
                        HandleRequestResult::Ok => {}
                        HandleRequestResult::Closed(callback) => {
                            *session.close_callback.lock() = Some(callback);
                            break;
                        }
                    }
                }
                Ok(())
            },
            registration,
        )
        .await
        .unwrap_or_else(|e| Err(e.into()));

        let _ = session.fifo.signal(zx::Signals::empty(), SHUTDOWN_SIGNAL);

        result
    }

    fn open_mapper_session(
        orchestrator: Arc<Self::Orchestrator>,
        session: fidl::endpoints::ServerEnd<fblock::MapperSessionMarker>,
        mapping_vmo: zx::Vmo,
        _block_size: u32,
        port: Option<zx::Port>,
        delivery_queue: Option<zx::Vmo>,
    ) -> Result<impl Future<Output = Result<(), Error>> + Send, zx::Status> {
        let sm: &SessionManager<I> = orchestrator.as_ref().borrow();
        let service = sm.into_block_service(&orchestrator);
        let interface = sm.interface.clone();
        crate::mapper::serve_mapper_session(
            Arc::new(move |mapping_vmo: &zx::Vmo, dq: zx::Vmo| {
                interface.on_open_mapper_session(mapping_vmo, dq)
            }),
            service,
            session,
            mapping_vmo,
            port,
            delivery_queue,
        )
    }

    fn get_info(&self) -> Cow<'_, super::DeviceInfo> {
        self.interface.get_info()
    }

    fn active_requests(&self) -> &ActiveRequests<Arc<Session<I>>> {
        &self.active_requests
    }
}

impl<I: Interface + ?Sized> SessionManager<I> {
    /// Returns the memoized [`BlockService`] for this session manager.
    pub fn into_block_service(&self, orchestrator: &Arc<I::Orchestrator>) -> Arc<dyn BlockService> {
        self.block_service
            .get_or_init(|| self.interface.clone().into_block_service(orchestrator))
            .clone()
    }
}

impl<I: Interface + ?Sized> SessionManager<I> {
    pub fn new(interface: Arc<I>, block_size: u32) -> Self
    where
        I: Sized,
    {
        Self {
            interface,
            block_size,
            active_requests: ActiveRequests::default(),
            inflight_requests: Mutex::new(InflightRequests::default()),
            no_inflight_requests_condvar: Condvar::new(),
            inner: Mutex::new(SessionManagerInner { open_sessions: HashMap::new() }),
            no_open_sessions_condvar: Condvar::new(),
            block_service: OnceLock::new(),
        }
    }

    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Reports the given task as complete with a given status.
    pub fn complete_request(&self, request_id: RequestId, status: Result<(), zx::Status>) {
        let (callback, notify) = {
            let mut inflight = self.inflight_requests.lock();
            let callback = if request_id.0 & INTERNAL_REQUEST_FLAG != 0 {
                inflight.callbacks.remove(&request_id)
            } else {
                None
            };
            inflight.count -= 1;
            let notify = inflight.count == 0;
            (callback, notify)
        };
        if let Some(callback) = callback {
            callback(status);
        } else {
            self.complete_unsubmitted_request(request_id, status);
        }
        if notify {
            self.no_inflight_requests_condvar.notify_all();
        }
    }

    fn submit_internal_request(
        &self,
        make_request: impl FnOnce(RequestId) -> Request,
        callback: Box<dyn FnOnce(Result<(), zx::Status>) + Send>,
    ) {
        let req = {
            let mut inflight = self.inflight_requests.lock();
            let req_id = RequestId(inflight.next_internal_id | INTERNAL_REQUEST_FLAG);
            inflight.next_internal_id += 1;
            inflight.callbacks.insert(req_id, callback);
            inflight.count += 1;
            make_request(req_id)
        };
        self.interface.on_requests(&[req]);
    }

    fn submit_requests(&self, requests: &[Request]) {
        self.inflight_requests.lock().count += requests.len();
        self.interface.on_requests(requests);
    }

    /// Waits for there to be no requests in-flight.
    ///
    /// NOTE: To void TOCTOUs, this must be called on the same thread which calls
    /// [`Self::submit_requests`].
    fn wait_for_no_inflight_requests(&self) {
        let mut guard = self.inflight_requests.lock();
        self.no_inflight_requests_condvar.wait_while(&mut guard, |inflight| inflight.count > 0);
    }

    /// Called instead of `[Self::complete_request]` when a request is completed before it was
    /// actually submitted.
    fn complete_unsubmitted_request(&self, request_id: RequestId, status: Result<(), zx::Status>) {
        if let Some((session, response)) =
            self.active_requests.complete_and_take_response(request_id, status)
        {
            session.send_response(response);
        }
    }

    /// Terminates the session manager.  Blocks until all sessions have terminated.
    pub fn terminate(&self) {
        {
            // We must drop references to sessions whilst we're not holding the lock for
            // `open_sessions` because `Session::drop` needs to take that same lock.
            #[allow(clippy::collection_is_never_read)]
            let mut terminated_sessions = Vec::new();
            for (_, session) in &self.inner.lock().open_sessions {
                if let Some(session) = session.upgrade() {
                    session.terminate_async();
                    terminated_sessions.push(session);
                }
            }
        }
        let mut guard = self.inner.lock();
        self.no_open_sessions_condvar.wait_while(&mut guard, |s| !s.open_sessions.is_empty());
    }
}

pub struct Session<I: Interface + ?Sized> {
    helper: SessionHelper<SessionManager<I>>,
    fifo: zx::Fifo<BlockFifoRequest, BlockFifoResponse>,
    queue: Mutex<SessionQueue>,
    abort_handle: AbortHandle,
    close_callback: Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>,
}

#[derive(Default)]
struct SessionQueue {
    responses: VecDeque<BlockFifoResponse>,
}

pub const MAX_REQUESTS: usize = super::FIFO_MAX_REQUESTS;

struct DecodedRequests {
    requests: [MaybeUninit<Request>; MAX_REQUESTS],
    count: usize,
}

impl Default for DecodedRequests {
    fn default() -> Self {
        Self { requests: unsafe { MaybeUninit::uninit().assume_init() }, count: 0 }
    }
}

impl DecodedRequests {
    fn push(&mut self, request: Request) {
        assert!(self.count < MAX_REQUESTS);
        // To ensure drop runs, we must initialize each element at most once.
        // As long as we only go via this function, that's satisfied.
        self.requests[self.count].write(request);
        self.count += 1;
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn is_full(&self) -> bool {
        self.count == MAX_REQUESTS
    }

    fn clear(&mut self) {
        for i in 0..self.count {
            // SAFETY: We initialized `count` elements via [`Self::push`].
            unsafe { self.requests[i].assume_init_drop() };
        }
        self.count = 0;
    }
}

impl Drop for DecodedRequests {
    fn drop(&mut self) {
        self.clear();
    }
}

impl std::ops::Deref for DecodedRequests {
    type Target = [Request];

    fn deref(&self) -> &Self::Target {
        // SAFETY: We wrote the request in [`Self::push`].
        unsafe { std::slice::from_raw_parts(self.requests[0].as_ptr(), self.count) }
    }
}

impl std::ops::DerefMut for DecodedRequests {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: We wrote the request in [`Self::push`].
        unsafe { std::slice::from_raw_parts_mut(self.requests[0].as_mut_ptr(), self.count) }
    }
}

impl<I: Interface + ?Sized> Session<I> {
    /// Begins processing the session's FIFO.  Blocks until completion (either because of
    /// termination, or due to unrecoverable error).
    pub fn run(self: &Arc<Self>) {
        self.fifo_loop();
        self.abort_handle.abort();
        // NB: We cannot call [`drop_active_requests`] here, because requests which have already
        // been submitted are no longer in control by this thread, and will be later completed by
        // another thread.  If we dropped them here, then they would be completed twice.
        self.helper.close_active_groups(|s| Arc::ptr_eq(s, self));
    }

    fn fifo_loop(self: &Arc<Self>) {
        let mut requests = [MaybeUninit::uninit(); MAX_REQUESTS];

        loop {
            // Send queued responses.
            let is_queue_empty = {
                let mut queue = self.queue.lock();
                while !queue.responses.is_empty() {
                    let (front, _) = queue.responses.as_slices();
                    match self.fifo.write(front) {
                        Ok(count) => {
                            let full = count.get() < front.len();
                            queue.responses.drain(..count.get());
                            if full {
                                break;
                            }
                        }
                        Err(zx::Status::SHOULD_WAIT) => break,
                        Err(_) => return,
                    }
                }
                queue.responses.is_empty()
            };

            // Process pending reads.
            match self.fifo.read_uninit(&mut requests) {
                Ok(valid_requests) => self.handle_requests(valid_requests.iter_mut()),
                Err(zx::Status::SHOULD_WAIT) => {
                    let mut signals =
                        zx::Signals::OBJECT_READABLE | SHUTDOWN_SIGNAL | FIFO_WAKE_SIGNAL;
                    if !is_queue_empty {
                        signals |= zx::Signals::OBJECT_WRITABLE;
                    }
                    let Ok(signals) =
                        self.fifo.wait_one(signals, zx::MonotonicInstant::INFINITE).to_result()
                    else {
                        return;
                    };
                    if signals.contains(SHUTDOWN_SIGNAL) {
                        return;
                    }
                    // Clear FIFO_WAKE_SIGNAL if it's set.
                    if signals.contains(FIFO_WAKE_SIGNAL) {
                        let _ = self.fifo.signal(FIFO_WAKE_SIGNAL, zx::Signals::empty());
                    }
                }
                Err(_) => return,
            }
        }
    }

    /// Synchronously performs a device flush.
    fn pre_flush(self: &Arc<Self>, request_id: RequestId) -> Result<(), zx::Status> {
        let trace_flow_id = {
            let mut request = self.helper.session_manager().active_requests.request(request_id);
            if let Some(id) = request.trace_flow_id {
                fuchsia_trace::async_instant!(
                    fuchsia_trace::Id::from(id.get()),
                    c"storage",
                    c"block_server::SimulatedBarrier",
                    "request_id" => request_id.0
                );
            }
            request.count += 1;
            request.trace_flow_id
        };
        self.helper.session_manager().submit_requests(&[Request {
            request_id,
            operation: Operation::Flush,
            trace_flow_id,
            vmo: None,
        }]);
        self.helper.session_manager().wait_for_no_inflight_requests();
        let status = self.helper.session_manager().active_requests.request(request_id).status;
        match status {
            Ok(()) => Ok(()),
            Err(status) => {
                // Respond for the unsubmitted request too.
                self.helper.session_manager().complete_unsubmitted_request(request_id, Err(status));
                Err(status)
            }
        }
    }

    /// Synchronously completes `decoded_requests`, and inserts a post-flush into `decoded_requests`
    /// to be submitted later.
    fn post_flush(self: &Arc<Self>, request_id: RequestId, decoded_requests: &mut DecodedRequests) {
        if !decoded_requests.is_empty() {
            self.helper.session_manager().submit_requests(decoded_requests);
            decoded_requests.clear();
        }
        self.helper.session_manager().wait_for_no_inflight_requests();
        let request = self.helper.session_manager().active_requests.request(request_id);
        match request.status {
            Ok(()) => decoded_requests.push(Request {
                request_id,
                operation: Operation::Flush,
                trace_flow_id: request.trace_flow_id,
                vmo: None,
            }),
            Err(status) => {
                drop(request);
                self.helper.session_manager().complete_unsubmitted_request(request_id, Err(status))
            }
        }
    }

    fn handle_requests<'a>(
        self: &Arc<Self>,
        requests: impl Iterator<Item = &'a mut BlockFifoRequest>,
    ) {
        let manager = &self.helper.session_manager();
        let mut decoded_requests = DecodedRequests::default();

        for request in requests {
            match self.helper.decode_fifo_request(self.clone(), request) {
                Ok(DecodedRequest { operation: Operation::CloseVmo, request_id, .. }) => {
                    manager.complete_unsubmitted_request(request_id, Ok(()));
                }
                Ok(mut request) => {
                    let request_id = request.request_id;

                    // Strip the PRE_BARRIER flag if we don't support it, and simulate the barrier
                    // with a pre-flush.
                    if !manager
                        .interface
                        .get_info()
                        .device_flags()
                        .contains(fblock::DeviceFlag::BARRIER_SUPPORT)
                        && request.operation.take_write_flag(WriteFlags::PRE_BARRIER)
                        && self.pre_flush(request_id).is_err()
                    {
                        continue;
                    }
                    // Strip the FORCE_ACCESS flag if we don't support it, and simulate the FUA with
                    // a post-flush.
                    let simulate_fua = !manager
                        .interface
                        .get_info()
                        .device_flags()
                        .contains(fblock::DeviceFlag::FUA_SUPPORT)
                        && request.operation.take_write_flag(WriteFlags::FORCE_ACCESS);

                    if simulate_fua {
                        // Account for the additional request we need at the end.
                        manager.active_requests.request(request_id).count += 1;
                    }

                    loop {
                        let result = self
                            .helper
                            .map_request(request, &mut manager.active_requests.request(request_id));
                        match result {
                            Ok((
                                DecodedRequest { request_id, operation, vmo, trace_flow_id },
                                remainder,
                            )) => {
                                decoded_requests.push(Request {
                                    request_id,
                                    operation,
                                    trace_flow_id,
                                    vmo,
                                });

                                if decoded_requests.is_full() {
                                    manager.submit_requests(&*decoded_requests);
                                    decoded_requests.clear();
                                }

                                if let Some(r) = remainder {
                                    request = r;
                                } else {
                                    break;
                                }
                            }
                            Err(status) => {
                                manager.complete_unsubmitted_request(request_id, Err(status));
                                break;
                            }
                        }
                    }

                    if simulate_fua {
                        self.post_flush(request_id, &mut decoded_requests);
                    }
                }
                Err(None) => {}
                Err(Some(response)) => self.send_response(response),
            }
        }

        if !decoded_requests.is_empty() {
            manager.submit_requests(&decoded_requests);
        }
    }

    fn send_response(&self, response: BlockFifoResponse) {
        let mut queue = self.queue.lock();
        if queue.responses.is_empty() {
            match self.fifo.write_one(&response) {
                Ok(()) => {
                    return;
                }
                Err(_) => {
                    // Wake the FIFO loop up so that we can send the response later.
                    let _ = self.fifo.signal(zx::Signals::empty(), FIFO_WAKE_SIGNAL);
                }
            }
        }
        queue.responses.push_back(response);
    }

    /// Asynchronously request to terminate the Session.  The session's thread will eventually stop
    /// running.
    pub fn terminate_async(&self) {
        let _ = self.fifo.signal(zx::Signals::empty(), SHUTDOWN_SIGNAL);
        self.abort_handle.abort();
    }
}

impl<I: Interface + ?Sized> Drop for Session<I> {
    fn drop(&mut self) {
        let callback = std::mem::take(&mut *self.close_callback.lock());
        if let Some(callback) = callback {
            callback();
        }
        let notify = {
            let mut inner = self.helper.session_manager().inner.lock();
            inner.open_sessions.remove(&(self as *const _ as usize));
            inner.open_sessions.is_empty()
        };
        if notify {
            self.helper.session_manager().no_open_sessions_condvar.notify_all();
        }
    }
}

impl<I: Interface + ?Sized> Drop for SessionManager<I> {
    fn drop(&mut self) {
        self.terminate();
    }
}

impl<I: Interface<Orchestrator = SessionManager<I>>> IntoOrchestrator for Arc<SessionManager<I>> {
    type SM = SessionManager<I>;

    fn into_orchestrator(self) -> Arc<I::Orchestrator> {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockInterface;
    use block_protocol::{BlockFifoCommand, BlockFifoRequest, BlockFifoResponse};
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_storage_block as fblock;
    use fuchsia_async as fasync;

    const BLOCK_SIZE: u32 = 512;

    #[fuchsia::test]
    async fn test_basic_request() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let _server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        })
        .detach();

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let vmo = zx::Vmo::create(8192).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::Read.into_primitive(),
                ..Default::default()
            },
            reqid: 123,
            group: 0,
            vmoid: vmo_id.id,
            length: 1,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };
        fifo.write(&[req]).unwrap();

        let r = rx.recv().unwrap();
        assert_eq!(r.request_id.0, 0);
        assert!(matches!(r.operation, Operation::Read { .. }));

        session_manager.complete_request(r.request_id, Ok(()));

        let signals =
            fifo.wait_one(zx::Signals::FIFO_READABLE, zx::MonotonicInstant::INFINITE).unwrap();
        assert!(signals.contains(zx::Signals::FIFO_READABLE));

        let mut resp = [BlockFifoResponse::default()];
        fifo.read(&mut resp).unwrap();
        assert_eq!(resp[0].reqid, 123);
        assert_eq!(resp[0].status, zx::sys::ZX_OK);

        std::mem::drop(proxy);
    }
    #[fuchsia::test]
    async fn test_write_request() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let _server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        })
        .detach();

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let vmo = zx::Vmo::create(8192).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::Write.into_primitive(),
                ..Default::default()
            },
            reqid: 124,
            group: 0,
            vmoid: vmo_id.id,
            length: 1,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };
        fifo.write(&[req]).unwrap();

        let r = rx.recv().unwrap();
        assert_eq!(r.request_id.0, 0);
        assert!(matches!(r.operation, Operation::Write { .. }));

        session_manager.complete_request(r.request_id, Ok(()));

        let signals =
            fifo.wait_one(zx::Signals::FIFO_READABLE, zx::MonotonicInstant::INFINITE).unwrap();
        assert!(signals.contains(zx::Signals::FIFO_READABLE));

        let mut resp = [BlockFifoResponse::default()];
        fifo.read(&mut resp).unwrap();
        assert_eq!(resp[0].reqid, 124);
        assert_eq!(resp[0].status, zx::sys::ZX_OK);
    }

    #[fuchsia::test]
    async fn test_flush_request() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let _server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        })
        .detach();

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::Flush.into_primitive(),
                ..Default::default()
            },
            reqid: 125,
            group: 0,
            vmoid: fblock::VMOID_INVALID,
            length: 0,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };
        fifo.write(&[req]).unwrap();

        let r = rx.recv().unwrap();
        assert_eq!(r.request_id.0, 0);
        assert!(matches!(r.operation, Operation::Flush { .. }));

        session_manager.complete_request(r.request_id, Ok(()));

        let signals =
            fifo.wait_one(zx::Signals::FIFO_READABLE, zx::MonotonicInstant::INFINITE).unwrap();
        assert!(signals.contains(zx::Signals::FIFO_READABLE));

        let mut resp = [BlockFifoResponse::default()];
        fifo.read(&mut resp).unwrap();
        assert_eq!(resp[0].reqid, 125);
        assert_eq!(resp[0].status, zx::sys::ZX_OK);
    }

    #[fuchsia::test]
    async fn test_trim_request() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let _server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        })
        .detach();

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::Trim.into_primitive(),
                ..Default::default()
            },
            reqid: 126,
            group: 0,
            vmoid: fblock::VMOID_INVALID,
            length: 1,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };
        fifo.write(&[req]).unwrap();

        let r = rx.recv().unwrap();
        assert_eq!(r.request_id.0, 0);
        assert!(matches!(r.operation, Operation::Trim { .. }));

        session_manager.complete_request(r.request_id, Ok(()));

        let signals =
            fifo.wait_one(zx::Signals::FIFO_READABLE, zx::MonotonicInstant::INFINITE).unwrap();
        assert!(signals.contains(zx::Signals::FIFO_READABLE));

        let mut resp = [BlockFifoResponse::default()];
        fifo.read(&mut resp).unwrap();
        assert_eq!(resp[0].reqid, 126);
        assert_eq!(resp[0].status, zx::sys::ZX_OK);
    }

    #[fuchsia::test]
    async fn test_close_vmo() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let _server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        })
        .detach();

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let vmo = zx::Vmo::create(8192).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::CloseVmo.into_primitive(),
                ..Default::default()
            },
            reqid: 127,
            group: 0,
            vmoid: vmo_id.id,
            length: 0,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };
        fifo.write(&[req]).unwrap();

        let signals =
            fifo.wait_one(zx::Signals::FIFO_READABLE, zx::MonotonicInstant::INFINITE).unwrap();
        assert!(signals.contains(zx::Signals::FIFO_READABLE));

        let mut resp = [BlockFifoResponse::default()];
        fifo.read(&mut resp).unwrap();
        assert_eq!(resp[0].reqid, 127);
        assert_eq!(resp[0].status, zx::sys::ZX_OK);

        // CloseVmo is handled automatically. Interface should not receive anything.
        assert!(rx.try_recv().is_err());
    }

    #[fuchsia::test]
    async fn test_error() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let _server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        })
        .detach();

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::Flush.into_primitive(),
                ..Default::default()
            },
            reqid: 128,
            group: 0,
            vmoid: fblock::VMOID_INVALID,
            length: 0,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };
        fifo.write(&[req]).unwrap();

        let r = rx.recv().unwrap();
        session_manager.complete_request(r.request_id, Err(zx::Status::IO));

        let signals =
            fifo.wait_one(zx::Signals::FIFO_READABLE, zx::MonotonicInstant::INFINITE).unwrap();
        assert!(signals.contains(zx::Signals::FIFO_READABLE));

        let mut resp = [BlockFifoResponse::default()];
        fifo.read(&mut resp).unwrap();
        assert_eq!(resp[0].reqid, 128);
        assert_eq!(resp[0].status, zx::sys::ZX_ERR_IO);
    }

    #[fuchsia::test]
    async fn test_teardown_with_active_requests() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        });

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let vmo = zx::Vmo::create(8192).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::Read.into_primitive(),
                ..Default::default()
            },
            reqid: 129,
            group: 0,
            vmoid: vmo_id.id,
            length: 1,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };
        // Start a request and wait until it's been submitted.
        fifo.write(&[req]).unwrap();
        let r = rx.recv().unwrap();

        // Close the client, which will eventually cause the FIFO loop to exit.
        drop(session_proxy);
        fasync::Timer::new(std::time::Duration::from_millis(50)).await;

        // Complete the request, simulating a completion after the FIFO loop has exited.
        session_manager.complete_request(r.request_id, Ok(()));

        drop(proxy);
        fasync::unblock(move || session_manager.terminate()).await;
        server_task.await;
    }

    #[fuchsia::test]
    async fn test_teardown_with_active_grouped_requests() {
        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        });

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let vmo = zx::Vmo::create(8192).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let mut req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::Read.into_primitive(),
                flags: fblock::BlockIoFlag::GROUP_ITEM.bits(),
                ..Default::default()
            },
            reqid: 1,
            group: 1,
            vmoid: vmo_id.id,
            length: 1,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };

        // Start two groups of requests and wait until they're been submitted.
        fifo.write(&[req]).unwrap();
        req.reqid = 2;
        req.group = 2;
        fifo.write(&[req]).unwrap();
        let r1 = rx.recv().unwrap();
        let r2 = rx.recv().unwrap();
        req.reqid = 3;
        req.group = 2;
        fifo.write(&[req]).unwrap();
        let r3 = rx.recv().unwrap();

        // Complete requests 1,2.  This leaves two groups in the following states:
        // - Group 1: 0 active requests, still waiting for END
        // - Group 2: 1 active request, still waiting for END
        // Neither group will be able to complete yet.
        session_manager.complete_request(r1.request_id, Ok(()));
        session_manager.complete_request(r2.request_id, Ok(()));

        // Close the client, which will eventually cause the FIFO loop to exit.
        // Group 1 should complete now.  Group 2 can't yet.
        drop(session_proxy);
        fasync::Timer::new(std::time::Duration::from_millis(50)).await;

        // At some later time, complete request 3, which should complete group 2.
        session_manager.complete_request(r3.request_id, Ok(()));

        drop(proxy);

        fasync::unblock(move || session_manager.terminate()).await;
        server_task.await;
    }

    #[fuchsia::test]
    async fn test_session_close_is_synchronous() {
        use futures::FutureExt as _;

        let (tx, rx) = std::sync::mpsc::channel();
        let interface = Arc::new(MockInterface::new(tx));
        let session_manager = Arc::new(SessionManager::new(interface.clone(), BLOCK_SIZE));

        let sm_clone = session_manager.clone();
        let (proxy, stream) = create_proxy_and_stream::<fblock::BlockMarker>();
        let server_task = fasync::Task::spawn(async move {
            let server = crate::BlockServer::new(BLOCK_SIZE, sm_clone);
            server.handle_requests(stream).await.unwrap();
        });

        let (session_proxy, session_server_end) =
            fidl::endpoints::create_proxy::<fblock::SessionMarker>();
        proxy.open_session(session_server_end).unwrap();

        let fifo_handle = session_proxy.get_fifo().await.unwrap().unwrap();
        let fifo: zx::Fifo<BlockFifoResponse, BlockFifoRequest> = zx::Fifo::from(fifo_handle);

        let vmo = zx::Vmo::create(8192).unwrap();
        let vmo_id = session_proxy
            .attach_vmo(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
            .await
            .unwrap()
            .unwrap();

        let req = BlockFifoRequest {
            command: BlockFifoCommand {
                opcode: fblock::BlockOpcode::Read.into_primitive(),
                ..Default::default()
            },
            reqid: 123,
            group: 0,
            vmoid: vmo_id.id,
            length: 1,
            vmo_offset: 0,
            dev_offset: 0,
            trace_flow_id: 0,
            ..Default::default()
        };
        fifo.write(&[req]).unwrap();

        let r = rx.recv().unwrap();

        // The close request shouldn't complete yet because the read is still hanging.
        let mut close_fut = std::pin::pin!(session_proxy.close().fuse());
        let mut timer_fut =
            std::pin::pin!(fasync::Timer::new(std::time::Duration::from_millis(100)).fuse());
        futures::select! {
            res = close_fut => panic!("close completed too early: {res:?}"),
            _ = timer_fut => {}
        }

        session_manager.complete_request(r.request_id, Ok(()));

        // Verify that close() now completes.
        close_fut.await.unwrap().unwrap();

        std::mem::drop(proxy);
        server_task.await;
    }
}
