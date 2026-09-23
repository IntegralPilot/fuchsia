// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::UtilsError;
use fidl_fuchsia_component_sandbox::{
    self as fsandbox, AggregateSource, Capability, CapabilityId, DictionaryItem, DirConnector,
    DirConnectorRouterRouteResponse, RouteRequest,
};
use std::cell::Cell;
use std::collections::HashMap;

pub struct DictionaryUtil {
    capability_store: fsandbox::CapabilityStoreProxy,
    cap_id: Cell<CapabilityId>,
}

impl DictionaryUtil {
    pub fn new(capability_store: fsandbox::CapabilityStoreProxy) -> Self {
        Self { capability_store, cap_id: Cell::new(0) }
    }
}

impl DictionaryUtil {
    pub async fn import_dictionary(
        &self,
        dictionary: fsandbox::DictionaryRef,
    ) -> Result<CapabilityId, UtilsError> {
        let dest_id = self.next_cap_id();
        let capability = Capability::Dictionary(dictionary);
        self.capability_store.import(dest_id, capability).await??;
        Ok(dest_id)
    }

    pub async fn copy_export_dictionary(
        &self,
        dictionary_id: CapabilityId,
    ) -> Result<fsandbox::DictionaryRef, UtilsError> {
        let dest_id = self.next_cap_id();
        self.capability_store.dictionary_copy(dictionary_id, dest_id).await??;
        let exported = self.capability_store.export(dest_id).await??;
        if let Capability::Dictionary(d) = exported {
            Ok(d)
        } else {
            Err(UtilsError::UnexpectedCapabilityType(
                "Dictionary".to_string(),
                format!("{:?}", exported),
            ))
        }
    }

    pub async fn dictionary_dir_connector_route(
        &self,
        dictionary_id: CapabilityId,
        service_name: &str,
    ) -> Result<DirConnector, UtilsError> {
        let dest_id = self.next_cap_id();
        self.capability_store.dictionary_get(dictionary_id, service_name, dest_id).await??;
        let exported = self.capability_store.export(dest_id).await??;
        let Capability::DirConnectorRouter(router) = exported else {
            return Err(UtilsError::UnexpectedCapabilityType(
                "DirConnectorRouter".to_string(),
                format!("{:?}", exported),
            ));
        };

        let routed = router.into_proxy().route(RouteRequest { ..Default::default() }).await??;
        let DirConnectorRouterRouteResponse::DirConnector(connector) = routed else {
            return Err(UtilsError::UnexpectedCapabilityType(
                "DirConnector".to_string(),
                format!("{:?}", routed),
            ));
        };

        Ok(connector)
    }

    pub async fn create_aggregate_dictionary(
        &self,
        sources: HashMap<String, Vec<AggregateSource>>,
    ) -> Result<CapabilityId, UtilsError> {
        let dest_id = self.next_cap_id();
        self.capability_store.dictionary_create(dest_id).await??;

        for (service_name, sources) in sources.into_iter() {
            let aggregate = self.capability_store.create_service_aggregate(sources).await??;

            let imported = self.next_cap_id();
            self.capability_store.import(imported, Capability::DirConnector(aggregate)).await??;

            self.capability_store
                .dictionary_insert(dest_id, &DictionaryItem { key: service_name, value: imported })
                .await??;
        }

        Ok(dest_id)
    }

