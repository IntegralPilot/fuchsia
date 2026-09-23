// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{ComponentInstanceError, PrettyPrintRef, RouteVerb, RoutingError};
use crate::rights::{Rights, validate_rights};
use async_trait::async_trait;
use capability_source::CapabilitySource;
use cm_rust::{CapabilityTypeName, FidlIntoNative, NativeIntoFidl};
use cm_rust_derive::FidlDecl;
use cm_types::RelativePath;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_io as fio;
use moniker::{ChildName, Moniker};
use router_error::RouterError;
use runtime_capabilities::{
    Capability, CapabilityBound, Connector, Data, Dictionary, DirConnector, Routable, Router,
    RouterErrorInfo, WeakInstanceToken,
};
use std::sync::{Arc, Weak};

#[cfg(target_os = "fuchsia")]
use {cm_types::IterablePath, fuchsia_trace as trace, itertools::Itertools};

pub enum WeakDictionaryOrRouter {
    Dictionary(Weak<Dictionary>),
    Router(Weak<Router<Dictionary>>),
}

impl From<Weak<Dictionary>> for WeakDictionaryOrRouter {
    fn from(d: Weak<Dictionary>) -> Self {
        Self::Dictionary(d)
    }
}

impl From<Weak<Router<Dictionary>>> for WeakDictionaryOrRouter {
    fn from(r: Weak<Router<Dictionary>>) -> Self {
        Self::Router(r)
    }
}

/// RoutingError is big, and we're going to hold a lot of IntermediateRouter types, so instead of
/// pre-creating a RoutingError we hold the pieces we need to make a
/// RoutingError::RouteSourceNotFound and construct it as needed.
struct NotFoundErrorContext {
    verb: RouteVerb,
    source: PrettyPrintRef,
    type_name: CapabilityTypeName,
}

impl NotFoundErrorContext {
    fn to_router_error(&self, intermediate_router: &IntermediateRouter) -> RouterError {
        RoutingError::RouteSourceNotFound {
            moniker: intermediate_router.moniker.clone(),
            verb: self.verb,
            counter_verb: match &self.source {
                PrettyPrintRef::Parent => RouteVerb::Offer,
                PrettyPrintRef::Child(_)
                | PrettyPrintRef::ChildInCollection(_, _)
                | PrettyPrintRef::Collection(_) => RouteVerb::Expose,
                PrettyPrintRef::Self_ | PrettyPrintRef::Capability(_) => RouteVerb::Declare,
                PrettyPrintRef::Framework
                | PrettyPrintRef::Debug
                | PrettyPrintRef::Void
                | PrettyPrintRef::Environment => RouteVerb::Contain,
            },
            source: self.source.clone(),
            capability_type: self.type_name,
            capability_name: intermediate_router.source_path.clone(),
        }
        .into()
    }
}

#[derive(FidlDecl, Debug, Clone, PartialEq)]
#[fidl_decl(fidl_table = "fruntime::RouteRequest")]
pub struct RouteRequest {
    pub build_type_name: cm_rust::CapabilityTypeName,
    pub availability: Option<cm_rust::Availability>,
    pub directory_rights: Option<fio::Flags>,
    pub directory_intermediate_rights: Option<fio::Flags>,
    pub inherit_rights: Option<bool>,
    pub sub_directory_path: Option<RelativePath>,
    pub isolated_storage_path: Option<RelativePath>,
    pub storage_sub_directory_path: Option<RelativePath>,
    pub storage_source_moniker: Option<Moniker>,
    pub event_stream_scope_moniker: Option<Moniker>,
    pub event_stream_scope: Option<Box<[cm_rust::EventScope]>>,
    pub skip_policy_checks: Option<bool>,
}

impl Default for RouteRequest {
    fn default() -> Self {
        Self {
            // Having Default implemented reduces verbosity in places where we assemble this
            // struct, but it's also more convenient to not have `build_type_name` wrapped in an
            // `Option`. We set the default to `Protocol` here, but anyone using this should
            // overwrite that.
            build_type_name: cm_rust::CapabilityTypeName::Protocol,
            availability: None,
            directory_rights: None,
            directory_intermediate_rights: None,
            inherit_rights: None,
            sub_directory_path: None,
            isolated_storage_path: None,
            storage_sub_directory_path: None,
            storage_source_moniker: None,
            event_stream_scope_moniker: None,
            event_stream_scope: None,
            skip_policy_checks: None,
        }
    }
}

