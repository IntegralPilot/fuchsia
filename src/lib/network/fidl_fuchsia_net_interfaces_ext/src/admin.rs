// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for fuchsia.net.interfaces.admin.

use flex_fuchsia_net_interfaces as fnet_interfaces;
use flex_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use flex_fuchsia_net_resources as fnet_resources;
use futures::{Future, FutureExt as _, Stream, StreamExt as _, TryStreamExt as _};
use thiserror::Error;

/// Error type when using a [`fnet_interfaces_admin::AddressStateProviderProxy`].
#[derive(Error, Debug)]
pub enum AddressStateProviderError {
    /// Address removed error.
    #[error("address removed: {0:?}")]
    AddressRemoved(fnet_interfaces_admin::AddressRemovalReason),
    /// FIDL error.
    #[error("fidl error")]
    Fidl(#[from] fidl::Error),
    /// Channel closed.
    #[error("AddressStateProvider channel closed")]
    ChannelClosed,
}

impl From<TerminalError<fnet_interfaces_admin::AddressRemovalReason>>
    for AddressStateProviderError
{
    fn from(e: TerminalError<fnet_interfaces_admin::AddressRemovalReason>) -> Self {
        match e {
            TerminalError::Fidl(e) => AddressStateProviderError::Fidl(e),
            TerminalError::Terminal(r) => AddressStateProviderError::AddressRemoved(r),
        }
    }
}

/// Waits for the `OnAddressAdded` event to be received on the event stream.
///
/// Returns an error if an address removed event is received instead.
pub async fn wait_for_address_added_event(
    event_stream: &mut fnet_interfaces_admin::AddressStateProviderEventStream,
) -> Result<(), AddressStateProviderError> {
    let event = event_stream
        .next()
        .await
        .ok_or(AddressStateProviderError::ChannelClosed)?
        .map_err(AddressStateProviderError::Fidl)?;
    match event {
        fnet_interfaces_admin::AddressStateProviderEvent::OnAddressAdded {} => Ok(()),
        fnet_interfaces_admin::AddressStateProviderEvent::OnAddressRemoved { error } => {
            Err(AddressStateProviderError::AddressRemoved(error))
        }
    }
}

// TODO(https://fxbug.dev/42162477): Introduce type with better concurrency safety
// for hanging gets.
/// Returns a stream of assignment states obtained by watching on `address_state_provider`.
///
/// Note that this function calls the hanging get FIDL method
/// [`AddressStateProviderProxy::watch_address_assignment_state`] internally,
/// which means that this stream should not be polled concurrently with any
/// logic which calls the same hanging get. This also means that callers should
/// be careful not to drop the returned stream when it has been polled but yet
/// to yield an item, e.g. due to a timeout or if using select with another
/// stream, as doing so causes a pending hanging get to get lost, and may cause
/// future hanging get calls to fail or the channel to be closed.
pub fn assignment_state_stream(
    address_state_provider: fnet_interfaces_admin::AddressStateProviderProxy,
) -> impl Stream<Item = Result<fnet_interfaces::AddressAssignmentState, AddressStateProviderError>>
{
    let event_fut = address_state_provider
        .take_event_stream()
        .filter_map(|event| {
            futures::future::ready(match event {
                Ok(event) => match event {
                    fnet_interfaces_admin::AddressStateProviderEvent::OnAddressAdded {} => None,
                    fnet_interfaces_admin::AddressStateProviderEvent::OnAddressRemoved {
                        error,
                    } => Some(AddressStateProviderError::AddressRemoved(error)),
                },
                Err(e) => Some(AddressStateProviderError::Fidl(e)),
            })
        })
        .into_future()
        .map(|(event, _stream)| event.unwrap_or(AddressStateProviderError::ChannelClosed));
    futures::stream::try_unfold(
        (address_state_provider, event_fut),
        |(address_state_provider, event_fut)| {
            // NB: Rely on the fact that select always polls the left future
            // first to guarantee that if a terminal event was yielded by the
            // right future, then we don't have an assignment state to emit to
            // clients.
            futures::future::select(
                address_state_provider.watch_address_assignment_state(),
                event_fut,
            )
            .then(|s| match s {
                futures::future::Either::Left((state_result, event_fut)) => match state_result {
                    Ok(state) => {
                        futures::future::ok(Some((state, (address_state_provider, event_fut))))
                            .left_future()
                    }
                    Err(e) if e.is_closed() => event_fut.map(Result::Err).right_future(),
                    Err(e) => {
                        futures::future::err(AddressStateProviderError::Fidl(e)).left_future()
                    }
                },
                futures::future::Either::Right((error, _state_fut)) => {
                    futures::future::err(error).left_future()
                }
            })
        },
    )
}

// TODO(https://fxbug.dev/42162477): Introduce type with better concurrency safety
// for hanging gets.
/// Wait until the Assigned state is observed on `stream`.
///
/// After this async function resolves successfully, the underlying
/// `AddressStateProvider` may be used as usual. If an error is returned, a
/// terminal error has occurred on the underlying channel.
pub async fn wait_assignment_state<S>(
    stream: S,
    want: fnet_interfaces::AddressAssignmentState,
) -> Result<(), AddressStateProviderError>
where
    S: Stream<Item = Result<fnet_interfaces::AddressAssignmentState, AddressStateProviderError>>
        + Unpin,
{
    stream
        .try_filter_map(|state| futures::future::ok((state == want).then_some(())))
        .try_next()
        .await
        .and_then(|opt| opt.ok_or_else(|| AddressStateProviderError::ChannelClosed))
}

type ControlEventStreamFutureToReason =
    fn(
        (
            Option<Result<fnet_interfaces_admin::ControlEvent, fidl::Error>>,
            fnet_interfaces_admin::ControlEventStream,
        ),
    ) -> Result<Option<fnet_interfaces_admin::InterfaceRemovedReason>, fidl::Error>;

/// Convert [`fnet_resources::GrantForInterfaceAuthorization`] to
/// [`fnet_resources::ProofOfInterfaceAuthorization`] with fewer
/// permissions.
///
/// # Panics
///
/// Panics when the Event handle does not have the DUPLICATE right. Callers
/// need not worry about this if providing a grant received from
/// [`GetAuthorizationForInterface`].
#[cfg(not(feature = "fdomain"))]
pub fn proof_from_grant(
    grant: &fnet_resources::GrantForInterfaceAuthorization,
) -> fnet_resources::ProofOfInterfaceAuthorization {
    let fnet_resources::GrantForInterfaceAuthorization { interface_id, token } = grant;

    // The handle duplication is expected to succeed since the input
    // `GrantFromInterfaceAuthorization` is retrieved directly from FIDL and has
    // `zx::Rights::DUPLICATE`. Failure may occur if memory is limited, but this
    // problem cannot be easily resolved via userspace.
    fnet_resources::ProofOfInterfaceAuthorization {
        interface_id: *interface_id,
        token: token.duplicate_handle(fidl::Rights::TRANSFER).unwrap(),
    }
}

/// A wrapper for fuchsia.net.interfaces.admin/Control that observes terminal
/// events.
#[derive(Clone)]
pub struct Control {
    proxy: fnet_interfaces_admin::ControlProxy,
    // Keeps a shared future that will resolve when the first event is seen on a
    // ControlEventStream. The shared future makes the observed terminal event
    // "sticky" for as long as we clone the future before polling it. Note that
    // we don't drive the event stream to completion, the future is resolved
    // when the first event is seen. That means this relies on the terminal
    // event contract but does *not* enforce that the channel is closed
    // immediately after or that no other events are issued.
    terminal_event_fut: futures::future::Shared<
        futures::future::Map<
            futures::stream::StreamFuture<fnet_interfaces_admin::ControlEventStream>,
            ControlEventStreamFutureToReason,
        >,
    >,
}

/// Waits for response on query result and terminal event. If the query has a
/// result, returns that. Otherwise, returns the terminal event.
async fn or_terminal_event<QR, QF, TR>(
    query_fut: QF,
    terminal_event_fut: TR,
) -> Result<QR, TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>>
where
    QR: Unpin,
    QF: Unpin + Future<Output = Result<QR, fidl::Error>>,
    TR: Unpin
        + Future<Output = Result<Option<fnet_interfaces_admin::InterfaceRemovedReason>, fidl::Error>>,
{
    match futures::future::select(query_fut, terminal_event_fut).await {
        futures::future::Either::Left((query_result, terminal_event_fut)) => match query_result {
            Ok(ok) => Ok(ok),
            Err(e) if e.is_closed() => match terminal_event_fut.await {
                Ok(Some(reason)) => Err(TerminalError::Terminal(reason)),
                Ok(None) | Err(_) => Err(TerminalError::Fidl(e)),
            },
            Err(e) => Err(TerminalError::Fidl(e)),
        },
        futures::future::Either::Right((event, query_fut)) => {
            // We need to poll the query response future one more time,
            // because of the following scenario:
            //
            // 1. select() polls the query response future, which returns
            //    pending.
            // 2. The server sends the query response and terminal event in
            //    that order.
            // 3. The FIDL client library dequeues both of these and wakes
            //    the respective futures.
            // 4. select() polls the terminal event future, which is now
            //    ready.
            //
            // In that case, both futures will be ready, so we can use
            // now_or_never() to check whether the query result future has a
            // result, since we always want to process that result first.
            if let Some(query_result) = query_fut.now_or_never() {
                match query_result {
                    Ok(ok) => Ok(ok),
                    Err(e) if e.is_closed() => match event {
                        Ok(Some(reason)) => Err(TerminalError::Terminal(reason)),
                        Ok(None) | Err(_) => Err(TerminalError::Fidl(e)),
                    },
                    Err(e) => Err(TerminalError::Fidl(e)),
                }
            } else {
                match event.map_err(|e| TerminalError::Fidl(e))? {
                    Some(removal_reason) => Err(TerminalError::Terminal(removal_reason)),
                    None => Err(TerminalError::Fidl(fidl::Error::ClientChannelClosed {
                        epitaph: fidl::Epitaph::PeerClosed,
                        protocol_name: <fnet_interfaces_admin::ControlMarker as flex_client::fidl::ProtocolMarker>::DEBUG_NAME,
                        #[cfg(not(target_os = "fuchsia"))]
                        reason: None,
                    })),
                }
            }
        }
    }
}

impl Control {
    /// Calls `AddAddress` on the proxy.
    pub fn add_address(
        &self,
        address: &flex_fuchsia_net::Subnet,
        parameters: &fnet_interfaces_admin::AddressParameters,
        address_state_provider: flex_client::fidl::ServerEnd<
            fnet_interfaces_admin::AddressStateProviderMarker,
        >,
    ) -> Result<(), TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>> {
        self.or_terminal_event_no_return(self.proxy.add_address(
            address,
            parameters,
            address_state_provider,
        ))
    }

