// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod admin_protocol;
use crate::model::component::{ComponentInstance, StartReason, WeakComponentInstance};
use crate::model::start::Start;
use crate::model::storage::admin_protocol::StorageAdmin;
use crate::sandbox_util::LaunchTaskOnReceive;
use ::routing::component_instance::ComponentInstanceInterface;
use ::routing::error::{RouteVerb, RoutingError};
use ::routing::intermediate_router::{IntermediateRouter, RouteRequest};
use capability_source::{
    CapabilitySource, CapabilityToCapabilitySource, ComponentCapability, ComponentSource,
    NamespaceSource, StorageBackingDirectorySource,
};
use cm_rust::{CapabilityTypeName, NativeIntoFidl, StorageDirectorySource};
use cm_types::RelativePath;
use component_id_index::InstanceId;
use derivative::Derivative;
use errors::{ModelError, StorageError};
use fidl::endpoints::{ServerEnd, create_proxy};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_sys2 as fsys;
use futures::FutureExt;
use moniker::{ChildName, Moniker};
use runtime_capabilities::{Dictionary, DirConnector, Router};
use std::path::PathBuf;
use std::sync::Arc;
use vfs::ToObjectRequest;
use vfs::directory::entry::OpenRequest;

// TODO: The `use` declaration for storage implicitly carries these rights. While this is
// correct, it would be more consistent to get the rights from `CapabilityState`.
const FLAGS: fio::Flags =
    fio::PERM_READABLE.union(fio::PERM_WRITABLE).union(fio::Flags::PROTOCOL_DIRECTORY);

/// Information returned by the route_storage_capability function on the backing directory source
/// of a storage capability.
#[derive(Debug, Derivative)]
#[derivative(Clone(bound = ""))]
pub struct BackingDirectoryInfo {
    /// The component that's providing the backing directory capability for this storage
    /// capability. If None, then the backing directory comes from component_manager's namespace.
    pub storage_provider: Option<Arc<ComponentInstance>>,

    /// The path to the backing directory in the providing component's outgoing directory (or
    /// component_manager's namespace).
    pub backing_directory_path: cm_types::Path,

    /// The subdirectory inside of the backing directory capability to use, if any
    pub backing_directory_subdir: RelativePath,

    /// The subdirectory inside of the backing directory's sub-directory to use, if any. The
    /// difference between this and backing_directory_subdir is that backing_directory_subdir is
    /// appended to backing_directory_path first, and component_manager will create this subdir if
    /// it doesn't exist but won't create backing_directory_subdir.
    pub storage_subdir: RelativePath,

    /// The moniker of the component that defines the storage capability. This is used for
    /// generating moniker-based storage paths.
    pub storage_source_moniker: Moniker,
}

impl PartialEq for BackingDirectoryInfo {
    fn eq(&self, other: &Self) -> bool {
        let self_source_component = self.storage_provider.as_ref().map(|s| s.moniker());
        let other_source_component = other.storage_provider.as_ref().map(|s| s.moniker());
        self_source_component == other_source_component
            && self.backing_directory_path == other.backing_directory_path
            && self.backing_directory_subdir == other.backing_directory_subdir
            && self.storage_subdir == other.storage_subdir
    }
}

