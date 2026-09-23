// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::framework::resolve_with_pinned_url;
use crate::model::component::{ComponentInstance, WeakComponentInstance};
use crate::model::namespace::create_namespace;
use crate::model::storage::admin_protocol::StorageAdmin;
use crate::sandbox_util::take_handle_as_stream;
use cm_rust::NativeIntoFidl;
use cm_types::{Name, Url};
use fidl::endpoints::{ClientEnd, ServerEnd};
use fidl_fuchsia_component_decl as fcdecl;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_sys2 as fsys;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use log::warn;
use measure_tape_for_instance::Measurable;
use moniker::Moniker;
use router_error::Explain;
use routing::component_instance::{ComponentInstanceInterface, ResolvedInstanceInterface};
use routing::resolving::ComponentAddress;
use std::sync::Arc;
use vfs::ToObjectRequest;
use vfs::directory::entry::OpenRequest;
use vfs::directory::entry_container::Directory;
use zx::sys::ZX_CHANNEL_MAX_MSG_BYTES;

// Number of bytes the header of a vector occupies in a fidl message.
// TODO(https://fxbug.dev/42181010): This should be a constant in a FIDL library.
const FIDL_VECTOR_HEADER_BYTES: usize = 16;

// Number of bytes the header of a fidl message occupies.
// TODO(https://fxbug.dev/42181010): This should be a constant in a FIDL library.
const FIDL_HEADER_BYTES: usize = 16;

// Number of bytes of a manifest that can fit in a single message
// sent on a zircon channel.
const FIDL_MANIFEST_MAX_MSG_BYTES: usize =
    (ZX_CHANNEL_MAX_MSG_BYTES as usize) - (FIDL_HEADER_BYTES + FIDL_VECTOR_HEADER_BYTES);

pub fn serve(
    server_end: zx::Channel,
    _target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        let stream = take_handle_as_stream::<fsys::RealmQueryMarker>(server_end);
        serve_inner(source, stream).boxed().await;
        Ok(())
    }
    .boxed()
}

async fn serve_inner(scope: WeakComponentInstance, mut stream: fsys::RealmQueryRequestStream) {
    while let Some(request) = stream.next().await {
        match request {
            Ok(req) => {
                if let Err(error) = handle_request(&scope, req).boxed().await {
                    warn!(error:?; "Could not respond to RealmQuery request");
                    break;
                }
            }
            Err(error) => {
                warn!(error:?; "Could not get next RealmQuery request");
                break;
            }
        }
    }
}

async fn handle_request(
    scope: &WeakComponentInstance,
    request: fsys::RealmQueryRequest,
) -> Result<(), fidl::Error> {
    match request {
        fsys::RealmQueryRequest::GetInstance { moniker, responder } => {
            let result = get_instance(scope, &moniker).await;
            responder.send(result.as_ref().map_err(|e| *e))
        }
        fsys::RealmQueryRequest::GetResolvedDeclaration { moniker, responder } => {
            let result = get_resolved_declaration(scope, &moniker).await;
            responder.send(result)
        }
        fsys::RealmQueryRequest::ResolveDeclaration { parent, child_location, url, responder } => {
            let result = resolve_declaration(scope, &parent, &child_location, &url).await;
            responder.send(result)
        }
        fsys::RealmQueryRequest::GetStructuredConfig { moniker, responder } => {
            let result = get_structured_config(scope, &moniker).await;
            responder.send(result.as_ref().map_err(|e| *e))
        }
        fsys::RealmQueryRequest::GetAllInstances { responder } => {
            let result = get_all_instances(scope).await;
            responder.send(result)
        }
        fsys::RealmQueryRequest::ConstructNamespace { moniker, responder } => {
            let result = construct_namespace(scope, &moniker).await;
            responder.send(result)
        }
        fsys::RealmQueryRequest::OpenDirectory { moniker, dir_type, object, responder } => {
            let result = open_directory(scope, &moniker, dir_type, object).await;
            responder.send(result)
        }
        fsys::RealmQueryRequest::ConnectToStorageAdmin {
            moniker,
            storage_name,
            server_end,
            responder,
        } => {
            let result = connect_to_storage_admin(scope, &moniker, storage_name, server_end).await;
            responder.send(result)
        }
        fsys::RealmQueryRequest::OpenStorageAdmin {
            moniker,
            storage_name,
            server_end,
            responder,
        } => {
            let result = connect_to_storage_admin(
                scope,
                &moniker,
                storage_name,
                ServerEnd::new(server_end.into_channel()),
            )
            .await;
            responder.send(result)
        }
    }
}

