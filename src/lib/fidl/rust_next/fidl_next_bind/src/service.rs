// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;
use core::ops::Deref;

use fidl_next_protocol::ServiceHandler;

/// A discoverable service.
pub trait DiscoverableService {
    /// The name of this service.
    const SERVICE_NAME: &'static str;
    /// The members of this service.
    const MEMBER_NAMES: &'static [&'static str];
}

/// A trait indicating that a service has members of the given type.
///
/// Services are currently limited to only having protocol members with a single
/// type, so only one instance of this trait will be implemented for each
/// service.
pub trait HasServiceRequest<T> {}

/// A FIDL service.
///
/// # Safety
///
/// The associated `Connector` type must be a `#[repr(transparent)]` wrapper
/// around `C`.
pub trait Service<C>: DiscoverableService {
    /// The connector for the service. It must be a `#[repr(transparent)]`
    /// wrapper around `C`.
    type Connector;
}

/// A strongly-typed member connector for a FIDL service.
#[repr(transparent)]
pub struct ServiceConnector<S, C> {
    connector: C,
    service: PhantomData<S>,
}

unsafe impl<S, C: Send> Send for ServiceConnector<S, C> {}
unsafe impl<S, C: Sync> Sync for ServiceConnector<S, C> {}

impl<S, C> ServiceConnector<S, C> {
    /// Returns a new `ServiceConnector`from an untyped service connector.
    pub fn from_untyped(connector: C) -> Self {
        Self { connector, service: PhantomData }
    }
}

impl<S: Service<C>, C> Deref for ServiceConnector<S, C> {
    type Target = S::Connector;

    fn deref(&self) -> &Self::Target {
        // SAFETY: `S::Connector` is a `#[repr(transparent)]` wrapper around `C`.
        unsafe { &*(self as *const Self).cast::<S::Connector>() }
    }
}

/// A trait that can be implemented to transform service instance
/// transport handles from their parent server's transport handles.
pub trait InstanceFromServiceTransport<T> {
    /// Converts the given service transport handle of type `T` to [`Self`]
    fn from_service_transport(handle: T) -> Self;
}

/// A given transport can always be converted to itself
impl<T> InstanceFromServiceTransport<T> for T {
    fn from_service_transport(handle: T) -> Self {
        handle
    }
}

/// A service which dispatches incoming connections to a handler.
pub trait DispatchServiceHandler<H, T> {
    /// Handles a received connection request with the given handler.
    fn on_connection(handler: &H, member: &str, server_end: T);
}

/// An adapter for a FIDL service handler.
pub struct ServiceHandlerAdapter<S, H> {
    handler: H,
    _service: PhantomData<S>,
}

impl<S, H: Clone> Clone for ServiceHandlerAdapter<S, H> {
    fn clone(&self) -> Self {
        Self { handler: self.handler.clone(), _service: PhantomData }
    }
}

unsafe impl<S, H> Send for ServiceHandlerAdapter<S, H> where H: Send {}
unsafe impl<S, H> Sync for ServiceHandlerAdapter<S, H> where H: Sync {}

impl<S, H> ServiceHandlerAdapter<S, H> {
    /// Creates a new service handler from a supported handler.
    pub fn from_untyped(handler: H) -> Self {
        Self { handler, _service: PhantomData }
    }
}

impl<S, H, T> ServiceHandler<T> for ServiceHandlerAdapter<S, H>
where
    S: DispatchServiceHandler<H, T>,
{
    fn on_connection(&self, member: &str, server_end: T) {
        S::on_connection(&self.handler, member, server_end)
    }
}