/// A router that attempts to find a router to forward to in a source dictionary, validates (and
/// potentially mutates) the route request, and then forwards the request to next router.
pub struct IntermediateRouter {
    /// The dictionary (or router for a dictionary) which will hold the router that we forward to.
    source_dictionary: WeakDictionaryOrRouter,

    /// The path in the source dictionary at which we should find the router to forward to.
    source_path: RelativePath,

    /// The request that will be used if the route request is empty. The route request will also be
    /// compared against this "default" request, and if it's requesting something different/larger
    /// in scope than the default then the request will be rejected. For example, any incoming
    /// requests must have the same build type name as the default request and they must not
    /// request greater directory rights than are listed in the default request.
    default_request: RouteRequest,

    /// The "default" instance token to use when performing routes in order to find the router we
    /// will forward the request to.
    default_token: Arc<WeakInstanceToken>,

    /// The moniker of the component which created this router.
    moniker: Moniker,

    /// Additional context about this step in the route, which is used to generate error values
    /// when the source router cannot be found.
    not_found_context: NotFoundErrorContext,

    /// Whether or not to emit tracing events for routes. This field is ignored during host
    /// routing, where tracing events are never emitted.
    #[allow(unused)]
    enable_tracing: bool,
}

enum RouterCapabilityOrSource<C: CapabilityBound> {
    Router(Arc<Router<C>>),
    Capability(Arc<C>),
    Source(Box<CapabilitySource>),
}

impl IntermediateRouter {
    /// Returns an IntermediateRouter wrapped in a Router type, with an appropriate type for the
    /// `default_request.build_type_name`.
    pub fn new(
        source_dictionary: WeakDictionaryOrRouter,
        source_path: RelativePath,
        default_request: RouteRequest,
        default_token: Arc<WeakInstanceToken>,
        moniker: Moniker,
        route_verb: RouteVerb,
        source: fdecl::Ref,
    ) -> Capability {
        assert!(source_path.len() != 0);
        let type_name = default_request.build_type_name;

        let enable_tracing = route_verb == RouteVerb::Use;

        let self_ = Self {
            source_dictionary,
            source_path,
            default_request,
            default_token,
            moniker,
            not_found_context: NotFoundErrorContext {
                verb: route_verb,
                source: source.into(),
                type_name,
            },
            enable_tracing,
        };

        match type_name {
            CapabilityTypeName::Protocol
            | CapabilityTypeName::Runner
            | CapabilityTypeName::Resolver => Router::<Connector>::new(self_).into(),
            CapabilityTypeName::Service
            | CapabilityTypeName::Directory
            | CapabilityTypeName::Storage => Router::<DirConnector>::new(self_).into(),
            CapabilityTypeName::EventStream | CapabilityTypeName::Dictionary => {
                Router::<Dictionary>::new(self_).into()
            }
            CapabilityTypeName::Config => Router::<Data>::new(self_).into(),
        }
    }

    /// Returns the moniker that owns the source dictionary.
    fn get_upgrade_failure_moniker(&self) -> Moniker {
        match &self.not_found_context.source {
            PrettyPrintRef::Parent => self.moniker.parent().unwrap_or_else(|| Moniker::root()),

            PrettyPrintRef::Child(name) => {
                self.moniker.child(ChildName::new(name.clone().into(), None))
            }
            PrettyPrintRef::ChildInCollection(name, collection) => {
                self.moniker.child(ChildName::new(name.clone(), Some(collection.clone())))
            }

            PrettyPrintRef::Collection(_)
            | PrettyPrintRef::Capability(_)
            | PrettyPrintRef::Debug
            | PrettyPrintRef::Environment
            | PrettyPrintRef::Framework
            | PrettyPrintRef::Self_
            | PrettyPrintRef::Void => self.moniker.clone(),
        }
    }