/// Create the state matching the given moniker string in this scope
async fn get_instance(
    scope: &WeakComponentInstance,
    moniker_str: &str,
) -> Result<fsys::Instance, fsys::GetInstanceError> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker = Moniker::try_from(moniker_str).map_err(|_| fsys::GetInstanceError::BadMoniker)?;
    let moniker = scope.moniker.concat(&moniker);

    let scope = scope.upgrade().map_err(|_| fsys::GetInstanceError::InstanceNotFound)?;
    let instance = scope
        .find_absolute(&moniker)
        .await
        .map_err(|_| fsys::GetInstanceError::InstanceNotFound)?;
    let instance_id = scope.context.component_id_index().id_for_moniker(&instance.moniker).cloned();

    let (resolved_info, environment_name) = {
        let state = instance.lock_state().await;

        if let Some(resolved_state) = state.get_resolved_state() {
            let resolved_url = Some(
                resolved_state
                    .address()
                    .await
                    .map_err(|_| fsys::GetInstanceError::InstanceNotFound)?
                    .url()
                    .to_string(),
            );
            let execution_info =
                state.get_started_state().map(|started_state| fsys::ExecutionInfo {
                    start_reason: Some(started_state.start_reason.to_string()),
                    ..Default::default()
                });
            (
                Some(fsys::ResolvedInfo { resolved_url, execution_info, ..Default::default() }),
                resolved_state.sandbox.component_input.environment().name(),
            )
        } else {
            (None, None)
        }
    };

    Ok(fsys::Instance {
        moniker: Some(moniker.to_string()),
        url: Some(instance.component_url.to_string()),
        environment: environment_name.map(|n| n.to_string()),
        instance_id: instance_id.map(|id| id.to_string()),
        resolved_info,
        ..Default::default()
    })
}

/// Encode the component manifest of an instance into a standalone persistable FIDL format.
async fn get_resolved_declaration(
    scope: &WeakComponentInstance,
    moniker_str: &str,
) -> Result<ClientEnd<fsys::ManifestBytesIteratorMarker>, fsys::GetDeclarationError> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker =
        Moniker::try_from(moniker_str).map_err(|_| fsys::GetDeclarationError::BadMoniker)?;
    let moniker = scope.moniker.concat(&moniker);

    let scope = scope.upgrade().map_err(|_| fsys::GetDeclarationError::InstanceNotFound)?;
    let instance = scope
        .find_absolute(&moniker)
        .await
        .map_err(|_| fsys::GetDeclarationError::InstanceNotFound)?;

    let decl = resolve_with_pinned_url(&instance)
        .await
        .map_err(fsys::GetDeclarationError::from)?
        .native_into_fidl();

    let bytes = fidl::persist(&decl).map_err(|error| {
        warn!(moniker:%, error:%; "RealmQuery failed to encode manifest");
        fsys::GetDeclarationError::EncodeFailed
    })?;

    // Attach the iterator task to the scope root.
    let (client_end, server_end) =
        fidl::endpoints::create_endpoints::<fsys::ManifestBytesIteratorMarker>();
    scope.execution_scope.spawn(serve_manifest_bytes_iterator(server_end, bytes));

    Ok(client_end)
}

/// Encode the component manifest of a potential instance into a standalone persistable FIDL format.
async fn resolve_declaration(
    scope: &WeakComponentInstance,
    parent_moniker_str: &str,
    child_location: &fsys::ChildLocation,
    url: &str,
) -> Result<ClientEnd<fsys::ManifestBytesIteratorMarker>, fsys::GetDeclarationError> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let parent_moniker =
        Moniker::try_from(parent_moniker_str).map_err(|_| fsys::GetDeclarationError::BadMoniker)?;
    let parent_moniker = scope.moniker.concat(&parent_moniker);

    let collection = match child_location {
        fsys::ChildLocation::Collection(coll) => coll.to_owned(),
        _ => return Err(fsys::GetDeclarationError::BadChildLocation),
    };

    let scope = scope.upgrade().map_err(|_| fsys::GetDeclarationError::InstanceNotFound)?;
    let instance = scope
        .find_absolute(&parent_moniker)
        .await
        .map_err(|_| fsys::GetDeclarationError::InstanceNotFound)?;

    let (address, collection_input) = {
        // this lock needs to be dropped before we try to call resolve, since routing the resolver
        // may also need to take this lock
        let state = instance.lock_state().await;
        let resolved_state =
            state.get_resolved_state().ok_or(fsys::GetDeclarationError::InstanceNotResolved)?;
        let address = if url.starts_with("#") {
            resolved_state
                .address_for_relative_url(url)
                .await
                .map_err(|_| fsys::GetDeclarationError::BadUrl)?
        } else {
            Url::new(url)
                .ok()
                .and_then(|url| ComponentAddress::from_absolute_url(&url).ok())
                .ok_or(fsys::GetDeclarationError::BadUrl)?
        };
        let collection_input = resolved_state
            .sandbox
            .collection_inputs
            .get(
                &Name::new(collection)
                    .map_err(|_| fsys::GetDeclarationError::InstanceNotResolved)?,
            )
            .ok_or(fsys::GetDeclarationError::InstanceNotResolved)?;
        (address, collection_input)
    };

    let resolved = instance
        .perform_resolve(Some(collection_input), &address)
        .await
        .map_err(|_| fsys::GetDeclarationError::InstanceNotResolved)?;

    let bytes = fidl::persist(&resolved.decl.native_into_fidl()).map_err(|error| {
        warn!(parent:% = parent_moniker, error:%; "RealmQuery failed to encode manifest");
        fsys::GetDeclarationError::EncodeFailed
    })?;

    // Attach the iterator task to the scope root.
    let (client_end, server_end) =
        fidl::endpoints::create_endpoints::<fsys::ManifestBytesIteratorMarker>();
    scope.execution_scope.spawn(serve_manifest_bytes_iterator(server_end, bytes));

    Ok(client_end)
}

