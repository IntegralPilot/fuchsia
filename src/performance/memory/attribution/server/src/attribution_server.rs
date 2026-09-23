// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::Error::ClientChannelClosed;
use fidl_fuchsia_memory_attribution as fattribution;
use fuchsia_sync::Mutex;
use log::error;
use measure_tape_for_attribution::Measurable;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

mod key {
    /// Identifier used for disambiguation;
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct Key(u64);

    /// Generates unique [Key] objects.
    pub struct KeyGenerator {
        next: Key,
    }

    impl Default for KeyGenerator {
        fn default() -> Self {
            Self { next: Key(0) }
        }
    }

    impl KeyGenerator {
        /// Generates the next [Key] object.
        pub fn next(&mut self) -> Key {
            let next_key = self.next;
            self.next = Key(self.next.0.checked_add(1).expect("Key generator overflow"));
            next_key
        }
    }
}

/// Function of this type returns a vector of attribution updates, and is used
/// as the type of the callback in [AttributionServer::new].
type GetAttributionFn = dyn Fn() -> Vec<fattribution::AttributionUpdate> + Send;

/// Error types that may be used by the async hanging-get server.
#[derive(Error, Debug)]
pub enum AttributionServerObservationError {
    #[error("multiple pending observations for the same Observer")]
    GetUpdateAlreadyPending,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct PrincipalIdentifier(u64);

/// Main structure for the memory attribution hanging get server.
///
/// Components that wish to expose attribution information should create a
/// single [AttributionServer] object.
/// Each inbound fuchsia.attribution.Provider connection should get its own
/// [Observer] object using [AttributionServer::new_observer].
/// [Publisher]s, created using [AttributionServer::new_publisher], should be
/// used to push attribution changes.
#[derive(Clone)]
pub struct AttributionServerHandle {
    inner: Arc<Mutex<AttributionServer>>,
}

impl AttributionServerHandle {
    /// Create a new [Observer] that represents a single client.
    ///
    /// Each FIDL client connection should get its own [Observer] object.
    pub fn new_observer(&self, control_handle: fattribution::ProviderControlHandle) -> Observer {
        AttributionServer::register(&self.inner, control_handle)
    }

    /// Create a new [Publisher] that can push updates to observers.
    pub fn new_publisher(&self) -> Publisher {
        Publisher { inner: self.inner.clone() }
    }
}

/// An `Observer` can be used to register observation requests, corresponding to individual hanging
/// get calls. These will be notified when the state changes or immediately the first time
/// an `Observer` registers an observation.
pub struct Observer {
    inner: Arc<Mutex<AttributionServer>>,
    subscription_id: key::Key,
}

impl Observer {
    /// Register a new observation request.
    ///
    /// A newly-created observer will first receive the current state. After
    /// the first call, the observer will be notified only if the state changes.
    ///
    /// Errors occur when an Observer attempts to wait for an update when there
    /// is a update request already pending.
    pub fn next(&self, responder: fattribution::ProviderGetResponder) {
        self.inner.lock().next(responder)
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.inner.lock().unregister(self.subscription_id);
    }
}

/// A [Publisher] should be used to send updates to [Observer]s.
pub struct Publisher {
    inner: Arc<Mutex<AttributionServer>>,
}

impl Publisher {
    /// Registers an update to the state observed.
    ///
    /// `partial_state` is a function that returns the update.
    pub fn on_update(&self, updates: Vec<fattribution::AttributionUpdate>) {
        // [update_generator] is a `Fn` and not an `FnOnce` in order to be called multiple times,
        // once for each [Observer].
        self.inner.lock().on_update(updates)
    }
}

pub struct AttributionServer {
    state: Box<GetAttributionFn>,
    consumer: Option<AttributionConsumer>,
    key_generator: key::KeyGenerator,
}

impl AttributionServer {
    /// Create a new memory attribution server.
    ///
    /// `state` is a function returning the complete attribution state (not partial updates).
    pub fn new(state: Box<GetAttributionFn>) -> AttributionServerHandle {
        AttributionServerHandle {
            inner: Arc::new(Mutex::new(AttributionServer {
                state,
                consumer: None,
                key_generator: Default::default(),
            })),
        }
    }