    fn next_cap_id(&self) -> CapabilityId {
        self.cap_id.set(self.cap_id.get() + 1);
        self.cap_id.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{Proxy, create_proxy_and_stream};
    use futures::StreamExt;

    #[fuchsia::test]
    async fn test_import_dictionary() {
        let (proxy, mut stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let util = DictionaryUtil::new(proxy);

        let task = fuchsia_async::Task::local(async move {
            if let Some(Ok(request)) = stream.next().await {
                match request {
                    fsandbox::CapabilityStoreRequest::Import { id, capability, responder } => {
                        assert_eq!(id, 1);
                        assert!(matches!(capability, Capability::Dictionary(_)));
                        responder.send(Ok(())).unwrap();
                    }
                    _ => panic!("Unexpected request"),
                }
            }
        });

        let dictionary = fsandbox::DictionaryRef { token: fidl::EventPair::create().0 };
        let id = util.import_dictionary(dictionary).await.expect("Import failed");
        assert_eq!(id, 1);
        task.await;
    }

    #[fuchsia::test]
    async fn test_copy_export_dictionary() {
        let (proxy, mut stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let util = DictionaryUtil::new(proxy);

        let task = fuchsia_async::Task::local(async move {
            // dictionary_copy
            if let Some(Ok(fsandbox::CapabilityStoreRequest::DictionaryCopy {
                id,
                dest_id,
                responder,
            })) = stream.next().await
            {
                assert_eq!(id, 100);
                assert_eq!(dest_id, 1);
                responder.send(Ok(())).unwrap();
            } else {
                panic!("Expected DictionaryCopy");
            }

            // export
            if let Some(Ok(fsandbox::CapabilityStoreRequest::Export { id, responder })) =
                stream.next().await
            {
                assert_eq!(id, 1);
                responder
                    .send(Ok(Capability::Dictionary(fsandbox::DictionaryRef {
                        token: fidl::EventPair::create().0,
                    })))
                    .unwrap();
            } else {
                panic!("Expected Export");
            }
        });

        let _ = util.copy_export_dictionary(100).await.expect("Copy export failed");
        task.await;
    }

    #[fuchsia::test]
    async fn test_dictionary_dir_connector_route() {
        let (proxy, mut stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let util = DictionaryUtil::new(proxy);

        let task = fuchsia_async::Task::local(async move {
            // dictionary_get
            if let Some(Ok(fsandbox::CapabilityStoreRequest::DictionaryGet {
                id,
                key,
                dest_id,
                responder,
            })) = stream.next().await
            {
                assert_eq!(id, 100);
                assert_eq!(key, "service");
                assert_eq!(dest_id, 1);
                responder.send(Ok(())).unwrap();
            } else {
                panic!("Expected DictionaryGet");
            }

            // export
            if let Some(Ok(fsandbox::CapabilityStoreRequest::Export { id, responder })) =
                stream.next().await
            {
                assert_eq!(id, 1);

                let (router_client, mut router_stream) =
                    create_proxy_and_stream::<fsandbox::DirConnectorRouterMarker>();
                fuchsia_async::Task::local(async move {
                    if let Some(Ok(fsandbox::DirConnectorRouterRequest::Route {
                        payload: _,
                        responder,
                    })) = router_stream.next().await
                    {
                        let connector =
                            fsandbox::DirConnector { token: fidl::EventPair::create().0 };
                        responder
                            .send(Ok(DirConnectorRouterRouteResponse::DirConnector(connector)))
                            .unwrap();
                    }
                })
                .detach();

                responder
                    .send(Ok(Capability::DirConnectorRouter(
                        router_client.into_client_end().unwrap(),
                    )))
                    .unwrap();
            } else {
                panic!("Expected Export");
            }
        });

        let _ = util.dictionary_dir_connector_route(100, "service").await.expect("Route failed");
        task.await;
    }

    #[fuchsia::test]
    async fn test_create_aggregate_dictionary() {
        let (proxy, mut stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let util = DictionaryUtil::new(proxy);

        let task = fuchsia_async::Task::local(async move {
            // dictionary_create
            if let Some(Ok(fsandbox::CapabilityStoreRequest::DictionaryCreate { id, responder })) =
                stream.next().await
            {
                assert_eq!(id, 1);
                responder.send(Ok(())).unwrap();
            } else {
                panic!("Expected DictionaryCreate");
            }

            // create_service_aggregate
            if let Some(Ok(fsandbox::CapabilityStoreRequest::CreateServiceAggregate {
                sources: _,
                responder,
            })) = stream.next().await
            {
                let connector = fsandbox::DirConnector { token: fidl::EventPair::create().0 };
                responder.send(Ok(connector)).unwrap();
            } else {
                panic!("Expected CreateServiceAggregate");
            }

            // import
            if let Some(Ok(fsandbox::CapabilityStoreRequest::Import {
                id,
                capability,
                responder,
            })) = stream.next().await
            {
                assert_eq!(id, 2);
                assert!(matches!(capability, Capability::DirConnector(_)));
                responder.send(Ok(())).unwrap();
            } else {
                panic!("Expected Import");
            }

            // dictionary_insert
            if let Some(Ok(fsandbox::CapabilityStoreRequest::DictionaryInsert {
                id,
                item,
                responder,
            })) = stream.next().await
            {
                assert_eq!(id, 1);
                assert_eq!(item.key, "service");
                assert_eq!(item.value, 2);
                responder.send(Ok(())).unwrap();
            } else {
                panic!("Expected DictionaryInsert");
            }
        });

        let mut sources = HashMap::new();
        sources.insert(
            "service".to_string(),
            vec![fsandbox::AggregateSource {
                dir_connector: Some(fsandbox::DirConnector { token: fidl::EventPair::create().0 }),
                ..Default::default()
            }],
        );
        let id = util.create_aggregate_dictionary(sources).await.expect("Create aggregate failed");
        assert_eq!(id, 1);
        task.await;
    }
}