    /// Calls `GetId` on the proxy.
    pub async fn get_id(
        &self,
    ) -> Result<u64, TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>> {
        self.or_terminal_event(self.proxy.get_id()).await
    }

    /// Calls `RemoveAddress` on the proxy.
    pub async fn remove_address(
        &self,
        address: &flex_fuchsia_net::Subnet,
    ) -> Result<
        fnet_interfaces_admin::ControlRemoveAddressResult,
        TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>,
    > {
        self.or_terminal_event(self.proxy.remove_address(address)).await
    }

    /// Calls `SetConfiguration` on the proxy.
    pub async fn set_configuration(
        &self,
        config: &fnet_interfaces_admin::Configuration,
    ) -> Result<
        fnet_interfaces_admin::ControlSetConfigurationResult,
        TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>,
    > {
        self.or_terminal_event(self.proxy.set_configuration(config)).await
    }

    /// Calls `GetConfiguration` on the proxy.
    pub async fn get_configuration(
        &self,
    ) -> Result<
        fnet_interfaces_admin::ControlGetConfigurationResult,
        TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>,
    > {
        self.or_terminal_event(self.proxy.get_configuration()).await
    }

    /// Calls `GetAuthorizationForInterface` on the proxy.
    pub async fn get_authorization_for_interface(
        &self,
    ) -> Result<
        fnet_resources::GrantForInterfaceAuthorization,
        TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>,
    > {
        self.or_terminal_event(self.proxy.get_authorization_for_interface()).await
    }

