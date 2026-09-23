// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Wrapper types for the endpoints of a connection.

use crate::epitaph::ChannelEpitaphExt;
use crate::{
    AsHandleRef, AsyncChannel, Channel, Epitaph, Error, HandleRef, NullableHandle, OnSignalsRef,
    ServeInner,
};
use futures::{Stream, TryStream};
use std::marker::PhantomData;
use std::sync::Arc;

/// A marker for a particular FIDL protocol.
///
/// Implementations of this trait can be used to manufacture instances of a FIDL
/// protocol and get metadata about a particular protocol.
pub trait ProtocolMarker: Sized + Send + Sync + 'static {
    /// The type of the structure against which FIDL requests are made.
    /// Queries made against the proxy are sent to the paired `ServerEnd`.
    type Proxy: Proxy<Protocol = Self>;

    /// The type of the structure against which thread-blocking FIDL requests are made.
    /// Queries made against the proxy are sent to the paired `ServerEnd`.
    #[cfg(target_os = "fuchsia")]
    type SynchronousProxy: SynchronousProxy<Protocol = Self>;

    /// The type of the stream of requests coming into a server.
    type RequestStream: RequestStream<Protocol = Self>;

    /// The name of the protocol suitable for debug purposes.
    ///
    /// For discoverable protocols, this should be identical to
    /// `<Self as DiscoverableProtocolMarker>::PROTOCOL_NAME`.
    const DEBUG_NAME: &'static str;
}

/// A marker for a particular FIDL protocol that is also discoverable.
///
/// Discoverable protocols may be referred to by a string name, and can be
/// conveniently exported in a service directory via an entry of that name.
///
/// If you get an error about this trait not being implemented, you probably
/// need to add the `@discoverable` attribute to the FIDL protocol, like this:
///
/// ```fidl
/// @discoverable
/// protocol MyProtocol { ... };
/// ```
pub trait DiscoverableProtocolMarker: ProtocolMarker {
    /// The name of the protocol (to be used for service lookup and discovery).
    const PROTOCOL_NAME: &'static str = <Self as ProtocolMarker>::DEBUG_NAME;
}

/// A type which allows querying a remote FIDL server over a channel.
pub trait Proxy: Sized + Send + Sync {
    /// The protocol which this `Proxy` controls.
    type Protocol: ProtocolMarker<Proxy = Self>;

    /// Create a proxy over the given channel.
    fn from_channel(inner: AsyncChannel) -> Self;

    /// Attempt to convert the proxy back into a channel.
    ///
    /// This will only succeed if there are no active clones of this proxy
    /// and no currently-alive `EventStream` or response futures that came from
    /// this proxy.
    fn into_channel(self) -> Result<AsyncChannel, Self>;

    /// Attempt to convert the proxy back into a client end.
    ///
    /// This will only succeed if there are no active clones of this proxy
    /// and no currently-alive `EventStream` or response futures that came from
    /// this proxy.
    fn into_client_end(self) -> Result<ClientEnd<Self::Protocol>, Self> {
        match self.into_channel() {
            Ok(channel) => Ok(ClientEnd::new(channel.into_zx_channel())),
            Err(proxy) => Err(proxy),
        }
    }

    /// Get a reference to the proxy's underlying channel.
    ///
    /// This should only be used for non-effectful operations. Reading or
    /// writing to the channel is unsafe because the proxy assumes it has
    /// exclusive control over these operations.
    fn as_channel(&self) -> &AsyncChannel;

    /// Returns true if the proxy has received the `PEER_CLOSED` signal.
    fn is_closed(&self) -> bool {
        self.as_channel().is_closed()
    }

    /// Returns a future that completes when the proxy receives the
    /// `PEER_CLOSED` signal.
    fn on_closed(&self) -> OnSignalsRef<'_> {
        self.as_channel().on_closed()
    }
}

/// This gives native Zircon proxies a domain method like FDomain proxies have.
/// This makes it easier in some cases to build the same code for both FDomain
/// and regular FIDL.
pub trait ProxyHasDomain {
    /// Get a "client" for this proxy. This is just an object which has methods
    /// for a few common handle creation operations.
    fn domain(&self) -> ZirconClient {
        ZirconClient
    }
}