    /// Upgrades and returns the dictionary which holds the router we will forward the request to.
    /// Initiates a routing operation for that dictionary if necessary.
    async fn upgrade_source_dictionary(
        &self,
        dictionary_request: &fruntime::RouteRequest,
    ) -> Result<Arc<Dictionary>, RouterError> {
        match &self.source_dictionary {
            WeakDictionaryOrRouter::Dictionary(dictionary) => dictionary.upgrade().ok_or(
                RoutingError::from(ComponentInstanceError::InstanceNotFound {
                    moniker: self.get_upgrade_failure_moniker(),
                })
                .into(),
            ),
            WeakDictionaryOrRouter::Router(router) => {
                let router = router.upgrade().ok_or(RoutingError::from(
                    ComponentInstanceError::InstanceNotFound {
                        moniker: self.get_upgrade_failure_moniker(),
                    },
                ))?;
                let dictionary = router
                    .route(dictionary_request.clone(), self.default_token.clone())
                    .await?
                    .expect("routers for source dictionaries should never return None");
                Ok(dictionary)
            }
        }
    }

    /// Upgrades self.source_dictionary and attempts to find a router at self.source_path in it of
    /// type Arc<Router<C>>. It does this by walking self.source_path, stepping down into each
    /// successive dictionary (routing dictionary routers as needed).
    async fn get_source_router<C: CapabilityBound>(
        &self,
        request: &fruntime::RouteRequest,
        debug: bool,
    ) -> Result<RouterCapabilityOrSource<C>, RouterError>
    where
        Arc<Router<C>>: TryFrom<Capability>,
        Arc<C>: TryFrom<Capability>,
        Router<C>: CapabilityBound,
    {
        let dictionary_request = fruntime::RouteRequest {
            build_type_name: Some(CapabilityTypeName::Dictionary.to_string()),
            ..request.clone()
        };
        // Get the dictionary holding the source router (if the dictionary still exists).
        let mut source_dictionary = self.upgrade_source_dictionary(&dictionary_request).await?;

        // Get the source router from the dictionary
        let mut path_to_walk = self.source_path.clone();
        let mut most_recent_source = None;
        while path_to_walk.len() > 1 {
            let next_step = path_to_walk.pop_front().expect("we checked that this isn't empty");
            match source_dictionary.get(&next_step) {
                Some(Capability::Dictionary(d)) => source_dictionary = d,
                Some(Capability::DictionaryRouter(r)) => {
                    match r.route(dictionary_request.clone(), self.default_token.clone()).await? {
                        Some(d) => {
                            if debug {
                                most_recent_source = Some(
                                    r.route_debug(
                                        dictionary_request.clone(),
                                        self.default_token.clone(),
                                    )
                                    .await?,
                                );
                            }
                            source_dictionary = d;
                        }
                        None => {
                            // The next step along our path is unavailable! If this is a debug
                            // route, we'll want the source of this unavailable dictionary.
                            let source = r
                                .route_debug(dictionary_request.clone(), self.default_token.clone())
                                .await?;
                            return Ok(RouterCapabilityOrSource::Source(Box::new(source)));
                        }
                    }
                }
                Some(capability) => {
                    return Err(RoutingError::BedrockWrongCapabilityType {
                        actual: capability.debug_typename().to_string(),
                        expected: Dictionary::debug_typename().to_string(),
                        moniker: self.moniker.clone().into(),
                    }
                    .into());
                }
                None => {
                    return Err(self.not_found_context.to_router_error(self));
                }
            }
        }
        let capability_name = path_to_walk
            .pop_front()
            .expect("we stopped the above loop before fully draining the path");
        let maybe_source_router = source_dictionary
            .get(&capability_name)
            .ok_or_else(|| self.not_found_context.to_router_error(self))?;

        let maybe_c: Option<Arc<C>> = maybe_source_router.clone().try_into().ok();
        if let Some(c) = maybe_c {
            if !debug {
                return Ok(RouterCapabilityOrSource::Capability(c));
            } else {
                let source = most_recent_source.ok_or_else(|| RoutingError::SourceUnknown {
                    capability_id: capability_name.to_string(),
                    moniker: self.moniker.clone().into(),
                })?;
                return Ok(RouterCapabilityOrSource::Source(Box::new(source)));
            }
        }

        let capability_type_name = maybe_source_router.debug_typename();
        let router: Arc<Router<C>> = maybe_source_router.try_into().map_err(|_| {
            RoutingError::BedrockWrongCapabilityType {
                actual: capability_type_name.to_string(),
                expected: Router::<C>::debug_typename().to_string(),
                moniker: self.moniker.clone().into(),
            }
        })?;
        Ok(RouterCapabilityOrSource::Router(router))
    }

