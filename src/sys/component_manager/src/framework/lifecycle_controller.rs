// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::{ActionsManager, StopAction};
use crate::model::component::{
    ComponentInstance, IncomingCapabilities, StartReason, WeakComponentInstance,
};
use crate::sandbox_util::take_handle_as_stream;
use ::routing::component_instance::ComponentInstanceInterface;
use cm_rust::FidlIntoNative;
use fidl::endpoints::ServerEnd;
use futures::future::BoxFuture;
use futures::prelude::*;
use log::warn;
use moniker::{ChildName, Moniker, MonikerError};
use std::sync::Arc;
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_sys2 as fsys,
};

pub fn serve(
    server_end: zx::Channel,
    _target: WeakComponentInstance,
    scope: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        let mut stream = take_handle_as_stream::<fsys::LifecycleControllerMarker>(server_end);
        let scope = scope.upgrade()?;
        loop {
            let operation = match stream.try_next().await {
                Ok(Some(operation)) => operation,
                Ok(None) => return Ok(()),
                Err(_e) => continue,
            };
            match operation {
                fsys::LifecycleControllerRequest::ResolveInstance { moniker, responder } => {
                    let res = resolve_instance(&scope, moniker).await;
                    responder.send(res).unwrap_or_else(
                        |error| warn!(error:%; "LifecycleController.ResolveInstance failed to send"),
                    );
                }
                fsys::LifecycleControllerRequest::UnresolveInstance { moniker, responder } => {
                    let res = unresolve_instance(&scope, moniker).await;
                    responder.send(res).unwrap_or_else(
                        |error| warn!(error:%; "LifecycleController.UnresolveInstance failed to send"),
                    );
                }
                fsys::LifecycleControllerRequest::StartInstance { moniker, binder, responder } => {
                    let res = start_instance(&scope, moniker, binder).await;
                    responder.send(res).unwrap_or_else(
                        |error| warn!(error:%; "LifecycleController.StartInstance failed to send"),
                    );
                }
                fsys::LifecycleControllerRequest::StartInstanceWithArgs {
                    moniker,
                    binder,
                    args,
                    responder,
                } => {
                    let res = start_instance_with_args(
                        &scope,
                        moniker,
                        binder,
                        args,
                    )
                    .await;
                    responder.send(res).unwrap_or_else(
                        |error| warn!(error:%; "LifecycleController.StartInstanceWithArgs failed to send"),
                    );
                }
                fsys::LifecycleControllerRequest::StopInstance { moniker, responder } => {
                    let res = stop_instance(&scope, moniker).await;
                    responder.send(res).unwrap_or_else(
                        |error| warn!(error:%; "LifecycleController.StopInstance failed to send"),
                    );
                }
                fsys::LifecycleControllerRequest::CreateInstance {
                    parent_moniker,
                    collection,
                    decl,
                    args,
                    responder,
                } => {
                    let res = create_instance(
                        &scope,
                        parent_moniker,
                        collection,
                        decl,
                        args,
                    )
                    .await;
                    responder.send(res).unwrap_or_else(
                        |error| warn!(error:%; "LifecycleController.CreateInstance failed to send"),
                    );
                }
                fsys::LifecycleControllerRequest::DestroyInstance {
                    parent_moniker,
                    child,
                    responder,
                } => {
                    let res =
                        destroy_instance(&scope, parent_moniker, child).await;
                    responder.send(res).unwrap_or_else(
                        |error| warn!(error:%; "LifecycleController.DestroyInstance failed to send"),
                    );
                }
            }
        }
    }.boxed()
}

async fn resolve_instance(
    scope: &Arc<ComponentInstance>,
    moniker: String,
) -> Result<(), fsys::ResolveError> {
    let moniker =
        join_monikers(&scope.moniker, &moniker).map_err(|_| fsys::ResolveError::BadMoniker)?;
    let instance =
        scope.find_absolute(&moniker).await.map_err(|_| fsys::ResolveError::InstanceNotFound)?;
    instance.resolve().await.map(|_| ()).map_err(|error| {
        warn!(moniker:%, error:%; "failed to resolve instance");
        error.into()
    })
}