impl<T: Proxy> ProxyHasDomain for T {}

/// The fake "client" produced by `ProxyHasDomain`. Analogous to an FDomain client.
pub struct ZirconClient;

impl ZirconClient {
    /// Equivalent to [`EventPair::create`]
    pub fn create_event_pair(&self) -> (crate::EventPair, crate::EventPair) {
        crate::EventPair::create()
    }

    /// Equivalent to [`Event::create`]
    pub fn create_event(&self) -> crate::Event {
        crate::Event::create()
    }

    /// Equivalent to [`Socket::create_stream`]
    pub fn create_stream_socket(&self) -> (crate::Socket, crate::Socket) {
        crate::Socket::create_stream()
    }

    /// Equivalent to [`Socket::create_datagram`]
    pub fn create_datagram_socket(&self) -> (crate::Socket, crate::Socket) {
        crate::Socket::create_datagram()
    }

    /// Equivalent to [`Channel::create`]
    pub fn create_channel(&self) -> (Channel, Channel) {
        Channel::create()
    }

    /// Equivalent to the module level [`create_endpoints`]
    pub fn create_endpoints<T: ProtocolMarker>(&self) -> (ClientEnd<T>, ServerEnd<T>) {
        create_endpoints::<T>()
    }

    /// Equivalent to the module level [`create_proxy`]
    pub fn create_proxy<T: ProtocolMarker>(&self) -> (T::Proxy, ServerEnd<T>) {
        create_proxy::<T>()
    }

    /// Equivalent to the module level [`create_request_stream`]
    pub fn create_request_stream<T: ProtocolMarker>(&self) -> (ClientEnd<T>, T::RequestStream) {
        create_request_stream::<T>()
    }

    /// Equivalent to the module level [`create_proxy_and_stream`]
    pub fn create_proxy_and_stream<T: ProtocolMarker>(&self) -> (T::Proxy, T::RequestStream) {
        create_proxy_and_stream::<T>()
    }
}

/// A type which allows querying a remote FIDL server over a channel, blocking the calling thread.
#[cfg(target_os = "fuchsia")]
pub trait SynchronousProxy: Sized + Send + Sync {
    /// The async proxy for the same protocol.
    type Proxy: Proxy<Protocol = Self::Protocol>;

    /// The protocol which this `Proxy` controls.
    type Protocol: ProtocolMarker<Proxy = Self::Proxy>;

    /// Create a proxy over the given channel.
    fn from_channel(inner: Channel) -> Self;

    /// Convert the proxy back into a channel.
    fn into_channel(self) -> Channel;

    /// Get a reference to the proxy's underlying channel.
    ///
    /// This should only be used for non-effectful operations. Reading or
    /// writing to the channel is unsafe because the proxy assumes it has
    /// exclusive control over these operations.
    fn as_channel(&self) -> &Channel;

    /// Returns true if the proxy has received the `PEER_CLOSED` signal.
    ///
    /// # Errors
    ///
    /// See https://fuchsia.dev/reference/syscalls/object_wait_one?hl=en#errors for a full list of
    /// errors. Note that `Status::TIMED_OUT` errors are converted to `Ok(false)` and all other
    /// errors are propagated.
    fn is_closed(&self) -> Result<bool, zx::Status> {
        use zx::Peered;
        self.as_channel().is_closed()
    }
}

/// A stream of requests coming into a FIDL server over a channel.
pub trait RequestStream: Sized + Send + Stream + TryStream<Error = crate::Error> + Unpin {
    /// The protocol which this `RequestStream` serves.
    type Protocol: ProtocolMarker<RequestStream = Self>;

    /// The control handle for this `RequestStream`.
    type ControlHandle: ControlHandle;

    /// Returns a copy of the `ControlHandle` for the given stream.
    /// This handle can be used to send events or shut down the request stream.
    fn control_handle(&self) -> Self::ControlHandle;

    /// Create a request stream from the given channel.
    fn from_channel(inner: AsyncChannel) -> Self;

    /// Convert this channel into its underlying components.
    fn into_inner(self) -> (Arc<ServeInner>, bool);

    /// Create this channel from its underlying components.
    fn from_inner(inner: Arc<ServeInner>, is_terminated: bool) -> Self;

