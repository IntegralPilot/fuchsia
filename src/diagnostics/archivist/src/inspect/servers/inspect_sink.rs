// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::router::EventConsumer;
use crate::events::types::{Event, EventPayload, InspectSinkRequestedPayload};
use crate::identity::ComponentIdentity;
use crate::inspect::container::InspectHandle;
use crate::inspect::repository::InspectRepository;
use anyhow::Error;
use fidl::endpoints::{ControlHandle, Responder};
use futures::StreamExt;
use log::{debug, warn};
use std::sync::Arc;
use {fidl_fuchsia_inspect as finspect, fuchsia_async as fasync};

pub struct InspectSinkServer {
    /// Shared repository holding the Inspect handles.
    repo: Arc<InspectRepository>,

    /// Scope holding all tasks associated with this server.
    scope: fasync::Scope,
}

impl InspectSinkServer {
    /// Construct a server.
    pub fn new(repo: Arc<InspectRepository>, scope: fasync::Scope) -> Self {
        Self { repo, scope }
    }

    /// Handle incoming events. Mainly for use in EventConsumer impl.
    fn spawn(&self, component: Arc<ComponentIdentity>, stream: finspect::InspectSinkRequestStream) {
        let repo = Arc::clone(&self.repo);
        self.scope.spawn(async move {
            if let Err(e) = Self::handle_requests(repo, component, stream).await {
                warn!("error handling InspectSink requests: {e}");
            }
        });
    }

    async fn handle_requests(
        repo: Arc<InspectRepository>,
        component: Arc<ComponentIdentity>,
        mut stream: finspect::InspectSinkRequestStream,
    ) -> Result<(), Error> {
        while let Some(Ok(request)) = stream.next().await {
            match request {
                finspect::InspectSinkRequest::Publish {
                    payload: finspect::InspectSinkPublishRequest { tree: Some(tree), name, .. },
                    ..
                } => repo.add_inspect_handle(
                    Arc::clone(&component),
                    InspectHandle::tree(tree.into_proxy(), name),
                ),
                finspect::InspectSinkRequest::Publish {
                    payload: finspect::InspectSinkPublishRequest { tree: None, name, .. },
                    control_handle,
                } => {
                    warn!(name:?, component:%; "InspectSink/Publish without a tree");
                    control_handle.shutdown();
                }
                finspect::InspectSinkRequest::Escrow {
                    payload:
                        finspect::InspectSinkEscrowRequest {
                            vmo: Some(vmo),
                            name,
                            token: Some(token),
                            tree,
                            ..
                        },
                    ..
                } => {
                    repo.escrow_handle(
                        Arc::clone(&component),
                        vmo,
                        token,
                        name,
                        tree.map(zx::Koid::from_raw),
                    );
                }
                finspect::InspectSinkRequest::Escrow {
                    control_handle,
                    payload: finspect::InspectSinkEscrowRequest { vmo, token, .. },
                } => {
                    warn!(
                        component:%,
                        has_vmo = vmo.is_some(),
                        has_token = token.is_some();
                        "Attempted to escrow inspect without required data"
                    );
                    control_handle.shutdown();
                }
                finspect::InspectSinkRequest::FetchEscrow {
                    responder,
                    payload:
                        finspect::InspectSinkFetchEscrowRequest { tree, token: Some(token), .. },
                } => {
                    let vmo = repo.fetch_escrow(Arc::clone(&component), token, tree);
                    if let Err(err) = responder.send(finspect::InspectSinkFetchEscrowResponse {
                        vmo,
                        ..Default::default()
                    }) {
                        debug!("Failed sending FetchEscrowResponse: {err:?}");
                    }
                }
                finspect::InspectSinkRequest::FetchEscrow {
                    responder,
                    payload: finspect::InspectSinkFetchEscrowRequest { token: None, .. },
                } => {
                    warn!(component:%; "Attempted to fetch escrowed inspect with invalid data");
                    responder.control_handle().shutdown();
                }
                finspect::InspectSinkRequest::_UnknownMethod {
                    ordinal,
                    control_handle,
                    method_type,
                    ..
                } => {
                    warn!(ordinal, method_type:?; "Received unknown request for InspectSink");
                    // Close the connection if we receive an unknown interaction.
                    control_handle.shutdown();
                }
            }
        }

        Ok(())
    }
}