    /// Check to see if `request` does not request anything different or larger in scope than
    /// `self.default_request`. `request` will be set to `self.default_request` if it is empty.
    fn handle_new_request(&self, request: &mut fruntime::RouteRequest) -> Result<(), RouterError> {
        if *request == fruntime::RouteRequest::default() {
            *request = self.default_request.clone().native_into_fidl();
            return Ok(());
        }

        self.check_build_type_name(request)?;
        self.handle_availability(request)?;
        self.handle_directory_rights(request)?;
        self.handle_sub_directory(request)?;
        self.handle_event_stream_scope(request);

        Ok(())
    }

    fn check_build_type_name(
        &self,
        request: &mut fruntime::RouteRequest,
    ) -> Result<(), RouterError> {
        if request.build_type_name != Some(self.default_request.build_type_name.to_string()) {
            Err(RoutingError::BedrockWrongCapabilityType {
                moniker: self.moniker.clone().into(),
                actual: request
                    .build_type_name
                    .as_ref()
                    .map(Clone::clone)
                    .unwrap_or_else(|| "".to_string()),
                expected: self.default_request.build_type_name.to_string(),
            })?;
        }
        Ok(())
    }

    fn handle_availability(&self, request: &mut fruntime::RouteRequest) -> Result<(), RouterError> {
        if self.default_request.availability.is_none() {
            return Ok(());
        }
        let request_availability: fidl_fuchsia_component_decl::Availability = *request
            .availability
            .as_ref()
            .ok_or_else(|| RoutingError::RouteRequestMissingField {
                moniker: self.moniker.clone().into(),
                missing_field: "availability".to_string(),
            })?;
        let request_availability: cm_rust::Availability = request_availability.fidl_into_native();
        let default_availability =
            self.default_request.availability.expect("default request is missing availability");
        let new_availability = crate::availability::advance(
            &self.moniker.clone().into(),
            request_availability,
            default_availability,
        )
        .map_err(|e| RoutingError::from(e))?;
        request.availability = Some(new_availability.native_into_fidl());
        Ok(())
    }

    fn handle_directory_rights(
        &self,
        request: &mut fruntime::RouteRequest,
    ) -> Result<(), RouterError> {
        let Some(directory_rights) = self.default_request.directory_rights else {
            return Ok(());
        };
        let rights = Rights::from(directory_rights);
        validate_rights(self.moniker.clone().into(), rights.into(), request)?;
        request.directory_intermediate_rights = Some(fio::Flags::from(rights));
        Ok(())
    }

    fn handle_sub_directory(
        &self,
        request: &mut fruntime::RouteRequest,
    ) -> Result<(), RouterError> {
        let Some(mut new_subdir) = self.default_request.sub_directory_path.clone() else {
            return Ok(());
        };

        let Some(current_subdir) = request.sub_directory_path.as_ref() else {
            request.sub_directory_path =
                self.default_request.sub_directory_path.clone().map(|p| p.to_string());
            return Ok(());
        };
        let current_subdir = RelativePath::new(current_subdir).map_err(|e| {
            RoutingError::RouteRequestFailedToParseField {
                moniker: self.moniker.clone().into(),
                field: "sub_directory_path".to_string(),
                parse_error: format!("{e:?}"),
            }
        })?;

        let success = new_subdir.extend(current_subdir);
        if !success {
            return Err(RoutingError::PathTooLong {
                moniker: self.moniker.clone().into(),
                path: self.default_request.sub_directory_path.clone().unwrap().to_string(),
                keyword: request.sub_directory_path.clone().unwrap(),
            }
            .into());
        }

        request.sub_directory_path = Some(new_subdir.native_into_fidl());
        Ok(())
    }

