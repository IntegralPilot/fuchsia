// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::Deref;
use core::ptr::NonNull;
use core::task::Poll;

use sapphire_collections::deque::Deque;

use sapphire_collections::storage::StorageFamily;
use sapphire_sync::mutex::Mutex;
use sapphire_sync::mutex::raw::RawMutex;
use thiserror::Error;

use crate::global_index::GlobalIndex;
use crate::notification::Notification;

pub trait RpcCfg {
    /// Raw mutex fundamental used to synchronize internal channel state.
    type Mtx: RawMutex;
    /// Storage container family used for the enqueued request queue.
    type Chan: StorageFamily;
}

/// Configuration trait defining the types and synchronization fundamentals for an [`RpcChannel`].
pub trait Rpc {
    /// Request type enqueued by the Client.
    type Request;
    /// Response returned by the Server.
    type Response;
}

/// Raw waker and response slot pointers representing an enqueued RPC request.
///
/// Used by the Client to safely yield exclusive waker and slot ownership to the Server
/// without heap allocations.
pub struct RpcRequestInfo<R: Rpc, Cfg: RpcCfg> {
    request: R::Request,
    response_slot: NonNull<MaybeUninit<R::Response>>,
    waker: NonNull<Notification<Cfg::Mtx>>,
}

// The thread safety semantics of RpcRequestInfo are as follows:
//
// 1. request is an owned type so essentially the same as an auto trait Send impl.
// 2. response slot is essentially a &mut Response.
// 3. Waker is essentially a `&Notification`.

// SAFETY: Stems from the semantics outlined above + the ownership semantics
// enforced by the async cancellation mechanism
unsafe impl<R: Rpc, Cfg: RpcCfg> Send for RpcRequestInfo<R, Cfg>
where
    R::Request: Send,
    R::Response: Send,
    Notification<Cfg::Mtx>: Sync,
{
}

// NOTE: There's no point in even implementing Sync for RpcRequestInfo since this is always
// passed by ownership.

/// State machine for an RPC request from request to response or cancellation
pub enum RpcRequestState<R: Rpc, Cfg: RpcCfg> {
    /// Request has been added to the queue
    Requested(RpcRequestInfo<R, Cfg>),
    /// Request has been accepted by a worker but it's not yet completed
    Accepted,
    /// Request has been fulfilled.
    Completed,
    /// Client cancelled this request.
    Cancelled,
}

impl<R: Rpc, Cfg: RpcCfg> RpcRequestState<R, Cfg> {
    /// Transitions the state from `Requested` to `Accepted`, extracting and returning
    /// the inner `RpcRequestInfo` payload.
    ///
    /// If the state is not `Requested`, leaves it unchanged and returns `None`.
    fn accept_request(&mut self) -> Option<RpcRequestInfo<R, Cfg>> {
        let old_state = core::mem::replace(self, RpcRequestState::Accepted);
        if let RpcRequestState::Requested(info) = old_state {
            Some(info)
        } else {
            *self = old_state;
            None
        }
    }
}

/// An asynchronous Request-Response (RPC) Channel.
///
/// `RpcChannel` enables asynchronous bidirectional Request-Response interactions between a single
/// `Client` and a `Server`. It achieves **zero heap allocations** during requests by passing a
/// response slot with pointers to the client stack's local waker and response buffer inside the
/// enqueued request.
///
/// # Examples
///
/// Basic concurrent RPC call:
///
/// ```
/// use sapphire_async::rpc::{RpcChannel, Rpc, RpcCfg};
/// use sapphire_async::testing::TestExecutor;
/// use sapphire_async::executor::BoundedExecutor;
/// use sapphire_collections::storage::ArrayStorage;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
///
/// struct TestRpc;
/// impl Rpc for TestRpc {
///     type Request = i32;
///     type Response = i32;
/// }
///
/// struct TestRpcCfg;
/// impl RpcCfg for TestRpcCfg {
///     type Mtx = SingleThreadMutex;
///     type Chan = ArrayStorage<3>;
/// }
///
/// let mut channel = RpcChannel::<TestRpc, TestRpcCfg>::new();
/// let (client, server) = channel.split();
///
/// # BoundedExecutor::new(TestExecutor::new(), |s| {
/// #     s.spawn(async move {
/// let (req, responder) = server.recv().await.unwrap();
/// assert_eq!(req, 42);
/// responder.respond(100);
/// #     });
/// #
/// #     s.block_on(async move {
/// let res = client.call(42).await.unwrap();
/// assert_eq!(res, 100);
/// #     });
/// # });
/// ```
///
/// An `RpcChannel` configured with `SingleThreadMutex` cannot be shared across threads (compile_fail):
///
/// ```compile_fail
/// use sapphire_async::rpc::{RpcChannel, Rpc, RpcCfg};
/// use sapphire_collections::storage::ArrayStorage;
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
///
/// struct SimpleRpc;
/// impl Rpc for SimpleRpc {
///     type Request = i32;
///     type Response = i32;
/// }
///
/// struct NonSyncCfg;
/// impl RpcCfg for NonSyncCfg {
///     type Mtx = SingleThreadMutex;
///     type Chan = ArrayStorage<3>;
/// }
///
/// type NonSyncRpcChannel = RpcChannel<SimpleRpc, NonSyncCfg>;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<NonSyncRpcChannel>(); // Correctly fails to compile because SingleThreadMutex is !Sync!
/// ```