async fn start_instance(
    scope: &Arc<ComponentInstance>,
    moniker: String,
    binder: ServerEnd<fcomponent::BinderMarker>,
) -> Result<(), fsys::StartError> {
    start_instance_with_args(scope, moniker, binder, fcomponent::StartChildArgs::default()).await
}

async fn start_instance_with_args(
    scope: &Arc<ComponentInstance>,
    moniker: String,
    binder: ServerEnd<fcomponent::BinderMarker>,
    args: fcomponent::StartChildArgs,
) -> Result<(), fsys::StartError> {
    let moniker =
        join_monikers(&scope.moniker, &moniker).map_err(|_| fsys::StartError::BadMoniker)?;
    let instance =
        scope.find_absolute(&moniker).await.map_err(|_| fsys::StartError::InstanceNotFound)?;
    let incoming =
        IncomingCapabilities::from_start_child_args(scope.context.remote_capabilities(), args)
            .map_err(|_| fsys::StartError::InvalidArguments)?;
    instance.start(&StartReason::Debug, None, incoming).await.map(|_| ()).map_err(|error| {
        warn!(moniker:%, error:%; "failed to start instance");
        error
    })?;
    instance.scope_to_runtime(binder.into_channel()).await;
    Ok(())
}

async fn stop_instance(
    scope: &Arc<ComponentInstance>,
    moniker: String,
) -> Result<(), fsys::StopError> {
    let moniker =
        join_monikers(&scope.moniker, &moniker).map_err(|_| fsys::StopError::BadMoniker)?;
    let instance =
        scope.find_absolute(&moniker).await.map_err(|_| fsys::StopError::InstanceNotFound)?;
    ActionsManager::register(instance.clone(), StopAction::new(false)).await.map_err(|error| {
        warn!(moniker:%, error:%; "failed to stop instance");
        error
    })?;
    Ok(())
}

async fn unresolve_instance(
    scope: &Arc<ComponentInstance>,
    moniker: String,
) -> Result<(), fsys::UnresolveError> {
    let moniker =
        join_monikers(&scope.moniker, &moniker).map_err(|_| fsys::UnresolveError::BadMoniker)?;
    let component =
        scope.find_absolute(&moniker).await.map_err(|_| fsys::UnresolveError::InstanceNotFound)?;
    component.unresolve().await.map_err(|error| {
        warn!(moniker:%, error:%; "failed to unresolve instance");
        error
    })?;
    Ok(())
}

async fn create_instance(
    scope: &Arc<ComponentInstance>,
    parent_moniker: String,
    collection: fdecl::CollectionRef,
    child_decl: fdecl::Child,
    child_args: fcomponent::CreateChildArgs,
) -> Result<(), fsys::CreateError> {
    let parent_moniker = join_monikers(&scope.moniker, &parent_moniker)
        .map_err(|_| fsys::CreateError::BadMoniker)?;
    let parent_component = scope
        .find_absolute(&parent_moniker)
        .await
        .map_err(|_| fsys::CreateError::InstanceNotFound)?;

    cm_fidl_validator::validate_dynamic_child(&child_decl).map_err(|error| {
        warn!(parent_moniker:%, error:%; "failed to create dynamic child. child decl is invalid");
        fsys::CreateError::BadChildDecl
    })?;
    let child_decl = child_decl.fidl_into_native();

    parent_component
        .add_dynamic_child(collection.name.clone(), &child_decl, child_args)
        .await
        .map_err(|error| {
            warn!(parent_moniker:%, error:%; "failed to add dynamic child");
            error.into()
        })
}