    pub fn on_update(&mut self, updates: Vec<fattribution::AttributionUpdate>) {
        if let Some(consumer) = &mut self.consumer {
            return consumer.update_and_notify(updates);
        }
    }

    /// Get the next attribution state.
    pub fn next(&mut self, responder: fattribution::ProviderGetResponder) {
        let entry = self.consumer.as_mut().unwrap();
        entry.get_update(responder, self.state.as_ref());
    }

    pub fn register(
        inner: &Arc<Mutex<Self>>,
        control_handle: fattribution::ProviderControlHandle,
    ) -> Observer {
        let mut locked_inner = inner.lock();

        if locked_inner.consumer.is_some() {
            log::warn!("Multiple connection requests to AttributionProvider");
            // The shutdown of the observer will be done when the old [AttributionConsumer] is
            // dropped.
        }

        let key = locked_inner.key_generator.next();

        locked_inner.consumer = Some(AttributionConsumer::new(control_handle, key.clone()));
        Observer { inner: inner.clone(), subscription_id: key }
    }

    /// Deregister the current observer. No observer can be registered as long
    /// as another observer is already registered.
    pub fn unregister(&mut self, key: key::Key) {
        if let Some(consumer) = &self.consumer {
            if consumer.subscription_id == key {
                self.consumer = None;
            }
        }
    }
}

/// CoalescedUpdate contains all the pending updates for a given principal.
#[derive(Default)]
struct CoalescedUpdate {
    add: Option<fattribution::AttributionUpdate>,
    update: Option<fattribution::AttributionUpdate>,
    remove: Option<fattribution::AttributionUpdate>,
}

/// Should the update be kept, or can it be discarded.
#[derive(PartialEq)]
enum ShouldKeepUpdate {
    KEEP,
    DISCARD,
}

impl CoalescedUpdate {
    /// Merges updates of a given Principal, discarding the ones that become irrelevant.
    pub fn update(&mut self, u: fattribution::AttributionUpdate) -> ShouldKeepUpdate {
        match u {
            fattribution::AttributionUpdate::Add(u) => {
                self.add = Some(fattribution::AttributionUpdate::Add(u));
                self.update = None;
                self.remove = None;
            }
            fattribution::AttributionUpdate::Update(u) => {
                self.update = Some(fattribution::AttributionUpdate::Update(u));
            }
            fattribution::AttributionUpdate::Remove(u) => {
                if self.add.is_some() {
                    // We both added and removed the principal, so it is a no-op.
                    return ShouldKeepUpdate::DISCARD;
                }
                self.remove = Some(fattribution::AttributionUpdate::Remove(u));
            }
            fattribution::AttributionUpdateUnknown!() => {
                error!("Unknown attribution update type");
            }
        };
        ShouldKeepUpdate::KEEP
    }

    pub fn get_updates(self) -> Vec<fattribution::AttributionUpdate> {
        let mut result = Vec::new();
        if let Some(u) = self.add {
            result.push(u);
        }
        if let Some(u) = self.update {
            result.push(u);
        }
        if let Some(u) = self.remove {
            result.push(u);
        }
        result
    }

    pub fn size(&self) -> (usize, usize) {
        let (mut bytes, mut handles) = (0, 0);
        if let Some(u) = &self.add {
            let m = u.measure();
            bytes += m.num_bytes;
            handles += m.num_handles;
        }
        if let Some(u) = &self.update {
            let m = u.measure();
            bytes += m.num_bytes;
            handles += m.num_handles;
        }
        if let Some(u) = &self.remove {
            let m = u.measure();
            bytes += m.num_bytes;
            handles += m.num_handles;
        }
        (bytes, handles)
    }
}

/// AttributionConsumer tracks pending updates and observation requests for a given id.
struct AttributionConsumer {
    /// Whether we sent the first full state, or not.
    first: bool,