pub struct RpcChannel<R: Rpc, Cfg: RpcCfg> {
    state: Mutex<Cfg::Mtx, RpcChannelInner<R, Cfg>>,
    not_full: Notification<Cfg::Mtx>,
    not_empty: Notification<Cfg::Mtx>,
}

/// The internal, synchronized state of an [`RpcChannel`].
///
/// Holds the underlying inbox queue and tracks the global logical offset index
/// to ensure disjoint client waker and slot allocations remain stable across drops.
struct RpcChannelInner<R: Rpc, Cfg: RpcCfg> {
    inbox: Deque<RpcRequestState<R, Cfg>, Cfg::Chan>,
    head: GlobalIndex,
    server_count: usize,
    client_count: usize,
}

/// The Client endpoint of an [`RpcChannel`].
///
/// Used to asynchronously submit requests and wait for responses via [`Client::call`].
#[derive(Debug)]
pub struct Client<C: RpcHandles> {
    channel: C,
}

/// The Server endpoint of an [`RpcChannel`].
///
/// Used to asynchronously receive enqueued requests via [`Server::recv`].
#[derive(Debug)]
pub struct Server<C: RpcHandles> {
    channel: C,
}

impl<C: RpcHandles> Drop for Client<C> {
    fn drop(&mut self) {
        self.channel.drop_client();
    }
}

impl<C: RpcHandles + Clone> Clone for Client<C> {
    fn clone(&self) -> Self {
        self.channel.clone_client();
        Self { channel: self.channel.clone() }
    }
}

impl<C: RpcHandles> Drop for Server<C> {
    fn drop(&mut self) {
        self.channel.drop_server();
    }
}

impl<C: RpcHandles + Clone> Clone for Server<C> {
    fn clone(&self) -> Self {
        self.channel.clone_server();
        Self { channel: self.channel.clone() }
    }
}

pub trait RpcHandles {
    fn clone_client(&self);
    fn clone_server(&self);
    fn drop_client(&self);
    fn drop_server(&self);
}

impl<R: Rpc, Cfg: RpcCfg, Chan: Deref<Target = RpcChannel<R, Cfg>>> RpcHandles for Chan {
    fn clone_client(&self) {
        self.state.lock().client_count += 1;
    }

    fn clone_server(&self) {
        self.state.lock().server_count += 1;
    }

    fn drop_client(&self) {
        let mut guard = self.state.lock();
        guard.client_count -= 1;
        if guard.client_count == 0 {
            self.not_empty.notify_all();
        }
    }

    fn drop_server(&self) {
        let mut guard = self.state.lock();
        guard.server_count -= 1;
        if guard.server_count == 0 {
            self.not_full.notify_all();
            for req in guard.inbox.iter() {
                if let RpcRequestState::Requested(info) = req {
                    unsafe { info.waker.as_ref().notify_one() };
                }
            }
        }
    }
}

impl<R: Rpc, Cfg: RpcCfg> Default for RpcChannel<R, Cfg>
where
    Deque<RpcRequestState<R, Cfg>, Cfg::Chan>: Default,
{
    fn default() -> Self {
        Self {
            state: Mutex::new(RpcChannelInner {
                inbox: Deque::default(),
                head: GlobalIndex::new(0),
                server_count: 0,
                client_count: 0,
            }),
            not_full: Notification::new(),
            not_empty: Notification::new(),
        }
    }
}

impl<R: Rpc, Cfg: RpcCfg> RpcChannel<R, Cfg> {
    /// Creates a new, empty `RpcChannel` with the configured wakers and mutex fundamentals.
    pub fn new() -> Self
    where
        Self: Default,
    {
        Self::default()
    }

    /// Returns the number of used request slots in the channel inbox.
    pub fn used_slots(&self) -> usize {
        self.state.lock().inbox.len()
    }