    /// Calls `Enable` on the proxy.
    pub async fn enable(
        &self,
    ) -> Result<
        fnet_interfaces_admin::ControlEnableResult,
        TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>,
    > {
        self.or_terminal_event(self.proxy.enable()).await
    }

    /// Calls `Remove` on the proxy.
    pub async fn remove(
        &self,
    ) -> Result<
        fnet_interfaces_admin::ControlRemoveResult,
        TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>,
    > {
        self.or_terminal_event(self.proxy.remove()).await
    }

    /// Calls `Disable` on the proxy.
    pub async fn disable(
        &self,
    ) -> Result<
        fnet_interfaces_admin::ControlDisableResult,
        TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>,
    > {
        self.or_terminal_event(self.proxy.disable()).await
    }

    /// Calls `Detach` on the proxy.
    pub fn detach(
        &self,
    ) -> Result<(), TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>> {
        self.or_terminal_event_no_return(self.proxy.detach())
    }

    /// Creates a new `Control` wrapper from `proxy`.
    pub fn new(proxy: fnet_interfaces_admin::ControlProxy) -> Self {
        let terminal_event_fut = proxy
            .take_event_stream()
            .into_future()
            .map::<_, ControlEventStreamFutureToReason>(|(event, _stream)| {
                event
                    .map(|r| {
                        r.map(
                            |fnet_interfaces_admin::ControlEvent::OnInterfaceRemoved { reason }| {
                                reason
                            },
                        )
                    })
                    .transpose()
            })
            .shared();
        Self { proxy, terminal_event_fut }
    }