async fn open_storage_root(
    storage_source_info: &BackingDirectoryInfo,
) -> Result<fio::DirectoryProxy, ModelError> {
    let (mut dir_proxy, local_server_end) = create_proxy::<fio::DirectoryMarker>();
    let mut full_backing_directory_path = storage_source_info.backing_directory_path.clone();
    if !full_backing_directory_path.extend(storage_source_info.backing_directory_subdir.clone()) {
        return Err(RoutingError::PathTooLong {
            moniker: storage_source_info.storage_source_moniker.clone().into(),
            path: format!(
                "{}/{}",
                full_backing_directory_path, storage_source_info.backing_directory_subdir
            ),
            keyword: "backing_directory_path".into(),
        }
        .into());
    }
    let path = full_backing_directory_path.to_string();
    if let Some(dir_source_component) = storage_source_info.storage_provider.as_ref() {
        // TODO(https://fxbug.dev/42127827): This should be StartReason::AccessCapability, but we haven't
        // plumbed in all the details needed to use it.
        dir_source_component.ensure_started(&StartReason::StorageAdmin).await?;
        let path = path.try_into().map_err(|_| ModelError::BadPath)?;
        let mut object_request = FLAGS.to_object_request(local_server_end.into_channel());
        dir_source_component
            .open_outgoing(OpenRequest::new(
                dir_source_component.execution_scope.clone(),
                FLAGS,
                path,
                &mut object_request,
            ))
            .await?;
    } else {
        // If storage_source_info.storage_provider is None, the directory comes from component_manager's namespace
        fuchsia_fs::directory::open_channel_in_namespace(&path, FLAGS, local_server_end).map_err(
            |e| {
                ModelError::from(StorageError::open_root(
                    None,
                    storage_source_info.backing_directory_path.clone(),
                    e,
                ))
            },
        )?;
    }
    if !storage_source_info.storage_subdir.is_dot() {
        dir_proxy = fuchsia_fs::directory::create_directory_recursive(
            &dir_proxy,
            &storage_source_info.storage_subdir.to_string(),
            FLAGS,
        )
        .await
        .map_err(|e| {
            ModelError::from(StorageError::open_root(
                storage_source_info.storage_provider.as_ref().map(|r| r.moniker().clone()),
                storage_source_info.backing_directory_path.clone(),
                e,
            ))
        })?;
    }
    Ok(dir_proxy)
}