    /// Convert this FIDL request stream into a request stream of another FIDL protocol.
    fn cast_stream<T: RequestStream>(self) -> T {
        let inner = self.into_inner();
        T::from_inner(inner.0, inner.1)
    }
}

/// The Request type associated with a Marker.
pub type Request<Marker> = <<Marker as ProtocolMarker>::RequestStream as futures::TryStream>::Ok;

/// A type associated with a `RequestStream` that can be used to send FIDL
/// events or to shut down the request stream.
pub trait ControlHandle {
    /// Set the server to shutdown. The underlying channel is only closed the
    /// next time the stream is polled.
    // TODO(https://fxbug.dev/42161447): Fix behavior or above docs.
    fn shutdown(&self);

    /// Sets the server to shutdown with an epitaph. The underlying channel is
    /// only closed the next time the stream is polled.
    // TODO(https://fxbug.dev/42161447): Fix behavior or above docs.
    fn shutdown_with_epitaph(&self, status: Epitaph);

    /// Returns true if the server has received the `PEER_CLOSED` signal.
    fn is_closed(&self) -> bool;

    /// Returns a future that completes when the server receives the
    /// `PEER_CLOSED` signal.
    fn on_closed(&self) -> OnSignalsRef<'_>;

    /// Sets and clears the signals provided on peer handle.
    #[cfg(target_os = "fuchsia")]
    fn signal_peer(
        &self,
        clear_mask: zx::Signals,
        set_mask: zx::Signals,
    ) -> Result<(), zx_status::Status>;
}

/// A type associated with a particular two-way FIDL method, used by servers to
/// send a response to the client.
pub trait Responder {
    /// The control handle for this protocol.
    type ControlHandle: ControlHandle;

    /// Returns the `ControlHandle` for this protocol.
    fn control_handle(&self) -> &Self::ControlHandle;

    /// Drops the responder without setting the channel to shutdown.
    ///
    /// This method shouldn't normally be used. Instead, send a response to
    /// prevent the channel from shutting down.
    fn drop_without_shutdown(self);
}

/// A marker for a particular FIDL service.
#[cfg(target_os = "fuchsia")]
pub trait ServiceMarker: Clone + Sized + Send + Sync + 'static {
    /// The type of the proxy object upon which calls are made to a remote FIDL service.
    type Proxy: ServiceProxy<Service = Self>;

    /// The request type for this particular FIDL service.
    type Request: ServiceRequest<Service = Self>;

    /// The name of the service. Used for service lookup and discovery.
    const SERVICE_NAME: &'static str;
}

/// A request to initiate a connection to a FIDL service.
#[cfg(target_os = "fuchsia")]
pub trait ServiceRequest: Sized + Send + Sync {
    /// The FIDL service for which this request is destined.
    type Service: ServiceMarker<Request = Self>;

    /// Dispatches a connection attempt to this FIDL service's member protocol
    /// identified by `name`, producing an instance of this trait.
    fn dispatch(name: &str, channel: AsyncChannel) -> Self;

    /// Returns an array of the service members' names.
    fn member_names() -> &'static [&'static str];
}

/// Proxy by which a client sends messages to a FIDL service.
#[cfg(target_os = "fuchsia")]
pub trait ServiceProxy: Sized {
    /// The FIDL service this proxy represents.
    type Service: ServiceMarker<Proxy = Self>;

    /// Create a proxy from a MemberOpener implementation.
    #[doc(hidden)]
    fn from_member_opener(opener: Box<dyn MemberOpener>) -> Self;
}

/// Used to create an indirection between the fuchsia.io.Directory protocol
/// and this library, which cannot depend on fuchsia.io.
#[doc(hidden)]
#[cfg(target_os = "fuchsia")]
pub trait MemberOpener: Send + Sync {
    /// Opens a member protocol of a FIDL service by name, serving that protocol
    /// on the given channel.
    fn open_member(&self, member: &str, server_end: Channel) -> Result<(), Error>;

    /// Returns the name of the instance that was opened.
    fn instance_name(&self) -> &str;
}

/// The `Client` end of a FIDL connection.
#[derive(Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ClientEnd<T> {
    inner: Channel,
    phantom: PhantomData<T>,
}