    /// Waits for interface removal.
    pub async fn wait_termination(
        self,
    ) -> TerminalError<fnet_interfaces_admin::InterfaceRemovedReason> {
        let Self { proxy: _, terminal_event_fut } = self;
        match terminal_event_fut.await {
            Ok(Some(event)) => TerminalError::Terminal(event),
            Ok(None) => TerminalError::Fidl(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::PeerClosed,
                protocol_name: <fnet_interfaces_admin::ControlMarker as flex_client::fidl::ProtocolMarker>::DEBUG_NAME,
                #[cfg(not(target_os = "fuchsia"))]
                reason: None,
            }),
            Err(e) => TerminalError::Fidl(e),
        }
    }

    /// Creates a new `Control` and its `ServerEnd`.
    #[cfg(not(feature = "fdomain"))]
    pub fn create_endpoints()
    -> Result<(Self, fidl::endpoints::ServerEnd<fnet_interfaces_admin::ControlMarker>), fidl::Error>
    {
        let (proxy, server_end) = fidl::endpoints::create_proxy();
        Ok((Self::new(proxy), server_end))
    }

    async fn or_terminal_event<R: Unpin, F: Unpin + Future<Output = Result<R, fidl::Error>>>(
        &self,
        fut: F,
    ) -> Result<R, TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>> {
        or_terminal_event(fut, self.terminal_event_fut.clone()).await
    }

    fn or_terminal_event_no_return(
        &self,
        r: Result<(), fidl::Error>,
    ) -> Result<(), TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>> {
        r.map_err(|err| {
            if !err.is_closed() {
                return TerminalError::Fidl(err);
            }
            // TODO(https://fxbug.dev/42178907): The terminal event may have been
            // sent by the server but the future may not resolve immediately,
            // resulting in the terminal event being missed and a FIDL error
            // being returned to the user.
            //
            // Poll event stream to see if we have a terminal event to return
            // instead of a FIDL closed error.
            match self.terminal_event_fut.clone().now_or_never() {
                Some(Ok(Some(terminal_event))) => TerminalError::Terminal(terminal_event),
                Some(Err(e)) => {
                    // Prefer the error observed by the proxy.
                    let _: fidl::Error = e;
                    TerminalError::Fidl(err)
                }
                None | Some(Ok(None)) => TerminalError::Fidl(err),
            }
        })
    }
}

impl std::fmt::Debug for Control {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { proxy, terminal_event_fut: _ } = self;
        fmt.debug_struct("Control").field("proxy", proxy).finish()
    }
}

/// Errors observed from wrapped terminal events.
#[derive(Debug)]
pub enum TerminalError<E> {
    /// Terminal event was observed.
    Terminal(E),
    /// A FIDL error occurred.
    Fidl(fidl::Error),
}

impl<E> std::fmt::Display for TerminalError<E>
where
    E: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerminalError::Terminal(e) => write!(f, "terminal event: {:?}", e),
            TerminalError::Fidl(e) => write!(f, "fidl error: {}", e),
        }
    }
}

impl<E: std::fmt::Debug> std::error::Error for TerminalError<E> {}