/// Routes a backing directory for a storage capability to its source, returning the data needed to
/// open the storage capability.
///
/// If the capability is not allowed to be routed to the `target`, per the
/// [`crate::model::policy::GlobalPolicyChecker`], then an error is returned.
///
/// REQUIRES: `storage_source` is `ComponentCapability::Storage`.
pub async fn route_backing_directory(
    target: &Arc<ComponentInstance>,
    storage_source: CapabilitySource,
) -> Result<BackingDirectoryInfo, RoutingError> {
    let (storage_decl, storage_component) = match storage_source {
        CapabilitySource::Component(ComponentSource {
            capability: ComponentCapability::Storage(storage_decl),
            moniker,
        }) => {
            let component = target.find_absolute(&moniker).await?;
            (storage_decl, component)
        }
        CapabilitySource::StorageBackingDirectory(StorageBackingDirectorySource {
            capability,
            moniker,
            backing_dir_subdir,
            storage_subdir,
            storage_source_moniker,
        }) => {
            let component = target.find_absolute(&moniker).await?;
            return Ok(BackingDirectoryInfo {
                storage_provider: Some(component),
                backing_directory_path: capability
                    .source_path()
                    .expect("directory missing source path")
                    .clone(),
                backing_directory_subdir: backing_dir_subdir,
                storage_subdir,
                storage_source_moniker,
            });
        }
        r => {
            return Err(RoutingError::unsupported_route_source(
                target.moniker.clone(),
                r.type_name().map(|s| s.to_string()).unwrap_or_else(|| "<unknown>".to_string()),
            ));
        }
    };

    let source_dictionary = match &storage_decl.source {
        StorageDirectorySource::Parent => {
            storage_component.component_sandbox().await?.component_input.capabilities()
        }
        StorageDirectorySource::Self_ => {
            storage_component.component_sandbox().await?.program_output_dict.clone()
        }
        StorageDirectorySource::Child(name) => {
            let child_name = ChildName::parse(name)
                .expect("invalid child name, this should be prevented by manifest validation");
            let child_component = ComponentInstanceInterface::lock_resolved_state(&storage_component).await?.get_child(&child_name).expect("resolver registration references nonexistent static child, this should be prevented by manifest validation");
            let child_sandbox = child_component.component_sandbox().await?;
            child_sandbox.component_output.capabilities().clone()
        }
    };
    let backing_dir_router: Arc<Router<DirConnector>> = IntermediateRouter::new(
        Arc::downgrade(&source_dictionary).into(),
        vec![storage_decl.backing_dir.clone()].into(),
        RouteRequest {
            build_type_name: CapabilityTypeName::Directory,
            availability: Some(cm_rust::Availability::Required),
            directory_rights: Some(fio::PERM_READABLE | fio::PERM_WRITABLE),
            sub_directory_path: Some(RelativePath::dot()),
            inherit_rights: Some(false),
            ..Default::default()
        },
        target.as_weak().into(),
        target.moniker().clone(),
        RouteVerb::Declare,
        storage_decl.source.clone().native_into_fidl(),
    )
    .try_into()
    .expect("wrong type from intermediate router");
    let source = backing_dir_router
        .route_debug(
            routing::bedrock::request_metadata::directory_metadata(
                cm_types::Availability::Transitional,
                Some(fio::RW_STAR_DIR.into()),
                None,
            ),
            storage_component.as_weak().into(),
        )
        .await
        .map_err(|e| RoutingError::try_from(e).expect("invalid routing error"))?;

    let (dir_source_path, dir_source_instance, dir_subdir) = match source {
        CapabilitySource::StorageBackingDirectory(StorageBackingDirectorySource {
            capability,
            moniker,
            backing_dir_subdir,
            ..
        }) => {
            let dir_source_instance = storage_component.find_absolute(&moniker).await?;
            (
                capability.source_path().expect("directory has no source path?").clone(),
                Some(dir_source_instance),
                backing_dir_subdir,
            )
        }
        CapabilitySource::Namespace(NamespaceSource { capability, .. }) => (
            capability.source_path().expect("directory has no source path?").clone(),
            None,
            RelativePath::dot(),
        ),
        CapabilitySource::Component(ComponentSource {
            capability: ComponentCapability::Directory(decl),
            moniker,
        }) => {
            let dir_source_instance = storage_component.find_absolute(&moniker).await?;
            let source_path = decl.source_path.ok_or(RoutingError::UnsupportedRouteSource {
                source_type: "component_without_path".to_string(),
                moniker: moniker.clone().into(),
            })?;
            (source_path, Some(dir_source_instance), RelativePath::dot())
        }
        s => {
            return Err(RoutingError::UnsupportedRouteSource {
                source_type: format!("{:?}", s.type_name()),
                moniker: target.moniker.clone().into(),
            }
            .into());
        }
    };

    Ok(BackingDirectoryInfo {
        storage_provider: dir_source_instance,
        backing_directory_path: dir_source_path,
        backing_directory_subdir: dir_subdir,
        storage_subdir: storage_decl.subdir.clone(),
        storage_source_moniker: storage_component.moniker().clone(),
    })
}

/// Open the isolated storage sub-directory from the given storage capability source, creating it
/// if necessary. The storage sub-directory is based on provided instance ID if present, otherwise
/// it is based on the provided moniker.
#[cfg(all(test, not(feature = "src_model_tests")))]
pub async fn open_isolated_storage(
    storage_source_info: &BackingDirectoryInfo,
    moniker: Moniker,
    instance_id: Option<&InstanceId>,
) -> Result<fio::DirectoryProxy, ModelError> {
    let root_dir = open_storage_root(storage_source_info).await?;
    let storage_path = match instance_id {
        Some(id) => generate_instance_id_based_storage_path(id),
        None => generate_moniker_based_storage_path(&moniker),
    };
    fuchsia_fs::directory::create_directory_recursive(
        &root_dir,
        storage_path.to_str().expect("must be utf-8"),
        FLAGS,
    )
    .await
    .map_err(|e| {
        ModelError::from(StorageError::open(
            storage_source_info.storage_provider.as_ref().map(|r| r.moniker().clone()),
            storage_source_info.backing_directory_path.clone(),
            moniker.clone(),
            instance_id.cloned(),
            e,
        ))
    })
}

