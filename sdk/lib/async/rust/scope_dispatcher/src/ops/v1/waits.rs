// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::ptr::NonNull;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use fuchsia_async::{PacketReceiver, ReceiverRegistration};
use libasync_sys::{async_dispatcher_t, async_wait_t};
use zx::sys::{ZX_OK, zx_packet_signal_t, zx_status_t};
use zx::{HandleRef, Packet, Signals, Status, WaitAsyncOpts};

use crate::ScopeDispatcher;

#[derive(Default, Debug)]
pub struct PendingWaits {
    waits: HashMap<Wait, ReceiverRegistration<PendingWait>>,
}

impl PendingWaits {
    /// Starts a wait on the dispatcher
    fn start_wait(&mut self, dispatcher: &Arc<ScopeDispatcher>, wait: Wait) -> Result<(), Status> {
        // don't queue a new wait if we're shutting down.
        if dispatcher.is_shutting_down() {
            return Err(Status::BAD_STATE);
        }

        // TODO(543947796): With a more direct API to fuchsia-async's underlying registration, we
        // could directly pass the wait pointer and a custom VTable and get back the key, which
        // we could use to directly cancel the registration. This would require either moving this
        // library into fuchsia-async or exposing an API like that for this library to use.
        let registration = dispatcher.global_handle().register_receiver(PendingWait {
            wait: Wait(wait.0),
            dispatcher: Arc::downgrade(dispatcher),
        });
        wait.handle().wait_async(
            registration.port(),
            registration.key(),
            wait.trigger(),
            wait.options(),
        )?;

        self.waits.insert(wait, registration);

        Ok(())
    }

    fn cancel_wait(&mut self, wait: Wait) -> Result<(), Status> {
        // if we pull the wait out while in a mutable borrow, that means the PendingWait receiver
        // will not be able to get it to call the callback and we can return success. Otherwise
        // we say we couldn't deregister it.
        if self.waits.remove(&wait).is_some() { Ok(()) } else { Err(Status::NOT_FOUND) }
    }

    // Returns all the outstanding waits for cancellation.
    pub fn get_all_waits(&mut self) -> impl Iterator<Item = Wait> {
        self.waits.drain().map(|(wait, _)| wait)
    }
}

#[derive(Debug)]
struct PendingWait {
    wait: Wait,
    // TODO(543947796): Potential performance improvement: Embed a raw pointer to the scope
    // dispatcher into the wait's state fields.
    dispatcher: Weak<ScopeDispatcher>,
}

impl PacketReceiver for PendingWait {
    fn receive_packet(&self, packet: Packet) {
        let zx::PacketContents::SignalOne(signal) = packet.contents() else {
            panic!("unexpected signal type waiting for signal packet");
        };
        // if this fails, it means the dispatcher is gone and we shouldn't do anything.
        let Some(dispatcher) = self.dispatcher.upgrade() else { return };
        // if we can't remove the wait object from the pending waits, that means the user has
        // canceled the wait, either by shutting down the dispatcher or directly, so we should not
        // call the callback.
        if dispatcher.pending_waits.lock().waits.remove(&self.wait).is_none() {
            return;
        }
        // we have now de-registered the wait and taken ownership of the wait object itself so
        // we can safely call the callback.
        self.wait.run(dispatcher, signal.raw_packet(), Ok(()));
    }
}

/// The representation of a wait object.
#[derive(Eq, PartialEq, Hash)]
pub struct Wait(NonNull<async_wait_t>);

// SAFETY: The wait structure is owned by the dispatcher once it's been queued, and the executor
// treats it as read only and does not modify it while owning it, so it is thread safe.
unsafe impl Send for Wait {}
// SAFETY: The wait structure is owned by the dispatcher once it's been queued, and the executor
// treats it as read only and does not modify it while owning it, so it is thread safe.
unsafe impl Sync for Wait {}

impl Wait {
    /// Runs the wait handler with the appropriate arguments.
    pub fn run(
        &self,
        dispatcher: Arc<ScopeDispatcher>,
        signal: *const zx_packet_signal_t,
        status: Result<(), Status>,
    ) {
        // SAFETY: The caller provided a valid non-null wait to `begin_wait`, and is expected to
        // keep it alive until either it is successfully canceled or its callback is called.
        let Some(callback) = unsafe { self.0.as_ref() }.handler else { return };
        // SAFETY: The caller is expected to provide a valid function with the correct signature.
        unsafe {
            callback(
                dispatcher.as_ptr() as *mut _,
                self.0.as_ptr(),
                Status::result_into_raw(status),
                signal,
            )
        };
    }