impl EventConsumer for InspectSinkServer {
    fn handle(self: Arc<Self>, event: Event) {
        match event.payload {
            EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                component,
                request_stream,
            }) => {
                self.spawn(component, request_stream);
            }
            _ => unreachable!("InspectSinkServer is only subscribed to InspectSinkRequested"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::router::EventConsumer;
    use crate::events::types::{Event, EventPayload, InspectSinkRequestedPayload};
    use crate::identity::ComponentIdentity;
    use crate::inspect::container::InspectHandle;
    use crate::inspect::repository::InspectRepository;
    use crate::pipeline::StaticHierarchyAllowlist;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_json_diff;
    use fidl::endpoints::{ClientEnd, create_proxy_and_stream};
    use fidl_fuchsia_diagnostics as fdiagnostics;
    use fidl_fuchsia_inspect::{
        InspectSinkMarker, InspectSinkProxy, InspectSinkPublishRequest, TreeMarker,
    };
    use fuchsia_inspect::reader::read;
    use fuchsia_inspect::{Inspector, InspectorConfig};
    use futures::Future;
    use inspect_runtime::TreeServerSendPreference;
    use inspect_runtime::service::spawn_tree_server_with_stream;
    use selectors::VerboseError;
    use std::collections::HashMap;
    use std::sync::Arc;
    use zx::{self as zx, AsHandleRef};

    struct TestHarness {
        /// The underlying repository.
        repo: Arc<InspectRepository>,

        /// Component-specific state.
        components: HashMap<Arc<ComponentIdentity>, TestComponent>,

        /// The server that would be held by the Archivist.
        _server: Arc<InspectSinkServer>,

        /// Scope running InspectSinkServer.
        scope: Option<fasync::Scope>,
    }

    struct TestComponent {
        proxy: Option<InspectSinkProxy>,
        /// Scope running Tree server(s).
        scope: Option<fasync::Scope>,
    }

    impl TestHarness {
        /// Construct an InspectSinkServer with a ComponentIdentity/InspectSinkProxy pair
        /// for each input ComponentIdentity.
        fn new(identity: Vec<Arc<ComponentIdentity>>) -> Self {
            let repo = Arc::new(InspectRepository::new(vec![], fasync::Scope::new()));
            let scope = fasync::Scope::new();
            let server = Arc::new(InspectSinkServer::new(Arc::clone(&repo), scope.new_child()));

            let components = identity
                .into_iter()
                .map(|id| {
                    let (proxy, request_stream) = create_proxy_and_stream::<InspectSinkMarker>();

                    Arc::clone(&server).handle(Event {
                        timestamp: zx::BootInstant::get(),
                        payload: EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                            component: Arc::clone(&id),
                            request_stream,
                        }),
                    });

                    (id, TestComponent { proxy: Some(proxy), scope: Some(fasync::Scope::new()) })
                })
                .collect();

            Self { repo, _server: server, scope: Some(scope), components }
        }

        /// Publish `tree` via the proxy associated with `component`.
        fn publish(
            &mut self,
            id: &Arc<ComponentIdentity>,
            tree: ClientEnd<TreeMarker>,
        ) -> zx::Koid {
            let koid = tree.as_handle_ref().koid().unwrap();
            let component = self.components.get(id).expect("unknown component");
            let proxy = component.proxy.as_ref().expect("InspectSink proxy stopped");
            proxy
                .publish(InspectSinkPublishRequest { tree: Some(tree), ..Default::default() })
                .unwrap();
            koid
        }

        /// Start a TreeProxy server and return the proxy.
        fn serve(
            &mut self,
            component: &Arc<ComponentIdentity>,
            inspector: Inspector,
            settings: TreeServerSendPreference,
        ) -> ClientEnd<TreeMarker> {
            let (tree, stream) = fidl::endpoints::create_request_stream::<TreeMarker>();
            self.serve_stream(component, inspector, settings, stream);
            tree
        }

        /// Start a TreeProxy server over the provided ServerEnd.
        fn serve_stream(
            &mut self,
            component: &Arc<ComponentIdentity>,
            inspector: Inspector,
            settings: TreeServerSendPreference,
            stream: finspect::TreeRequestStream,
        ) {
            let component = self.components.get_mut(component).expect("unknown component");
            let scope = component.scope.as_ref().expect("already dropped tree server");
            spawn_tree_server_with_stream(inspector, settings, stream, scope);
        }

        /// Drop the server(s) associated with `component`, as initialized by `serve`.
        async fn drop_tree_servers(&mut self, component: &Arc<ComponentIdentity>) {
            let component = self.components.get_mut(component).expect("unknown component");
            let scope = component.scope.take().expect("tree server(s) already dropped");
            scope.cancel().await;
        }