/// Open the isolated storage sub-directory from the given storage capability source, creating it
/// if necessary. The storage sub-directory is based on provided instance ID.
pub async fn open_isolated_storage_by_id(
    storage_source_info: &BackingDirectoryInfo,
    instance_id: &InstanceId,
) -> Result<fio::DirectoryProxy, ModelError> {
    let root_dir = open_storage_root(storage_source_info).await?;
    let storage_path = generate_instance_id_based_storage_path(instance_id);

    fuchsia_fs::directory::create_directory_recursive(
        &root_dir,
        storage_path.to_str().expect("must be utf-8"),
        FLAGS,
    )
    .await
    .map_err(|e| {
        ModelError::from(StorageError::open_by_id(
            storage_source_info.storage_provider.as_ref().map(|r| r.moniker().clone()),
            storage_source_info.backing_directory_path.clone(),
            instance_id.clone(),
            e,
        ))
    })
}

/// Delete the isolated storage sub-directory for the given component.  `dir_source_component` and
/// `dir_source_path` are the component hosting the directory and its capability path. Note that
/// this removes the backing storage directory, meaning if this is called while the using
/// component is still alive, that component's storage handle will start returning errors.
pub async fn delete_isolated_storage(
    storage_source_info: BackingDirectoryInfo,
    moniker: Moniker,
    instance_id: Option<&InstanceId>,
) -> Result<(), ModelError> {
    let root_dir = open_storage_root(&storage_source_info).await?;

    let (dir, name) = if let Some(instance_id) = instance_id {
        let storage_path = generate_instance_id_based_storage_path(instance_id);
        let file_name = storage_path
            .file_name()
            .ok_or_else(|| {
                StorageError::invalid_storage_path(moniker.clone(), Some(instance_id.clone()))
            })?
            .to_str()
            .expect("must be utf-8")
            .to_string();

        let parent_path = storage_path.parent().ok_or_else(|| {
            StorageError::invalid_storage_path(moniker.clone(), Some(instance_id.clone()))
        })?;
        let parent_path_str = parent_path.to_str().expect("must be utf-8");
        let dir = if parent_path_str.is_empty() {
            root_dir
        } else {
            fuchsia_fs::directory::open_directory_async(&root_dir, parent_path_str, FLAGS).map_err(
                |e| {
                    StorageError::open(
                        storage_source_info.storage_provider.as_ref().map(|r| r.moniker().clone()),
                        storage_source_info.backing_directory_path.clone(),
                        moniker.clone(),
                        None,
                        e,
                    )
                },
            )?
        };
        (dir, file_name)
    } else {
        let storage_path = generate_moniker_based_storage_path(&moniker);
        // We want to strip off the "data" portion of the path, and then one more level to get to the
        // directory holding the target component's storage.
        let storage_path_parent = storage_path
            .parent()
            .ok_or_else(|| StorageError::invalid_storage_path(moniker.clone(), None))?;
        let dir_path = storage_path_parent
            .parent()
            .ok_or_else(|| StorageError::invalid_storage_path(moniker.clone(), None))?;
        let name = storage_path_parent
            .file_name()
            .ok_or_else(|| StorageError::invalid_storage_path(moniker.clone(), None))?;
        let name = name.to_str().expect("must be utf-8");
        let dir = if dir_path.parent().is_none() {
            root_dir
        } else {
            fuchsia_fs::directory::open_directory_async(
                &root_dir,
                dir_path.to_str().unwrap(),
                FLAGS,
            )
            .map_err(|e| {
                StorageError::open(
                    storage_source_info.storage_provider.as_ref().map(|r| r.moniker().clone()),
                    storage_source_info.backing_directory_path.clone(),
                    moniker.clone(),
                    None,
                    e,
                )
            })?
        };
        (dir, name.to_string())
    };

    // TODO(https://fxbug.dev/42111898): This function is subject to races. If another process has a handle to the
    // isolated storage directory, it can add files while this function is running. That could
    // cause it to spin or fail because a subdir was not empty after it removed all the contents.
    // It's also possible that the directory was already deleted by the backing component or a
    // prior run.
    fuchsia_fs::directory::remove_dir_recursive(&dir, &name).await.map_err(|e| {
        StorageError::remove(
            storage_source_info.storage_provider.as_ref().map(|r| r.moniker().clone()),
            storage_source_info.backing_directory_path.clone(),
            moniker.clone(),
            instance_id.cloned(),
            e,
        )
    })?;
    Ok(())
}

