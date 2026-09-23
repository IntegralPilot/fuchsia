// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::LocalPrincipalIdentifier;
use attribution_processing::{
    GlobalPrincipalIdentifier, GlobalPrincipalIdentifierFactory, PrincipalDescription,
    PrincipalType,
};
use fuchsia_sync::Mutex;
use log::error;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;
use {fidl_fuchsia_component as fcomponent, fidl_fuchsia_memory_attribution as fattribution};

const ROOT_COMPONENT_NAME: &str = "component_manager";

/// An error of the attribution client.
#[derive(Debug)]
pub enum AttributionClientError {
    /// An unknown field is used in the provider's message.
    UnknownField(String),
    /// A mandatory field is missing in the provider's message.
    MissingField(String),
    /// The client has been unable to resolve a component moniker.
    FailToGetMoniker(fcomponent::Error),
    /// A FIDL connection failed.
    ConnectionFailure(fidl::Error),
    /// An error within the Attribution Provider protocol.
    AttributionProtocolError(fattribution::Error),
}

impl Display for AttributionClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttributionClientError::UnknownField(name) => {
                write!(f, "UnknownField({})", name)
            }
            AttributionClientError::MissingField(name) => {
                write!(f, "MissingField({})", name)
            }
            AttributionClientError::FailToGetMoniker(err) => {
                write!(f, "FailToGetMoniker({:?})", err)
            }
            AttributionClientError::ConnectionFailure(err) => {
                write!(f, "ConnectionFailure({:?})", err)
            }
            AttributionClientError::AttributionProtocolError(err) => {
                write!(f, "AttributionProtocolError({:?})", err)
            }
        }
    }
}

impl From<fidl::Error> for AttributionClientError {
    fn from(err: fidl::Error) -> Self {
        AttributionClientError::ConnectionFailure(err)
    }
}

impl From<fcomponent::Error> for AttributionClientError {
    fn from(err: fcomponent::Error) -> Self {
        AttributionClientError::FailToGetMoniker(err)
    }
}

impl From<fattribution::Error> for AttributionClientError {
    fn from(err: fattribution::Error) -> Self {
        AttributionClientError::AttributionProtocolError(err)
    }
}

/// Definition of a Principal.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct PrincipalDefinition {
    /// Parent principal in the AttributionProvider hierarchy, responsible for providing
    /// attribution information for this principal.
    pub attributor: Option<GlobalPrincipalIdentifier>,
    pub id: GlobalPrincipalIdentifier,
    pub description: Option<PrincipalDescription>,
    pub principal_type: PrincipalType,
}

/// Holds principals and attributions emitted by a single Principal.
/// [LocalPrincipalIdentifier] is unique within this [AttributionProvider].
#[derive(Clone)]
pub struct AttributionProvider {
    /// Principals defined by this provider, indexed by local identifier.
    pub definitions: HashMap<LocalPrincipalIdentifier, PrincipalDefinition>,
    /// Resource attributiona defined by this provider, indexed by recipient.
    pub resources: HashMap<LocalPrincipalIdentifier, Vec<fattribution::Resource>>,
}

/// AttributionStateManager stores the state of the published attribution claims by recipient.
#[derive(Default, Clone)]
pub struct AttributionState(pub HashMap<GlobalPrincipalIdentifier, AttributionProvider>);

impl AttributionState {
    /// Returns the identifier of the first principal whose description matches `description`.
    pub fn find_principal_by_description(
        &self,
        description: &PrincipalDescription,
    ) -> Option<GlobalPrincipalIdentifier> {
        for provider in self.0.values() {
            for definition in provider.definitions.values() {
                if definition.description.as_ref() == Some(description) {
                    return Some(definition.id.clone());
                }
            }
        }
        None
    }
}

/// AttributionStateManager manages the state of the published attribution claims as known by
/// [AttributionClient].
struct AttributionStateManager {
    /// Source to Principal definitions.
    attribution_providers: AttributionState,

    /// Counter for GlobalPrincipalIdentifiers.
    next_global_id: GlobalPrincipalIdentifierFactory,
}

