// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::marker::PhantomData;

use fidl::endpoints::{ServiceMarker, ServiceRequest};
use fidl_fuchsia_component_decl::{NameMapping, OfferService};
use fidl_fuchsia_driver_framework::Offer;
use fidl_next::HasServiceRequest;
use fuchsia_component::DEFAULT_SERVICE_INSTANCE;
use fuchsia_component::server::{FidlServiceMember, ServiceFs, ServiceObjTrait};

/// A builder for creating [`Offer`]-compatible values for [`crate::NodeBuilder::add_offer`] that
/// serves a fidl service.
///
/// The methods on this that start with `add_` are helpers that will both register a service handler
/// with a [`ServiceFs`] and register the instance name in the structure. If you're handling adding
/// your service handlers to the outgoing directory yourself, you can just use the non-`add_`
/// methods to register them.
///
/// If no calls to add any instances are made, then when this is transformed into a service offer
/// it will be as if a single default instance with the default name was added.
pub struct ServiceOffer<S> {
    service_name: String,
    instances: Vec<NameMapping>,
    _p: PhantomData<S>,
}

impl<S: fidl_next::DiscoverableService> Default for ServiceOffer<S> {
    fn default() -> Self {
        Self::new_next()
    }
}

impl<S> ServiceOffer<S> {
    /// Builds an offer for a zircon transport service based on the [`ServiceMarker`] for `S`.
    ///
    /// If the compiler can't deduce the type of `S` (which may be the case if you're not using the
    /// `add_` methods to add to a [`ServiceFs`] at the same time), you can use [`Self::new_marker`]
    /// to make it explicit.
    pub fn new() -> Self
    where
        S: ServiceMarker,
    {
        let service_name = S::SERVICE_NAME.to_owned();
        let instances = vec![];
        Self { service_name, instances, _p: PhantomData }
    }

    /// Builds an offer for a service based on the [`fidl_next::DiscoverableService`] for `S`.
    ///
    /// If the compiler can't deduce the type of `S` (which may be the case if you're not using the
    /// `add_` methods to add to a [`ServiceFs`] at the same time), you can use
    /// [`Self::new_marker_next`] to make it explicit.
    pub fn new_next() -> Self
    where
        S: fidl_next::DiscoverableService,
    {
        let service_name = S::SERVICE_NAME.to_owned();
        let instances = vec![];
        Self { service_name, instances, _p: PhantomData }
    }

    /// Builds an offer for a service with a custom service name.
    pub fn new_with_name(service_name: impl Into<String>) -> Self {
        Self { service_name: service_name.into(), instances: vec![], _p: PhantomData }
    }

    /// Builds an offer for a zircon transport service based on the given [`ServiceMarker`].
    ///
    /// This is mostly useful if the compiler can't derive the type of `S` on its own.
    pub fn new_marker(_marker: S) -> Self
    where
        S: ServiceMarker,
    {
        let service_name = S::SERVICE_NAME.to_owned();
        let instances = vec![];
        Self { service_name, instances, _p: PhantomData }
    }

    /// Builds an offer for a service based on the [`fidl_next::DiscoverableService`].
    ///
    /// This is mostly useful if the compiler can't derive the type of `S` on its own.
    pub fn new_marker_next(_marker: S) -> Self
    where
        S: fidl_next::DiscoverableService,
    {
        Self::new_next()
    }

    /// Adds the given service instance to this offer and to the [`ServiceFs`] passed in, using the
    /// generator function `f`. The type of the service will be derived from the result of the
    /// generator function and it will be added with the name `name` which will be mapped to the
    /// default instance name to child components ([`DEFAULT_SERVICE_INSTANCE`]).
    pub fn add_default_named<O: ServiceObjTrait, F, SR>(
        self,
        fs: &mut ServiceFs<O>,
        name: impl Into<String>,
        f: F,
    ) -> Self
    where
        F: Fn(SR) -> O::Output,
        F: Clone,
        SR: ServiceRequest<Service = S>,
        FidlServiceMember<F, SR, O::Output>: Into<O>,
    {
        let name = name.into();
        fs.dir("svc").add_fidl_service_instance_at(self.service_name.clone(), name.clone(), f);
        self.named_default_instance(name)
    }

