// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::framework::capabilities::RemotedRuntimeCapabilities;
use crate::model::component::WeakComponentInstance;
use crate::sandbox_util::take_handle_as_stream;
use ::routing::component_instance::ComponentInstanceInterface;
use cm_types::Path;
use fidl::endpoints;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_sandbox as fsandbox;
use fuchsia_async as fasync;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures::future::BoxFuture;
use futures::prelude::*;
use log::warn;
use namespace::NamespaceError;
use runtime_capabilities::{Dictionary, WeakInstanceToken};
use serve_processargs::{BuildNamespaceError, NamespaceBuilder};
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

pub fn serve(
    server_end: zx::Channel,
    _target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        let source = source.upgrade()?;
        let namespace_scope = source.execution_scope.clone();
        let remote_capabilities = source.context.remote_capabilities().clone();
        let stream = take_handle_as_stream::<fcomponent::NamespaceMarker>(server_end);
        serve_inner(namespace_scope, remote_capabilities, stream, source.as_weak().into())
            .await
            .map_err(Into::into)
    }
    .boxed()
}

async fn serve_inner(
    namespace_scope: ExecutionScope,
    remote_capabilities: Arc<RemotedRuntimeCapabilities>,
    mut stream: fcomponent::NamespaceRequestStream,
    target: Arc<WeakInstanceToken>,
) -> Result<(), fidl::Error> {
    let (store, store_stream) =
        endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
    let target_clone = target.clone();
    let _store_task = fasync::Task::spawn(async move {
        let receiver_scope = fasync::Scope::new();
        let _ = runtime_capabilities::serve_capability_store(
            store_stream,
            &receiver_scope,
            target_clone,
        )
        .await;
    });
    while let Some(request) = stream.try_next().await? {
        let method_name = request.method_name();
        let result =
            handle_request(&namespace_scope, &store, &remote_capabilities, request, target.clone())
                .await;
        match result {
            // If the error was PEER_CLOSED then we don't need to log it as a client can
            // disconnect while we are processing its request.
            Err(error) if !error.is_closed() => {
                warn!(method_name:%, error:%; "Couldn't send Namespace response");
            }
            _ => {}
        }
    }
    Ok(())
}

async fn handle_request(
    namespace_scope: &ExecutionScope,
    store: &fsandbox::CapabilityStoreProxy,
    #[allow(unused)] remote_capabilities: &Arc<RemotedRuntimeCapabilities>,
    request: fcomponent::NamespaceRequest,
    target: Arc<WeakInstanceToken>,
) -> Result<(), fidl::Error> {
    match request {
        fcomponent::NamespaceRequest::Create { entries, responder } => {
            let res = create(namespace_scope, store, entries, target).await;
            responder.send(res)?;
        }
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        fcomponent::NamespaceRequest::Create2 { entries, responder } => {
            let res = create2(namespace_scope, remote_capabilities, entries, target).await;
            responder.send(res)?;
        }
        fcomponent::NamespaceRequest::_UnknownMethod { ordinal, .. } => {
            warn!(ordinal:%; "fuchsia.component/Namespace received unknown method");
        }
    }
    Ok(())
}

async fn create(
    namespace_scope: &ExecutionScope,
    store: &fsandbox::CapabilityStoreProxy,
    entries: Vec<fcomponent::NamespaceInputEntry>,
    target: Arc<WeakInstanceToken>,
) -> Result<Vec<fcomponent::NamespaceEntry>, fcomponent::NamespaceError> {
    let mut namespace_builder =
        NamespaceBuilder::new(namespace_scope.clone(), ignore_not_found(), target);
    for entry in entries {
        const ERR: fcomponent::NamespaceError = fcomponent::NamespaceError::DictionaryRead;

        // This API accepts legacy [Dictionary] channel. Round-trip through the import/export
        // CapabilityStore API to convert the channel to a local Dictionary object that we can
        // enumerate.
        let path = entry.path;
        let dict_id = 1;
        store
            .dictionary_legacy_import(dict_id, entry.dictionary.into())
            .await
            .map_err(|_| ERR)?
            .map_err(|_| ERR)?;
        let dict = store.export(dict_id).await.map_err(|_| ERR)?.map_err(|_| ERR)?;
        let dict = runtime_capabilities::Capability::try_from(dict).map_err(|_| ERR)?;
        let runtime_capabilities::Capability::Dictionary(dict) = dict else {
            return Err(ERR);
        };
        for (key, capability) in dict.enumerate() {
            let path = Path::new(format!("{}/{}", path, key))
                .map_err(|_| fcomponent::NamespaceError::BadEntry)?;
            namespace_builder.add_object(capability, &path).map_err(error_to_fidl)?;
        }
    }
    let namespace = namespace_builder.serve().map_err(error_to_fidl)?;
    let out = namespace.flatten().into_iter().map(Into::into).collect();
    Ok(out)
}