impl AttributionStateManager {
    // |root_job_koid| is used to anchor the attribution hierarchy to the root of the Zircon
    // job hierarchy. This ensures no memory is left unaccounted.
    pub fn new_with_root_job(
        root_job_koid: u64,
    ) -> (AttributionStateManager, GlobalPrincipalIdentifier) {
        let mut attribution_providers = AttributionState::default();

        let mut principal_id_factory = GlobalPrincipalIdentifierFactory::default();

        let root_identifier = principal_id_factory.next();
        let principal = PrincipalDefinition {
            attributor: None,
            id: root_identifier,
            description: Some(PrincipalDescription::Component(ROOT_COMPONENT_NAME.to_owned())),
            principal_type: PrincipalType::Runnable,
        };
        attribution_providers.0.insert(
            root_identifier,
            AttributionProvider {
                definitions: HashMap::from([(
                    LocalPrincipalIdentifier::self_identifier(),
                    principal,
                )]),
                resources: HashMap::from([(
                    LocalPrincipalIdentifier::self_identifier(),
                    vec![fattribution::Resource::KernelObject(root_job_koid)],
                )]),
            },
        );

        (
            AttributionStateManager { attribution_providers, next_global_id: principal_id_factory },
            root_identifier,
        )
    }

    /// Add a new Attribution Provider, from principal `identifier`, to the state.
    fn add_new_provider(&mut self, identifier: GlobalPrincipalIdentifier) {
        self.attribution_providers.0.insert(
            identifier,
            AttributionProvider { definitions: HashMap::new(), resources: HashMap::new() },
        );
    }

    /// Add a new principal, from `attributor`, to the state.
    fn add_new_principal(
        &mut self,
        attributor: GlobalPrincipalIdentifier,
        local: LocalPrincipalIdentifier,
        description: Option<PrincipalDescription>,
        principal_type: PrincipalType,
    ) -> GlobalPrincipalIdentifier {
        let global_id = self.next_global_id.next();
        self.attribution_providers.0.get_mut(&attributor).unwrap().definitions.insert(
            local,
            PrincipalDefinition {
                attributor: Some(attributor),
                id: global_id,
                description,
                principal_type,
            },
        );
        global_id
    }
}

pub trait AttributionClient: Send + Sync {
    fn get_attributions(&self) -> AttributionState;
}

/// AttributionClient is a client of a hierarchy of attribution providers, who provide the Provider
/// FIDL protocol. It resolves component moniker names using the Introspector protocol from
/// Component Manager.
pub struct AttributionClientImpl {
    introspector: fcomponent::IntrospectorProxy,

    attribution_state: Mutex<AttributionStateManager>,
}

impl AttributionClientImpl {
    /// Creates and starts the memory attribution client.
    pub fn new(
        root_provider: fattribution::ProviderProxy,
        introspector: fcomponent::IntrospectorProxy,
        root_job_koid: zx::Koid,
    ) -> Arc<AttributionClientImpl> {
        let (manager, root_id) =
            AttributionStateManager::new_with_root_job(root_job_koid.raw_koid());
        let client = Arc::new(AttributionClientImpl {
            introspector: introspector,
            attribution_state: Mutex::new(manager),
        });
        Self::attribute_memory(root_id, root_provider, client.clone());
        client
    }

    /// Starts a new asynchronous task to gather memory attribution information from the
    /// `source_principal` Principal exposing the `provider` interface, and stores this information
    /// into `client`.
    fn attribute_memory(
        source_principal: GlobalPrincipalIdentifier,
        provider: fattribution::ProviderProxy,
        client: Arc<AttributionClientImpl>,
    ) {
        fuchsia_async::Task::spawn(Self::attribute_memory_logging_inner(
            source_principal,
            provider,
            client,
        ))
        .detach();
    }

    async fn attribute_memory_logging_inner(
        source_principal: GlobalPrincipalIdentifier,
        provider: fattribution::ProviderProxy,
        client: Arc<AttributionClientImpl>,
    ) {
        let result = Self::attribute_memory_inner(source_principal, provider, client.clone()).await;
        if let Err(err) = result {
            error!("Error while attributing memory: {:?}", err)
        }
        client.attribution_state.lock().attribution_providers.0.remove(&source_principal);
    }