impl<T> ClientEnd<T> {
    /// Create a new client from the provided channel.
    pub fn new(inner: Channel) -> Self {
        ClientEnd { inner, phantom: PhantomData }
    }

    /// Get a reference to the underlying channel
    pub fn channel(&self) -> &Channel {
        &self.inner
    }

    /// Extract the underlying channel.
    pub fn into_channel(self) -> Channel {
        self.inner
    }
}

impl<T: ProtocolMarker> ClientEnd<T> {
    /// Convert the `ClientEnd` into a `Proxy` through which FIDL calls may be made.
    ///
    /// # Panics
    ///
    /// If called outside the context of an active async executor.
    pub fn into_proxy(self) -> T::Proxy {
        T::Proxy::from_channel(AsyncChannel::from_channel(self.inner))
    }

    /// Convert the `ClientEnd` into a `SynchronousProxy` through which thread-blocking FIDL calls
    /// may be made.
    #[cfg(target_os = "fuchsia")]
    pub fn into_sync_proxy(self) -> T::SynchronousProxy {
        T::SynchronousProxy::from_channel(self.inner)
    }
}

impl<T> AsHandleRef for ClientEnd<T> {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        self.inner.as_handle_ref()
    }
}

impl<T> From<ClientEnd<T>> for NullableHandle {
    fn from(client: ClientEnd<T>) -> NullableHandle {
        client.into_channel().into()
    }
}

impl<T> From<ClientEnd<T>> for Channel {
    fn from(client: ClientEnd<T>) -> Channel {
        client.into_channel()
    }
}

impl<T> From<NullableHandle> for ClientEnd<T> {
    fn from(handle: NullableHandle) -> Self {
        ClientEnd { inner: handle.into(), phantom: PhantomData }
    }
}

impl<T> From<Channel> for ClientEnd<T> {
    fn from(chan: Channel) -> Self {
        ClientEnd { inner: chan, phantom: PhantomData }
    }
}

impl<T: ProtocolMarker> ::std::fmt::Debug for ClientEnd<T> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "ClientEnd(name={}, channel={:?})", T::DEBUG_NAME, self.inner)
    }
}

/// Trait implemented by types that can be converted from a client.
pub trait FromClient {
    /// The protocol.
    type Protocol: ProtocolMarker;

    /// Converts from a client.
    fn from_client(value: ClientEnd<Self::Protocol>) -> Self;
}

impl<T: ProtocolMarker> FromClient for ClientEnd<T> {
    type Protocol = T;

    fn from_client(value: ClientEnd<Self::Protocol>) -> Self {
        value
    }
}

// NOTE: We can only have one blanket implementation. Synchronous proxies have an implementation
// that is generated by the compiler.
impl<T: Proxy> FromClient for T {
    type Protocol = T::Protocol;

    fn from_client(value: ClientEnd<Self::Protocol>) -> Self {
        Self::from_channel(AsyncChannel::from_channel(value.into_channel()))
    }
}

/// The `Server` end of a FIDL connection.
#[derive(Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ServerEnd<T> {
    inner: Channel,
    phantom: PhantomData<T>,
}

impl<T> ServerEnd<T> {
    /// Create a new `ServerEnd` from the provided channel.
    pub fn new(inner: Channel) -> ServerEnd<T> {
        ServerEnd { inner, phantom: PhantomData }
    }

    /// Get a reference to the underlying channel
    pub fn channel(&self) -> &Channel {
        &self.inner
    }

    /// Extract the inner channel.
    pub fn into_channel(self) -> Channel {
        self.inner
    }

    /// Create a stream of requests off of the channel.
    ///
    /// # Panics
    ///
    /// If called outside the context of an active async executor.
    pub fn into_stream(self) -> T::RequestStream
    where
        T: ProtocolMarker,
    {
        T::RequestStream::from_channel(AsyncChannel::from_channel(self.inner))
    }

    /// Create a stream of requests and an event-sending handle
    /// from the channel.
    ///
    /// # Panics
    ///
    /// If called outside the context of an active async executor.
    pub fn into_stream_and_control_handle(
        self,
    ) -> (T::RequestStream, <T::RequestStream as RequestStream>::ControlHandle)
    where
        T: ProtocolMarker,
    {
        let stream = self.into_stream();
        let control_handle = stream.control_handle();
        (stream, control_handle)
    }