/// This can be provided on interface creation to appoint a route table into
/// which netstack managed routes are installed.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetstackManagedRoutesDesignation {
    /// The netstack managed routes are installed in the main table.
    #[default]
    Main,
    /// The netstack managed routes are installed in an interface-local table.
    ///
    /// The interface creates local tables (one for each IP version). When the
    /// interface is removed and all the outstanding `RouteTableV{4,6}` protocol
    /// channels are closed, the local table is removed.
    InterfaceLocal,
}

/// Unknown FIDL value for NetstackManagedRoutesDesignation.
#[derive(Error, Debug)]
#[error("unknown designation for netsack managed routes: {0}")]
pub struct UnknownNetstackManagedRoutesDesignation(pub u64);

impl TryFrom<fnet_interfaces_admin::NetstackManagedRoutesDesignation>
    for NetstackManagedRoutesDesignation
{
    type Error = UnknownNetstackManagedRoutesDesignation;

    fn try_from(
        value: fnet_interfaces_admin::NetstackManagedRoutesDesignation,
    ) -> Result<Self, Self::Error> {
        match value {
            fnet_interfaces_admin::NetstackManagedRoutesDesignation::Main(
                fnet_interfaces_admin::Empty,
            ) => Ok(Self::Main),
            fnet_interfaces_admin::NetstackManagedRoutesDesignation::InterfaceLocal(
                fnet_interfaces_admin::Empty,
            ) => Ok(Self::InterfaceLocal),
            fnet_interfaces_admin::NetstackManagedRoutesDesignation::__SourceBreaking {
                unknown_ordinal,
            } => Err(UnknownNetstackManagedRoutesDesignation(unknown_ordinal)),
        }
    }
}