async fn create2(
    namespace_scope: &ExecutionScope,
    remote_capabilities: &Arc<RemotedRuntimeCapabilities>,
    entries: Vec<fcomponent::NamespaceInputEntry2>,
    target: Arc<WeakInstanceToken>,
) -> Result<Vec<fcomponent::NamespaceEntry>, fcomponent::NamespaceError> {
    let mut namespace_builder =
        NamespaceBuilder::new(namespace_scope.clone(), ignore_not_found(), target);
    for entry in entries {
        const ERR: fcomponent::NamespaceError = fcomponent::NamespaceError::DictionaryRead;
        let path = entry.path;

        let Ok(dictionary) = remote_capabilities.get::<Dictionary>(entry.capability) else {
            return Err(ERR);
        };

        for (key, capability) in dictionary.enumerate() {
            let path = Path::new(format!("{}/{}", path, key))
                .map_err(|_| fcomponent::NamespaceError::BadEntry)?;
            namespace_builder.add_object(capability, &path).map_err(error_to_fidl)?;
        }
    }
    let namespace = namespace_builder.serve().map_err(error_to_fidl)?;
    let out = namespace.flatten().into_iter().map(Into::into).collect();
    Ok(out)
}

fn error_to_fidl(e: BuildNamespaceError) -> fcomponent::NamespaceError {
    match e {
        BuildNamespaceError::NamespaceError(e) => match e {
            NamespaceError::Shadow(_) => fcomponent::NamespaceError::Shadow,
            NamespaceError::Duplicate(_) => fcomponent::NamespaceError::Duplicate,
            NamespaceError::EntryError(_) => fcomponent::NamespaceError::BadEntry,
        },
        BuildNamespaceError::Conversion { .. } | BuildNamespaceError::Serve { .. } => {
            fcomponent::NamespaceError::Conversion
        }
    }
}