    /// Writes an epitaph into the underlying channel before closing it.
    pub fn close_with_epitaph(
        self,
        status: impl Into<Result<(), zx_status::Status>>,
    ) -> Result<(), Error> {
        self.inner.close_with_epitaph(status)
    }
}

impl<T> AsHandleRef for ServerEnd<T> {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        self.inner.as_handle_ref()
    }
}

impl<T> From<ServerEnd<T>> for NullableHandle {
    fn from(server: ServerEnd<T>) -> NullableHandle {
        server.into_channel().into()
    }
}

impl<T> From<ServerEnd<T>> for Channel {
    fn from(server: ServerEnd<T>) -> Channel {
        server.into_channel()
    }
}

impl<T> From<NullableHandle> for ServerEnd<T> {
    fn from(handle: NullableHandle) -> Self {
        ServerEnd { inner: handle.into(), phantom: PhantomData }
    }
}

impl<T> From<Channel> for ServerEnd<T> {
    fn from(chan: Channel) -> Self {
        ServerEnd { inner: chan, phantom: PhantomData }
    }
}

impl<T: ProtocolMarker> ::std::fmt::Debug for ServerEnd<T> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(f, "ServerEnd(name={}, channel={:?})", T::DEBUG_NAME, self.inner)
    }
}

/// Creates client and server endpoints connected to by a channel.
pub fn create_endpoints<T: ProtocolMarker>() -> (ClientEnd<T>, ServerEnd<T>) {
    let (client, server) = Channel::create();
    let client_end = ClientEnd::<T>::new(client);
    let server_end = ServerEnd::new(server);
    (client_end, server_end)
}

/// Create a client proxy and a server endpoint connected to it by a channel.
///
/// Useful for sending channel handles to calls that take arguments
/// of type `server_end:SomeProtocol`
///
/// # Panics
///
/// If called outside the context of an active async executor.
pub fn create_proxy<T: ProtocolMarker>() -> (T::Proxy, ServerEnd<T>) {
    let (client, server) = create_endpoints();
    (client.into_proxy(), server)
}

/// Create a synchronous client proxy and a server endpoint connected to it by a channel.
///
/// Useful for sending channel handles to calls that take arguments
/// of type `server_end:SomeProtocol`
#[cfg(target_os = "fuchsia")]
pub fn create_sync_proxy<T: ProtocolMarker>() -> (T::SynchronousProxy, ServerEnd<T>) {
    let (client, server) = create_endpoints();
    (client.into_sync_proxy(), server)
}

/// Create a request stream and a client endpoint connected to it by a channel.
///
/// Useful for sending channel handles to calls that take arguments
/// of type `client_end:SomeProtocol`
///
/// # Panics
///
/// If called outside the context of an active async executor.
pub fn create_request_stream<T: ProtocolMarker>() -> (ClientEnd<T>, T::RequestStream) {
    let (client, server) = create_endpoints();
    (client, server.into_stream())
}

/// Create a request stream and proxy connected to one another.
///
/// Useful for testing where both the request stream and proxy are
/// used in the same process.
///
/// # Panics
///
/// If called outside the context of an active async executor.
pub fn create_proxy_and_stream<T: ProtocolMarker>() -> (T::Proxy, T::RequestStream) {
    let (client, server) = create_endpoints::<T>();
    (client.into_proxy(), server.into_stream())
}

/// Create a request stream and synchronous proxy connected to one another.
///
/// Useful for testing where both the request stream and proxy are
/// used in the same process.
///
/// # Panics
///
/// If called outside the context of an active async executor.
#[cfg(target_os = "fuchsia")]
pub fn create_sync_proxy_and_stream<T: ProtocolMarker>() -> (T::SynchronousProxy, T::RequestStream)
{
    let (client, server) = create_endpoints::<T>();
    (client.into_sync_proxy(), server.into_stream())
}

/// The type of a client-initiated method.
#[derive(Copy, Clone, Debug)]
pub enum MethodType {
    /// One-way method, also known as fire-and-forget.
    OneWay,
    /// Two-way method.
    TwoWay,
}