/// Generates the path into a directory the provided component will be afforded for storage
///
/// The path of the sub-directory for a component that uses a storage capability is based on each
/// component instance's child moniker as given in the `children` section of its parent's manifest,
/// for each component instance in the path from the `storage` declaration to the
/// `use` declaration.
///
/// These names are used as path elements, separated by elements of the name "children". The
/// string "data" is then appended to this path for compatibility reasons.
///
/// For example, if the following component instance tree exists, with `a` declaring storage
/// capabilities, and then storage being offered down the chain to `d`:
///
/// ```
///  a  <- declares storage "cache", offers "cache" to b
///  |
///  b  <- offers "cache" to c
///  |
///  c  <- offers "cache" to d
///  |
///  d  <- uses "cache" storage as `/my_cache`
/// ```
///
/// When `d` attempts to access `/my_cache` the framework creates the sub-directory
/// `b:0/children/c:0/children/d:0/data` in the directory used by `a` to declare storage
/// capabilities.  Then, the framework gives 'd' access to this new directory.
///
/// Note the ":0" suffix is for backwards compatibility and has no independent meaning (it used to
/// indicate a unique instance id).
fn generate_moniker_based_storage_path(moniker: &Moniker) -> PathBuf {
    assert!(
        !moniker.is_root(),
        "storage capability appears to have been exposed or used by its source"
    );

    let path = moniker.path();
    let mut path = path.iter();
    let mut dir_path = vec![format!("{}:0", path.next().unwrap())];
    while let Some(p) = path.next() {
        dir_path.push("children".to_string());
        dir_path.push(format!("{p}:0"));
    }

    // Storage capabilities used to have a hardcoded set of types, which would be appended
    // here. To maintain compatibility with the old paths (and thus not lose data when this was
    // migrated) we append "data" here. This works because this is the only type of storage
    // that was actually used in the wild.
    //
    // This is only temporary, until the storage instance id migration changes this layout.
    dir_path.push("data".to_string());
    dir_path.into_iter().collect()
}

/// Generates the component storage directory path for the provided component instance.
///
/// Components which do not have an instance ID use a generate moniker-based storage path instead.
pub(crate) fn generate_instance_id_based_storage_path(instance_id: &InstanceId) -> PathBuf {
    instance_id.to_string().into()
}