impl From<NetstackManagedRoutesDesignation>
    for fnet_interfaces_admin::NetstackManagedRoutesDesignation
{
    fn from(value: NetstackManagedRoutesDesignation) -> Self {
        match value {
            NetstackManagedRoutesDesignation::Main => Self::Main(fnet_interfaces_admin::Empty),
            NetstackManagedRoutesDesignation::InterfaceLocal => {
                Self::InterfaceLocal(fnet_interfaces_admin::Empty)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::task::Poll;

    use flex_client::fidl::{ProtocolMarker, RequestStream};
    use flex_fuchsia_net_interfaces as fnet_interfaces;
    use flex_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
    use fnet_interfaces_admin::InterfaceRemovedReason;
    use test_case::test_case;

    use super::*;

    // Test that the terminal event is observed when the server closes its end.
    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_assignment_state_stream() {
        let client = flex_local::local_client_empty();
        let (address_state_provider, server_end) =
            client.create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();
        let state_stream = assignment_state_stream(address_state_provider);
        futures::pin_mut!(state_stream);

        const REMOVAL_REASON_INVALID: fnet_interfaces_admin::AddressRemovalReason =
            fnet_interfaces_admin::AddressRemovalReason::Invalid;
        {
            let (mut request_stream, control_handle) = server_end.into_stream_and_control_handle();

            const ASSIGNMENT_STATE_ASSIGNED: fnet_interfaces::AddressAssignmentState =
                fnet_interfaces::AddressAssignmentState::Assigned;
            let state_fut = state_stream.try_next().map(|r| {
                assert_eq!(
                    r.expect("state stream error").expect("state stream ended"),
                    ASSIGNMENT_STATE_ASSIGNED
                )
            });
            let handle_fut = request_stream.try_next().map(|r| match r.expect("request stream error").expect("request stream ended") {
                fnet_interfaces_admin::AddressStateProviderRequest::WatchAddressAssignmentState { responder } => {
                    responder.send(ASSIGNMENT_STATE_ASSIGNED).expect("failed to send stubbed assignment state");
                }
                req => panic!("unexpected method called: {:?}", req),
            });
            let ((), ()) = futures::join!(state_fut, handle_fut);

            control_handle
                .send_on_address_removed(REMOVAL_REASON_INVALID)
                .expect("failed to send fake INVALID address removal reason event");
        }

        assert_matches::assert_matches!(
            state_stream.try_collect::<Vec<_>>().await,
            Err(AddressStateProviderError::AddressRemoved(got)) if got == REMOVAL_REASON_INVALID
        );
    }

    // Test that only one error is returned on the assignment state stream when
    // an error observable on both the client proxy and the event stream occurs.
    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_assignment_state_stream_single_error() {
        let client = flex_local::local_client_empty();
        let (address_state_provider, server_end) =
            client.create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();
        let state_stream = assignment_state_stream(address_state_provider);

        server_end
            .close_with_epitaph(fidl::Status::INTERNAL)
            .expect("failed to send INTERNAL epitaph");

        let states_fut = state_stream.collect::<Vec<_>>();

        #[cfg(not(feature = "fdomain"))]
        let states = states_fut.now_or_never().expect("state stream not immediately ready");

        // FDomain needs to pump requests through its bowels a bit so the stream
        // won't be immediately ready.
        #[cfg(feature = "fdomain")]
        let states = states_fut.await;

        // Use collect rather than try_collect to ensure that we don't observe
        // multiple errors on this stream.
        assert_matches::assert_matches!(
            states.as_slice(),
            [Err(AddressStateProviderError::Fidl(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::Explicit(Err(fidl::Status::INTERNAL)),
                #[cfg(not(target_os = "fuchsia"))]
                reason: None,
                ..
            }))]
        );
    }

    // Test that if an assignment state and a terminal event is available at
    // the same time, the state is yielded first.
    #[fuchsia_async::run_singlethreaded(test)]
    async fn assignment_state_stream_state_before_event() {
        let client = flex_local::local_client_empty();
        let (address_state_provider, mut request_stream) =
            client.create_proxy_and_stream::<fnet_interfaces_admin::AddressStateProviderMarker>();

        const ASSIGNMENT_STATE_ASSIGNED: fnet_interfaces::AddressAssignmentState =
            fnet_interfaces::AddressAssignmentState::Assigned;
        const REMOVAL_REASON_INVALID: fnet_interfaces_admin::AddressRemovalReason =
            fnet_interfaces_admin::AddressRemovalReason::Invalid;

        let ((), ()) = futures::future::join(
            async move {
                request_stream
                    .try_next()
                    .await
                    .expect("request stream error")
                    .expect("request stream ended")
                    .into_watch_address_assignment_state()
                    .expect("unexpected request")
                    .send(ASSIGNMENT_STATE_ASSIGNED)
                    .expect("failed to send stubbed assignment state");
                request_stream
                    .control_handle()
                    .send_on_address_removed(REMOVAL_REASON_INVALID)
                    .expect("failed to send fake INVALID address removal reason event");
            },
            async move {
                let got = assignment_state_stream(address_state_provider).collect::<Vec<_>>().await;
                assert_matches::assert_matches!(
                    got.as_slice(),
                    &[
                        Ok(got_state),
                        Err(AddressStateProviderError::AddressRemoved(got_reason)),
                    ] => {
                        assert_eq!(got_state, ASSIGNMENT_STATE_ASSIGNED);
                        assert_eq!(got_reason, REMOVAL_REASON_INVALID);
                    }
                );
            },
        )
        .await;
    }

    // Tests that terminal event is observed when using ControlWrapper.
    #[fuchsia_async::run_singlethreaded(test)]
    async fn control_terminal_event() {
        let client = flex_local::local_client_empty();
        let (control, mut request_stream) =
            client.create_proxy_and_stream::<fnet_interfaces_admin::ControlMarker>();
        let control = super::Control::new(control);
        const EXPECTED_EVENT: fnet_interfaces_admin::InterfaceRemovedReason =
            fnet_interfaces_admin::InterfaceRemovedReason::BadPort;
        const ID: u64 = 15;
        let ((), ()) = futures::future::join(
            async move {
                assert_matches::assert_matches!(control.get_id().await, Ok(ID));
                assert_matches::assert_matches!(
                    control.get_id().await,
                    Err(super::TerminalError::Terminal(got)) if got == EXPECTED_EVENT
                );
            },
            async move {
                let responder = request_stream
                    .try_next()
                    .await
                    .expect("operating request stream")
                    .expect("stream ended unexpectedly")
                    .into_get_id()
                    .expect("unexpected request");
                responder.send(ID).expect("failed to send response");
                request_stream
                    .control_handle()
                    .send_on_interface_removed(EXPECTED_EVENT)
                    .expect("sending terminal event");
            },
        )
        .await;
    }

    // Tests that terminal error is observed when using ControlWrapper if no
    // event is issued.
    #[fuchsia_async::run_singlethreaded(test)]
    async fn control_missing_terminal_event() {
        let client = flex_local::local_client_empty();
        let (control, mut request_stream) =
            client.create_proxy_and_stream::<fnet_interfaces_admin::ControlMarker>();
        let control = super::Control::new(control);
        let ((), ()) = futures::future::join(
            async move {
                assert_matches::assert_matches!(
                    control.get_id().await,
                    Err(super::TerminalError::Fidl(fidl::Error::ClientChannelClosed {
                        epitaph: fidl::Epitaph::PeerClosed,
                        protocol_name: flex_fuchsia_net_interfaces_admin::ControlMarker::DEBUG_NAME,
                        #[cfg(not(target_os = "fuchsia"))]
                        reason: None,
                        ..
                    }))
                );
            },
            async move {
                match request_stream
                    .try_next()
                    .await
                    .expect("operating request stream")
                    .expect("stream ended unexpectedly")
                {
                    fnet_interfaces_admin::ControlRequest::GetId { responder } => {
                        // Just close the channel without issuing a response.
                        std::mem::drop(responder);
                    }
                    request => panic!("unexpected request {:?}", request),
                }
            },
        )
        .await;
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn control_pipelined_error() {
        let client = flex_local::local_client_empty();
        let (control, request_stream) =
            client.create_proxy_and_stream::<fnet_interfaces_admin::ControlMarker>();
        let control = super::Control::new(control);
        const CLOSE_REASON: fnet_interfaces_admin::InterfaceRemovedReason =
            fnet_interfaces_admin::InterfaceRemovedReason::BadPort;
        request_stream
            .control_handle()
            .send_on_interface_removed(CLOSE_REASON)
            .expect("send terminal event");
        std::mem::drop(request_stream);
        #[cfg(feature = "fdomain")]
        {
            let control_clone = control.clone();
            let _ = control_clone.wait_termination().await;
        }
        assert_matches::assert_matches!(control.or_terminal_event_no_return(Ok(())), Ok(()));
        assert_matches::assert_matches!(
            control.or_terminal_event_no_return(Err(fidl::Error::ClientWrite(
                zx_status::Status::INTERNAL.into()
            ))),
            Err(super::TerminalError::Fidl(fidl::Error::ClientWrite(
                fidl::TransportError::Status(zx_status::Status::INTERNAL)
            )))
        );
        #[cfg(target_os = "fuchsia")]
        assert_matches::assert_matches!(
            control.or_terminal_event_no_return(Err(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::PeerClosed,
                protocol_name: fnet_interfaces_admin::ControlMarker::DEBUG_NAME,
            })),
            Err(super::TerminalError::Terminal(CLOSE_REASON))
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn control_wait_termination() {
        let client = flex_local::local_client_empty();
        let (control, request_stream) =
            client.create_proxy_and_stream::<fnet_interfaces_admin::ControlMarker>();
        let control = super::Control::new(control);
        const CLOSE_REASON: fnet_interfaces_admin::InterfaceRemovedReason =
            fnet_interfaces_admin::InterfaceRemovedReason::BadPort;
        request_stream
            .control_handle()
            .send_on_interface_removed(CLOSE_REASON)
            .expect("send terminal event");
        std::mem::drop(request_stream);
        assert_matches::assert_matches!(
            control.wait_termination().await,
            super::TerminalError::Terminal(CLOSE_REASON)
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn control_respond_and_drop() {
        const ID: u64 = 15;
        let client = flex_local::local_client_empty();
        let (control, mut request_stream) =
            client.create_proxy_and_stream::<fnet_interfaces_admin::ControlMarker>();
        let control = super::Control::new(control);
        let ((), ()) = futures::future::join(
            async move {
                assert_matches::assert_matches!(control.get_id().await, Ok(ID));
            },
            async move {
                let responder = request_stream
                    .try_next()
                    .await
                    .expect("operating request stream")
                    .expect("stream ended unexpectedly")
                    .into_get_id()
                    .expect("unexpected request");
                responder.send(ID).expect("failed to send response");
            },
        )
        .await;
    }

    // This test is for the case found in https://fxbug.dev/328297563.  The
    // query result and terminal event futures both become ready after the query
    // result is polled and returns pending. This test does not handle the case
    // for when there is no query result.
    #[test_case(Ok(()), Ok(Some(InterfaceRemovedReason::User)), Ok(()); "success")]
    #[test_case(
        Err(fidl::Error::InvalidHeader),
        Ok(Some(InterfaceRemovedReason::User)),
        Err(TerminalError::Fidl(fidl::Error::InvalidHeader));
        "returns query error when not closed"
    )]
    #[test_case(
        Err(fidl::Error::ClientChannelClosed {
            epitaph: fidl::Epitaph::PeerClosed,
            protocol_name: fnet_interfaces_admin::ControlMarker::DEBUG_NAME,
            #[cfg(not(target_os = "fuchsia"))]
            reason: None,
        }),
        Ok(Some(InterfaceRemovedReason::User)),
        Err(TerminalError::Terminal(InterfaceRemovedReason::User));
        "returns terminal error when channel closed"
    )]
    #[test_case(
        Err(fidl::Error::ClientChannelClosed {
            epitaph: fidl::Epitaph::PeerClosed,
            protocol_name: fnet_interfaces_admin::ControlMarker::DEBUG_NAME,
            #[cfg(not(target_os = "fuchsia"))]
            reason: None,
        }),
        Ok(None),
        Err(TerminalError::Fidl(
            fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::PeerClosed,
                protocol_name: fnet_interfaces_admin::ControlMarker::DEBUG_NAME,
                #[cfg(not(target_os = "fuchsia"))]
                reason: None,
            }
        ));
        "returns query error when no terminal error"
    )]
    #[test_case(
        Err(fidl::Error::ClientChannelClosed {
            epitaph: fidl::Epitaph::PeerClosed,
            protocol_name: fnet_interfaces_admin::ControlMarker::DEBUG_NAME,
            #[cfg(not(target_os = "fuchsia"))]
            reason: None,
        }),
        Err(fidl::Error::InvalidHeader),
        Err(TerminalError::Fidl(
            fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::PeerClosed,
                protocol_name: fnet_interfaces_admin::ControlMarker::DEBUG_NAME,
                #[cfg(not(target_os = "fuchsia"))]
                reason: None,
            }
        ));
        "returns query error when terminal event returns a fidl error"
    )]
    #[fuchsia_async::run_singlethreaded(test)]
    async fn control_polling_race(
        left_future_result: Result<(), fidl::Error>,
        right_future_result: Result<
            Option<fnet_interfaces_admin::InterfaceRemovedReason>,
            fidl::Error,
        >,
        expected: Result<(), TerminalError<fnet_interfaces_admin::InterfaceRemovedReason>>,
    ) {
        let mut polled = false;
        let first_future = std::future::poll_fn(|_cx| {
            if polled {
                Poll::Ready(left_future_result.clone())
            } else {
                polled = true;
                Poll::Pending
            }
        })
        .fuse();

        let second_future =
            std::future::poll_fn(|_cx| Poll::Ready(right_future_result.clone())).fuse();

        let res = or_terminal_event(first_future, second_future).await;
        match (res, expected) {
            (Ok(()), Ok(())) => (),
            (Err(TerminalError::Terminal(res)), Err(TerminalError::Terminal(expected)))
                if res == expected => {}
            // fidl::Error doesn't implement Eq, but this lack of an actual
            // equality check does not matter for this test.
            (Err(TerminalError::Fidl(_)), Err(TerminalError::Fidl(_))) => (),
            (res, expected) => panic!("expected {:?} got {:?}", expected, res),
        }
    }

    #[cfg(not(feature = "fdomain"))]
    #[test]
    fn convert_proof_to_grant() {
        use assert_matches::assert_matches;
        // I don't know why we need this unused. The trait is definitely used
        // and this function is only in the non-FDomain variant so we shouldn't
        // be building an FDomain version.
        #[allow(unused)]
        use fidl::{AsHandleRef, Rights};
        // The default Event has more Rights than the token within the Grant returned from
        // [`GetAuthorizationForInterface`], but can still be converted to be used in the
        // [`ProofOfInterfaceAuthorization`], since only `zx::Rights::DUPLICATE` and
        // `zx::Rights::TRANSFER` is required.
        let event = fidl::Event::create();
        let grant = fnet_resources::GrantForInterfaceAuthorization {
            interface_id: Default::default(),
            token: event,
        };

        let fnet_resources::ProofOfInterfaceAuthorization { interface_id, token } =
            proof_from_grant(&grant);
        assert_eq!(interface_id, Default::default());
        assert_matches!(token.basic_info(), Ok(info) if info.rights == Rights::TRANSFER);
    }
}