    /// Splits the `RpcChannel` into a [`Client`] and a [`Server`] pair.
    pub fn split(&mut self) -> (Client<&'_ Self>, Server<&'_ Self>) {
        self.state.get_mut().server_count = 1;
        self.state.get_mut().client_count = 1;
        (Client { channel: &*self }, Server { channel: &*self })
    }

    #[cfg(feature = "std")]
    /// Splits the `RpcChannel` into a [`Client`] and a [`Server`] pair wrapped in an Arc.
    pub fn split_to_arc(mut self) -> (Client<std::sync::Arc<Self>>, Server<std::sync::Arc<Self>>) {
        self.state.get_mut().server_count = 1;
        self.state.get_mut().client_count = 1;
        let this = std::sync::Arc::new(self);
        (Client { channel: this.clone() }, Server { channel: this })
    }

    #[cfg(feature = "std")]
    /// Splits the `RpcChannel` into a [`Client`] and a [`Server`] pair wrapped in an Arc.
    pub fn split_to_rc(mut self) -> (Client<std::rc::Rc<Self>>, Server<std::rc::Rc<Self>>) {
        self.state.get_mut().server_count = 1;
        self.state.get_mut().client_count = 1;
        let this = std::rc::Rc::new(self);
        (Client { channel: this.clone() }, Server { channel: this })
    }

    /// Submits a request to the inbox queue asynchronously, returning a unique [`GlobalIndex`].
    ///
    /// Blocks the calling client task if the queue buffer is currently full, re-evaluating
    /// predicate availability when enqueued requests are completed or cancelled.
    async fn send(&self, request: RpcRequestInfo<R, Cfg>) -> Result<GlobalIndex, CallError> {
        let mut request = Some(RpcRequestState::Requested(request));
        let guard = self.state.lock();

        let idx = self
            .not_full
            .when(guard, |chan| {
                if chan.server_count == 0 {
                    return Poll::Ready(Err(CallError::Closed));
                }
                match chan.inbox.push_back(request.take().expect("Missing request from option")) {
                    Ok(()) => {
                        // NOTE: Can't underflow since we just succeeded to push_back
                        let idx = chan.head + (chan.inbox.len() - 1);
                        // Notify one server that a request is enqueued
                        self.not_empty.notify_one();
                        Poll::Ready(Ok(idx))
                    }
                    Err(r) => {
                        request.replace(r);
                        Poll::Pending
                    }
                }
            })
            .await;

        idx
    }
}

impl<R: Rpc, Cfg: RpcCfg> RpcChannelInner<R, Cfg> {
    /// Safely returns a shared reference to the request state associated with the `idx`.
    ///
    /// Translates the monotonic global `idx` to local deque index. Returns `None` if the request
    /// was already popped/reclaimed from the queue.
    fn get(&self, idx: GlobalIndex) -> Option<&RpcRequestState<R, Cfg>> {
        let offset = idx - self.head;
        let offset = offset.try_into().ok()?;
        Some(self.inbox.get(offset).expect("Invalid logical index to a future element"))
    }

    /// Safely returns a mutable reference to the request state associated with the `idx`.
    ///
    /// Translates the monotonic global `idx` to local deque index. Returns `None` if the request
    /// was already popped/reclaimed from the queue.
    fn get_mut(&mut self, idx: GlobalIndex) -> Option<&mut RpcRequestState<R, Cfg>> {
        let offset = idx - self.head;

        let offset = offset.try_into().ok()?;
        Some(self.inbox.get_mut(offset).expect("Invalid logical index to a future element"))
    }

    /// Reclaims contiguous cancelled and completed requests from the front of the deque.
    fn clean_up_channel(&mut self, waker: &Notification<Cfg::Mtx>) {
        let mut cleaned_up = 0;
        while let Some(_) = self.inbox.pop_front_if(|r| matches!(r, RpcRequestState::Completed)) {
            self.head += 1;
            cleaned_up += 1;
        }
        if cleaned_up > 0 {
            waker.notify_many(cleaned_up);
        }
    }

    /// Scans the inbox queue, returning the index and payload of the first unprocessed request.
    ///
    /// Automatically transitions the matched slot from `Requested` to `Accepted`.
    fn next_request(&mut self) -> Result<Option<(GlobalIndex, RpcRequestInfo<R, Cfg>)>, RecvError> {
        if self.client_count == 0 {
            // NOTE: We don't even handle requests if the clients are all gone since all requests would have been cancelled anyway.
            return Err(RecvError::Closed);
        }
        for (i, req) in self.inbox.iter_mut().enumerate() {
            if let Some(request) = req.accept_request() {
                let idx = self.head + i;
                return Ok(Some((idx, request)));
            }
        }
        Ok(None)
    }
}