/// Get the structured config of an instance
async fn get_structured_config(
    scope: &WeakComponentInstance,
    moniker_str: &str,
) -> Result<fcdecl::ResolvedConfig, fsys::GetStructuredConfigError> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker =
        Moniker::try_from(moniker_str).map_err(|_| fsys::GetStructuredConfigError::BadMoniker)?;
    let moniker = scope.moniker.concat(&moniker);

    let scope = scope.upgrade().map_err(|_| fsys::GetStructuredConfigError::InstanceNotFound)?;
    let instance = scope
        .find_absolute(&moniker)
        .await
        .map_err(|_| fsys::GetStructuredConfigError::InstanceNotFound)?;

    let state = instance.lock_state().await;

    let arc_config = state
        .get_resolved_state()
        .ok_or(fsys::GetStructuredConfigError::InstanceNotResolved)?
        .config()
        .ok_or(fsys::GetStructuredConfigError::NoConfig)?;

    let config = (**arc_config).clone().into();
    Ok(config)
}

async fn construct_namespace(
    scope: &WeakComponentInstance,
    moniker_str: &str,
) -> Result<Vec<fcrunner::ComponentNamespaceEntry>, fsys::ConstructNamespaceError> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker =
        Moniker::try_from(moniker_str).map_err(|_| fsys::ConstructNamespaceError::BadMoniker)?;
    let moniker = scope.moniker.concat(&moniker);

    let scope = scope.upgrade().map_err(|_| fsys::ConstructNamespaceError::InstanceNotFound)?;
    let instance = scope
        .find_absolute(&moniker)
        .await
        .map_err(|_| fsys::ConstructNamespaceError::InstanceNotFound)?;
    let state = instance.lock_state().await;
    let resolved_state =
        state.get_resolved_state().ok_or(fsys::ConstructNamespaceError::InstanceNotResolved)?;
    let namespace = create_namespace(
        resolved_state.package(),
        &instance,
        &resolved_state.storage_service_use_decls,
        &resolved_state.sandbox.program_input.namespace(),
        instance.execution_scope.clone(),
    )
    .await
    .unwrap();
    let ns = namespace.serve().unwrap();
    Ok(ns.into())
}