    /// Pending updates waiting to be sent.
    pending: HashMap<PrincipalIdentifier, CoalescedUpdate>,

    /// Control handle for the FIDL connection.
    observer_control_handle: fattribution::ProviderControlHandle,

    /// FIDL responder for a pending hanging get call.
    responder: Option<fattribution::ProviderGetResponder>,

    /// Matches an AttributionConsumer with an Observer.
    subscription_id: key::Key,
}

impl Drop for AttributionConsumer {
    fn drop(&mut self) {
        self.observer_control_handle.shutdown_with_epitaph(zx::Status::CANCELED);
    }
}

impl AttributionConsumer {
    /// Create a new [AttributionConsumer] without an `observer` and an initial `dirty`
    /// value of `true`.
    pub fn new(
        observer_control_handle: fattribution::ProviderControlHandle,
        key: key::Key,
    ) -> Self {
        AttributionConsumer {
            first: true,
            pending: HashMap::new(),
            observer_control_handle: observer_control_handle,
            responder: None,
            subscription_id: key,
        }
    }

    /// Register a new observation request. The observer will be notified immediately if
    /// the [AttributionConsumer] has pending updates, or hasn't sent anything yet. The
    /// request will be stored for future notification if the [AttributionConsumer] does
    /// not have anything to send yet.
    pub fn get_update(
        &mut self,
        responder: fattribution::ProviderGetResponder,
        gen_state: &GetAttributionFn,
    ) {
        if self.responder.is_some() {
            self.observer_control_handle.shutdown_with_epitaph(zx::Status::BAD_STATE);
            return;
        }
        if self.first {
            self.first = false;
            self.pending.clear();
            self.responder = Some(responder);
            self.update_and_notify(gen_state());
            return;
        }
        self.responder = Some(responder);
        self.maybe_notify();
    }

    /// Take in new memory attribution updates.
    pub fn update_and_notify(&mut self, updated_state: Vec<fattribution::AttributionUpdate>) {
        for update in updated_state {
            let principal: PrincipalIdentifier = match &update {
                fattribution::AttributionUpdate::Add(added_attribution) => {
                    PrincipalIdentifier(added_attribution.identifier.unwrap())
                }
                fattribution::AttributionUpdate::Update(update_attribution) => {
                    PrincipalIdentifier(update_attribution.identifier.unwrap())
                }
                fattribution::AttributionUpdate::Remove(remove_attribution) => {
                    PrincipalIdentifier(*remove_attribution)
                }
                &fattribution::AttributionUpdateUnknown!() => {
                    unimplemented!()
                }
            };
            if self.pending.entry(principal.clone()).or_insert(Default::default()).update(update)
                == ShouldKeepUpdate::DISCARD
            {
                self.pending.remove(&principal);
            }
        }
        self.maybe_notify();
    }

    /// Notify of the pending updates if a responder is available.
    fn maybe_notify(&mut self) {
        if self.pending.is_empty() {
            return;
        }

        match self.responder.take() {
            Some(observer) => {
                let mut iterator = self.pending.drain().peekable();
                let mut current_size: usize = 32;
                let mut current_handles: usize = 0;
                let mut update = Vec::new();
                while let Some((_, next)) = iterator.peek() {
                    let (update_size, update_handles) = next.size();

                    if current_size + update_size > zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize {
                        break;
                    }
                    if current_handles + update_handles
                        > zx::sys::ZX_CHANNEL_MAX_MSG_HANDLES as usize
                    {
                        break;
                    }
                    current_size += update_size;
                    current_handles += update_handles;
                    update.extend(iterator.next().unwrap().1.get_updates().into_iter());
                }

                self.pending = iterator.collect();
                Self::send_update(update, observer)
            }
            None => {}
        }
    }