async fn destroy_instance(
    scope: &Arc<ComponentInstance>,
    parent_moniker: String,
    child: fdecl::ChildRef,
) -> Result<(), fsys::DestroyError> {
    let parent_moniker = join_monikers(&scope.moniker, &parent_moniker)
        .map_err(|_| fsys::DestroyError::BadMoniker)?;
    let parent_component = scope
        .find_absolute(&parent_moniker)
        .await
        .map_err(|_| fsys::DestroyError::InstanceNotFound)?;

    child.collection.as_ref().ok_or(fsys::DestroyError::BadChildRef)?;
    let child_moniker = ChildName::try_new(&child.name, child.collection.as_ref())
        .map_err(|_| fsys::DestroyError::BadChildRef)?;

    parent_component.remove_dynamic_child(&child_moniker).await.map_err(|error| {
        warn!(parent_moniker:%, error:%; "failed to destroy dynamic child");
        error.into()
    })
}

/// Takes the scoped component's moniker and a moniker string and joins them into an
/// moniker.
fn join_monikers(scope_moniker: &Moniker, moniker_str: &str) -> Result<Moniker, MonikerError> {
    let moniker = Moniker::try_from(moniker_str)?;
    Ok(scope_moniker.concat(&moniker))
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::actions::test_utils::{is_discovered, is_resolved, is_shutdown};
    use crate::model::testing::test_helpers::{TestEnvironmentBuilder, lifecycle_controller};
    use cm_rust_testing::*;
    use fidl_fuchsia_component_decl::{ChildRef, CollectionRef};
    use {fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl};

    #[fuchsia::test]
    async fn lifecycle_controller_test() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("a").eager())
                    .child(
                        ChildBuilder::new()
                            .name("cant-resolve")
                            .url("cant-resolve://cant-resolve")
                            .eager(),
                    )
                    .build(),
            ),
            ("a", ComponentDeclBuilder::new().child(ChildBuilder::new().name("b").eager()).build()),
            ("b", ComponentDeclBuilder::new().build()),
        ];

        let test_model_result =
            TestEnvironmentBuilder::new().set_components(components).build().await;
        let lifecycle_proxy = lifecycle_controller(&test_model_result).await;

        assert_eq!(lifecycle_proxy.resolve_instance(".").await.unwrap(), Ok(()));

        assert_eq!(lifecycle_proxy.resolve_instance("./a").await.unwrap(), Ok(()));

        assert_eq!(
            lifecycle_proxy.resolve_instance(".\\scope-escape-attempt").await.unwrap(),
            Err(fsys::ResolveError::BadMoniker)
        );

        assert_eq!(
            lifecycle_proxy.resolve_instance("./doesnt-exist").await.unwrap(),
            Err(fsys::ResolveError::InstanceNotFound)
        );

        assert_eq!(
            lifecycle_proxy.resolve_instance("./cant-resolve").await.unwrap(),
            Err(fsys::ResolveError::Internal)
        );
    }

    #[fuchsia::test]
    async fn lifecycle_controller_unresolve_component_test() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new().child(ChildBuilder::new().name("top").eager()).build(),
            ),
            (
                "top",
                ComponentDeclBuilder::new().child(ChildBuilder::new().name("a").eager()).build(),
            ),
            ("a", ComponentDeclBuilder::new().child(ChildBuilder::new().name("b").eager()).build()),
            ("b", ComponentDeclBuilder::new().build()),
        ];

        let test_model_result =
            TestEnvironmentBuilder::new().set_components(components).build().await;
        let lifecycle_proxy = lifecycle_controller(&test_model_result).await;
        let root = test_model_result.model.root();
        let top = root.find_and_maybe_resolve(&["top"].try_into().unwrap()).await.unwrap();

        // We (un)resolve `top` instead of `.` because unresolving `.` would have the
        // effect of closing the server endpoint of the `lifecycle_proxy` bound to it.
        lifecycle_proxy.resolve_instance("top").await.unwrap().unwrap();
        let component_a = top.find_and_maybe_resolve(&["a"].try_into().unwrap()).await.unwrap();
        let component_b =
            top.find_and_maybe_resolve(&["a", "b"].try_into().unwrap()).await.unwrap();
        assert!(is_resolved(&component_a).await);
        assert!(is_resolved(&component_b).await);

        lifecycle_proxy.unresolve_instance("top").await.unwrap().unwrap();
        assert!(is_discovered(&top).await);
        assert!(is_shutdown(&component_a).await);
        assert!(is_shutdown(&component_b).await);

        assert_eq!(
            lifecycle_proxy.unresolve_instance("nonesuch").await.unwrap(),
            Err(fsys::UnresolveError::InstanceNotFound)
        );

        // Unresolve again, which is ok because UnresolveAction is idempotent.
        assert_eq!(lifecycle_proxy.unresolve_instance("top").await.unwrap(), Ok(()));
        assert!(is_discovered(&top).await);
        assert!(is_shutdown(&component_a).await);
        assert!(is_shutdown(&component_b).await);
    }

    #[fuchsia::test]
    async fn lifecycle_create_and_destroy_test() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .collection_default("coll")
                    .child_default("child")
                    .build(),
            ),
            ("child", ComponentDeclBuilder::new().build()),
        ];

        let test_model_result =
            TestEnvironmentBuilder::new().set_components(components).build().await;
        let lifecycle_proxy = lifecycle_controller(&test_model_result).await;

        assert_eq!(
            lifecycle_proxy
                .create_instance(
                    "./",
                    &CollectionRef { name: "coll".to_string() },
                    &fdecl::Child {
                        name: Some("child".to_string()),
                        url: Some("test:///child".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .unwrap(),
            Ok(())
        );

        assert_eq!(lifecycle_proxy.resolve_instance("./coll:child").await.unwrap(), Ok(()));

        assert_eq!(
            lifecycle_proxy
                .destroy_instance(
                    "./",
                    &ChildRef {
                        name: "child".parse().unwrap(),
                        collection: Some("coll".to_string())
                    }
                )
                .await
                .unwrap(),
            Ok(())
        );

        assert_eq!(
            lifecycle_proxy.resolve_instance("./coll:child").await.unwrap(),
            Err(fsys::ResolveError::InstanceNotFound)
        );
    }

    #[fuchsia::test]
    async fn lifecycle_create_fail_test() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .collection_default("coll")
                    .child_default("child")
                    .build(),
            ),
            ("child", ComponentDeclBuilder::new().build()),
        ];

        let test_model_result =
            TestEnvironmentBuilder::new().set_components(components).build().await;
        let lifecycle_proxy = lifecycle_controller(&test_model_result).await;

        assert_eq!(
            lifecycle_proxy
                .create_instance(
                    "&#^$%",
                    &CollectionRef { name: "coll".to_string() },
                    &fdecl::Child {
                        name: Some("child".to_string()),
                        url: Some("test:///child".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .unwrap(),
            Err(fsys::CreateError::BadMoniker)
        );

        assert_eq!(
            lifecycle_proxy
                .create_instance(
                    "./does_not_exist",
                    &CollectionRef { name: "coll".to_string() },
                    &fdecl::Child {
                        name: Some("child".to_string()),
                        url: Some("test:///child".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .unwrap(),
            Err(fsys::CreateError::InstanceNotFound)
        );

        assert_eq!(
            lifecycle_proxy
                .create_instance(
                    "./",
                    &CollectionRef { name: "not_coll".to_string() },
                    &fdecl::Child {
                        name: Some("child".to_string()),
                        url: Some("test:///child".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .unwrap(),
            Err(fsys::CreateError::CollectionNotFound)
        );

        assert_eq!(
            lifecycle_proxy
                .create_instance(
                    "./",
                    &CollectionRef { name: "coll".to_string() },
                    &fdecl::Child {
                        name: Some("&*^%&@#$".to_string()),
                        url: Some("test:///child".to_string()),
                        startup: Some(fdecl::StartupMode::Lazy),
                        environment: None,
                        on_terminate: None,
                        ..Default::default()
                    },
                    fcomponent::CreateChildArgs::default(),
                )
                .await
                .unwrap(),
            Err(fsys::CreateError::BadChildDecl)
        );
    }
}