async fn open_directory(
    scope: &WeakComponentInstance,
    moniker_str: &str,
    dir_type: fsys::OpenDirType,
    object: ServerEnd<fio::DirectoryMarker>,
) -> Result<(), fsys::OpenError> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker = Moniker::try_from(moniker_str).map_err(|_| fsys::OpenError::BadMoniker)?;
    let moniker = scope.moniker.concat(&moniker);

    let scope = scope.upgrade().map_err(|_| fsys::OpenError::InstanceNotFound)?;
    let instance =
        scope.find_absolute(&moniker).await.map_err(|_| fsys::OpenError::InstanceNotFound)?;

    // Request all possible rights for the resulting directory connection.
    const FLAGS: fio::Flags = fio::PERM_READABLE
        .union(fio::Flags::PERM_INHERIT_EXECUTE)
        .union(fio::Flags::PERM_INHERIT_WRITE);
    let mut request = FLAGS.to_object_request(object);
    let path = vfs::path::Path::dot();

    match dir_type {
        fsys::OpenDirType::OutgoingDir => {
            let scope: package_directory::ExecutionScope = instance.execution_scope.clone();
            instance
                .open_outgoing(OpenRequest::new(scope, FLAGS, path, &mut request))
                .await
                .map_err(|e| {
                    request.shutdown(e.as_zx_status());
                    e.into()
                })
        }
        fsys::OpenDirType::RuntimeDir => {
            let state = instance.lock_state().await;
            let runtime_dir = state
                .get_started_state()
                .ok_or(fsys::OpenError::InstanceNotRunning)?
                .runtime_dir()
                .ok_or(fsys::OpenError::NoSuchDir)?;
            runtime_dir
                .open(path.as_ref(), FLAGS, &Default::default(), request.into_channel())
                .map_err(|_| fsys::OpenError::FidlError)
        }
        fsys::OpenDirType::PackageDir => {
            let state = instance.lock_state().await;
            let resolved =
                state.get_resolved_state().ok_or(fsys::OpenError::InstanceNotResolved)?;
            let package_dir = &resolved.package().ok_or(fsys::OpenError::NoSuchDir)?.package_dir;
            package_dir
                .open(path.as_ref(), FLAGS, &Default::default(), request.into_channel())
                .map_err(|_| fsys::OpenError::FidlError)
        }
        fsys::OpenDirType::ExposedDir => {
            let scope: package_directory::ExecutionScope = instance.execution_scope.clone();
            instance.open_exposed(OpenRequest::new(scope, FLAGS, path, &mut request)).await.map_err(
                |e| {
                    request.shutdown(e.as_zx_status());
                    e.into()
                },
            )
        }
        fsys::OpenDirType::NamespaceDir => {
            let state = instance.lock_state().await;
            let resolved =
                state.get_resolved_state().ok_or(fsys::OpenError::InstanceNotResolved)?;
            let namespace_dir =
                resolved.namespace_dir().await.map_err(|_| fsys::OpenError::NoSuchDir)?;
            let scope: package_directory::ExecutionScope = instance.execution_scope.clone();
            namespace_dir
                .open(scope, path, FLAGS, &mut request)
                .map_err(|_| fsys::OpenError::FidlError)
        }
        _ => Err(fsys::OpenError::BadDirType),
    }
}

async fn connect_to_storage_admin(
    scope: &WeakComponentInstance,
    moniker_str: &str,
    storage_name: String,
    server_end: ServerEnd<fsys::StorageAdminMarker>,
) -> Result<(), fsys::ConnectToStorageAdminError> {
    // Construct the complete moniker using the scope moniker and the moniker string.
    let moniker =
        Moniker::try_from(moniker_str).map_err(|_| fsys::ConnectToStorageAdminError::BadMoniker)?;
    let moniker = scope.moniker.concat(&moniker);

    let scope = scope.upgrade().map_err(|_| fsys::ConnectToStorageAdminError::InstanceNotFound)?;
    let instance = scope
        .find_absolute(&moniker)
        .await
        .map_err(|_| fsys::ConnectToStorageAdminError::InstanceNotFound)?;

    let storage_decl = {
        let decl = resolve_with_pinned_url(&instance)
            .await
            .map_err(|_e| fsys::ConnectToStorageAdminError::InstanceNotResolved)?;
        decl.find_storage_source(
            &storage_name.parse().map_err(|_| fsys::ConnectToStorageAdminError::BadCapability)?,
        )
        .ok_or(fsys::ConnectToStorageAdminError::StorageNotFound)?
        .clone()
    };

    let storage_admin = StorageAdmin::new(storage_decl, instance.as_weak())
        .await
        .map_err(|_| fsys::ConnectToStorageAdminError::BadCapability)?;
    let scope = instance.execution_scope.clone();

    scope.spawn(async move {
        if let Err(error) = storage_admin.serve(server_end.into_stream()).await {
            warn!(
                moniker:%, error:%; "StorageAdmin created by LifecycleController failed to serve",
            );
        };
    });
    Ok(())
}

/// Take a snapshot of all instances in the given scope and serves an instance iterator
/// over the snapshots.
async fn get_all_instances(
    scope: &WeakComponentInstance,
) -> Result<ClientEnd<fsys::InstanceIteratorMarker>, fsys::GetAllInstancesError> {
    let mut instances = vec![];

    // Only take instances contained within the scope realm
    let scope = scope.upgrade().map_err(|_| fsys::GetAllInstancesError::InstanceNotFound)?;

    let mut queue = vec![scope.clone()];

    while !queue.is_empty() {
        let cur = queue.pop().unwrap();

        let (instance, mut children) = get_fidl_instance_and_children(&scope, &cur).await;
        instances.push(instance);
        queue.append(&mut children);
    }

    let (client_end, server_end) =
        fidl::endpoints::create_endpoints::<fsys::InstanceIteratorMarker>();

    // Attach the iterator task to the scope root.
    scope.execution_scope.spawn(serve_instance_iterator(server_end, instances));

    Ok(client_end)
}