/// Error sending a payload to the channel.
#[derive(Debug, Clone, Error, Eq, PartialEq)]
pub enum CallError {
    #[error("All server handles have been closed")]
    Closed,
    #[error("The server dropped the request")]
    ServerCancel,
}

impl<R: Rpc, Cfg: RpcCfg, C: Deref<Target = RpcChannel<R, Cfg>>> Client<C> {
    /// Returns the number of used request slots in the channel inbox.
    pub fn used_slots(&self) -> usize {
        self.channel.used_slots()
    }

    /// Submits an RPC request to the server asynchronously and blocks until the response is returned.
    ///
    /// # Cancel Safety
    ///
    /// Cancelling this Future may notify the server that the request may not need to be evaluated
    /// and will be discarded. However, if the request has been accepted, the server may continue
    /// its handler for the request but will avoid writing the response back to the client
    pub async fn call(&self, request: R::Request) -> Result<R::Response, CallError> {
        let mut response_slot = MaybeUninit::uninit();
        let response_slot = NonNull::from(&mut response_slot);
        let waker = Notification::new();
        let request = RpcRequestInfo { request, response_slot, waker: NonNull::from(&waker) };

        let idx = self.channel.send(request).await?;
        let cleanup = OnDrop::new(|| {
            let mut guard = self.channel.state.lock();
            match guard.get_mut(idx) {
                Some(req @ RpcRequestState::Accepted) => {
                    // Server didn't complete the request, so the response slot remains uninitialized.
                    // Server has acknowledged the request so they must be the ones to complete and reclaim
                    *req = RpcRequestState::Cancelled;
                }
                Some(req @ RpcRequestState::Cancelled | req @ RpcRequestState::Requested(_)) => {
                    // Server cancelled the request or it was never picked up. Regardless, the
                    // server will never see this request so it's safe to reclaim.
                    *req = RpcRequestState::Completed;
                }
                Some(RpcRequestState::Completed) | None => {
                    // SAFETY: Request was completed so server must have written to the slot.
                    // Drop the initialized response via raw pointer to prevent unique stack borrow retags.
                    unsafe { (response_slot.as_ptr() as *mut R::Response).drop_in_place() }
                }
            }
            guard.clean_up_channel(&self.channel.not_full);
        });
        let chan = self.channel.state.lock();
        waker
            .when(chan, |chan| match chan.get(idx) {
                Some(RpcRequestState::Requested(_) | RpcRequestState::Accepted) => {
                    if chan.server_count == 0 {
                        Poll::Ready(Err(CallError::Closed))
                    } else {
                        Poll::Pending
                    }
                }
                Some(RpcRequestState::Completed) | None => Poll::Ready(Ok(())),
                Some(RpcRequestState::Cancelled) => Poll::Ready(Err(CallError::ServerCancel)),
            })
            .await?;
        cleanup.disarm();
        // SAFETY: Request was completed so server must have written to the slot.
        Ok(unsafe { response_slot.as_ref().assume_init_read() })
    }
}

/// Responder endpoint used by the Server to respond to an active request.
pub struct Responder<R: Rpc, Cfg: RpcCfg, C: Deref<Target = RpcChannel<R, Cfg>>> {
    request_idx: GlobalIndex,
    chan: C,
    response_slot: NonNull<MaybeUninit<R::Response>>,
    waker: NonNull<Notification<Cfg::Mtx>>,
}

/// Error sending a payload to the channel.
#[derive(Debug, Clone, Error)]
pub enum RecvError {
    #[error("All sender handles have been closed")]
    Closed,
}

impl<R: Rpc, Cfg: RpcCfg, C: Deref<Target = RpcChannel<R, Cfg>> + Clone> Server<C> {
    /// Returns the number of used request slots in the channel inbox.
    pub fn used_slots(&self) -> usize {
        self.channel.used_slots()
    }

    /// Asynchronously blocks until the next Client request is received.
    ///
    /// Returns a pair containing the request payload and a [`Responder`] endpoint.
    pub async fn recv(&self) -> Result<(R::Request, Responder<R, Cfg, C>), RecvError> {
        let guard = self.channel.state.lock();

        let (idx, info) = self
            .channel
            .not_empty
            .when(guard, |chan| {
                chan.clean_up_channel(&self.channel.not_full);
                match chan.next_request() {
                    Ok(Some(request)) => Poll::Ready(Ok(request)),
                    Ok(None) => Poll::Pending,
                    Err(e) => Poll::Ready(Err(e)),
                }
            })
            .await?;
        let RpcRequestInfo { request, response_slot, waker } = info;
        Ok((
            request,
            Responder { request_idx: idx, chan: self.channel.clone(), response_slot, waker },
        ))
    }

