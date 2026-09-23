// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.cti

use crate::framework;
use crate::model::component::{ComponentInstance, WeakComponentInstance};
use crate::sandbox_util::take_handle_as_stream;
use ::routing::component_instance::ComponentInstanceInterface;
use anyhow::Error;
use cm_rust::FidlIntoNative;
use cm_types::FLAGS_MAX_POSSIBLE_RIGHTS;
use errors::OpenExposedDirError;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use futures::future::BoxFuture;
use futures::prelude::*;
use log::{debug, error, warn};
use moniker::ChildName;
use std::cmp;
use std::sync::Arc;
use vfs::ToObjectRequest;
use vfs::directory::entry::OpenRequest;
use vfs::path::Path;

pub fn serve(
    server_end: zx::Channel,
    _target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        let stream = take_handle_as_stream::<fcomponent::RealmMarker>(server_end);
        serve_inner(source, stream).await.map_err(Into::into)
    }
    .boxed()
}

async fn serve_inner(
    component: WeakComponentInstance,
    mut stream: fcomponent::RealmRequestStream,
) -> Result<(), fidl::Error> {
    while let Some(request) = stream.try_next().await? {
        let method_name = request.method_name();
        let result = handle_request(request, &component).await;
        match result {
            // If the error was PEER_CLOSED then we don't need to log it as a client can
            // disconnect while we are processing its request.
            Err(error) if !error.is_closed() => {
                warn!(method_name:%, error:%; "Couldn't send Realm response");
            }
            _ => {}
        }
    }
    Ok(())
}

async fn handle_request(
    request: fcomponent::RealmRequest,
    component: &WeakComponentInstance,
) -> Result<(), fidl::Error> {
    match request {
        fcomponent::RealmRequest::CreateChild { responder, collection, decl, args } => {
            let res = async { create_child(component, collection, decl, args).await }.await;
            responder.send(res)?;
        }
        fcomponent::RealmRequest::DestroyChild { responder, child } => {
            let res = destroy_child(component, child).await;
            responder.send(res)?;
        }
        fcomponent::RealmRequest::ListChildren { responder, collection, iter } => {
            let res = list_children(component, collection, iter).await;
            responder.send(res)?;
        }
        fcomponent::RealmRequest::OpenExposedDir { responder, child, exposed_dir } => {
            let res = open_exposed_dir(component, child, exposed_dir).await;
            responder.send(res)?;
        }
        fcomponent::RealmRequest::OpenController { child, controller, responder } => {
            let res = open_controller(component, child, controller).await;
            responder.send(res)?;
        }
        fcomponent::RealmRequest::GetResolvedInfo { responder } => {
            let res = get_resolved_info(component).await;
            responder.send(res)?;
        }
        fcomponent::RealmRequest::GetChildOutputDictionaryDeprecated { responder, child } => {
            let res = get_child_output_dictionary_deprecated(component, child).await;
            responder.send(res)?;
        }
        fcomponent::RealmRequest::GetChildOutputDictionary { responder, child } => {
            let res = get_child_output_dictionary(component, child).await;
            responder.send(res)?;
        }
    }
    Ok(())
}

async fn create_child(
    weak: &WeakComponentInstance,
    collection: fdecl::CollectionRef,
    child_decl: fdecl::Child,
    child_args: fcomponent::CreateChildArgs,
) -> Result<(), fcomponent::Error> {
    let component = weak.upgrade().map_err(|_| fcomponent::Error::InstanceDied)?;

    cm_fidl_validator::validate_dynamic_child(&child_decl).map_err(|error| {
        warn!(error:%; "failed to create dynamic child. child decl is invalid");
        fcomponent::Error::InvalidArguments
    })?;
    let child_decl = child_decl.fidl_into_native();

    component.add_dynamic_child(collection.name.clone(), &child_decl, child_args).await.map_err(
        |err| {
            warn!(
                "Failed to create child \"{}\" in collection \"{}\" of component \"{}\": {}",
                child_decl.name, collection.name, component.moniker, err
            );

            err.into()
        },
    )
}

async fn open_controller(
    component: &WeakComponentInstance,
    child: fdecl::ChildRef,
    controller: ServerEnd<fcomponent::ControllerMarker>,
) -> Result<(), fcomponent::Error> {
    match get_child(component, &child).await? {
        Some(child) => {
            child.execution_scope.spawn(framework::controller::run_controller(
                child.as_weak(),
                controller.into_stream(),
            ));
        }
        None => {
            debug!(child:?; "open_controller() failed: instance not found");
            return Err(fcomponent::Error::InstanceNotFound);
        }
    }
    Ok(())
}