        /// Execute closure `assertions` on the `InspectArtifactsContainer` associated with
        /// `identity`.
        ///
        /// This function will wait for data to be available in `self.repo`, and therefore
        /// might hang indefinitely if the data never appears. This is not a problem since
        /// it is a unit test and `fx test` has timeouts available.
        async fn assert<const N: usize, F, Fut>(
            &self,
            identity: &Arc<ComponentIdentity>,
            koids: [zx::Koid; N],
            assertions: F,
        ) where
            F: FnOnce([Arc<InspectHandle>; N]) -> Fut,
            Fut: Future<Output = ()>,
        {
            self.assert_on_selector(
                identity,
                koids,
                assertions,
                selectors::parse_selector::<VerboseError>(&format!("{identity}:root"))
                    .expect("parse selector"),
            )
            .await
        }

        async fn assert_on_selector<const N: usize, F, Fut>(
            &self,
            identity: &Arc<ComponentIdentity>,
            koids: [zx::Koid; N],
            assertions: F,
            selector: fdiagnostics::Selector,
        ) where
            F: FnOnce([Arc<InspectHandle>; N]) -> Fut,
            Fut: Future<Output = ()>,
        {
            self.repo.wait_for_artifact(identity).await;
            let containers = self.repo.fetch_inspect_data(
                &Some(vec![selector]),
                StaticHierarchyAllowlist::new_disabled(),
            );
            assert_eq!(containers.len(), 1);
            assertions(
                koids
                    .iter()
                    .map(|koid| {
                        containers[0]
                            .inspect_handles
                            .iter()
                            .filter_map(|h| h.upgrade())
                            .find(|handle| handle.koid() == *koid)
                            .unwrap()
                    })
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap(),
            )
            .await;
        }