    /// Sends the attribution update to the provided responder.
    fn send_update(
        state: Vec<fattribution::AttributionUpdate>,
        responder: fattribution::ProviderGetResponder,
    ) {
        match responder.send(Ok(fattribution::ProviderGetResponse {
            attributions: Some(state),
            ..Default::default()
        })) {
            Ok(()) => {} // indicates that the observer was successfully updated
            Err(e) => {
                // `send()` ensures that the channel is shut down in case of error.
                if let ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. } = e {
                    // Skip if this is simply our client closing the channel.
                    return;
                }
                error!("Failed to send memory state to observer: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;
    use fidl::endpoints::RequestStream;
    use fuchsia_async as fasync;
    use futures::TryStreamExt;

    /// Tests that the ELF runner can tell us about the resources used by the component it runs.
    #[test]
    fn test_attribute_memory() {
        let mut exec = fasync::TestExecutor::new();
        let server = AttributionServer::new(Box::new(|| {
            let new_principal = fattribution::NewPrincipal {
                identifier: Some(0),
                description: Some(fattribution::Description::Part("part".to_owned())),
                principal_type: Some(fattribution::PrincipalType::Runnable),
                detailed_attribution: None,
                __source_breaking: fidl::marker::SourceBreaking,
            };
            vec![fattribution::AttributionUpdate::Add(new_principal)]
        }));
        let (snapshot_provider, snapshot_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fattribution::ProviderMarker>();

        let observer = server.new_observer(snapshot_request_stream.control_handle());
        fasync::Task::spawn(async move {
            serve(observer, snapshot_request_stream).await.unwrap();
        })
        .detach();

        let attributions =
            exec.run_singlethreaded(snapshot_provider.get()).unwrap().unwrap().attributions;
        assert!(attributions.is_some());

        let attributions_vec = attributions.unwrap();
        // It should contain one component, the one we just launched.
        assert_eq!(attributions_vec.len(), 1);
        let new_attrib = attributions_vec.get(0).unwrap();
        let fattribution::AttributionUpdate::Add(added_principal) = new_attrib else {
            panic!("Not a new principal");
        };
        assert_eq!(added_principal.identifier, Some(0));
        assert_eq!(added_principal.principal_type, Some(fattribution::PrincipalType::Runnable));

        server.new_publisher().on_update(vec![fattribution::AttributionUpdate::Update(
            fattribution::UpdatedPrincipal { identifier: Some(0), ..Default::default() },
        )]);
        let attributions =
            exec.run_singlethreaded(snapshot_provider.get()).unwrap().unwrap().attributions;
        assert!(attributions.is_some());

        let attributions_vec = attributions.unwrap();
        // It should contain one component, the one we just launched.
        assert_eq!(attributions_vec.len(), 1);
        let updated_attrib = attributions_vec.get(0).unwrap();
        let fattribution::AttributionUpdate::Update(updated_principal) = updated_attrib else {
            panic!("Not an updated principal");
        };
        assert_eq!(updated_principal.identifier, Some(0));
    }

    pub async fn serve(
        observer: Observer,
        mut stream: fattribution::ProviderRequestStream,
    ) -> Result<(), fidl::Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                fattribution::ProviderRequest::Get { responder } => {
                    observer.next(responder);
                }
                fattribution::ProviderRequest::_UnknownMethod { .. } => {
                    assert!(false);
                }
            }
        }
        Ok(())
    }

    /// Tests that a new Provider connection cancels a previous one.
    #[test]
    fn test_disconnect_on_new_connection() {
        let mut exec = fasync::TestExecutor::new();
        let server = AttributionServer::new(Box::new(|| {
            vec![fattribution::AttributionUpdate::Add(fattribution::NewPrincipal {
                identifier: Some(1),
                description: Some(fattribution::Description::Part("part1".to_owned())),
                principal_type: Some(fattribution::PrincipalType::Runnable),
                detailed_attribution: None,
                ..Default::default()
            })]
        }));
        let (snapshot_provider, snapshot_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fattribution::ProviderMarker>();

        let observer = server.new_observer(snapshot_request_stream.control_handle());

        let (new_snapshot_provider, new_snapshot_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fattribution::ProviderMarker>();

        let new_observer = server.new_observer(new_snapshot_request_stream.control_handle());
        fasync::Task::spawn(async move {
            serve(new_observer, new_snapshot_request_stream).await.unwrap();
        })
        .detach();

        drop(observer);
        let result = exec.run_singlethreaded(snapshot_provider.get());
        assert_matches!(
            result,
            Err(ClientChannelClosed { epitaph, .. }) if epitaph == zx::Status::CANCELED
        );

        let result = exec.run_singlethreaded(new_snapshot_provider.get());
        assert!(result.is_ok());
        server.new_publisher().on_update(vec![fattribution::AttributionUpdate::Add(
            fattribution::NewPrincipal {
                identifier: Some(2),
                description: Some(fattribution::Description::Part("part2".to_owned())),
                principal_type: Some(fattribution::PrincipalType::Runnable),
                detailed_attribution: None,
                ..Default::default()
            },
        )]);
        let result = exec.run_singlethreaded(new_snapshot_provider.get());
        assert!(result.is_ok());
    }

    /// Tests that a new [Provider::get] call while another call is still pending
    /// generates an error.
    #[test]
    fn test_disconnect_on_two_pending_gets() {
        let mut exec = fasync::TestExecutor::new();
        let server = AttributionServer::new(Box::new(|| {
            let new_principal = fattribution::NewPrincipal {
                identifier: Some(0),
                principal_type: Some(fattribution::PrincipalType::Runnable),
                ..Default::default()
            };
            vec![fattribution::AttributionUpdate::Add(new_principal)]
        }));
        let (snapshot_provider, snapshot_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fattribution::ProviderMarker>();

        let observer = server.new_observer(snapshot_request_stream.control_handle());
        fasync::Task::spawn(async move {
            serve(observer, snapshot_request_stream).await.unwrap();
        })
        .detach();

        // The first call should succeed right away.
        exec.run_singlethreaded(snapshot_provider.get())
            .expect("Connection dropped")
            .expect("Get call failed");

        // The next call should block until an update is pushed on the provider side.
        let mut future = snapshot_provider.get();

        let _ = exec.run_until_stalled(&mut future);

        // The second parallel get() call should fail.
        let result = exec.run_singlethreaded(snapshot_provider.get());

        let result2 = exec.run_singlethreaded(future);

        assert_matches!(
            result2,
            Err(ClientChannelClosed { epitaph, .. }) if epitaph == zx::Status::BAD_STATE
        );
        assert_matches!(
            result,
            Err(ClientChannelClosed { epitaph, .. }) if epitaph == zx::Status::BAD_STATE
        );
    }

    /// Tests that the first get call returns the full state, not updates.
    #[test]
    fn test_no_update_on_first_call() {
        let mut exec = fasync::TestExecutor::new();
        let server = AttributionServer::new(Box::new(|| {
            let new_principal = fattribution::NewPrincipal {
                identifier: Some(0),
                principal_type: Some(fattribution::PrincipalType::Runnable),
                ..Default::default()
            };
            vec![fattribution::AttributionUpdate::Add(new_principal)]
        }));
        let (snapshot_provider, snapshot_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fattribution::ProviderMarker>();

        let observer = server.new_observer(snapshot_request_stream.control_handle());
        fasync::Task::spawn(async move {
            serve(observer, snapshot_request_stream).await.unwrap();
        })
        .detach();

        server.new_publisher().on_update(vec![fattribution::AttributionUpdate::Update(
            fattribution::UpdatedPrincipal { identifier: Some(0), ..Default::default() },
        )]);

        // As this is the first call, we should get the full state, not the update.
        let attributions =
            exec.run_singlethreaded(snapshot_provider.get()).unwrap().unwrap().attributions;
        assert!(attributions.is_some());

        let attributions_vec = attributions.unwrap();
        // It should contain one component, the one we just launched.
        assert_eq!(attributions_vec.len(), 1);
        let new_attrib = attributions_vec.get(0).unwrap();
        let fattribution::AttributionUpdate::Add(added_principal) = new_attrib else {
            panic!("Not a new principal");
        };
        assert_eq!(added_principal.identifier, Some(0));
        assert_eq!(added_principal.principal_type, Some(fattribution::PrincipalType::Runnable));
    }
}