async fn open_exposed_dir(
    component: &WeakComponentInstance,
    child: fdecl::ChildRef,
    exposed_dir: ServerEnd<fio::DirectoryMarker>,
) -> Result<(), fcomponent::Error> {
    match get_child(component, &child).await? {
        Some(child) => {
            // Resolve child in order to instantiate exposed_dir.
            child.resolve().await.map_err(|e| {
                warn!(
                    "resolve failed for child {:?} of component {}: {}",
                    child, component.moniker, e
                );
                return fcomponent::Error::InstanceCannotResolve;
            })?;
            // We request the maximum possible rights from the parent directory connection.
            let flags = FLAGS_MAX_POSSIBLE_RIGHTS | fio::Flags::PROTOCOL_DIRECTORY;
            let mut object_request = flags.to_object_request(exposed_dir);
            child
                .open_exposed(OpenRequest::new(
                    child.execution_scope.clone(),
                    flags,
                    Path::dot(),
                    &mut object_request,
                ))
                .await
                .map_err(|error| match error {
                    OpenExposedDirError::InstanceDestroyed
                    | OpenExposedDirError::InstanceNotResolved => fcomponent::Error::InstanceDied,
                    OpenExposedDirError::Open(_) => fcomponent::Error::Internal,
                })?;
        }
        None => {
            debug!(child:?; "open_exposed_dir() failed: instance not found");
            return Err(fcomponent::Error::InstanceNotFound);
        }
    }
    Ok(())
}

async fn destroy_child(
    component: &WeakComponentInstance,
    child: fdecl::ChildRef,
) -> Result<(), fcomponent::Error> {
    let component = component.upgrade().map_err(|_| fcomponent::Error::InstanceDied)?;
    child.collection.as_ref().ok_or(fcomponent::Error::InvalidArguments)?;
    let child_moniker = ChildName::try_new(&child.name, child.collection.as_ref())
        .map_err(|_| fcomponent::Error::InvalidArguments)?;
    component.remove_dynamic_child(&child_moniker).await.map_err(|error| {
        debug!(error:%, child:?; "remove_dynamic_child() failed");
        error
    })?;
    Ok(())
}

async fn get_child(
    parent: &WeakComponentInstance,
    child: &fdecl::ChildRef,
) -> Result<Option<Arc<ComponentInstance>>, fcomponent::Error> {
    let parent = parent.upgrade().map_err(|_| fcomponent::Error::InstanceDied)?;
    let state = parent.lock_resolved_state().await.map_err(|error| {
        debug!(error:%, moniker:% = parent.moniker; "failed to resolve instance");
        fcomponent::Error::InstanceCannotResolve
    })?;
    let child_moniker = ChildName::try_new(&child.name, child.collection.as_ref())
        .map_err(|_| fcomponent::Error::InvalidArguments)?;
    Ok(state.get_child(&child_moniker).map(|r| r.clone()))
}