/// Builds a dictionary where each key is the name of a storage capability declared in this
/// component, and the value is a router for a storage admin protocol from that storage capability.
pub fn build_storage_admin_dictionary(
    component: &Arc<ComponentInstance>,
    decl: &cm_rust::ComponentDecl,
) -> Arc<Dictionary> {
    let storage_admin_dictionary = Dictionary::new();
    for storage_decl in decl.capabilities.iter().filter_map(|capability| match capability {
        cm_rust::CapabilityDecl::Storage(storage_decl) => Some(storage_decl.clone()),
        _ => None,
    }) {
        let capability_source = CapabilitySource::Capability(CapabilityToCapabilitySource {
            source_capability: ComponentCapability::Storage(storage_decl.clone()),
            moniker: component.moniker.clone(),
        });
        let storage_decl = storage_decl;
        let weak_component = WeakComponentInstance::new(component);
        storage_admin_dictionary.insert(
            storage_decl.name.clone(),
            LaunchTaskOnReceive::new(
                capability_source,
                component.execution_scope.as_weak(),
                "storage admin protocol",
                Some(component.context.policy().clone()),
                Arc::new(move |channel, _target, _, _| {
                    let storage_decl = storage_decl.clone();
                    let weak_component = weak_component.clone();
                    let stream = ServerEnd::<fsys::StorageAdminMarker>::new(channel).into_stream();
                    async move {
                        StorageAdmin::new(storage_decl, weak_component).await?.serve(stream).await
                    }
                    .boxed()
                }),
            )
            .into_router()
            .into(),
        );
    }
    storage_admin_dictionary
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::testing::routing_test_helpers::{RoutingTest, RoutingTestBuilder};
    use crate::model::testing::test_helpers::{self, component_decl_with_test_runner};
    use assert_matches::assert_matches;
    use cm_rust::*;
    use cm_rust_testing::*;
    use fidl_fuchsia_io as fio;
    use rand::RngExt as _;
    use rand::distr::Alphanumeric;

    #[fuchsia::test]
    async fn open_isolated_storage_test() {
        let components = vec![
            ("a", ComponentDeclBuilder::new().child_default("b").child_default("c").build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("data")
                            .source(ExposeSource::Self_)
                            .rights(fio::Operations::CONNECT),
                    )
                    .build(),
            ),
        ];
        let test = RoutingTest::new("a", components).await;
        let b_component = test
            .model
            .root()
            .find_and_maybe_resolve(&["b"].try_into().unwrap())
            .await
            .expect("failed to find component for b:0");
        let dir_source_path: cm_types::Path = "/data".parse().unwrap();
        let moniker = Moniker::try_from(["c", "coll:d"]).unwrap();

        // Open.
        let dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: Moniker::root(),
            },
            moniker.clone(),
            None,
        )
        .await
        .expect("failed to open isolated storage");
        assert_eq!(test_helpers::list_directory(&dir).await, Vec::<String>::new());
        test_helpers::write_file(&dir, "file", "hippos").await;
        assert_eq!(test_helpers::list_directory(&dir).await, vec!["file".to_string()]);

        // Open again.
        let dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: Moniker::root(),
            },
            moniker.clone(),
            None,
        )
        .await
        .expect("failed to open isolated storage");
        assert_eq!(test_helpers::list_directory(&dir).await, vec!["file".to_string()]);

        // Open another component's storage.
        let moniker = Moniker::try_from(["c", "coll:d", "e"]).unwrap();
        let dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: Moniker::root(),
            },
            moniker.clone(),
            None,
        )
        .await
        .expect("failed to open isolated storage");
        assert_eq!(test_helpers::list_directory(&dir).await, Vec::<String>::new());
        test_helpers::write_file(&dir, "file", "hippos").await;
        assert_eq!(test_helpers::list_directory(&dir).await, vec!["file".to_string()]);
    }

    #[fuchsia::test]
    async fn open_isolated_storage_instance_id() {
        let components = vec![
            ("a", ComponentDeclBuilder::new().child_default("b").child_default("c").build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("data")
                            .source(ExposeSource::Self_)
                            .rights(fio::Operations::CONNECT),
                    )
                    .build(),
            ),
        ];
        let test = RoutingTest::new("a", components).await;
        let b_component = test
            .model
            .root()
            .find_and_maybe_resolve(&["b"].try_into().unwrap())
            .await
            .expect("failed to find component for b:0");
        let dir_source_path: cm_types::Path = "/data".parse().unwrap();
        let moniker = Moniker::try_from(["c", "coll:d"]).unwrap();

        // open the storage directory using instance ID.
        let instance_id = InstanceId::new_random(&mut rand::rng());
        let mut dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: Moniker::root(),
            },
            moniker.clone(),
            Some(&instance_id),
        )
        .await
        .expect("failed to open isolated storage");

        // ensure the directory is actually open before querying its parent about it.
        let _: Vec<_> = dir.query().await.expect("failed to open directory");

        // check that an instance-ID based directory was created:
        assert!(
            test_helpers::list_directory(&test.test_dir_proxy)
                .await
                .contains(&instance_id.to_string())
        );

        // check that a moniker-based directory was NOT created:
        assert!(!test_helpers::list_directory_recursive(&test.test_dir_proxy).await.contains(
            &generate_moniker_based_storage_path(&moniker).to_str().unwrap().to_string()
        ));

        // check that the directory is writable by writing a marker file in it.
        let marker_file_name: String =
            rand::rng().sample_iter(&Alphanumeric).take(7).map(char::from).collect();
        assert_eq!(test_helpers::list_directory(&dir).await, Vec::<String>::new());
        test_helpers::write_file(&dir, &marker_file_name, "contents").await;
        assert_eq!(test_helpers::list_directory(&dir).await, vec![marker_file_name.clone()]);

        // check that re-opening the directory gives us the same marker file.
        dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: Moniker::root(),
            },
            moniker.clone(),
            Some(&instance_id),
        )
        .await
        .expect("failed to open isolated storage");

        assert_eq!(test_helpers::list_directory(&dir).await, vec![marker_file_name.clone()]);
    }

    // TODO: test with different subdirs

    #[fuchsia::test]
    async fn open_isolated_storage_failure_test() {
        let components = vec![("a", component_decl_with_test_runner())];

        // Create a universe with a single component, whose outgoing directory service
        // simply closes the channel of incoming requests.
        let test = RoutingTestBuilder::new("a", components)
            .set_component_outgoing_host_fn("a", Box::new(|_| {}))
            .build()
            .await;
        test.start_instance_and_wait_start(&Moniker::root()).await.unwrap();

        // Try to open the storage. We expect an error.
        let moniker = Moniker::try_from(["c", "coll:d"]).unwrap();
        let res = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&test.model.root())),
                backing_directory_path: "/data".parse().unwrap(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: Moniker::root(),
            },
            moniker.clone(),
            None,
        )
        .await;
        assert_matches!(res, Err(ModelError::StorageError { err: StorageError::Open { .. } }));
    }

    #[fuchsia::test]
    async fn delete_isolated_storage_test() {
        let components = vec![
            ("a", ComponentDeclBuilder::new().child_default("b").child_default("c").build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("data")
                            .source(ExposeSource::Self_)
                            .rights(fio::Operations::CONNECT),
                    )
                    .build(),
            ),
        ];
        let test = RoutingTest::new("a", components).await;
        let b_component = test
            .model
            .root()
            .find_and_maybe_resolve(&["b"].try_into().unwrap())
            .await
            .expect("failed to find component for b:0");
        let dir_source_path: cm_types::Path = "/data".parse().unwrap();
        let storage_moniker = Moniker::try_from(["c"]).unwrap();
        let parent_moniker = Moniker::try_from(["c"]).unwrap();
        let child_moniker = Moniker::try_from(["c", "coll:d"]).unwrap();

        // Open and write to the storage for child.
        let dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: storage_moniker.clone(),
            },
            child_moniker.clone(),
            None,
        )
        .await
        .expect("failed to open isolated storage");
        assert_eq!(test_helpers::list_directory(&dir).await, Vec::<String>::new());
        test_helpers::write_file(&dir, "file", "hippos").await;
        assert_eq!(test_helpers::list_directory(&dir).await, vec!["file".to_string()]);

        // Open parent's storage.
        let dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: storage_moniker.clone(),
            },
            parent_moniker.clone(),
            None,
        )
        .await
        .expect("failed to open isolated storage");
        assert_eq!(test_helpers::list_directory(&dir).await, Vec::<String>::new());
        test_helpers::write_file(&dir, "file", "hippos").await;
        assert_eq!(test_helpers::list_directory(&dir).await, vec!["file".to_string()]);

        // Delete the child's storage.
        delete_isolated_storage(
            BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: storage_moniker.clone(),
            },
            child_moniker.clone(),
            None,
        )
        .await
        .expect("failed to delete child's isolated storage");

        // Open parent's storage again. Should work.
        let dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: storage_moniker.clone(),
            },
            parent_moniker.clone(),
            None,
        )
        .await
        .expect("failed to open isolated storage");
        assert_eq!(test_helpers::list_directory(&dir).await, vec!["file".to_string()]);

        // Open list of children from parent. Should not contain child directory.
        assert_eq!(
            test.list_directory_in_storage(None, parent_moniker.clone(), None, "children").await,
            Vec::<String>::new(),
        );

        // Error -- tried to delete nonexistent storage.
        let err = delete_isolated_storage(
            BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path,
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: storage_moniker.clone(),
            },
            child_moniker.clone(),
            None,
        )
        .await
        .expect_err("delete isolated storage not meant to succeed");
        match err {
            ModelError::StorageError { err: StorageError::Remove { .. } } => {}
            _ => {
                panic!("unexpected error: {:?}", err);
            }
        }
    }

    #[fuchsia::test]
    async fn delete_isolated_storage_instance_id_test() {
        let components = vec![
            ("a", ComponentDeclBuilder::new().child_default("b").child_default("c").build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("data")
                            .source(ExposeSource::Self_)
                            .rights(fio::Operations::CONNECT),
                    )
                    .build(),
            ),
        ];
        let test = RoutingTest::new("a", components).await;
        let b_component = test
            .model
            .root()
            .find_and_maybe_resolve(&["b"].try_into().unwrap())
            .await
            .expect("failed to find component for b");
        let dir_source_path: cm_types::Path = "/data".parse().unwrap();
        let parent_moniker = Moniker::try_from(["c"]).unwrap();
        let child_moniker = Moniker::try_from(["c", "coll:d"]).unwrap();
        let instance_id = InstanceId::new_random(&mut rand::rng());
        // Open and write to the storage for child.
        let dir = open_isolated_storage(
            &BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: parent_moniker.clone(),
            },
            child_moniker.clone(),
            Some(&instance_id),
        )
        .await
        .expect("failed to open isolated storage");

        // ensure the directory is actually open before querying its parent about it.
        let _: Vec<_> = dir.query().await.expect("failed to open directory");

        // check that an instance-ID based directory was created:
        assert!(
            test_helpers::list_directory(&test.test_dir_proxy)
                .await
                .contains(&instance_id.to_string())
        );

        assert_eq!(test_helpers::list_directory(&dir).await, Vec::<String>::new());
        test_helpers::write_file(&dir, "file", "hippos").await;
        assert_eq!(test_helpers::list_directory(&dir).await, vec!["file".to_string()]);

        // Delete the child's storage.
        delete_isolated_storage(
            BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path.clone(),
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: parent_moniker.clone(),
            },
            child_moniker.clone(),
            Some(&instance_id),
        )
        .await
        .expect("failed to delete child's isolated storage");

        // check that an instance-ID based directory was deleted:
        assert!(
            !test_helpers::list_directory(&test.test_dir_proxy)
                .await
                .contains(&instance_id.to_string())
        );

        // Error -- tried to delete nonexistent storage.
        let err = delete_isolated_storage(
            BackingDirectoryInfo {
                storage_provider: Some(Arc::clone(&b_component)),
                backing_directory_path: dir_source_path,
                backing_directory_subdir: Default::default(),
                storage_subdir: Default::default(),
                storage_source_moniker: parent_moniker.clone(),
            },
            child_moniker,
            Some(&instance_id),
        )
        .await
        .expect_err("delete isolated storage not meant to succeed");
        match err {
            ModelError::StorageError { err: StorageError::Remove { .. } } => {}
            _ => {
                panic!("unexpected error: {:?}", err);
            }
        }
    }

    #[fuchsia::test]
    fn generate_moniker_based_storage_path_test() {
        for (moniker, expected_output) in vec![
            (["a"].try_into().unwrap(), "a:0/data"),
            (["a", "b"].try_into().unwrap(), "a:0/children/b:0/data"),
            (["a", "b", "c"].try_into().unwrap(), "a:0/children/b:0/children/c:0/data"),
        ] {
            assert_eq!(
                generate_moniker_based_storage_path(&moniker),
                PathBuf::from(expected_output)
            )
        }
    }
}