    fn handle(&self) -> HandleRef<'_> {
        // SAFETY: The registrant of the wait is expected to provide a valid handle and keep it
        // alive for as long as it is registered to the dispatcher, and the returned handle ref
        // is lifetime bound to this structure.
        unsafe { HandleRef::from_raw_handle(self.0.as_ref().object) }
    }

    fn trigger(&self) -> Signals {
        // SAFETY: The registrant of the wait is expected to provide a valid handle and keep it
        // alive for as long as it is registered to the dispatcher.
        Signals::from_bits_retain(unsafe { self.0.as_ref() }.trigger)
    }

    fn options(&self) -> WaitAsyncOpts {
        // SAFETY: The registrant of the wait is expected to provide a valid handle and keep it
        // alive for as long as it is registered to the dispatcher.
        WaitAsyncOpts::from_bits_retain(unsafe { self.0.as_ref() }.options)
    }
}

// manually implement debug because the bindgen impl doesn't include all of the fields
impl fmt::Debug for Wait {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: The caller provided a valid non-null wait to `post_wait`, and is expected to keep
        // it alive until either it is successfully canceled or its callback is called.
        let async_wait_t { state, handler, object, trigger, options } = unsafe { self.0.as_ref() };
        write!(
            f,
            "Wait {{ async_wait@{:?} {{ object: {object:?}, trigger: {trigger:?}, options: {options:?}, handler: {handler:?}, state: {state:?} }} }}",
            self.0,
        )
    }
}

/// Begins asynchronously waiting for an object to receive one or more signals
/// specified in |wait|.  Invokes the handler when the wait completes.
///
/// The wait's handler will be invoked exactly once unless the wait is canceled.
/// When the dispatcher is shutting down (being destroyed), the handlers of
/// all remaining waits will be invoked with a status of |ZX_ERR_CANCELED|.
///
/// Returns |ZX_OK| if the wait was successfully begun.
/// Returns |ZX_ERR_ACCESS_DENIED| if the object does not have |ZX_RIGHT_WAIT|.
/// Returns |ZX_ERR_BAD_STATE| if the dispatcher is shutting down.
/// Returns |ZX_ERR_NOT_SUPPORTED| if not supported by the dispatcher.
///
/// This operation is thread-safe.
pub unsafe extern "C" fn begin_wait(
    dispatcher_ptr: *mut async_dispatcher_t,
    wait_ptr: *mut async_wait_t,
) -> zx_status_t {
    // Safety: The caller is responsible for ensuring this is only ever called on a dispatcher
    // object that was originally obtained from [`ScopeDispatcher::as_ptr`].
    let dispatcher = unsafe { ScopeDispatcher::arc_from_ptr(dispatcher_ptr) };
    let wait_ptr = NonNull::new(wait_ptr).expect("invalid wait pointer");
    let wait = Wait(wait_ptr);

    if let Err(err) = dispatcher.pending_waits.lock().start_wait(&dispatcher, wait) {
        return err.into_raw();
    }
    ZX_OK
}