        /// Drops all published proxies, stops the InspectSink server, and waits for it to complete.
        async fn stop_all(&mut self) {
            for component in self.components.values_mut() {
                component.proxy = None;
            }
            self.scope.take().unwrap().close().await;
        }
    }

    #[fuchsia::test]
    async fn connect() {
        let identity: Arc<ComponentIdentity> = Arc::new(vec!["a", "b", "foo.cm"].into());

        let mut test = TestHarness::new(vec![Arc::clone(&identity)]);

        let insp = Inspector::default();
        insp.root().record_int("int", 0);
        let tree = test.serve(&identity, insp, TreeServerSendPreference::default());
        let koid = test.publish(&identity, tree);

        test.assert(&identity, [koid], |handles| async move {
            assert_matches!(
                handles[0].as_ref(),
                InspectHandle::Tree { proxy: tree, .. } => {
                   let hierarchy = read(tree).await.unwrap();
                   assert_json_diff!(hierarchy, root: {
                       int: 0i64,
                   });
            });
        })
        .await;
    }

    #[fuchsia::test]
    async fn publish_multiple_times_on_the_same_connection() {
        let identity: Arc<ComponentIdentity> = Arc::new(vec!["a", "b", "foo.cm"].into());

        let mut test = TestHarness::new(vec![Arc::clone(&identity)]);

        let insp = Inspector::default();
        insp.root().record_int("int", 0);
        let tree = test.serve(&identity, insp, TreeServerSendPreference::default());

        let other_insp = Inspector::default();
        other_insp.root().record_double("double", 1.24);
        let other_tree = test.serve(&identity, other_insp, TreeServerSendPreference::default());

        let koid0 = test.publish(&identity, tree);
        let koid1 = test.publish(&identity, other_tree);

        test.assert(&identity, [koid0, koid1], |handles| async move {
            assert_matches!(
                handles[0].as_ref(),
                InspectHandle::Tree { proxy: tree, ..} => {
                   let hierarchy = read(tree).await.unwrap();
                   assert_json_diff!(hierarchy, root: {
                       int: 0i64,
                   });
            });

            assert_matches!(
                handles[1].as_ref(),
                InspectHandle::Tree { proxy: tree, .. } => {
                   let hierarchy = read(tree).await.unwrap();
                   assert_json_diff!(hierarchy, root: {
                       double: 1.24,
                   });
            });
        })
        .await;
    }

    #[fuchsia::test]
    async fn tree_remains_after_inspect_sink_disconnects() {
        let identity: Arc<ComponentIdentity> = Arc::new(vec!["a", "b", "foo.cm"].into());

        let mut test = TestHarness::new(vec![Arc::clone(&identity)]);

        let insp = Inspector::default();
        insp.root().record_int("int", 0);
        let tree = test.serve(&identity, insp, TreeServerSendPreference::default());
        let koid = test.publish(&identity, tree);

        test.assert(&identity, [koid], |handles| async move {
            assert_matches!(
                handles[0].as_ref(),
                InspectHandle::Tree { proxy: tree, .. } => {
                   let hierarchy = read(tree).await.unwrap();
                   assert_json_diff!(hierarchy, root: {
                       int: 0i64,
                   });
            });
        })
        .await;

        test.stop_all().await;

        // the data must remain present as long as the tree server started above is alive
        test.assert(&identity, [koid], |handles| async move {
            assert_matches!(
                handles[0].as_ref(),
                InspectHandle::Tree { proxy: tree, ..} => {
                   let hierarchy = read(tree).await.unwrap();
                   assert_json_diff!(hierarchy, root: {
                       int: 0i64,
                   });
            });
        })
        .await;
    }

    #[fuchsia::test]
    async fn connect_with_multiple_proxies() {
        let identities: Vec<Arc<ComponentIdentity>> = vec![
            Arc::new(vec!["a", "b", "foo.cm"].into()),
            Arc::new(vec!["a", "b", "foo2.cm"].into()),
        ];

        let mut test = TestHarness::new(identities.clone());

        let insp = Inspector::default();
        insp.root().record_int("int", 0);
        let tree = test.serve(&identities[0], insp, TreeServerSendPreference::default());

        let insp2 = Inspector::default();
        insp2.root().record_bool("is_insp2", true);
        let tree2 = test.serve(&identities[1], insp2, TreeServerSendPreference::default());

        let koid_component_0 = test.publish(&identities[0], tree);
        let koid_component_1 = test.publish(&identities[1], tree2);

        test.assert(&identities[0], [koid_component_0], |handles| async move {
            assert_matches!(
                handles[0].as_ref(),
                InspectHandle::Tree { proxy: tree, .. } => {
                   let hierarchy = read(tree).await.unwrap();
                   assert_json_diff!(hierarchy, root: {
                       int: 0i64,
                   });
            });
        })
        .await;

        test.assert(&identities[1], [koid_component_1], |handles| async move {
            assert_matches!(
                handles[0].as_ref(),
                InspectHandle::Tree { proxy: tree, .. } => {
                   let hierarchy = read(tree).await.unwrap();
                   assert_json_diff!(hierarchy, root: {
                       is_insp2: true,
                   });
            });
        })
        .await;
    }

    #[fuchsia::test]
    async fn dropping_tree_removes_component_identity_from_repo() {
        let identity: Arc<ComponentIdentity> = Arc::new(vec!["a", "b", "foo.cm"].into());

        let mut test = TestHarness::new(vec![Arc::clone(&identity)]);

        let tree = test.serve(&identity, Inspector::default(), TreeServerSendPreference::default());
        let koid = test.publish(&identity, tree);

        test.stop_all().await;

        // this executing to completion means the identity was present
        test.assert(&identity, [koid], |handles: [_; 1]| {
            assert_eq!(handles.len(), 1);
            async {}
        })
        .await;

        test.drop_tree_servers(&identity).await;

        // this executing to completion means the identity is not there anymore; we know
        // it previously was present
        test.repo.wait_until_gone(&identity).await;
    }

    #[fuchsia::test]
    async fn fetch_escrow() {
        let identity: Arc<ComponentIdentity> = Arc::new(vec!["a", "b", "foo.cm"].into());
        let test = TestHarness::new(vec![Arc::clone(&identity)]);
        let proxy = test.components.get(&identity).unwrap().proxy.as_ref().unwrap();

        // Escrow a VMO.
        let vmo = {
            let inspector = Inspector::default();
            inspector.root().record_int("val", 123);
            inspector.frozen_vmo_copy().unwrap()
        };
        let (token0, token1) = zx::EventPair::create();

        proxy
            .escrow(finspect::InspectSinkEscrowRequest {
                vmo: Some(vmo),
                token: Some(finspect::EscrowToken { token: token0 }),
                ..Default::default()
            })
            .unwrap();

        // Fetch the escrowed VMO.
        let vmo = assert_matches!(
            proxy
                .fetch_escrow(finspect::InspectSinkFetchEscrowRequest {
                    token: Some(finspect::EscrowToken { token: token1 }),
                    ..Default::default()
                })
                .await,
            Ok(finspect::InspectSinkFetchEscrowResponse { vmo: Some(vmo), .. }) => vmo
        );

        // Verify the fetched VMO.
        let inspector = Inspector::new(InspectorConfig::default().vmo(vmo));
        let hierarchy = read(&inspector).await.unwrap();
        assert_json_diff!(hierarchy, root: {
            val: 123i64,
        });
    }

    #[fuchsia::test]
    async fn fetch_escrow_with_tree() {
        let identity: Arc<ComponentIdentity> = Arc::new(vec!["a", "b", "foo.cm"].into());
        let mut test = TestHarness::new(vec![Arc::clone(&identity)]);

        // Escrow a VMO.
        let vmo = {
            let inspector = Inspector::default();
            inspector.root().record_int("val", 123);
            inspector.frozen_vmo_copy().unwrap()
        };
        let (token0, token1) = zx::EventPair::create();

        let proxy = test.components.get(&identity).unwrap().proxy.as_ref().unwrap();
        proxy
            .escrow(finspect::InspectSinkEscrowRequest {
                vmo: Some(vmo),
                token: Some(finspect::EscrowToken { token: token0 }),
                ..Default::default()
            })
            .unwrap();

        // Fetch the escrowed VMO, provide tree to publish with the same name.
        let (tree, stream) = fidl::endpoints::create_request_stream::<TreeMarker>();
        let koid = tree.as_handle_ref().koid().unwrap();
        let resp = proxy
            .fetch_escrow(finspect::InspectSinkFetchEscrowRequest {
                token: Some(finspect::EscrowToken { token: token1 }),
                tree: Some(tree),
                ..Default::default()
            })
            .await;

        let vmo = assert_matches!(
            resp, Ok(finspect::InspectSinkFetchEscrowResponse { vmo: Some(vmo), .. }) => vmo);

        // Verify the fetched VMO.
        let inspector = Inspector::new(InspectorConfig::default().vmo(vmo));
        let hierarchy = read(&inspector).await.unwrap();
        assert_json_diff!(hierarchy, root: {
            val: 123i64,
        });

        // Serve the escrowed VMO.
        test.serve_stream(&identity, inspector, TreeServerSendPreference::default(), stream);

        // Verify the published tree serves the escrowed content.
        test.assert(&identity, [koid], |handles| async move {
            let tree = assert_matches!(
                handles[0].as_ref(),
                InspectHandle::Tree { proxy, .. } => proxy
            );
            let hierarchy = read(tree).await.unwrap();
            assert_json_diff!(hierarchy, root: {
                val: 123i64,
            });
        })
        .await;
    }

    #[fuchsia::test]
    async fn fetch_escrow_with_tree_and_name() {
        const VMO_NAME: &str = "FetchEscrowWithTreeVmo";

        let identity: Arc<ComponentIdentity> = Arc::new(vec!["a", "b", "foo.cm"].into());
        let mut test = TestHarness::new(vec![Arc::clone(&identity)]);

        // Escrow a VMO.
        let vmo = {
            let inspector = Inspector::default();
            inspector.root().record_int("val", 123);
            inspector.frozen_vmo_copy().unwrap()
        };
        let (token0, token1) = zx::EventPair::create();

        let proxy = test.components.get(&identity).unwrap().proxy.as_ref().unwrap();
        proxy
            .escrow(finspect::InspectSinkEscrowRequest {
                vmo: Some(vmo),
                token: Some(finspect::EscrowToken { token: token0 }),
                name: Some(VMO_NAME.to_string()),
                ..Default::default()
            })
            .unwrap();

        // Fetch the escrowed VMO, provide tree to publish with the same name.
        let (tree, stream) = fidl::endpoints::create_request_stream::<TreeMarker>();
        let koid = tree.as_handle_ref().koid().unwrap();
        let resp = proxy
            .fetch_escrow(finspect::InspectSinkFetchEscrowRequest {
                token: Some(finspect::EscrowToken { token: token1 }),
                tree: Some(tree),
                ..Default::default()
            })
            .await;

        let vmo = assert_matches!(
            resp, Ok(finspect::InspectSinkFetchEscrowResponse { vmo: Some(vmo), .. }) => vmo);

        // Verify the fetched VMO.
        let inspector = Inspector::new(InspectorConfig::default().vmo(vmo));
        let hierarchy = read(&inspector).await.unwrap();
        assert_json_diff!(hierarchy, root: {
            val: 123i64,
        });

        // Serve the escrowed VMO.
        test.serve_stream(&identity, inspector, TreeServerSendPreference::default(), stream);

        // Verify the published tree serves the escrowed content.
        test.assert_on_selector(
            &identity,
            [koid],
            |handles| async move {
                let tree = assert_matches!(
                    handles[0].as_ref(),
                    InspectHandle::Tree { proxy, .. } => proxy
                );
                let hierarchy = read(tree).await.unwrap();
                assert_json_diff!(hierarchy, root: {
                    val: 123i64,
                });
            },
            selectors::parse_selector::<VerboseError>(&format!("{identity}:[name={VMO_NAME}]root"))
                .expect("parse selector"),
        )
        .await
    }
}