    /// Attempts to immediately receive a Client request without blocking.
    ///
    /// Returns `None` if there are no pending commands currently enqueued.
    pub fn try_recv(&self) -> Result<Option<(R::Request, Responder<R, Cfg, C>)>, RecvError> {
        let mut guard = self.channel.state.lock();
        guard.clean_up_channel(&self.channel.not_full);
        let Some((idx, info)) = guard.next_request()? else {
            return Ok(None);
        };
        let RpcRequestInfo { request, response_slot, waker } = info;
        Ok(Some((
            request,
            Responder { request_idx: idx, chan: self.channel.clone(), response_slot, waker },
        )))
    }
}

impl<R: Rpc, Cfg: RpcCfg, C: Deref<Target = RpcChannel<R, Cfg>>> Responder<R, Cfg, C> {
    /// Sends the response back to the Client and wakes their waker.
    pub fn respond(self, response: R::Response) {
        // Drop implementation just handles cancellation, so we can avoid double locking
        // by skipping Drop altogether if a response has come in.
        let this = ManuallyDrop::new(self);
        let mut guard = this.chan.state.lock();
        match guard.get_mut(this.request_idx) {
            Some(RpcRequestState::Requested(_) | RpcRequestState::Completed) | None => {
                unreachable!(
                    "Responder attached to request that was not accepted or that has been completed"
                )
            }
            Some(req @ RpcRequestState::Cancelled) => {
                // Client cancelled the request after accepting it. Update to completed for cleanup.
                //
                // NOTE: This is only okay because the client is guaranteed to have cancelled the
                // request, which means they won't ever look at the rpc request/response again.
                *req = RpcRequestState::Completed;
                guard.clean_up_channel(&this.chan.not_full);
                return;
            }
            Some(slot @ RpcRequestState::Accepted) => {
                *slot = RpcRequestState::Completed;
                // SAFETY: RpcRequest is not cancelled so the memory is still valid
                unsafe {
                    this.response_slot.write(MaybeUninit::new(response));
                    this.waker.as_ref().notify_one();
                }
                guard.clean_up_channel(&this.chan.not_full);
            }
        }
    }
}

impl<R: Rpc, Cfg: RpcCfg, C: Deref<Target = RpcChannel<R, Cfg>>> Drop for Responder<R, Cfg, C> {
    fn drop(&mut self) {
        let mut guard = self.chan.state.lock();
        match guard.get_mut(self.request_idx) {
            Some(state @ RpcRequestState::Accepted) => {
                // Request was accepted, but responder is dropped. Avoid deadlocking the client by cancelling the request.
                *state = RpcRequestState::Cancelled;
                // SAFETY: Rpc request is not cancelled so the waker is still valid.
                unsafe {
                    self.waker.as_ref().notify_one();
                }
            }
            Some(state @ RpcRequestState::Cancelled) => {
                // Request was already cancelled by the client. Transition to completed to enable cleanup
                *state = RpcRequestState::Completed;
                guard.clean_up_channel(&self.chan.not_full);
            }
            Some(RpcRequestState::Requested(_) | RpcRequestState::Completed) | None => {
                unreachable!(
                    "RPC Request must not be in `requested` state if a responder has been created"
                );
            }
        }
    }
}

/// A generic RAII scope guard that executes a closure when dropped.
///
/// `OnDrop` can be used to guarantee deferred cleanup actions are executed upon completion,
/// task cancellation, or panic stack unwinding.
///
/// The guard can be deactivated by calling [`OnDrop::disarm`], preventing the closure from running.
///
/// # Examples
///
/// Basic usage for deferred cleanup:
///
/// ```
/// # use sapphire_async::rpc::OnDrop;
/// # use std::cell::Cell;
///
/// let cleaned_up = Cell::new(false);
/// {
///     let guard = OnDrop::new(|| {
///         cleaned_up.set(true);
///     });
///     assert!(!cleaned_up.get());
/// } // guard goes out of scope and drops here
/// assert!(cleaned_up.get());
/// ```
///
/// Disarming the guard to skip cleanup on success:
///
/// ```
/// # use sapphire_async::rpc::OnDrop;
/// # use std::cell::Cell;
///
/// let cleaned_up = Cell::new(false);
/// {
///     let guard = OnDrop::new(|| {
///         cleaned_up.set(true);
///     });
///     guard.disarm();
/// } // guard dropped here, but nothing happens
/// assert!(!cleaned_up.get());
/// ```
pub struct OnDrop<F: FnOnce()> {
    fun: ManuallyDrop<F>,
}

impl<F: FnOnce()> OnDrop<F> {
    /// Creates a new `OnDrop` guard enclosing the provided cleanup closure.
    pub fn new(fun: F) -> Self {
        Self { fun: ManuallyDrop::new(fun) }
    }