    fn handle_event_stream_scope(&self, request: &mut fruntime::RouteRequest) {
        if request.event_stream_scope_moniker.is_some() {
            // If the scope is already set then it's a smaller scope (because we can't expose
            // these), so only set our scope if the request doesn't have one yet.
            return;
        }
        let Some(new_moniker) = self.default_request.event_stream_scope_moniker.as_ref() else {
            return;
        };
        let Some(new_scope) = self.default_request.event_stream_scope.as_ref() else {
            return;
        };
        request.event_stream_scope_moniker = Some(new_moniker.clone().native_into_fidl());
        request.event_stream_scope = Some(new_scope.clone().native_into_fidl());
    }
}

#[async_trait]
impl<C: CapabilityBound> Routable<C> for IntermediateRouter
where
    Arc<Router<C>>: TryFrom<Capability>,
    Arc<C>: TryFrom<Capability>,
    Router<C>: CapabilityBound,
    C: std::fmt::Debug,
{
    async fn route(
        &self,
        mut request: fruntime::RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<C>>, RouterError> {
        #[cfg(target_os = "fuchsia")]
        if self.enable_tracing {
            trace::duration_begin!(
                "component_manager", "route_capability",
                "target" => self.moniker.as_str(),
                "type" => self.default_request.build_type_name.to_string().as_str(),
                "capability" => self.source_path.iter_segments().join("/").as_str()
            );
        }

        self.handle_new_request(&mut request)?;
        let result = match self.get_source_router(&request, false).await? {
            RouterCapabilityOrSource::Capability(c) => Ok(Some(c)),
            RouterCapabilityOrSource::Source(source) => {
                match &*source {
                    CapabilitySource::Void(_) => (),
                    other_source => {
                        panic!(
                            "should only return source for non-debug routes when source is void, but the source is {other_source:?}"
                        );
                    }
                }
                Err(RoutingError::SourceCapabilityIsVoid { moniker: source.source_moniker() }
                    .into())
            }
            RouterCapabilityOrSource::Router(router) => router.route(request, target).await,
        };

        #[cfg(target_os = "fuchsia")]
        if self.enable_tracing {
            trace::duration_end!(
                "component_manager", "route_capability",
                "target" => self.moniker.as_str(),
                "type" => self.default_request.build_type_name.to_string().as_str(),
                "capability" => self.source_path.iter_segments().join("/").as_str()
            );
        }

        result
    }

    async fn route_debug(
        &self,
        mut request: fruntime::RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        #[cfg(target_os = "fuchsia")]
        if self.enable_tracing {
            trace::duration_begin!(
                "component_manager", "route_capability_debug",
                "target" => self.moniker.as_str(),
                "type" => self.default_request.build_type_name.to_string().as_str(),
                "capability" => self.source_path.iter_segments().join("/").as_str()
            );
        }

        self.handle_new_request(&mut request)?;
        let result = match self.get_source_router(&request, true).await? {
            RouterCapabilityOrSource::Capability(_) => {
                panic!("returned capability for debug operation")
            }
            RouterCapabilityOrSource::Source(source) => Ok(*source),
            RouterCapabilityOrSource::Router(router) => router.route_debug(request, target).await,
        };

        #[cfg(target_os = "fuchsia")]
        if self.enable_tracing {
            trace::duration_end!(
                "component_manager", "route_capability_debug",
                "target" => self.moniker.as_str(),
                "type" => self.default_request.build_type_name.to_string().as_str(),
                "capability" => self.source_path.iter_segments().join("/").as_str()
            );
        }

        result
    }

    fn error_info(&self) -> Option<RouterErrorInfo> {
        Some(RouterErrorInfo {
            capability_type: self.not_found_context.type_name,
            name: self.source_path.basename().unwrap().to_owned(),
            availability: self.default_request.availability.unwrap(),
        })
    }
}