    async fn attribute_memory_inner(
        source_principal: GlobalPrincipalIdentifier,
        provider: fattribution::ProviderProxy,
        client: Arc<AttributionClientImpl>,
    ) -> Result<(), AttributionClientError> {
        loop {
            let attributions = match provider.get().await {
                Ok(response) => response?,
                Err(err) => {
                    if let fidl::Error::ClientChannelClosed { .. } = err {
                        // The server disconnected voluntarily. This is the expected behavior when
                        // a Principal shuts down, so we just need to clean up without throwing an
                        // error.
                        return Ok(());
                    } else {
                        // This is a real error, so we need to report it.
                        return Err(err.into());
                    }
                }
            };

            // If there are children, resources assigned to this node by its parent
            // will be re-assigned to children if applicable.
            for attribution in attributions.attributions.into_iter().flatten() {
                // Recursively attribute memory in this child principal.
                match attribution {
                    fattribution::AttributionUpdate::Add(new_principal) => {
                        let local_identifier = LocalPrincipalIdentifier(
                            new_principal.identifier.ok_or_else(|| {
                                AttributionClientError::MissingField(
                                    "NewPrincipal::identifier".to_owned(),
                                )
                            })?,
                        );

                        let introspector = &client.introspector;
                        let description = match AttributionClientImpl::new_principal_description(
                            new_principal.description,
                            introspector,
                        )
                        .await
                        {
                            Ok(description) => Some(description),
                            Err(AttributionClientError::FailToGetMoniker(err)) => {
                                log::warn!(
                                    "Unable to get moniker for principal {:?}: {:?}. This might be a zombie principal.",
                                    new_principal.identifier,
                                    err
                                );
                                None
                            }
                            Err(err) => return Err(err.into()),
                        };
                        let principal_type = AttributionClientImpl::new_principal_type(
                            new_principal.principal_type.ok_or_else(|| {
                                AttributionClientError::MissingField(
                                    "NewPrincipal::type_".to_owned(),
                                )
                            })?,
                        )?;
                        let child_id = client.attribution_state.lock().add_new_principal(
                            source_principal.clone(),
                            local_identifier,
                            description,
                            principal_type,
                        );

                        if let Some(child_provider) = new_principal.detailed_attribution {
                            client.attribution_state.lock().add_new_provider(child_id);
                            Self::attribute_memory(
                                child_id,
                                child_provider.into_proxy(),
                                client.clone(),
                            );
                        };
                    }
                    fattribution::AttributionUpdate::Update(updated_principal) => {
                        let identifier = LocalPrincipalIdentifier(
                            updated_principal.identifier.ok_or_else(|| {
                                AttributionClientError::MissingField(
                                    "UpdatedPrincipal::identifier".to_owned(),
                                )
                            })?,
                        );

                        let resources_opt =
                            updated_principal.resources.map(|resources| match resources {
                                fattribution::Resources::Data(d) => d.resources,
                                fattribution::Resources::Buffer(b) => {
                                    let mapping =
                                        mapped_vmo::ImmutableMapping::create_from_vmo(&b, false)
                                            .unwrap();
                                    let resource_vector: fattribution::Data =
                                        fidl::unpersist(&mapping).unwrap();
                                    resource_vector.resources
                                }
                                _ => todo!(),
                            });
                        match resources_opt {
                            None => {
                                client
                                    .attribution_state
                                    .lock()
                                    .attribution_providers
                                    .0
                                    .get_mut(&source_principal)
                                    .unwrap()
                                    .resources
                                    .remove(&identifier);
                            }
                            Some(resources) => {
                                client
                                    .attribution_state
                                    .lock()
                                    .attribution_providers
                                    .0
                                    .get_mut(&source_principal)
                                    .unwrap()
                                    .resources
                                    .insert(identifier.clone(), resources);
                            }
                        };
                    }
                    fattribution::AttributionUpdate::Remove(identifier) => {
                        debug_assert_ne!(identifier, 0);
                        let mut attribution_state = client.attribution_state.lock();
                        attribution_state
                            .attribution_providers
                            .0
                            .get_mut(&source_principal)
                            .unwrap()
                            .definitions
                            .remove(&LocalPrincipalIdentifier(identifier));
                        attribution_state
                            .attribution_providers
                            .0
                            .get_mut(&source_principal)
                            .unwrap()
                            .resources
                            .remove(&LocalPrincipalIdentifier(identifier));
                    }
                    _ => panic!("Unimplemented"),
                };
            }
        }
    }

    async fn new_principal_description(
        description: Option<fattribution::Description>,
        introspector: &fcomponent::IntrospectorProxy,
    ) -> Result<PrincipalDescription, AttributionClientError> {
        match description.ok_or_else(|| {
            AttributionClientError::MissingField("NewPrincipal::description".to_owned())
        })? {
            fattribution::Description::Component(moniker_token) => {
                Ok(PrincipalDescription::Component(introspector.get_moniker(moniker_token).await??))
            }
            fattribution::Description::Part(part_name) => Ok(PrincipalDescription::Part(part_name)),
            fattribution::Description::__SourceBreaking { unknown_ordinal: _ } => {
                Err(AttributionClientError::UnknownField("Description".to_owned()))
            }
        }
    }