/// Cancels the wait associated with |wait|.
///
/// If successful, the wait's handler will not run.
///
/// Returns |ZX_OK| if the wait was pending and it has been successfully
/// canceled; its handler will not run again and can be released immediately.
/// Returns |ZX_ERR_NOT_FOUND| if there was no pending wait either because it
/// already completed, had not been started, or its completion packet has been
/// dequeued from the port and is pending delivery to its handler (perhaps on
/// another thread).
/// Returns |ZX_ERR_NOT_SUPPORTED| if not supported by the dispatcher.
///
/// This operation is thread-safe.
pub unsafe extern "C" fn cancel_wait(
    dispatcher_ptr: *mut async_dispatcher_t,
    wait_ptr: *mut async_wait_t,
) -> zx_status_t {
    // Safety: The caller is responsible for ensuring this is only ever called on a dispatcher
    // object that was originally obtained from [`ScopeDispatcher::as_ptr`].
    let dispatcher = unsafe { ScopeDispatcher::from_ptr(dispatcher_ptr) };
    let wait_ptr = NonNull::new(wait_ptr).expect("invalid wait pointer");
    let wait = Wait(wait_ptr);

    if let Err(err) = dispatcher.pending_waits.lock().cancel_wait(wait) {
        return err.into_raw();
    }
    ZX_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::task::Poll;
    use fuchsia_async::TestExecutor;
    use futures::future::pending;
    use futures::poll;
    use libasync::DispatcherSignalExt;

    #[fuchsia::test]
    fn wait_on_signals() {
        // This test is a bit complicated because we want to make sure we exercise the full
        // cycle through the PacketReceiver and not just always poll it and immediately resolve.
        let mut test_executor = TestExecutor::new();
        let scope_dispatcher =
            ScopeDispatcher::new_on_executor(test_executor.global_handle().clone());
        let event = zx::Event::create();

        // first create the future and register it
        let mut fut = scope_dispatcher.on_signals(&event, zx::Signals::EVENT_SIGNALED);
        assert_eq!(test_executor.run_until_stalled(&mut fut), Poll::Pending);

        // then signal the event so that the event loop will pick it up.
        event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();

        // poll a never-resolving future to run the loop and call the packet receiver without
        // directly polling the signal future.
        assert_eq!(test_executor.run_until_stalled(&mut pending::<()>()), Poll::Pending);

        // and finally poll the actual future and make sure it resolved correctly.
        let Poll::Ready(res) = test_executor.run_until_stalled(&mut fut) else {
            panic!("Future did not resolve after signal was set.");
        };
        assert!(res.is_ok());
        assert!(res.unwrap().contains(zx::Signals::EVENT_SIGNALED));

        assert_eq!(
            test_executor.run_until_stalled(&mut scope_dispatcher.shutdown()),
            Poll::Ready(())
        );
    }

    #[fuchsia::test]
    async fn wait_on_already_signaled() {
        let scope_dispatcher = ScopeDispatcher::new();
        let event = zx::Event::create();
        event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();

        let fut = scope_dispatcher.on_signals(&event, zx::Signals::EVENT_SIGNALED);

        let res = fut.await;
        assert!(res.is_ok());
        assert!(res.unwrap().contains(zx::Signals::EVENT_SIGNALED));

        scope_dispatcher.shutdown().await;
    }

    #[fuchsia::test]
    async fn drop_after_poll() {
        let scope_dispatcher = ScopeDispatcher::new();
        let event = zx::Event::create();
        let mut fut = scope_dispatcher.on_signals(&event, zx::Signals::EVENT_SIGNALED);
        assert_eq!(poll!(&mut fut), Poll::Pending);

        scope_dispatcher.shutdown().await;
    }

    #[fuchsia::test]
    async fn dispatcher_shutdown_cancel() {
        let event = zx::Event::create();

        let scope_dispatcher = ScopeDispatcher::new();
        let mut fut = scope_dispatcher.on_signals(&event, zx::Signals::EVENT_SIGNALED);
        assert_eq!(poll!(&mut fut), Poll::Pending);
        scope_dispatcher.shutdown().await;
        assert_eq!(fut.await, Err(Status::CANCELED));
    }

    #[fuchsia::test]
    async fn test_take_handle() {
        let scope_dispatcher = ScopeDispatcher::new();
        let event = zx::Event::create();
        let mut fut = scope_dispatcher.on_signals(event, zx::Signals::EVENT_SIGNALED);
        assert_eq!(poll!(&mut fut), Poll::Pending);
        let event = fut.take_handle().unwrap();
        event.signal(zx::Signals::empty(), zx::Signals::EVENT_SIGNALED).unwrap();

        assert!(fut.take_handle().is_none());
        assert_eq!(poll!(&mut fut), Poll::Ready(Err(Status::BAD_STATE)));
        assert_eq!(
            event
                .wait_one(zx::Signals::EVENT_SIGNALED, zx::MonotonicInstant::INFINITE_PAST)
                .unwrap(),
            zx::Signals::EVENT_SIGNALED
        );

        scope_dispatcher.shutdown().await;
    }
}
