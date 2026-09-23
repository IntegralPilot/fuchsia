// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Component that serves the `fuchsia.element.Manager` protocol.
//!
//! Elements launched through the `Manager` protocol are created in a collection as
//! children in of this component.
//!
//! # Lifecycle
//!
//! If a client closes its connection to `Manager`, any running elements that it
//! has proposed without an associated `fuchsia.element.Controller` will continue to run.

mod annotation;
mod element;
mod element_manager;

use crate::element_manager::{CollectionConfig, ElementManager};
use anyhow::Error;
use element_config::Config;
use fidl_connector::ServiceReconnector;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::{error, info};
use std::rc::Rc;
use {fidl_fuchsia_component as fcomponent, fidl_fuchsia_element as felement};

/// This enum allows the session to match on incoming messages.
enum ExposedServices {
    Manager(felement::ManagerRequestStream),
}

/// The maximum number of concurrent requests.
const NUM_CONCURRENT_REQUESTS: usize = 5;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    info!("Starting.");
    let config = Config::take_from_startup_handle();
    let inspector = fuchsia_inspect::component::inspector();
    inspector.root().record_child("config", |config_node| config.record_inspect(config_node));

    let realm = connect_to_protocol::<fcomponent::RealmMarker>()
        .expect("Failed to connect to Realm service");

    let graphical_presenter =
        Box::new(ServiceReconnector::<felement::GraphicalPresenterMarker>::new());

    let collection_config = CollectionConfig {
        url_to_collection: config
            .url_to_collection
            .into_iter()
            .map(|rule: String| -> (String, String) {
                let split: Vec<&str> = rule.split("|").collect();
                assert_eq!(split.len(), 2, "malformed url_to_collection entry: {:?}", rule);
                (split[0].to_owned(), split[1].to_owned())
            })
            .collect(),
        default_collection: config.default_collection.to_string(),
    };

    info!("Element collection policy: #{:?}", collection_config);

    let element_manager =
        Rc::new(ElementManager::new(realm, Some(graphical_presenter), collection_config));

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(ExposedServices::Manager);

    fs.take_and_serve_directory_handle()?;

    if let Err(err) = element_manager.start_persistent_elements().await {
        error!(err:?; "Unable to start persistent elements");
    }

    fs.for_each_concurrent(NUM_CONCURRENT_REQUESTS, |service_request: ExposedServices| async {
        let element_manager = element_manager.clone();
        match service_request {
            ExposedServices::Manager(request_stream) => {
                if let Err(err) = element_manager.handle_requests(request_stream).await {
                    error!(err:?; "Failed to handle element manager requests");
                }
            }
        }
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::element_manager::{CollectionConfig, ElementManager};
    use fidl::endpoints::{Proxy, create_proxy_and_stream};
    use fidl_connector::Connect;
    use fidl_test_util::spawn_stream_handler;
    use futures::channel::mpsc;
    use futures::{SinkExt, StreamExt};
    use session_testing::spawn_directory_server;
    use std::collections::HashMap;
    use std::sync::LazyLock;
    use test_util::Counter;
    use zx::sys::ZX_OK;
    use {
        fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
        fidl_fuchsia_element as felement, fidl_fuchsia_io as fio, fuchsia_async as fasync,
    };

    /// Spawns a local `Manager` server.
    ///
    /// # Parameters
    /// - `element_manager`: The `ElementManager` that Manager uses to launch elements.
    ///
    /// # Returns
    /// A `ManagerProxy` to the spawned server and the spawned task.
    fn spawn_manager_server(
        element_manager: Box<ElementManager>,
    ) -> (felement::ManagerProxy, fasync::Task<()>) {
        let (proxy, stream) = create_proxy_and_stream::<felement::ManagerMarker>();

        (
            proxy,
            fasync::Task::spawn(async move {
                element_manager
                    .handle_requests(stream)
                    .await
                    .expect("Failed to handle element manager requests");
            }),
        )
    }

    struct MockConnector<T: Proxy> {
        proxy: Option<T>,
    }

    impl<T: Proxy + Clone> MockConnector<T> {
        fn new(proxy: T) -> Self {
            Self { proxy: Some(proxy) }
        }
    }

    impl<T: Proxy + Clone> Connect for MockConnector<T> {
        type Proxy = T;

        fn connect(&self) -> Result<Self::Proxy, anyhow::Error> {
            self.proxy.clone().ok_or_else(|| anyhow::anyhow!("no proxy available"))
        }
    }

    /// Tests that ProposeElement launches the element as a child in a realm.
    #[fuchsia::test]
    async fn propose_element_launches_element() {
        static CREATE_CHILD_CALL_COUNT: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));

        let component_url = "fuchsia-pkg://fuchsia.com/simple_element#meta/simple_element.cm";

        let collection_config = CollectionConfig {
            url_to_collection: HashMap::new(),
            default_collection: "elements".to_string(),
        };

        let directory_request_handler = move |directory_request| match directory_request {
            fio::DirectoryRequest::ReadDirents { responder, .. } => {
                responder.send(ZX_OK, &[]).expect("Failed to service ReadDirents");
            }
            fio::DirectoryRequest::Rewind { responder } => {
                responder.send(ZX_OK).expect("Failed to service Rewind");
            }
            _ => panic!("Directory handler received an unexpected request"),
        };

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::CreateChild {
                    collection: _,
                    decl,
                    args: _,
                    responder,
                } => {
                    assert_eq!(decl.url.unwrap(), component_url);
                    assert_eq!(decl.startup.unwrap(), fdecl::StartupMode::Eager);
                    CREATE_CHILD_CALL_COUNT.inc();
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child: _, exposed_dir, responder } => {
                    spawn_directory_server(exposed_dir, directory_request_handler);
                    let _ = responder.send(Ok(()));
                }
                _ => {
                    panic!("Realm handler received unexpected request");
                }
            }
        });

        let graphical_presenter =
            spawn_stream_handler(move |graphical_presenter_request| async move {
                match graphical_presenter_request {
                    felement::GraphicalPresenterRequest::PresentView {
                        view_spec: _,
                        annotation_controller: _,
                        view_controller_request: _,
                        responder,
                    } => {
                        let _ = responder.send(Ok(()));
                    }
                }
            });
        let graphical_presenter_connector = Box::new(MockConnector::new(graphical_presenter));

        let element_manager: Box<ElementManager> = Box::new(ElementManager::new(
            realm,
            Some(graphical_presenter_connector),
            collection_config,
        ));
        let (manager_proxy, _task) = spawn_manager_server(element_manager);

        let result = manager_proxy
            .propose_element(
                felement::Spec {
                    component_url: Some(component_url.to_string()),
                    ..Default::default()
                },
                None,
            )
            .await;
        assert!(result.is_ok());

        assert_eq!(CREATE_CHILD_CALL_COUNT.get(), 1);
    }

    #[fuchsia::test]
    async fn propose_persistent_element() {
        static CREATE_CHILD_CALL_COUNT: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));

        let component_url = "fuchsia-pkg://fuchsia.com/simple_element#meta/simple_element.cm";

        let collection_config = CollectionConfig {
            url_to_collection: HashMap::new(),
            default_collection: "elements".to_string(),
        };

        let directory_request_handler = move |directory_request| match directory_request {
            fio::DirectoryRequest::ReadDirents { responder, .. } => {
                responder.send(ZX_OK, &[]).expect("Failed to service ReadDirents");
            }
            fio::DirectoryRequest::Rewind { responder } => {
                responder.send(ZX_OK).expect("Failed to service Rewind");
            }
            _ => panic!("Directory handler received an unexpected request"),
        };

        let (destroy_child_sender, mut destroy_child_receiver) = mpsc::unbounded();

        let realm: fcomponent::RealmProxy = spawn_stream_handler(move |realm_request| {
            let mut destroy_child_sender = destroy_child_sender.clone();
            async move {
                match realm_request {
                    fcomponent::RealmRequest::CreateChild {
                        collection: _,
                        decl,
                        args: _,
                        responder,
                    } => {
                        assert_eq!(decl.url.unwrap(), component_url);
                        assert_eq!(decl.name.unwrap(), "foo");
                        assert_eq!(decl.startup.unwrap(), fdecl::StartupMode::Eager);
                        CREATE_CHILD_CALL_COUNT.inc();
                        let _ = responder.send(Ok(()));
                    }
                    fcomponent::RealmRequest::OpenExposedDir {
                        child: _,
                        exposed_dir,
                        responder,
                    } => {
                        spawn_directory_server(exposed_dir, directory_request_handler);
                        let _ = responder.send(Ok(()));
                    }
                    fcomponent::RealmRequest::DestroyChild { child, responder } => {
                        assert_eq!(child.name, "foo");
                        let _ = destroy_child_sender.send(()).await;
                        let _ = responder.send(Ok(()));
                    }
                    _ => {
                        panic!("Realm handler received unexpected request");
                    }
                }
            }
        });

        let graphical_presenter: felement::GraphicalPresenterProxy =
            spawn_stream_handler(move |graphical_presenter_request| async move {
                match graphical_presenter_request {
                    felement::GraphicalPresenterRequest::PresentView {
                        view_spec: _,
                        annotation_controller: _,
                        view_controller_request: _,
                        responder,
                    } => {
                        let _ = responder.send(Ok(()));
                    }
                }
            });

        {
            let element_manager: Box<ElementManager> = Box::new(ElementManager::new(
                realm.clone(),
                Some(Box::new(MockConnector::new(graphical_presenter.clone()))),
                collection_config.clone(),
            ));
            let (manager_proxy, _task) = spawn_manager_server(element_manager);

            let result = manager_proxy
                .propose_element(
                    felement::Spec {
                        component_url: Some(component_url.to_string()),
                        annotations: Some(vec![
                            felement::Annotation {
                                key: felement::AnnotationKey {
                                    namespace: felement::MANAGER_NAMESPACE.to_string(),
                                    value: felement::ANNOTATION_KEY_PERSIST_ELEMENT.to_string(),
                                },
                                value: felement::AnnotationValue::Text(String::new()),
                            },
                            felement::Annotation {
                                key: felement::AnnotationKey {
                                    namespace: felement::MANAGER_NAMESPACE.to_string(),
                                    value: felement::ANNOTATION_KEY_NAME.to_string(),
                                },
                                value: felement::AnnotationValue::Text("foo".to_string()),
                            },
                        ]),
                        ..Default::default()
                    },
                    None,
                )
                .await;
            assert!(result.is_ok());
        }

        assert_eq!(CREATE_CHILD_CALL_COUNT.get(), 1);

        let _ = destroy_child_receiver.next().await.expect("Expected DestroyChild");

        {
            // Launch another instance of element manager and it should launch the child again.
            let element_manager: Box<ElementManager> = Box::new(ElementManager::new(
                realm.clone(),
                Some(Box::new(MockConnector::new(graphical_presenter.clone()))),
                collection_config.clone(),
            ));

            element_manager
                .start_persistent_elements()
                .await
                .expect("start_persistent_elements failed");

            let (manager_proxy, _task) = spawn_manager_server(element_manager);

            assert_eq!(CREATE_CHILD_CALL_COUNT.get(), 2);

            // Now remove the element.
            manager_proxy
                .remove_element("foo")
                .await
                .expect("remove_element failed (fidl)")
                .expect("remove_element failed");

            // That should destroy the child.
            let _ = destroy_child_receiver.next().await.expect("Expected DestroyChild");
        }

        {
            // And this time, when we launch again, it should *not* launch the element.
            let element_manager: Box<ElementManager> = Box::new(ElementManager::new(
                realm,
                Some(Box::new(MockConnector::new(graphical_presenter))),
                collection_config,
            ));

            element_manager
                .start_persistent_elements()
                .await
                .expect("start_persistent_elements failed");

            let (manager_proxy, _task) = spawn_manager_server(element_manager);

            // There should be no change in the count.
            assert_eq!(CREATE_CHILD_CALL_COUNT.get(), 2);

            // If we try and remove the element, it should report not found.
            assert_eq!(
                manager_proxy
                    .remove_element("foo")
                    .await
                    .expect("remove_element failed (fidl)")
                    .expect_err("remove_element succeeded"),
                felement::ManagerError::NotFound
            );
        }
    }
}