/// Create the detailed instance info matching the given moniker string in this scope
/// and return all live children of the instance.
async fn get_fidl_instance_and_children(
    scope: &Arc<ComponentInstance>,
    instance: &Arc<ComponentInstance>,
) -> (fsys::Instance, Vec<Arc<ComponentInstance>>) {
    let moniker = instance
        .moniker
        .strip_prefix(&scope.moniker)
        .expect("instance must have been a child of scope root");
    let instance_id = scope.context.component_id_index().id_for_moniker(&instance.moniker).cloned();

    let (resolved_info, children, environment_name) = {
        let state = instance.lock_state().await;

        if let Some(resolved_state) = state.get_resolved_state() {
            match resolved_state.address().await {
                Ok(address) => {
                    let resolved_url = Some(address.url().to_string());
                    let children = resolved_state.children().map(|(_, c)| c.clone()).collect();
                    let execution_info =
                        state.get_started_state().map(|started_state| fsys::ExecutionInfo {
                            start_reason: Some(started_state.start_reason.to_string()),
                            ..Default::default()
                        });
                    (
                        Some(fsys::ResolvedInfo {
                            resolved_url,
                            execution_info,
                            ..Default::default()
                        }),
                        children,
                        resolved_state.sandbox.component_input.environment().name(),
                    )
                }
                Err(err) => {
                    warn!(err:%, moniker:%; "GetAllInstances: could not fetch component address?");
                    (None, vec![], None)
                }
            }
        } else {
            (None, vec![], None)
        }
    };

    (
        fsys::Instance {
            moniker: Some(moniker.to_string()),
            url: Some(instance.component_url.to_string()),
            environment: environment_name.map(|n| n.to_string()),
            instance_id: instance_id.map(|id| id.to_string()),
            resolved_info,
            ..Default::default()
        },
        children,
    )
}

async fn serve_instance_iterator(
    server_end: ServerEnd<fsys::InstanceIteratorMarker>,
    instances: Vec<fsys::Instance>,
) {
    let mut remaining_instances = &instances[..];
    let mut stream: fsys::InstanceIteratorRequestStream = server_end.into_stream();
    while let Some(Ok(fsys::InstanceIteratorRequest::Next { responder })) = stream.next().await {
        let mut bytes_used: usize = FIDL_HEADER_BYTES + FIDL_VECTOR_HEADER_BYTES;
        let mut instance_count = 0;

        // Determine how many info objects can be sent in a single FIDL message.
        // TODO(https://fxbug.dev/42181010): This logic should be handled by FIDL.
        for instance in remaining_instances {
            bytes_used += instance.measure().num_bytes;
            if bytes_used > ZX_CHANNEL_MAX_MSG_BYTES as usize {
                break;
            }
            instance_count += 1;
        }

        let result = responder.send(&remaining_instances[..instance_count]);
        remaining_instances = &remaining_instances[instance_count..];
        if let Err(error) = result {
            warn!(error:?; "RealmQuery encountered error sending instance batch");
            break;
        }

        // Close the iterator because all the data was sent.
        if instance_count == 0 {
            break;
        }
    }
}