    /// Disarms the guard, preventing the enclosed cleanup closure from executing.
    pub fn disarm(mut self) {
        // SAFETY: We manually drop the inner closure since we are forgetting `self`
        // and prevent the double drop by consuming `self`.
        unsafe { ManuallyDrop::drop(&mut self.fun) };
        core::mem::forget(self);
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        // SAFETY: Drop must only be called once during the guard deconstruction.
        // We safely take ownership of the enclosed closure and evaluate it.
        let fun = unsafe { ManuallyDrop::take(&mut self.fun) };
        fun();
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::executor::BoundedExecutor;
    use crate::testing::TestExecutor;
    use core::cell::Cell;
    use sapphire_sync::mutex::raw::SingleThreadMutex;
    use std::task::Context;

    struct SimpleRpc;
    impl Rpc for SimpleRpc {
        type Request = i32;
        type Response = i32;
    }

    use sapphire_collections::storage::ArrayStorage;

    struct StackRpcCfg;
    impl RpcCfg for StackRpcCfg {
        type Mtx = SingleThreadMutex;
        type Chan = ArrayStorage<10>;
    }

    type TestRpcChannel = RpcChannel<SimpleRpc, StackRpcCfg>;

    #[test]
    fn test_rpc_basic() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.spawn(async move {
                let (req, responder) = server.recv().await.unwrap();
                assert_eq!(req, 42);
                responder.respond(100);
            });

            s.block_on(async move {
                let res = client.call(42).await.unwrap();
                assert_eq!(res, 100);
            });
        });
    }

    #[test]
    fn test_rpc_cancellation_before_recv() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |_s| {
            let mut call_fut = Box::pin(client.call(42));

            let waker = std::task::Waker::noop();
            let mut cx = Context::from_waker(&waker);

            // First poll: it will call send() and then block on waker.wait(), returning Pending.
            assert!(call_fut.as_mut().poll(&mut cx).is_pending());

            // Now we drop the future! This calls Drop on the future (Cleanup).
            drop(call_fut);

            // Now server tries to receive. It should block (return Pending) because the only request was cancelled!
            let mut recv_fut = Box::pin(server.recv());
            assert!(recv_fut.as_mut().poll(&mut cx).is_pending());
        });
    }

    #[test]
    fn test_rpc_cancellation_after_recv() {
        BoundedExecutor::new(TestExecutor::new(), |_s| {
            let mut channel = TestRpcChannel::new();
            {
                let (client, server) = channel.split();

                let mut call_fut = Box::pin(client.call(42));

                let waker = std::task::Waker::noop();
                let mut cx = Context::from_waker(&waker);

                // Client sends request
                assert!(call_fut.as_mut().poll(&mut cx).is_pending());

                // Server receives request
                let mut recv_fut = Box::pin(server.recv());
                let mut rx_cx = Context::from_waker(&waker);
                let poll_res = recv_fut.as_mut().poll(&mut rx_cx);

                let Poll::Ready(Ok((req, responder))) = poll_res else {
                    panic!("Server should have received the request");
                };
                assert_eq!(req, 42);

                // Now client cancels! (Drops the future)
                drop(call_fut);

                // Server responds. It should see Cancelled and return immediately without writing to memory.
                responder.respond(100);

                drop(recv_fut);
            }

            // Verify that the channel is clean
            let mut state = channel.state.lock();
            state.clean_up_channel(&channel.not_full);
            assert_eq!(state.inbox.len(), 0);
        });
    }

    #[test]
    fn test_rpc_index_overflow() {
        let mut channel = TestRpcChannel::new();

        // Force-initialize the logical counter to a wrapping boundary.
        // This simulates having successfully processed usize::MAX requests.
        {
            let mut state = channel.state.lock();
            state.head = GlobalIndex::new(usize::MAX);
        }

        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            // 1. Spawn server receiving a request and responding
            s.spawn(async move {
                let (req, responder) = server.recv().await.unwrap();
                assert_eq!(req, 111);
                responder.respond(222);
            });

            // 2. Synchronously drive client call
            s.block_on(async move {
                let res = client.call(111).await.unwrap();
                assert_eq!(res, 222); // Should succeed cleanly!
            });
        });
    }

    #[test]
    fn test_rpc_channel_closing_servers_before_call() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.block_on(async {
                drop(server); // All servers closed
                let res = client.call(10).await;
                assert!(matches!(res, Err(CallError::Closed)));
            });
        });
    }

    #[test]
    fn test_rpc_channel_closing_clients() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let handle = s.spawn(async {
                let res = server.recv().await;
                assert!(matches!(res, Err(RecvError::Closed)));
            });
            s.run_until_stalled();
            assert!(!handle.is_finished());
            s.spawn(async {
                drop(client); // All clients closed
            });
            s.run_until_stalled();
            assert!(handle.is_finished());
        });
    }

    #[test]
    fn test_rpc_channel_closing_server_pending_calls() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        let client_ran = Cell::new(false);
        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.spawn(async {
                let res = client.call(10).await;
                assert!(matches!(res, Err(CallError::Closed)));
                client_ran.set(true);
            });
            s.run_until_stalled();

            s.spawn(async move {
                drop(server); // All servers closed
            });
            s.run_until_stalled();
        });
        assert!(client_ran.into_inner());
    }

    #[test]
    fn test_rpc_responder_dropped() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            s.spawn(async move {
                let (req, responder) = server.recv().await.unwrap();
                assert_eq!(req, 42);
                // Drop responder without responding
                drop(responder);
            });

            s.block_on(async move {
                let res = client.call(42).await;
                assert!(matches!(res, Err(CallError::ServerCancel)));
            });
        });
    }

    #[test]
    fn test_rpc_call_response_no_leaks() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let client_handle = s.spawn(async {
                assert_eq!(client.call(5).await.unwrap(), 10);
            });
            s.run_until_stalled();
            assert_eq!(client.used_slots(), 1);
            let (val, responder) = server.try_recv().unwrap().unwrap();
            assert_eq!(val, 5);
            assert_eq!(client.used_slots(), 1);
            responder.respond(10);
            s.run_until_stalled();
            assert!(client_handle.is_finished());
            assert_eq!(client.used_slots(), 0);
        });
    }

    #[test]
    fn test_rpc_client_cancellation_no_leaks() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let client_handle = s.spawn(async {
                let _ = client.call(5).await;
            });
            s.run_until_stalled();
            assert_eq!(client.used_slots(), 1);
            let (val, responder) = server.try_recv().unwrap().unwrap();
            assert_eq!(val, 5);
            assert_eq!(client.used_slots(), 1);
            client_handle.cancel();
            assert_eq!(client.used_slots(), 1);
            responder.respond(0);
            assert_eq!(client.used_slots(), 0);
        });
    }

    #[test]
    fn test_rpc_server_cancellation_no_leaks() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let client_handle = s.spawn(async {
                assert_eq!(client.call(5).await.unwrap_err(), CallError::ServerCancel);
            });
            s.run_until_stalled();
            assert_eq!(client.used_slots(), 1);
            let (val, responder) = server.try_recv().unwrap().unwrap();
            assert_eq!(val, 5);
            assert_eq!(client.used_slots(), 1);
            drop(responder);
            assert_eq!(client.used_slots(), 1);
            s.run_until_stalled();
            assert!(client_handle.is_finished());
            assert_eq!(client.used_slots(), 0);
        });
    }

    #[test]
    fn test_rpc_double_cancellation_no_leaks() {
        let mut channel = TestRpcChannel::new();
        let (client, server) = channel.split();

        BoundedExecutor::new(TestExecutor::new(), |s| {
            let client_handle = s.spawn(async {
                assert_eq!(client.call(5).await.unwrap_err(), CallError::ServerCancel);
            });
            s.run_until_stalled();
            assert_eq!(client.used_slots(), 1);
            let (val, responder) = server.try_recv().unwrap().unwrap();
            assert_eq!(val, 5);
            assert_eq!(client.used_slots(), 1);
            drop(responder);
            assert_eq!(client.used_slots(), 1);
            client_handle.cancel();
            assert_eq!(client.used_slots(), 0);
        });
    }

    #[test]
    fn rpc_cancellation_exhaustive() {
        struct TinyRpcCfg;
        impl RpcCfg for TinyRpcCfg {
            type Mtx = SingleThreadMutex;
            type Chan = ArrayStorage<1>;
        }
        type TinyRpcChannel = RpcChannel<SimpleRpc, TinyRpcCfg>;

        {
            let mut completed = false;
            // Loop 1: Queue is empty. Client blocks in `waker.when` on first poll.

            let mut poll_count = 0;
            while !completed {
                let mut channel = TinyRpcChannel::new();
                let (client, server) = channel.split();

                BoundedExecutor::new(TestExecutor::new(), |s| {
                    let client_handle = s.spawn(async { client.call(42).await });

                    for _ in 0..poll_count {
                        let _ = client_handle.poll_once();
                    }
                    completed = client.used_slots() == 1;

                    client_handle.cancel();

                    // Spawn server to process any remaining or deferred actions
                    s.spawn(async move {
                        while let Ok((_req, responder)) = server.recv().await {
                            responder.respond(100);
                        }
                    });

                    s.run_until_stalled();
                    // Either the request never made it through or it got handled and/or cleaned up
                    assert_eq!(client.used_slots(), 0);
                });
                poll_count += 1;
            }
        }

        let mut completed = false;
        let mut poll_count = 0;
        // Loop 2: Queue is full. Client blocks in `send` on first poll, then progresses.
        while !completed {
            let mut channel = TinyRpcChannel::new();
            let (client, server) = channel.split();

            BoundedExecutor::new(TestExecutor::new(), |s| {
                // Fill the queue
                let filler_handle = s.spawn(async { client.call(1).await });
                assert_eq!(filler_handle.poll_once(), Poll::Pending);
                assert_eq!(client.used_slots(), 1);

                let client_handle = s.spawn(async { client.call(42).await });

                s.run_until_stalled();
                // Free up the slot.
                filler_handle.cancel();

                for _ in 0..poll_count {
                    let _ = client_handle.poll_once();
                }

                // If we manage to send the message then we ran through the test as far as we could.
                completed = client.used_slots() == 1;

                // Spawn server to process any remaining or deferred actions
                s.spawn(async move {
                    while let Ok((_req, responder)) = server.recv().await {
                        responder.respond(100);
                    }
                });

                s.run_until_stalled();
                assert_eq!(client.used_slots(), 0);
            });
            poll_count += 1;
        }
    }

    mod proptests {
        use super::*;
        use crate::executor::BoundedExecutor;
        use crate::testing::TestExecutor;
        use proptest::prelude::*;
        use sapphire_sync::mutex::raw::SingleThreadMutex;
        use std::future::Future;
        use std::pin::Pin;
        use std::task::Context;

        struct TestRpc;
        impl Rpc for TestRpc {
            type Request = i32;
            type Response = i32;
        }

        use sapphire_collections::storage::ArrayStorage;

        struct TestRpcCfg;
        impl RpcCfg for TestRpcCfg {
            type Mtx = SingleThreadMutex;
            type Chan = ArrayStorage<10>;
        }

        type TestRpcChannel = RpcChannel<TestRpc, TestRpcCfg>;

        #[derive(Debug, Clone)]
        enum RpcOp {
            Call(i32),
            ClientCancel(usize),
            ServerCancel(usize),
            Recv,
            Respond(usize, i32),
        }

        proptest! {
            #[test]
            fn test_rpc_proptest(
                ops in prop::collection::vec(
                    prop_oneof![
                        any::<i32>().prop_map(RpcOp::Call),
                        any::<usize>().prop_map(RpcOp::ClientCancel),
                        any::<usize>().prop_map(RpcOp::ServerCancel),
                        Just(RpcOp::Recv),
                        any::<(usize, i32)>().prop_map(|(idx, val)| RpcOp::Respond(idx, val)),
                    ],
                    0..50
                )
            ) {
                BoundedExecutor::new(TestExecutor::new(), |s| {
                    let mut channel = TestRpcChannel::new();
                    let (client, server) = channel.split();

                    let mut client_calls: Vec<Option<Pin<Box<dyn Future<Output = Result<i32, CallError>> + '_>>>> = Vec::new();
                    let mut responders: Vec<Option<Responder<TestRpc, TestRpcCfg, _>>> = Vec::new();

                    let waker = std::task::Waker::noop();
                    let mut cx = Context::from_waker(&waker);

                    for op in ops {
                        match op {
                            RpcOp::Call(val) => {
                                let mut call_fut = Box::pin(client.call(val));
                                assert!(call_fut.as_mut().poll(&mut cx).is_pending());
                                client_calls.push(Some(call_fut));
                            }
                            RpcOp::ClientCancel(idx) => {
                                let len = client_calls.len();
                                if len > 0 {
                                      let target_idx = idx % len;
                                      client_calls[target_idx] = None;
                                }
                            }
                            RpcOp::ServerCancel(idx) => {
                                let len = responders.len();
                                if len > 0 {
                                      let target_idx = idx % len;
                                      responders[target_idx] = None;
                                }
                            }
                            RpcOp::Recv => {
                                let mut recv_fut = Box::pin(server.recv());
                                if let Poll::Ready(Ok((_req, responder))) = recv_fut.as_mut().poll(&mut cx) {
                                    responders.push(Some(responder));
                                }
                            }
                            RpcOp::Respond(idx, val) => {
                                let len = responders.len();
                                if len > 0 {
                                      let target_idx = idx % len;
                                      if let Some(responder) = responders[target_idx].take() {
                                          responder.respond(val);
                                      }
                                }
                            }
                        }
                        s.run_until_stalled();
                    }
                });
            }
        }
    }
}