async fn list_children(
    component: &WeakComponentInstance,
    collection: fdecl::CollectionRef,
    iter: ServerEnd<fcomponent::ChildIteratorMarker>,
) -> Result<(), fcomponent::Error> {
    let component = component.upgrade().map_err(|_| fcomponent::Error::InstanceDied)?;
    let state = component.lock_resolved_state().await.map_err(|error| {
        error!(error:%; "failed to resolve InstanceState");
        fcomponent::Error::Internal
    })?;
    state
        .collection_decls
        .iter()
        .find(|c| c.name == collection.name)
        .ok_or(fcomponent::Error::CollectionNotFound)?;
    let mut children: Vec<_> = state
        .children()
        .filter_map(|(m, _)| match m.collection() {
            Some(c) => {
                if c.as_str() == &collection.name {
                    Some(fdecl::ChildRef {
                        name: m.name().to_string(),
                        collection: m.collection().map(|s| s.to_string()),
                    })
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    children.sort_unstable_by(|a, b| {
        let a = &a.name;
        let b = &b.name;
        if a == b {
            cmp::Ordering::Equal
        } else if a < b {
            cmp::Ordering::Less
        } else {
            cmp::Ordering::Greater
        }
    });
    let batch_size = component.context.runtime_config().list_children_batch_size;
    let stream = iter.into_stream();
    fasync::Task::spawn(async move {
        if let Err(error) = serve_child_iterator(children, stream, batch_size).await {
            // TODO: Set an epitaph to indicate this was an unexpected error.
            warn!(error:%; "serve_child_iterator failed");
        }
    })
    .detach();
    Ok(())
}

async fn serve_child_iterator(
    children: Vec<fdecl::ChildRef>,
    mut stream: fcomponent::ChildIteratorRequestStream,
    batch_size: usize,
) -> Result<(), Error> {
    let mut iter = children.chunks(batch_size);
    while let Some(request) = stream.try_next().await? {
        match request {
            fcomponent::ChildIteratorRequest::Next { responder } => {
                responder.send(iter.next().unwrap_or(&[]))?;
            }
        }
    }
    Ok(())
}

async fn get_resolved_info(
    component: &WeakComponentInstance,
) -> Result<fresolution::Component, fcomponent::Error> {
    let component = component.upgrade().map_err(|_| fcomponent::Error::InstanceDied)?;
    let resolved_state = component
        .lock_resolved_state()
        .await
        .map_err(|_| fcomponent::Error::InstanceCannotResolve)?;
    Ok((&resolved_state.resolved_component).into())
}

async fn get_child_output_dictionary_deprecated(
    component: &WeakComponentInstance,
    child: fdecl::ChildRef,
) -> Result<fsandbox::DictionaryRef, fcomponent::Error> {
    match get_child(component, &child).await? {
        Some(child) => Ok(child
            .lock_resolved_state()
            .await
            .map_err(|_| fcomponent::Error::InstanceCannotResolve)?
            .sandbox
            .component_output
            .capabilities()
            .to_fsandbox()),
        None => {
            debug!(child:?; "get_child_output_dictionary_deprecated() failed: instance not found");
            Err(fcomponent::Error::InstanceNotFound)
        }
    }
}

async fn get_child_output_dictionary(
    component: &WeakComponentInstance,
    child: fdecl::ChildRef,
) -> Result<zx::EventPair, fcomponent::Error> {
    let Some(child) = get_child(component, &child).await? else {
        debug!(child:?; "get_child_output_dictionary() failed: instance not found");
        return Err(fcomponent::Error::InstanceNotFound);
    };
    let output = child
        .lock_resolved_state()
        .await
        .map_err(|_| fcomponent::Error::InstanceCannotResolve)?
        .sandbox
        .component_output
        .capabilities();
    let (e1, e2) = zx::EventPair::create();
    child.context.remote_capabilities().store(e1, output).expect("we used a valid handle");
    Ok(e2)
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::builtin_environment::BuiltinEnvironment;
    use crate::model::component::StartReason;
    use crate::model::testing::mocks::*;
    use crate::model::testing::out_dir::OutDir;
    use crate::model::testing::test_helpers::*;
    use crate::model::testing::test_hook::*;
    use assert_matches::assert_matches;
    use cm_config::RuntimeConfig;
    use cm_rust::{ComponentDecl, ExposeSource};
    use cm_rust_testing::*;
    use fidl::endpoints;
    use fidl_fidl_examples_routing_echo as echo;
    use fidl_fuchsia_component as fcomponent;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_mem as fmem;
    use fuchsia_async as fasync;
    use fuchsia_component::client;
    use futures::lock::Mutex;
    use hooks::EventType;
    use moniker::Moniker;
    use routing_test_helpers::component_decl_with_exposed_binder;
    use std::collections::HashSet;

    struct RealmCapabilityTest {
        builtin_environment: Option<Arc<Mutex<BuiltinEnvironment>>>,
        mock_runner: Arc<MockRunner>,
        component: Option<Arc<ComponentInstance>>,
        realm_proxy: fcomponent::RealmProxy,
        hook: Arc<TestHook>,
    }

    impl RealmCapabilityTest {
        async fn new(
            components: Vec<(&'static str, ComponentDecl)>,
            component_moniker: Moniker,
        ) -> Self {
            // Init model.
            let config = RuntimeConfig { list_children_batch_size: 2, ..Default::default() };
            let hook = Arc::new(TestHook::new());
            let TestModelResult { model, builtin_environment, mock_runner, .. } =
                TestEnvironmentBuilder::new()
                    .set_runtime_config(config)
                    .set_components(components)
                    // Install TestHook at the front so that when we receive an event the hook has
                    // already run so the result is reflected in its printout
                    .set_front_hooks(hook.hooks())
                    .build()
                    .await;

            // Look up and start component.
            let component = model
                .root()
                .start_instance(&component_moniker, &StartReason::Eager)
                .await
                .expect("failed to start component");

            // Host framework service.
            let (realm_proxy, server) = endpoints::create_proxy::<fcomponent::RealmMarker>();
            let weak_component = component.as_weak();
            component.execution_scope.spawn(async move {
                serve(server.into_channel(), weak_component.clone(), weak_component).await.unwrap();
            });
            Self {
                builtin_environment: Some(builtin_environment),
                mock_runner,
                component: Some(component),
                realm_proxy,
                hook,
            }
        }

        fn component(&self) -> &Arc<ComponentInstance> {
            self.component.as_ref().unwrap()
        }

        fn drop_component(&mut self) {
            self.component = None;
            self.builtin_environment = None;
        }

        async fn new_event_stream(&self, events: Vec<EventType>) -> fcomponent::EventStreamProxy {
            let builtin_environment_guard = self
                .builtin_environment
                .as_ref()
                .expect("builtin_environment is none")
                .lock()
                .await;
            new_event_stream(&*builtin_environment_guard, events).await
        }
    }

    fn child_decl(name: &str) -> fdecl::Child {
        fdecl::Child {
            name: Some(name.to_owned()),
            url: Some(format!("test:///{}", name)),
            startup: Some(fdecl::StartupMode::Lazy),
            environment: None,
            on_terminate: None,
            ..Default::default()
        }
    }

    #[fuchsia::test]
    async fn create_dynamic_child() {
        // Set up model and realm service.
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                (
                    "system",
                    ComponentDeclBuilder::new()
                        .collection(CollectionBuilder::new().name("coll").allow_long_names())
                        .build(),
                ),
                // Eagerly launched so it needs a definition
                ("b", ComponentDeclBuilder::new().build()),
            ],
            ["system"].try_into().unwrap(),
        )
        .await;

        let event_stream = test.new_event_stream(vec![EventType::Started]).await;

        // Test that a dynamic child with a long name can also be created.
        let long_name = &"c".repeat(cm_types::MAX_LONG_NAME_LENGTH);

        // Create children "a", "b", and "<long_name>" in collection.
        // each.
        let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
        {
            // Create a child
            test.realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl("a"),
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .unwrap()
                .unwrap();
        }
        {
            // Create a child (eager)
            let mut child_decl = child_decl("b");
            child_decl.startup = Some(fdecl::StartupMode::Eager);
            test.realm_proxy
                .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
                .await
                .unwrap()
                .unwrap();

            // Ensure that an event exists for the new child
            let events = get_n_events(&event_stream, 1).await;
            assert_event_type_and_moniker(
                &events[0],
                fcomponent::EventType::Started,
                "system/coll:b",
            );
        }
        {
            // Create a child (long name)
            test.realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl(long_name),
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .unwrap()
                .unwrap();
        }

        // Verify that the component topology matches expectations.
        let actual_children = get_live_children(test.component()).await;
        let mut expected_children: HashSet<ChildName> = HashSet::new();
        expected_children.insert("coll:a".try_into().unwrap());
        expected_children.insert("coll:b".try_into().unwrap());
        expected_children.insert(format!("coll:{}", long_name).as_str().try_into().unwrap());
        assert_eq!(actual_children, expected_children);
    }

    #[fuchsia::test]
    async fn create_dynamic_child_errors() {
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                (
                    "system",
                    ComponentDeclBuilder::new()
                        .collection_default("coll")
                        .collection(
                            CollectionBuilder::new()
                                .name("pcoll")
                                .durability(fdecl::Durability::Transient)
                                .allow_long_names(),
                        )
                        .collection(
                            CollectionBuilder::new()
                                .name("dynoff")
                                .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                        )
                        .build(),
                ),
            ],
            ["system"].try_into().unwrap(),
        )
        .await;

        // Invalid arguments.
        {
            let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
            let child_decl = fdecl::Child {
                name: Some("a".to_string()),
                url: None,
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }
        {
            let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
            let child_decl = fdecl::Child {
                name: Some("a".to_string()),
                url: Some("test:///a".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                environment: Some("env".to_string()),
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }

        // Long dynamic child name violations.
        {
            // Name exceeds MAX_NAME_LENGTH when `allow_long_names` is not set.
            // The FIDL call succeeds but the server responds with an error.
            let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
            let child_decl = fdecl::Child {
                name: Some("a".repeat(cm_types::MAX_NAME_LENGTH + 1).to_string()),
                url: None,
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);

            // Name length exceeds the MAX_LONG_NAME_LENGTH when `allow_long_names` is set.
            // In this case the FIDL call fails to encode because the name field
            // is defined in the FIDL library as `string:MAX_LONG_NAME_LENGTH`.
            let collection_ref = fdecl::CollectionRef { name: "pcoll".to_string() };
            let child_decl = fdecl::Child {
                name: Some("a".repeat(cm_types::MAX_LONG_NAME_LENGTH + 1).to_string()),
                url: None,
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
                .await
                .expect_err("unexpected success");
            // When exceeding the long max name length, the FIDL call itself
            // fails because the name field is defined as `string:1024`.
            assert_matches!(err, fidl::Error::StringTooLong { .. });
        }

        // Instance already exists.
        {
            let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
            let res = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl("a"),
                    fcomponent::CreateChildArgs::default(),
                )
                .await;
            res.expect("fidl call failed").expect("failed to create child a");
            let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
            let err = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl("a"),
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InstanceAlreadyExists);
        }

        // Collection not found.
        {
            let collection_ref = fdecl::CollectionRef { name: "nonexistent".to_string() };
            let err = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl("a"),
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::CollectionNotFound);
        }

        fn sample_offer_from(source: fdecl::Ref) -> fdecl::Offer {
            fdecl::Offer::Protocol(fdecl::OfferProtocol {
                source: Some(source),
                source_name: Some("foo".to_string()),
                target_name: Some("foo".to_string()),
                dependency_type: Some(fdecl::DependencyType::Strong),
                ..Default::default()
            })
        }

        // Disallowed dynamic offers specified.
        {
            let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
            let child_decl = fdecl::Child {
                name: Some("b".to_string()),
                url: Some("test:///b".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl,
                    fcomponent::CreateChildArgs {
                        dynamic_offers: Some(vec![sample_offer_from(fdecl::Ref::Parent(
                            fdecl::ParentRef {},
                        ))]),
                        ..Default::default()
                    },
                )
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }

        // Malformed dynamic offers specified.
        {
            let collection_ref = fdecl::CollectionRef { name: "dynoff".to_string() };
            let child_decl = fdecl::Child {
                name: Some("b".to_string()),
                url: Some("test:///b".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl,
                    fcomponent::CreateChildArgs {
                        dynamic_offers: Some(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                            source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                            source_name: Some("foo".to_string()),
                            target_name: Some("foo".to_string()),
                            // Note: has no `dependency_type`.
                            ..Default::default()
                        })]),
                        ..Default::default()
                    },
                )
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }

        // Dynamic offer source is a static component that doesn't exist.
        {
            let collection_ref = fdecl::CollectionRef { name: "dynoff".to_string() };
            let child_decl = fdecl::Child {
                name: Some("b".to_string()),
                url: Some("test:///b".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl,
                    fcomponent::CreateChildArgs {
                        dynamic_offers: Some(vec![sample_offer_from(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "does_not_exist".to_string(),
                                collection: None,
                            },
                        ))]),
                        ..Default::default()
                    },
                )
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }

        // Source is a collection that doesn't exist (and using a Service).
        {
            let collection_ref = fdecl::CollectionRef { name: "dynoff".to_string() };
            let child_decl = fdecl::Child {
                name: Some("b".to_string()),
                url: Some("test:///b".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl,
                    fcomponent::CreateChildArgs {
                        dynamic_offers: Some(vec![fdecl::Offer::Service(fdecl::OfferService {
                            source: Some(fdecl::Ref::Collection(fdecl::CollectionRef {
                                name: "does_not_exist".to_string(),
                            })),
                            source_name: Some("foo".to_string()),
                            target_name: Some("foo".to_string()),
                            ..Default::default()
                        })]),
                        ..Default::default()
                    },
                )
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }

        // Source is a component in the same collection that doesn't exist.
        {
            let collection_ref = fdecl::CollectionRef { name: "dynoff".to_string() };
            let child_decl = fdecl::Child {
                name: Some("b".to_string()),
                url: Some("test:///b".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl,
                    fcomponent::CreateChildArgs {
                        dynamic_offers: Some(vec![sample_offer_from(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "does_not_exist".to_string(),
                                collection: Some("dynoff".to_string()),
                            },
                        ))]),
                        ..Default::default()
                    },
                )
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }

        // Source is the component itself, which would create a cycle.
        {
            let collection_ref = fdecl::CollectionRef { name: "dynoff".to_string() };
            let child_decl = fdecl::Child {
                name: Some("b".to_string()),
                url: Some("test:///b".to_string()),
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                ..Default::default()
            };
            let err = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl,
                    fcomponent::CreateChildArgs {
                        dynamic_offers: Some(vec![sample_offer_from(fdecl::Ref::Child(
                            fdecl::ChildRef {
                                name: "b".to_string(),
                                collection: Some("dynoff".to_string()),
                            },
                        ))]),
                        ..Default::default()
                    },
                )
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::DependencyCycle);
        }
    }

    #[fuchsia::test]
    async fn realm_instance_died() {
        let mut test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                ("system", ComponentDeclBuilder::new().collection_default("coll").build()),
            ],
            ["system"].try_into().unwrap(),
        )
        .await;
        let collection_ref = fdecl::CollectionRef { name: "coll".into() };
        test.realm_proxy
            .create_child(&collection_ref, &child_decl("a"), fcomponent::CreateChildArgs::default())
            .await
            .unwrap()
            .unwrap();

        // If the component is dropped, this should cancel the server task and close the channel.
        test.drop_component();
        assert_matches!(test.realm_proxy.take_event_stream().next().await, None);
    }

    #[fuchsia::test]
    async fn destroy_dynamic_child() {
        // Set up model and realm service.
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                ("system", ComponentDeclBuilder::new().collection_default("coll").build()),
                ("a", component_decl_with_exposed_binder()),
                ("b", component_decl_with_exposed_binder()),
            ],
            ["system"].try_into().unwrap(),
        )
        .await;

        let event_stream =
            test.new_event_stream(vec![EventType::Stopped, EventType::Destroyed]).await;

        // Create children "a" and "b" in collection, and start them.
        for name in &["a", "b"] {
            let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
            let res = test
                .realm_proxy
                .create_child(
                    &collection_ref,
                    &child_decl(name),
                    fcomponent::CreateChildArgs::default(),
                )
                .await;
            res.expect("fidl call failed")
                .unwrap_or_else(|_| panic!("failed to create child {}", name));
            let child_ref =
                fdecl::ChildRef { name: name.to_string(), collection: Some("coll".to_string()) };
            let (exposed_dir, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
            let () = test
                .realm_proxy
                .open_exposed_dir(&child_ref, server_end)
                .await
                .expect("OpenExposedDir FIDL")
                .expect("OpenExposedDir Error");
            let _: fcomponent::BinderProxy =
                client::connect_to_protocol_at_dir_root::<fcomponent::BinderMarker>(&exposed_dir)
                    .expect("Connection to fuchsia.component.Binder");
        }

        let child = get_live_child(test.component(), "coll:a").await;
        let instance_id = get_incarnation_id(test.component(), "coll:a").await;
        assert_eq!(child.component_url, "test:///a");
        assert_eq!(instance_id, 1);
        let child = get_live_child(test.component(), "coll:b").await;
        let instance_id = get_incarnation_id(test.component(), "coll:b").await;
        assert_eq!(child.component_url, "test:///b");
        assert_eq!(instance_id, 2);

        // Destroy "a". "a" is no longer live from the client's perspective, although it's still
        // being destroyed.
        let child_ref =
            fdecl::ChildRef { name: "a".to_string(), collection: Some("coll".to_string()) };
        let (f, destroy_handle) = test.realm_proxy.destroy_child(&child_ref).remote_handle();
        fasync::Task::spawn(f).detach();

        // The component should be stopped (shut down) before it is destroyed.
        let events = get_n_events(&event_stream, 2).await;
        assert_event_type_and_moniker(&events[0], fcomponent::EventType::Stopped, "system/coll:a");
        assert_event_type_and_moniker(
            &events[1],
            fcomponent::EventType::Destroyed,
            "system/coll:a",
        );

        // "a" is fully deleted now.
        assert!(!has_child(test.component(), "coll:a").await);
        {
            let actual_children = get_live_children(test.component()).await;
            let mut expected_children: HashSet<ChildName> = HashSet::new();
            expected_children.insert("coll:b".try_into().unwrap());
            let child_b = get_live_child(test.component(), "coll:b").await;
            assert!(!execution_is_shut_down(&child_b).await);
            assert_eq!(actual_children, expected_children);
        }

        let res = destroy_handle.await;
        res.expect("fidl call failed").expect("failed to destroy child a");

        // Recreate "a" and verify "a" is back (but it's a different "a"). The old "a" is gone
        // from the client's point of view, but it hasn't been cleaned up yet.
        let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
        let child_decl = fdecl::Child {
            name: Some("a".to_string()),
            url: Some("test:///a_alt".to_string()),
            startup: Some(fdecl::StartupMode::Lazy),
            environment: None,
            ..Default::default()
        };
        let res = test
            .realm_proxy
            .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
            .await;
        res.expect("fidl call failed").expect("failed to recreate child a");

        let child = get_live_child(test.component(), "coll:a").await;
        let instance_id = get_incarnation_id(test.component(), "coll:a").await;
        assert_eq!(child.component_url, "test:///a_alt");
        assert_eq!(instance_id, 3);
    }

    #[fuchsia::test]
    async fn destroy_dynamic_child_errors() {
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                ("system", ComponentDeclBuilder::new().collection_default("coll").build()),
            ],
            ["system"].try_into().unwrap(),
        )
        .await;

        // Create child "a" in collection.
        let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
        let res = test
            .realm_proxy
            .create_child(&collection_ref, &child_decl("a"), fcomponent::CreateChildArgs::default())
            .await;
        res.expect("fidl call failed").expect("failed to create child a");

        // Invalid arguments.
        {
            let child_ref = fdecl::ChildRef { name: "a".to_string(), collection: None };
            let err = test
                .realm_proxy
                .destroy_child(&child_ref)
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }

        // Instance not found.
        {
            let child_ref =
                fdecl::ChildRef { name: "b".to_string(), collection: Some("coll".to_string()) };
            let err = test
                .realm_proxy
                .destroy_child(&child_ref)
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InstanceNotFound);
        }
    }

    #[fuchsia::test]
    async fn dynamic_single_run_child() {
        // Set up model and realm service.
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                (
                    "system",
                    ComponentDeclBuilder::new()
                        .collection(
                            CollectionBuilder::new()
                                .name("coll")
                                .durability(fdecl::Durability::SingleRun),
                        )
                        .build(),
                ),
                ("a", component_decl_with_test_runner()),
            ],
            ["system"].try_into().unwrap(),
        )
        .await;

        let event_stream =
            test.new_event_stream(vec![EventType::Started, EventType::Destroyed]).await;

        // Create child "a" in collection. Expect a Started event.
        let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
        test.realm_proxy
            .create_child(&collection_ref, &child_decl("a"), fcomponent::CreateChildArgs::default())
            .await
            .unwrap()
            .unwrap();
        let events = get_n_events(&event_stream, 1).await;
        assert_event_type_and_moniker(&events[0], fcomponent::EventType::Started, "system/coll:a");

        // Started action completes.

        let child = {
            let state = test.component().lock_resolved_state().await.unwrap();
            let child = state.children().next().unwrap();
            assert_eq!("a", child.0.name().as_str());
            child.1.clone()
        };

        // The stop should trigger a delete/purge.
        child.stop_instance_internal(false).await.unwrap();

        let events = get_n_events(&event_stream, 1).await;
        assert_event_type_and_moniker(
            &events[0],
            fcomponent::EventType::Destroyed,
            "system/coll:a",
        );

        // Verify that the component topology matches expectations.
        let actual_children = get_live_children(test.component()).await;
        let expected_children: HashSet<ChildName> = HashSet::new();
        assert_eq!(actual_children, expected_children);
    }

    #[fuchsia::test]
    async fn list_children_errors() {
        // Create a root component with a collection.
        let test = RealmCapabilityTest::new(
            vec![("root", ComponentDeclBuilder::new().collection_default("coll").build())],
            Moniker::root(),
        )
        .await;

        // Collection not found.
        {
            let collection_ref = fdecl::CollectionRef { name: "nonexistent".to_string() };
            let (_, server_end) = endpoints::create_proxy();
            let err = test
                .realm_proxy
                .list_children(&collection_ref, server_end)
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::CollectionNotFound);
        }
    }

    #[fuchsia::test]
    async fn open_exposed_dir() {
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                (
                    "system",
                    ComponentDeclBuilder::new()
                        .protocol_default("foo")
                        .expose(
                            ExposeBuilder::protocol()
                                .name("foo")
                                .target_name("hippo")
                                .source(ExposeSource::Self_),
                        )
                        .build(),
                ),
            ],
            Moniker::root(),
        )
        .await;
        let event_stream =
            test.new_event_stream(vec![EventType::Resolved, EventType::Started]).await;
        let event_stream_2 = test.new_event_stream(vec![EventType::Started]).await;
        let mut out_dir = OutDir::new();
        out_dir.add_echo_protocol("/svc/foo".parse().unwrap());
        test.mock_runner.add_host_fn("test:///system", out_dir.host_fn());

        // Open exposed directory of child.
        let child_ref = fdecl::ChildRef { name: "system".to_string(), collection: None };
        let (dir_proxy, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
        let res = test.realm_proxy.open_exposed_dir(&child_ref, server_end).await;
        res.expect("fidl call failed").expect("open_exposed_dir() failed");

        // Assert that child was resolved.
        let events = get_n_events(&event_stream, 1).await;
        assert_event_type_and_moniker(&events[0], fcomponent::EventType::Resolved, "system");

        // Assert that there are no outstanding started messages. This ensures that
        // EventType::Started for "system" has not been registered.
        //
        // We do this on a separate event stream, because the `now_or_never` call leaves the stream
        // in a weird state and we're unable to pull events from it after this.
        assert!(event_stream_2.get_next().now_or_never().is_none());

        // Check flags on directory opened. This should match the maximum set of rights for every
        // directory connection along the open chain.
        let flags = dir_proxy.get_flags().await.expect("FIDL error").expect("GetFlags error");
        assert_eq!(
            flags,
            fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::PERM_EXECUTABLE
                | fio::Flags::PROTOCOL_DIRECTORY
        );

        // Now that it was asserted that "system:0" has yet to start,
        // assert that it starts after making connection below.
        let echo_proxy =
            client::connect_to_named_protocol_at_dir_root::<echo::EchoMarker>(&dir_proxy, "hippo")
                .expect("failed to open hippo service");
        let events = get_n_events(&event_stream, 1).await;
        assert_event_type_and_moniker(&events[0], fcomponent::EventType::Started, "system");
        let res = echo_proxy.echo_string(Some("hippos")).await;
        assert_eq!(res.expect("failed to use echo service"), Some("hippos".to_string()));

        // Verify topology matches expectations.
        let expected_urls = &["test:///root", "test:///system"];
        test.mock_runner.wait_for_urls(expected_urls).await;
        assert_eq!("(system)", test.hook.print());
    }

    #[fuchsia::test]
    async fn open_exposed_dir_dynamic_child() {
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().collection_default("coll").build()),
                (
                    "system",
                    ComponentDeclBuilder::new()
                        .protocol_default("foo")
                        .expose(
                            ExposeBuilder::protocol()
                                .name("foo")
                                .target_name("hippo")
                                .source(ExposeSource::Self_),
                        )
                        .build(),
                ),
            ],
            Moniker::root(),
        )
        .await;

        let event_stream =
            test.new_event_stream(vec![EventType::Resolved, EventType::Started]).await;
        let event_stream_2 = test.new_event_stream(vec![EventType::Started]).await;
        let mut out_dir = OutDir::new();
        out_dir.add_echo_protocol("/svc/foo".parse().unwrap());
        test.mock_runner.add_host_fn("test:///system", out_dir.host_fn());

        // Add "system" to collection.
        let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
        let res = test
            .realm_proxy
            .create_child(
                &collection_ref,
                &child_decl("system"),
                fcomponent::CreateChildArgs::default(),
            )
            .await;
        res.expect("fidl call failed").expect("failed to create child system");

        // Open exposed directory of child.
        let child_ref =
            fdecl::ChildRef { name: "system".to_string(), collection: Some("coll".to_owned()) };
        let (dir_proxy, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
        let res = test.realm_proxy.open_exposed_dir(&child_ref, server_end).await;
        res.expect("fidl call failed").expect("open_exposed_dir() failed");

        // Assert that child was resolved.
        let events = get_n_events(&event_stream, 1).await;
        assert_event_type_and_moniker(&events[0], fcomponent::EventType::Resolved, "coll:system");

        // Assert that there are no outstanding started events. This ensures that
        // EventType::Started for "system" has not been registered.
        //
        // We do this on a separate event stream, because the `now_or_never` call leaves the stream
        // in a weird state and we're unable to pull events from it after this.
        assert!(event_stream_2.get_next().now_or_never().is_none());

        // Now that it was asserted that "system" has yet to start,
        // assert that it starts after making connection below.
        let echo_proxy =
            client::connect_to_named_protocol_at_dir_root::<echo::EchoMarker>(&dir_proxy, "hippo")
                .expect("failed to open hippo service");
        let events = get_n_events(&event_stream, 1).await;
        assert_event_type_and_moniker(&events[0], fcomponent::EventType::Started, "coll:system");
        let res = echo_proxy.echo_string(Some("hippos")).await;
        assert_eq!(res.expect("failed to use echo service"), Some("hippos".to_string()));

        // Verify topology matches expectations.
        let expected_urls = &["test:///root", "test:///system"];
        test.mock_runner.wait_for_urls(expected_urls).await;
        assert_eq!("(coll:system)", test.hook.print());
    }

    #[fuchsia::test]
    async fn open_exposed_dir_errors() {
        let test = RealmCapabilityTest::new(
            vec![
                (
                    "root",
                    ComponentDeclBuilder::new()
                        .child_default("system")
                        .child_default("unresolvable")
                        .child_default("unrunnable")
                        .build(),
                ),
                ("system", component_decl_with_test_runner()),
                ("unrunnable", component_decl_with_test_runner()),
            ],
            Moniker::root(),
        )
        .await;
        test.mock_runner.cause_failure("unrunnable");

        // Instance not found.
        {
            let child_ref = fdecl::ChildRef { name: "missing".to_string(), collection: None };
            let (_, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
            let err = test
                .realm_proxy
                .open_exposed_dir(&child_ref, server_end)
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InstanceNotFound);
        }

        // Instance cannot resolve.
        {
            let child_ref = fdecl::ChildRef { name: "unresolvable".to_string(), collection: None };
            let (_, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
            let err = test
                .realm_proxy
                .open_exposed_dir(&child_ref, server_end)
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InstanceCannotResolve);
        }

        // Instance can't run.
        {
            let child_ref = fdecl::ChildRef { name: "unrunnable".to_string(), collection: None };
            let (dir_proxy, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
            let res = test.realm_proxy.open_exposed_dir(&child_ref, server_end).await;
            res.expect("fidl call failed").expect("open_exposed_dir() failed");
            let echo_proxy = client::connect_to_named_protocol_at_dir_root::<echo::EchoMarker>(
                &dir_proxy, "hippo",
            )
            .expect("failed to open hippo service");
            let res = echo_proxy.echo_string(Some("hippos")).await;
            assert!(res.is_err());
        }
    }

    #[fuchsia::test]
    async fn open_controller() {
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                ("system", component_decl_with_test_runner()),
            ],
            Moniker::root(),
        )
        .await;

        // Success.
        {
            let child_ref = fdecl::ChildRef { name: "system".to_string(), collection: None };
            let (_, server_end) = endpoints::create_proxy::<fcomponent::ControllerMarker>();
            test.realm_proxy
                .open_controller(&child_ref, server_end)
                .await
                .expect("fidl call failed")
                .expect("open_controller() failed");
            // Business logic tests for `fuchsia.component.Controller` is at
            // src/sys/component_manager/tests/controller/src/lib.rs
        }

        // Invalid ref.
        {
            let child_ref = fdecl::ChildRef { name: "-&*:(\\".to_string(), collection: None };
            let (_, server_end) = endpoints::create_proxy::<fcomponent::ControllerMarker>();
            let err = test
                .realm_proxy
                .open_controller(&child_ref, server_end)
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InvalidArguments);
        }

        // Instance not found.
        {
            let child_ref = fdecl::ChildRef { name: "missing".to_string(), collection: None };
            let (_, server_end) = endpoints::create_proxy::<fcomponent::ControllerMarker>();
            let err = test
                .realm_proxy
                .open_controller(&child_ref, server_end)
                .await
                .expect("fidl call failed")
                .expect_err("unexpected success");
            assert_eq!(err, fcomponent::Error::InstanceNotFound);
        }
    }

    #[fuchsia::test]
    async fn get_resolved_info() {
        let test = RealmCapabilityTest::new(
            vec![("root", ComponentDeclBuilder::new().child_default("system").build())],
            Moniker::root(),
        )
        .await;

        let fidl_resolved_info = test
            .realm_proxy
            .get_resolved_info()
            .await
            .expect("fidl call failed")
            .expect("get_resolved_info() failed");

        let internal_resolved_info: fresolution::Component =
            (&test.component().lock_resolved_state().await.unwrap().resolved_component).into();
        // We can't assert_eq!(fidl_resolved_info, internal_resolved_info) because they hold
        // handles, and even if the handles point to/represent the same data the actual handle
        // numbers are different.
        assert_eq!(fidl_resolved_info.url, internal_resolved_info.url);
        let read_buffer = |data: fmem::Data| match data {
            fmem::Data::Buffer(fmem::Buffer { vmo, size }) => Some(vmo.read_to_vec::<u8>(0, size)),
            _ => None,
        };
        assert_eq!(
            fidl_resolved_info.decl.and_then(read_buffer),
            internal_resolved_info.decl.and_then(read_buffer),
        );
    }

    #[fuchsia::test]
    async fn get_child_output_deprecated() {
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                (
                    "system",
                    ComponentDeclBuilder::new()
                        .protocol_default("fidl.examples.Echo")
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fidl.examples.Echo")
                                .source(ExposeSource::Self_),
                        )
                        .build(),
                ),
            ],
            Moniker::root(),
        )
        .await;

        let child_output_dictionary_ref = test
            .realm_proxy
            .get_child_output_dictionary_deprecated(&fdecl::ChildRef {
                name: "system".to_string(),
                collection: None,
            })
            .await
            .expect("fidl call failed")
            .expect("get_child_output_deprecated() failed");

        let child_output_dictionary =
            runtime_capabilities::Dictionary::try_from_fsandbox(child_output_dictionary_ref)
                .unwrap();
        assert_eq!(
            vec!["fidl.examples.Echo".to_string()],
            child_output_dictionary.snapshot_keys_as_strings(),
        );
    }

    #[fuchsia::test]
    async fn get_child_output() {
        let test = RealmCapabilityTest::new(
            vec![
                ("root", ComponentDeclBuilder::new().child_default("system").build()),
                (
                    "system",
                    ComponentDeclBuilder::new()
                        .protocol_default("fidl.examples.Echo")
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fidl.examples.Echo")
                                .source(ExposeSource::Self_),
                        )
                        .build(),
                ),
            ],
            Moniker::root(),
        )
        .await;

        let child_output_dictionary_handle = test
            .realm_proxy
            .get_child_output_dictionary(&fdecl::ChildRef {
                name: "system".to_string(),
                collection: None,
            })
            .await
            .expect("fidl call failed")
            .expect("get_child_output() failed");

        let child_output_dictionary: Arc<runtime_capabilities::Dictionary> = test
            .component()
            .context
            .remote_capabilities()
            .get(child_output_dictionary_handle)
            .expect("child output dictionary handle doesn't reference anything");

        assert_eq!(
            vec!["fidl.examples.Echo".to_string()],
            child_output_dictionary.snapshot_keys_as_strings(),
        );
    }
}