    /// Adds the given service instance to this offer and to the [`ServiceFs`] passed in, using the
    /// generator function `f`. The type of the service will be derived from the result of the
    /// generator function and it will be added with the name `name` which will be mapped to the
    /// default instance name to child components ([`DEFAULT_SERVICE_INSTANCE`]).
    pub fn add_default_named_next<H, O>(
        self,
        fs: &mut ServiceFs<O>,
        name: impl Into<String>,
        handler: H,
    ) -> Self
    where
        O: ServiceObjTrait,
        S: fidl_next::DiscoverableService
            + fidl_next::DispatchServiceHandler<H, zx::Channel>
            + 'static,
        H: Send + Sync + 'static,
    {
        let name = name.into();
        fs.dir("svc").add_fidl_next_service_instance_at::<S, _>(
            self.service_name.clone(),
            name.clone(),
            handler,
        );
        self.named_default_instance(name)
    }

    /// Adds the given service instance to this offer and to the [`ServiceFs`] passed in, using the
    /// generator function `f`. The type of the service will be derived from the result of the
    /// generator function and it will be added with the name `name`.
    pub fn add_named<O: ServiceObjTrait, F, SR>(
        self,
        fs: &mut ServiceFs<O>,
        name: impl Into<String>,
        f: F,
    ) -> Self
    where
        F: Fn(SR) -> O::Output,
        F: Clone,
        SR: ServiceRequest<Service = S>,
        FidlServiceMember<F, SR, O::Output>: Into<O>,
    {
        let name = name.into();
        fs.dir("svc").add_fidl_service_instance_at(self.service_name.clone(), name.clone(), f);
        self.named_instance(name)
    }

    /// Adds the given service instance to this offer and to the [`ServiceFs`] passed in, using the
    /// generator function `f`. The type of the service will be derived from the result of the
    /// generator function and it will be added with the name `name`.
    pub fn add_named_next<H, O>(
        self,
        fs: &mut ServiceFs<O>,
        name: impl Into<String>,
        handler: H,
    ) -> Self
    where
        O: ServiceObjTrait,
        S: fidl_next::DiscoverableService
            + fidl_next::DispatchServiceHandler<H, zx::Channel>
            + 'static,
        H: Send + Sync + 'static,
    {
        let name = name.into();
        fs.dir("svc").add_fidl_next_service_instance_at::<S, _>(
            self.service_name.clone(),
            name.clone(),
            handler,
        );
        self.named_instance(name)
    }

    /// Adds the named instance as the `default` instance of this service offer (as specified
    /// by [`DEFAULT_SERVICE_INSTANCE`]). If you are only offering a single instance that is
    /// already called `default`, you do not need to call this.
    pub fn named_default_instance(mut self, name: impl Into<String>) -> Self {
        self.instances.push(NameMapping {
            source_name: name.into(),
            target_name: DEFAULT_SERVICE_INSTANCE.to_owned(),
        });
        self
    }

    /// Adds the named instance to the offer without mapping it to the default instance name.
    /// You can use this to add additional instances offered in your outgoing directory.
    pub fn named_instance(mut self, name: impl Into<String>) -> Self {
        let source_name = name.into();
        let target_name = source_name.clone();
        self.instances.push(NameMapping { source_name, target_name });
        self
    }

    fn build_offer(self) -> fidl_fuchsia_component_decl::Offer {
        // if no instances were added, assume there's a single default instance
        let mut instances = self.instances;
        if instances.is_empty() {
            instances.push(NameMapping {
                source_name: DEFAULT_SERVICE_INSTANCE.to_owned(),
                target_name: DEFAULT_SERVICE_INSTANCE.to_owned(),
            });
        }
        let service = OfferService {
            source_name: Some(self.service_name.clone()),
            target_name: Some(self.service_name),
            source_instance_filter: Some(vec![DEFAULT_SERVICE_INSTANCE.to_owned()]),
            renamed_instances: Some(instances),
            ..Default::default()
        };
        fidl_fuchsia_component_decl::Offer::Service(service)
    }

    /// Finalize the construction of the [`Offer`] object for use with
    /// [`super::NodeBuilder::add_offer`] when the service is a zircon transport
    /// service.
    pub fn build_zircon_offer(self) -> Offer
    where
        S: ServiceMarker,
    {
        Offer::ZirconTransport(self.build_offer())
    }

    /// Finalize the construction of the [`Offer`] object for use with
    /// [`super::NodeBuilder::add_offer`] when the service is a zircon transport
    /// service with the new wire bindings.
    pub fn build_zircon_offer_next(self) -> Offer
    where
        S: HasServiceRequest<zx::Channel>,
    {
        Offer::ZirconTransport(self.build_offer())
    }

    /// Finalize the construction of the [`Offer`] object for use with
    /// [`super::NodeBuilder::add_offer`] when the service is a driver
    /// transport service.
    pub fn build_driver_offer(self) -> Offer
    where
        S: HasServiceRequest<fdf_fidl::DriverChannel>,
    {
        Offer::DriverTransport(self.build_offer())
    }
}