async fn serve_manifest_bytes_iterator(
    server_end: ServerEnd<fsys::ManifestBytesIteratorMarker>,
    mut bytes: Vec<u8>,
) {
    let mut stream: fsys::ManifestBytesIteratorRequestStream = server_end.into_stream();

    while let Some(Ok(fsys::ManifestBytesIteratorRequest::Next { responder })) = stream.next().await
    {
        let bytes_to_drain = std::cmp::min(FIDL_MANIFEST_MAX_MSG_BYTES, bytes.len());
        let batch: Vec<u8> = bytes.drain(0..bytes_to_drain).collect();
        let batch_size = batch.len();

        let result = responder.send(&batch);
        if let Err(error) = result {
            warn!(error:?; "RealmQuery encountered error sending manifest bytes");
            break;
        }

        // Close the iterator because all the data was sent.
        if batch_size == 0 {
            break;
        }
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::component::StartReason;
    use crate::model::start::Start;
    use crate::model::testing::test_helpers::{
        TestEnvironmentBuilder, TestModelResult, config_override, new_config_decl,
    };
    use ::routing::component_instance::ComponentInstanceInterface;
    use assert_matches::assert_matches;
    use cm_rust::*;
    use cm_rust_testing::*;
    use component_id_index::InstanceId;
    use fidl::endpoints;
    use fidl::endpoints::{create_endpoints, create_proxy};
    use fidl_fuchsia_component_decl as fcdecl;
    use fidl_fuchsia_io as fio;
    use routing_test_helpers::component_id_index::make_index_file;

    fn is_closed(handle: impl fidl::AsHandleRef) -> bool {
        handle
            .as_handle_ref()
            .wait_one(zx::Signals::OBJECT_PEER_CLOSED, zx::MonotonicInstant::from_nanos(0))
            .is_ok()
    }

    async fn realm_query(test: &TestModelResult) -> fsys::RealmQueryProxy {
        let (proxy, server) = endpoints::create_proxy::<fsys::RealmQueryMarker>();
        let weak_root = test.model.root().as_weak();
        test.model.root().execution_scope.spawn(async move {
            serve(server.into_channel(), weak_root.clone(), weak_root).await.unwrap();
        });
        proxy
    }

    #[fuchsia::test]
    async fn get_instance_test() {
        // Create index.
        let iid = format!("1234{}", "5".repeat(60)).parse::<InstanceId>().unwrap();
        let index = {
            let mut index = component_id_index::Index::default();
            index
                .insert(component_id_index::IndexEntry {
                    moniker: Moniker::parse_str("/").unwrap(),
                    instance_id: iid.clone(),
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
        };
        let index_file = make_index_file(index).unwrap();

        let components = vec![("root", ComponentDeclBuilder::new().build())];

        let test = TestEnvironmentBuilder::new()
            .set_components(components)
            .set_component_id_index_path(index_file.path().to_owned().try_into().unwrap())
            .build()
            .await;
        let query = realm_query(&test).await;

        test.model.start().await;

        let instance = query.get_instance(".").await.unwrap().unwrap();

        assert_eq!(instance.moniker.unwrap(), ".");
        assert_eq!(instance.url.unwrap(), "test:///root");
        assert_eq!(instance.instance_id.unwrap().parse::<InstanceId>().unwrap(), iid);

        let resolved = instance.resolved_info.unwrap();
        assert_eq!(resolved.resolved_url.unwrap(), "test:///root");

        let execution = resolved.execution_info.unwrap();
        assert_eq!(execution.start_reason.unwrap(), StartReason::Root.to_string());
    }

    #[fuchsia::test]
    async fn manifest_test() {
        // Try to create a manifest that will exceed the size of a Zircon channel message.
        let mut manifest = ComponentDeclBuilder::new();

        // This variable makes it easy to tune this to exactly the size manifest needed for
        // ensuring that the serialized manifest is larger than a channel message.
        let num_items = 200;

        for i in 0..num_items {
            let use_name = format!("use_{}", i);
            let expose_name = format!("expose_{}", i);
            let capability_path = format!("/svc/capability_{}", i);

            let use_decl = UseBuilder::protocol()
                .source(UseSource::Framework)
                .name(&use_name)
                .path(&capability_path)
                .build();
            let expose_decl =
                ExposeBuilder::protocol().source(ExposeSource::Self_).name(&expose_name).build();
            let capability_decl = CapabilityBuilder::protocol().name(&expose_name).build();

            manifest = manifest.use_(use_decl).expose(expose_decl).capability(capability_decl);
        }

        let components = vec![("root", manifest.build())];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let query = realm_query(&test).await;

        test.model.start().await;

        let iterator = query.get_resolved_declaration("./").await.unwrap().unwrap();
        let iterator = iterator.into_proxy();

        let mut bytes = vec![];

        loop {
            let mut batch = iterator.next().await.unwrap();
            if batch.is_empty() {
                break;
            }
            bytes.append(&mut batch);
        }

        // Validate that the manifest is actually large enough to span multiple messages, but not
        // too large as to be burdensome when running the test.
        assert!(
            bytes.len() > 64 * 1024,
            "Manifest is not larger than 64KiB in size, increase 'num_items' to make it larger"
        );
        assert!(
            bytes.len() < 192 * 1024,
            "Manifest is greater than than 192KB in size, decrease 'num_items' to make it smaller"
        );

        let manifest = fidl::unpersist::<fcdecl::Component>(&bytes).unwrap();

        // Component should have 'num_items' use and expose decls
        let uses = manifest.uses.unwrap();
        let exposes = manifest.exposes.unwrap();
        assert_eq!(uses.len(), num_items);

        for use_ in uses {
            let use_: cm_rust::UseDecl = use_.fidl_into_native();
            assert!(use_.source_name().as_str().starts_with("use_"));
            assert!(use_.path().unwrap().to_string().starts_with("/svc/capability_"));
        }

        assert_eq!(exposes.len(), num_items);

        for expose in exposes {
            let expose: cm_rust::ExposeDecl = expose.fidl_into_native();
            assert!(expose.source_name().as_str().starts_with("expose_"));
        }
    }

    #[fuchsia::test]
    async fn structured_config_test() {
        let (config, config_values, checksum) = new_config_decl();

        let components = vec![("root", ComponentDeclBuilder::new().config(config).build())];

        let test = TestEnvironmentBuilder::new()
            .set_components(components)
            .set_config_values(vec![("meta/root.cvf", config_values)])
            .build()
            .await;
        let query = realm_query(&test).await;

        test.model.start().await;

        let config = query.get_structured_config("./").await.unwrap().unwrap();

        // Component should have one config field with right value
        assert_eq!(config.fields.len(), 1);
        let field = &config.fields[0];
        assert_eq!(field.key, "my_field");
        assert_matches!(
            field.value,
            fcdecl::ConfigValue::Single(fcdecl::ConfigSingleValue::Bool(true))
        );
        assert_eq!(config.checksum, checksum.native_into_fidl());
    }

    #[fuchsia::test]
    async fn override_structured_config_test() {
        let (config, config_values, checksum) = new_config_decl();

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new().child(ChildBuilder::new().name("top").eager()).build(),
            ),
            ("top", ComponentDeclBuilder::new().config(config).build()),
        ];

        let test = TestEnvironmentBuilder::new()
            .set_components(components)
            .set_config_values(vec![("meta/root.cvf", config_values)])
            .build()
            .await;
        let query = realm_query(&test).await;
        let config_override_proxy = config_override(&test).await;

        test.model.start().await;

        let config = query.get_structured_config("top").await.unwrap().unwrap();

        // Component should have one config field with right value
        assert_eq!(config.fields.len(), 1);
        let field = &config.fields[0];
        assert_eq!(field.key, "my_field");
        assert_matches!(
            field.value,
            fcdecl::ConfigValue::Single(fcdecl::ConfigSingleValue::Bool(true))
        );
        assert_eq!(config.checksum, checksum.clone().native_into_fidl());

        // Override the config field value
        config_override_proxy
            .set_structured_config(
                "top",
                &[fcdecl::ConfigOverride {
                    key: Some("my_field".to_string()),
                    value: Some(fcdecl::ConfigValue::Single(fcdecl::ConfigSingleValue::Bool(
                        false,
                    ))),
                    ..Default::default()
                }],
            )
            .await
            .unwrap()
            .unwrap();
        // Unresolve and restart the component so that the configuration override will take effect.
        let top = test.model.root().find_and_maybe_resolve(&"top".parse().unwrap()).await.unwrap();
        top.unresolve().await.unwrap();
        top.ensure_started(&StartReason::Root).await.unwrap();
        let config = query.get_structured_config("top").await.unwrap().unwrap();

        // Component should have one config field with the override value
        assert_eq!(config.fields.len(), 1);
        let field = &config.fields[0];
        assert_eq!(field.key, "my_field");
        assert_matches!(
            field.value,
            fcdecl::ConfigValue::Single(fcdecl::ConfigSingleValue::Bool(false))
        );
        assert_eq!(config.checksum, checksum.native_into_fidl());
    }

    #[fuchsia::test]
    async fn open_test() {
        let use_decl = UseBuilder::protocol().source(UseSource::Framework).name("foo").build();
        let expose_decl = ExposeBuilder::protocol().source(ExposeSource::Self_).name("bar").build();

        let components = vec![(
            "root",
            ComponentDeclBuilder::new()
                .use_(use_decl)
                .expose(expose_decl)
                .protocol_default("bar")
                .build(),
        )];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let query = realm_query(&test).await;

        test.model.start().await;

        let (outgoing_dir, server_end) = create_endpoints::<fio::DirectoryMarker>();
        query
            .open_directory("./", fsys::OpenDirType::OutgoingDir, server_end)
            .await
            .unwrap()
            .unwrap();
        // The test runner has not been configured to serve the outgoing dir, so this directory
        // should just be closed.
        assert!(is_closed(outgoing_dir));

        let (runtime_dir, server_end) = create_endpoints::<fio::DirectoryMarker>();
        query
            .open_directory("./", fsys::OpenDirType::RuntimeDir, server_end)
            .await
            .unwrap()
            .unwrap();
        // The test runner has not been configured to serve the runtime dir, so this directory
        // should just be closed.
        assert!(is_closed(runtime_dir));

        let (pkg_dir, server_end) = create_proxy::<fio::DirectoryMarker>();
        query
            .open_directory("./", fsys::OpenDirType::PackageDir, server_end)
            .await
            .unwrap()
            .unwrap();

        let (exposed_dir, server_end) = create_proxy::<fio::DirectoryMarker>();
        query
            .open_directory("./", fsys::OpenDirType::ExposedDir, server_end)
            .await
            .unwrap()
            .unwrap();

        let (namespace_dir, server_end) = create_proxy::<fio::DirectoryMarker>();
        query
            .open_directory("./", fsys::OpenDirType::NamespaceDir, server_end)
            .await
            .unwrap()
            .unwrap();

        let (svc_dir, server_end) = create_proxy::<fio::DirectoryMarker>();
        namespace_dir
            .open("svc", fio::PERM_READABLE, &Default::default(), server_end.into_channel())
            .unwrap();

        // Test resolvers provide a pkg dir with a fake file
        let entries = fuchsia_fs::directory::readdir(&pkg_dir).await.unwrap();
        assert_eq!(
            entries,
            vec![fuchsia_fs::directory::DirEntry {
                name: "fake_file".to_string(),
                kind: fuchsia_fs::directory::DirentKind::File
            }]
        );

        // Component Manager serves the exposed dir with the `bar` protocol
        let entries = fuchsia_fs::directory::readdir(&exposed_dir).await.unwrap();
        assert_eq!(
            entries,
            vec![fuchsia_fs::directory::DirEntry {
                name: "bar".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Service
            }]
        );

        // Component Manager serves the namespace dir with the `foo` protocol.
        let entries = fuchsia_fs::directory::readdir(&svc_dir).await.unwrap();
        assert_eq!(
            entries,
            vec![fuchsia_fs::directory::DirEntry {
                name: "foo".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Service
            }]
        );
    }

    #[fuchsia::test]
    async fn construct_namespace_test() {
        let use_decl = UseBuilder::protocol().source(UseSource::Framework).name("foo").build();

        let components = vec![("root", ComponentDeclBuilder::new().use_(use_decl.clone()).build())];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let query = realm_query(&test).await;

        test.model.start().await;

        let mut ns = query.construct_namespace("./").await.unwrap().unwrap();

        assert_eq!(ns.len(), 2);
        ns.sort_by_key(|entry| entry.path.as_ref().unwrap().clone());

        // Test resolvers provide a pkg dir with a fake file
        let pkg_entry = ns.remove(0);
        assert_eq!(pkg_entry.path.unwrap(), "/pkg");
        let pkg_dir = pkg_entry.directory.unwrap().into_proxy();

        let entries = fuchsia_fs::directory::readdir(&pkg_dir).await.unwrap();
        assert_eq!(
            entries,
            vec![fuchsia_fs::directory::DirEntry {
                name: "fake_file".to_string(),
                kind: fuchsia_fs::directory::DirentKind::File
            }]
        );

        // The component requested the `foo` protocol.
        let svc_entry = ns.remove(0);
        assert_eq!(svc_entry.path.unwrap(), "/svc");
        let svc_dir = svc_entry.directory.unwrap().into_proxy();

        let entries = fuchsia_fs::directory::readdir(&svc_dir).await.unwrap();
        assert_eq!(
            entries,
            vec![fuchsia_fs::directory::DirEntry {
                name: "foo".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Service
            }]
        );
    }

    #[fuchsia::test]
    async fn get_storage_admin_test() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .child_default("a")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("data")
                            .backing_dir("fs")
                            .source(StorageDirectorySource::Child("a".into()))
                            .subdir("persistent"),
                    )
                    .build(),
            ),
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("fs")
                            .path("/fs/data")
                            .rights(fio::Operations::all()),
                    )
                    .expose(ExposeBuilder::directory().name("fs").source(ExposeSource::Self_))
                    .build(),
            ),
        ];

        let test = TestEnvironmentBuilder::new().set_components(components).build().await;
        let query = realm_query(&test).await;

        test.model.start().await;

        let (storage_admin, server_end) = create_proxy::<fsys::StorageAdminMarker>();

        query.connect_to_storage_admin("./", "data", server_end).await.unwrap().unwrap();

        let (it_proxy, it_server) = create_proxy::<fsys::StorageIteratorMarker>();

        storage_admin.list_storage_in_realm("./", it_server).await.unwrap().unwrap();

        let res = it_proxy.next().await.unwrap();
        assert!(res.is_empty());
    }
}