    fn new_principal_type(
        value: fattribution::PrincipalType,
    ) -> Result<PrincipalType, AttributionClientError> {
        match value {
            fattribution::PrincipalType::Runnable => Ok(PrincipalType::Runnable),
            fattribution::PrincipalType::Part => Ok(PrincipalType::Part),
            fattribution::PrincipalType::__SourceBreaking { unknown_ordinal: _ } => {
                Err(AttributionClientError::UnknownField("PrincipalType".to_owned()))
            }
        }
    }
}

impl AttributionClient for AttributionClientImpl {
    fn get_attributions(&self) -> AttributionState {
        self.attribution_state.lock().attribution_providers.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::RequestStream;
    use fuchsia_async as fasync;
    use futures::TryStreamExt;
    use maplit::hashmap;
    /// Tests a two-level attribution hierarchy.
    #[test]
    fn test_attribute_memory() {
        let mut exec = fasync::TestExecutor::new();
        let (root_provider, snapshot_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fattribution::ProviderMarker>();
        let (introspector, mut introspector_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fcomponent::IntrospectorMarker>();

        let root_job_koid = zx::Koid::from_raw(1);
        let attribution_client =
            AttributionClientImpl::new(root_provider, introspector, root_job_koid);

        let server = attribution_server::AttributionServer::new(Box::new(|| {
            let new_principal = fattribution::NewPrincipal {
                identifier: Some(2),
                description: Some(fattribution::Description::Part("part".to_owned())),
                principal_type: Some(fattribution::PrincipalType::Runnable),
                detailed_attribution: None,
                ..Default::default()
            };
            vec![fattribution::AttributionUpdate::Add(new_principal)]
        }));

        let observer = server.new_observer(snapshot_request_stream.control_handle());
        fasync::Task::spawn(async move {
            serve(observer, snapshot_request_stream).await.unwrap();
        })
        .detach();

        fasync::Task::spawn(async move {
            while let Some(request) = introspector_request_stream.try_next().await.unwrap() {
                match request {
                    fcomponent::IntrospectorRequest::GetMoniker {
                        component_instance,
                        responder,
                    } => {
                        responder.send(Ok(&format!("{:?}", component_instance))).unwrap();
                    }
                    fcomponent::IntrospectorRequest::_UnknownMethod { ordinal, .. } => {
                        unimplemented!("Unknown method {}", ordinal)
                    }
                }
            }
        })
        .detach();

        let mut never_finishing_future = std::future::pending::<()>();
        let _ = exec.run_until_stalled(&mut never_finishing_future);

        let state = attribution_client.attribution_state.lock().attribution_providers.clone();
        assert_eq!(state.0.len(), 1);
        let (_, provider) = state.0.iter().next().unwrap();
        assert_eq!(provider.definitions.len(), 2);
        assert_eq!(
            provider.definitions.get(&LocalPrincipalIdentifier(2)),
            Some(&PrincipalDefinition {
                attributor: Some(GlobalPrincipalIdentifier::new_for_test(1)),
                id: GlobalPrincipalIdentifier::new_for_test(2),
                description: Some(PrincipalDescription::Part("part".to_owned())),
                principal_type: PrincipalType::Runnable
            })
        );
    }

    pub async fn serve(
        observer: attribution_server::Observer,
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

    /// Tests a two-level attribution hierarchy with one moniker request failing.
    #[test]
    fn test_attribute_memory_moniker_fail() {
        let mut exec = fasync::TestExecutor::new();
        let (root_provider, snapshot_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fattribution::ProviderMarker>();
        let (introspector, mut introspector_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fcomponent::IntrospectorMarker>();

        let root_job_koid = zx::Koid::from_raw(1);
        let attribution_client =
            AttributionClientImpl::new(root_provider, introspector, root_job_koid);

        let server = attribution_server::AttributionServer::new(Box::new(|| {
            let new_principal = fattribution::NewPrincipal {
                identifier: Some(2),
                description: Some(fattribution::Description::Component(zx::Event::create())),
                principal_type: Some(fattribution::PrincipalType::Runnable),
                detailed_attribution: None,
                ..Default::default()
            };
            vec![fattribution::AttributionUpdate::Add(new_principal)]
        }));

        let observer = server.new_observer(snapshot_request_stream.control_handle());
        fasync::Task::spawn(async move {
            serve(observer, snapshot_request_stream).await.unwrap();
        })
        .detach();

        fasync::Task::spawn(async move {
            while let Some(request) = introspector_request_stream.try_next().await.unwrap() {
                match request {
                    fcomponent::IntrospectorRequest::GetMoniker {
                        component_instance: _,
                        responder,
                    } => {
                        responder.send(Err(fcomponent::Error::InstanceNotFound)).unwrap();
                    }
                    fcomponent::IntrospectorRequest::_UnknownMethod { ordinal, .. } => {
                        unimplemented!("Unknown method {}", ordinal)
                    }
                }
            }
        })
        .detach();

        let mut never_finishing_future = std::future::pending::<()>();
        let _ = exec.run_until_stalled(&mut never_finishing_future);

        let state = attribution_client.attribution_state.lock().attribution_providers.clone();
        assert_eq!(state.0.len(), 1);
        let (_, provider) = state.0.iter().next().unwrap();
        // Two principals are recorded, but only one features a description.
        assert_eq!(provider.definitions.len(), 2);
        assert_eq!(
            provider.definitions.get(&LocalPrincipalIdentifier(0)).unwrap().description,
            Some(PrincipalDescription::Component("component_manager".to_string()))
        );
        assert_eq!(
            provider.definitions.get(&LocalPrincipalIdentifier(2)).unwrap().description,
            None
        );
    }

    #[test]
    fn test_attribute_state() {
        let mut state = AttributionState::default();
        state.0.insert(
            GlobalPrincipalIdentifier::new_for_test(10),
            AttributionProvider {
                definitions: hashmap! {
                    LocalPrincipalIdentifier(11) => PrincipalDefinition {
                        attributor: None,
                        id: GlobalPrincipalIdentifier::new_for_test(11),
                        description: None,
                        principal_type: PrincipalType::Runnable
                    },
                    LocalPrincipalIdentifier(1) => PrincipalDefinition {
                        attributor: None,
                        id: GlobalPrincipalIdentifier::new_for_test(1),
                        description: Some(PrincipalDescription::Component("test1".to_string())),
                        principal_type: PrincipalType::Runnable
                    },
                    LocalPrincipalIdentifier(2) => PrincipalDefinition {
                        attributor: None,
                        id: GlobalPrincipalIdentifier::new_for_test(2),
                        description: Some(PrincipalDescription::Part("test2".to_string())),
                        principal_type: PrincipalType::Runnable
                    }
                },
                resources: hashmap! {},
            },
        );
        state.0.insert(
            GlobalPrincipalIdentifier::new_for_test(20),
            AttributionProvider {
                definitions: hashmap! {
                    LocalPrincipalIdentifier(22) => PrincipalDefinition {
                        attributor: None,
                        id: GlobalPrincipalIdentifier::new_for_test(22),
                        description: None,
                        principal_type: PrincipalType::Runnable
                    },
                    LocalPrincipalIdentifier(3) => PrincipalDefinition {
                        attributor: None,
                        id: GlobalPrincipalIdentifier::new_for_test(3),
                        description: Some(PrincipalDescription::Component("test3".to_string())),
                        principal_type: PrincipalType::Runnable
                    },
                    LocalPrincipalIdentifier(4) => PrincipalDefinition {
                        attributor: None,
                        id: GlobalPrincipalIdentifier::new_for_test(4),
                        description: Some(PrincipalDescription::Part("test4".to_string())),
                        principal_type: PrincipalType::Runnable
                    }
                },
                resources: hashmap! {},
            },
        );
        assert_eq!(
            Some(GlobalPrincipalIdentifier::new_for_test(1)),
            state.find_principal_by_description(&PrincipalDescription::Component(
                "test1".to_string()
            ))
        );
        assert_eq!(
            Some(GlobalPrincipalIdentifier::new_for_test(2)),
            state.find_principal_by_description(&PrincipalDescription::Part("test2".to_string()))
        );
        assert_eq!(
            Some(GlobalPrincipalIdentifier::new_for_test(3)),
            state.find_principal_by_description(&PrincipalDescription::Component(
                "test3".to_string()
            ))
        );
        assert_eq!(
            Some(GlobalPrincipalIdentifier::new_for_test(4)),
            state.find_principal_by_description(&PrincipalDescription::Part("test4".to_string()))
        );
    }
}