fn ignore_not_found() -> UnboundedSender<String> {
    let (sender, _receiver) = unbounded();
    sender
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::component::ComponentInstance;
    use crate::model::context::ModelContext;
    use ::routing::bedrock::structured_dict::ComponentInput;
    use ::routing::component_instance::ComponentInstanceInterface;
    use assert_matches::assert_matches;
    use fidl::endpoints::{ProtocolMarker, ServerEnd};
    use fidl_fidl_examples_routing_echo as fecho;
    use fuchsia_async as fasync;
    use fuchsia_component::client;
    use futures::TryStreamExt;
    use runtime_capabilities::{Capability, Connector, Dictionary};
    use std::sync::{Arc, Weak};

    #[cfg(fuchsia_api_level_less_than = "HEAD")]
    use fidl_fuchsia_component_sandbox as fsandbox;

    async fn handle_echo_request_stream(response: &str, mut stream: fecho::EchoRequestStream) {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fecho::EchoRequest::EchoString { value: _, responder } => {
                    responder.send(Some(response)).unwrap();
                }
            }
        }
    }

    async fn new_root() -> Arc<ComponentInstance> {
        ComponentInstance::new_root(
            ComponentInput::default(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "test:///root".parse().unwrap(),
        )
        .await
    }

    async fn namespace(
        instance: &Arc<ComponentInstance>,
    ) -> (fcomponent::NamespaceProxy, fasync::Task<()>) {
        let (proxy, server) = endpoints::create_proxy::<fcomponent::NamespaceMarker>();
        let weak_instance = instance.as_weak();
        let task = fasync::Task::spawn(async move {
            serve(server.into_channel(), weak_instance.clone(), weak_instance).await.unwrap();
        });
        (proxy, task)
    }

    #[cfg(fuchsia_api_level_less_than = "HEAD")]
    #[fuchsia::test]
    async fn namespace_create() {
        let tasks = fasync::Scope::new();
        let root = new_root().await;
        let (namespace_proxy, _task) = namespace(&root).await;

        let (store, stream) =
            endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let root_token = root.as_weak().into();
        tasks.spawn(async move {
            let receiver_scope = fasync::Scope::new();
            runtime_capabilities::serve_capability_store(stream, &receiver_scope, root_token)
                .await
                .unwrap()
        });

        let mut namespace_pairs = vec![];
        let mut next_id = 1;
        for (path, response) in [("/svc", "first"), ("/zzz/svc", "second")] {
            // Initialize the host and sender/receiver pair.
            let (receiver, sender) = Connector::new();

            // Serve an Echo request handler on the Receiver.
            tasks.spawn(async move {
                loop {
                    let msg = receiver.receive().await.unwrap();
                    let stream: fecho::EchoRequestStream =
                        ServerEnd::<fecho::EchoMarker>::from(msg.channel).into_stream();
                    handle_echo_request_stream(response, stream).await;
                }
            });

            // Create a dictionary and add the Sender to it.
            let dict = Dictionary::new();
            dict.insert(
                fecho::EchoMarker::DEBUG_NAME.parse().unwrap(),
                Capability::Connector(sender),
            )
            .expect("dict entry already exists");

            let dict_id = next_id;
            next_id += 1;
            store
                .import(
                    dict_id,
                    Capability::from(dict).into_fsandbox_capability(root.as_weak().into()),
                )
                .await
                .unwrap()
                .unwrap();
            let (client_end, server_end) = fidl::Channel::create();
            store.dictionary_legacy_export(dict_id, server_end).await.unwrap().unwrap();

            namespace_pairs.push(fcomponent::NamespaceInputEntry {
                path: path.into(),
                dictionary: client_end.into(),
            })
        }

        // Convert the dictionaries to a namespace.
        let mut namespace_entries = namespace_proxy.create(namespace_pairs).await.unwrap().unwrap();

        // Confirm that the Sender in the dictionary was converted to a service node, and we
        // can access the Echo protocol (served by the Receiver) through this node.
        let entry = namespace_entries.remove(0);
        assert_matches!(entry.path, Some(p) if p == "/svc");
        let dir = entry.directory.unwrap().into_proxy();
        let echo = client::connect_to_protocol_at_dir_root::<fecho::EchoMarker>(&dir).unwrap();
        let response = echo.echo_string(None).await.unwrap();
        assert_matches!(response, Some(m) if m == "first");

        let entry = namespace_entries.remove(0);
        assert!(namespace_entries.is_empty());
        assert_matches!(entry.path, Some(p) if p == "/zzz/svc");
        let dir = entry.directory.unwrap().into_proxy();
        let echo = client::connect_to_protocol_at_dir_root::<fecho::EchoMarker>(&dir).unwrap();
        let response = echo.echo_string(None).await.unwrap();
        assert_matches!(response, Some(m) if m == "second");
    }

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    #[fuchsia::test]
    async fn namespace_create() {
        let tasks = fasync::Scope::new();
        let root = new_root().await;
        let (namespace_proxy, _task) = namespace(&root).await;

        let mut namespace_pairs = vec![];
        for (path, response) in [("/svc", "first"), ("/zzz/svc", "second")] {
            // Initialize the host and sender/receiver pair.
            let (receiver, sender) = Connector::new();

            // Serve an Echo request handler on the Receiver.
            tasks.spawn(async move {
                loop {
                    let channel = receiver.receive().await.unwrap();
                    let stream: fecho::EchoRequestStream =
                        ServerEnd::<fecho::EchoMarker>::from(channel).into_stream();
                    handle_echo_request_stream(response, stream).await;
                }
            });

            // Create a dictionary and add the Sender to it.
            let dictionary = Dictionary::new();
            let prev = dictionary.insert(
                fecho::EchoMarker::DEBUG_NAME.parse().unwrap(),
                Capability::Connector(sender),
            );
            assert!(prev.is_none(), "dict entry already exists");

            let (dictionary_handle, handle_other_end) = zx::EventPair::create();
            root.context.remote_capabilities().store(handle_other_end, dictionary).unwrap();

            namespace_pairs.push(fcomponent::NamespaceInputEntry2 {
                path: path.into(),
                capability: dictionary_handle,
            })
        }

        // Convert the dictionaries to a namespace.
        let mut namespace_entries =
            namespace_proxy.create2(namespace_pairs).await.unwrap().unwrap();

        // Confirm that the Sender in the dictionary was converted to a service node, and we
        // can access the Echo protocol (served by the Receiver) through this node.
        let entry = namespace_entries.remove(0);
        assert_matches!(entry.path, Some(p) if p == "/svc");
        let dir = entry.directory.unwrap().into_proxy();
        let echo = client::connect_to_protocol_at_dir_root::<fecho::EchoMarker>(&dir).unwrap();
        let response = echo.echo_string(None).await.unwrap();
        assert_matches!(response, Some(m) if m == "first");

        let entry = namespace_entries.remove(0);
        assert!(namespace_entries.is_empty());
        assert_matches!(entry.path, Some(p) if p == "/zzz/svc");
        let dir = entry.directory.unwrap().into_proxy();
        let echo = client::connect_to_protocol_at_dir_root::<fecho::EchoMarker>(&dir).unwrap();
        let response = echo.echo_string(None).await.unwrap();
        assert_matches!(response, Some(m) if m == "second");
    }

    #[fuchsia::test]
    async fn namespace_create_err_shadow() {
        let tasks = fasync::Scope::new();
        let root = new_root().await;
        let (namespace_proxy, _task) = namespace(&root).await;

        // Two entries with a shadowing path.
        let mut namespace_pairs = vec![];
        #[allow(unused)]
        let mut next_id = 1;
        for path in ["/svc", "/svc/shadow"] {
            // Initialize the host and sender/receiver pair.
            let (receiver, sender) = Connector::new();

            // Serve an Echo request handler on the Receiver.
            tasks.spawn(async move {
                while let Some(channel) = receiver.receive().await {
                    let stream: fecho::EchoRequestStream =
                        ServerEnd::<fecho::EchoMarker>::from(channel).into_stream();
                    handle_echo_request_stream("hello", stream).await;
                }
            });

            // Create a dictionary and add the Sender to it.
            let dict = Dictionary::new();
            let prev = dict.insert(
                fecho::EchoMarker::DEBUG_NAME.parse().unwrap(),
                Capability::Connector(sender),
            );
            assert!(prev.is_none(), "dict entry already exists");

            #[cfg(fuchsia_api_level_less_than = "HEAD")]
            {
                let (store, stream) =
                    endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
                let root_token = root.as_weak().into();
                tasks.spawn(async move {
                    let receiver_scope = fasync::Scope::new();
                    runtime_capabilities::serve_capability_store(
                        stream,
                        &receiver_scope,
                        root_token,
                    )
                    .await
                    .unwrap()
                });

                let dict_id = next_id;
                next_id += 1;
                store
                    .import(
                        dict_id,
                        Capability::from(dict).into_fsandbox_capability(root.as_weak().into()),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                let (client_end, server_end) = fidl::Channel::create();
                store.dictionary_legacy_export(dict_id, server_end).await.unwrap().unwrap();

                namespace_pairs.push(fcomponent::NamespaceInputEntry {
                    path: path.into(),
                    dictionary: client_end.into(),
                });
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            {
                let (dictionary_handle, dictionary_handle_other_end) = zx::EventPair::create();
                root.context
                    .remote_capabilities()
                    .store(dictionary_handle_other_end, dict)
                    .unwrap();
                namespace_pairs.push(fcomponent::NamespaceInputEntry2 {
                    path: path.into(),
                    capability: dictionary_handle,
                });
            }
        }
        // Try to convert the dictionaries to a namespace. Expect an error because one path
        // shadows another.
        #[cfg(fuchsia_api_level_less_than = "HEAD")]
        let res = namespace_proxy.create(namespace_pairs).await.unwrap();
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        let res = namespace_proxy.create2(namespace_pairs).await.unwrap();
        assert_matches!(res, Err(fcomponent::NamespaceError::Shadow));
    }

    #[fuchsia::test]
    async fn namespace_create_err_dict_read() {
        let root = new_root().await;
        let (namespace_proxy, _task) = namespace(&root).await;

        // Create a dictionary and close the server end.
        #[cfg(fuchsia_api_level_less_than = "HEAD")]
        let res = {
            let (dict_proxy, _stream) =
                endpoints::create_proxy_and_stream::<fsandbox::DictionaryMarker>();
            let namespace_pairs = vec![fcomponent::NamespaceInputEntry {
                path: "/svc".into(),
                dictionary: dict_proxy.into_channel().unwrap().into_zx_channel().into(),
            }];
            namespace_proxy.create(namespace_pairs).await.unwrap()
        };
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        let res = {
            let (e1, _e2) = zx::EventPair::create();
            let namespace_pairs =
                vec![fcomponent::NamespaceInputEntry2 { path: "/svc".into(), capability: e1 }];
            namespace_proxy.create2(namespace_pairs).await.unwrap()
        };

        // Try to convert the dictionaries to a namespace. Expect an error because the dictionary
        // was unreadable.
        assert_matches!(res, Err(fcomponent::NamespaceError::DictionaryRead));
    }
}
