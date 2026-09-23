// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::{ComponentInstance, Package, WeakComponentInstance};
use ::routing::component_instance::ComponentInstanceInterface;
use cm_rust::{Availability, UseDecl};
use cm_types::{NamespacePath, Path};
use errors::CreateNamespaceError;
use fidl::endpoints::Proxy;
use fidl_fuchsia_io as fio;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use router_error::RouterError;
use routing::DictExt;
use routing::bedrock::request_metadata::storage_metadata;
use routing::error::RoutingError;
use runtime_capabilities::{Capability, Dictionary};
use serve_processargs::{BuildNamespaceError, NamespaceBuilder};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use vfs::execution_scope::ExecutionScope;

pub static PKG_PATH: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/pkg"));

/// Creates a component's namespace.
///
/// TODO(b/298106231): eventually this should only build a delivery map as
/// the program dict will be fetched from the resolved component state.
pub async fn create_namespace(
    package: Option<&Package>,
    component: &Arc<ComponentInstance>,
    use_decls: &[UseDecl],
    program_input_dict: &Arc<Dictionary>,
    scope: ExecutionScope,
) -> Result<NamespaceBuilder, CreateNamespaceError> {
    let not_found_sender = not_found_logging(component);
    let mut namespace =
        NamespaceBuilder::new(scope.clone(), not_found_sender, component.as_weak().into());
    if let Some(package) = package {
        let pkg_dir = fuchsia_fs::directory::clone(&package.package_dir).map_err(|e| {
            CreateNamespaceError::ClonePkgDirFailed { moniker: component.moniker.clone(), err: e }
        })?;
        add_pkg_directory(&mut namespace, pkg_dir).map_err(|e| {
            CreateNamespaceError::BuildNamespaceError { moniker: component.moniker.clone(), err: e }
        })?;
    }

    let mut dont_flatten_past = HashSet::new();
    for use_ in use_decls {
        match use_ {
            // In order to maintain the legacy error reporting contract for
            // storage capabilities, we need to verify that the target component
            // is in the ID index with debug routing, and if not, return an
            // error.
            decl @ cm_rust::UseDecl::Storage(_use_decl) => {
                if let Some(Capability::DirConnectorRouter(router)) = program_input_dict
                    .get_capability(use_.path().ok_or(
                        CreateNamespaceError::UseDeclWithoutPath {
                            moniker: component.moniker.clone(),
                            decl: decl.clone(),
                        },
                    )?)
                {
                    if let Err(RouterError::NotFound(e)) = router
                        .route_debug(
                            storage_metadata(Availability::Required),
                            component.as_weak().into(),
                        )
                        .await
                    {
                        if let Some(e @ RoutingError::ComponentNotInIdIndex { .. }) =
                            e.as_any().downcast_ref::<RoutingError>()
                        {
                            return Err(CreateNamespaceError::from(e.clone()));
                        }
                    }
                }
            }
            cm_rust::UseDecl::Service(decl) => {
                // Services should behave like protocols, and exist within a component manager hosted
                // directory instead of being directly placed in the namespace.
                //
                // Without this, using a service and a protocol both in /svc will cause a namespace
                // path conflict because the protocol would cause a directory to go at /svc and the
                // service would cause a directory to go at /svc/{service_name}.
                dont_flatten_past.insert(decl.target_path.parent());
            }
            _ => (),
        }
    }

    program_input_dict_to_namespace("", &mut namespace, program_input_dict, dont_flatten_past)
        .map_err(|e| CreateNamespaceError::BuildNamespaceError {
            moniker: component.moniker.clone(),
            err: e,
        })?;
    Ok(namespace)
}

/// Adds the package directory to the namespace under the path "/pkg".
fn add_pkg_directory(
    namespace: &mut NamespaceBuilder,
    pkg_dir: fio::DirectoryProxy,
) -> Result<(), BuildNamespaceError> {
    let pkg_handle = pkg_dir.into_channel().unwrap().into_zx_channel().into_handle();
    let sandbox_handle = runtime_capabilities::Handle::new(pkg_handle);
    let path = cm_types::NamespacePath::new(PKG_PATH.to_str().unwrap()).unwrap();
    namespace.add_entry(sandbox_handle.into(), &path)?;
    Ok(())
}

/// Adds namespace entries for a component's program input dictionary.
fn program_input_dict_to_namespace(
    prefix: &str,
    namespace: &mut NamespaceBuilder,
    program_input_dict: &Dictionary,
    dont_flatten_past: HashSet<NamespacePath>,
) -> Result<(), serve_processargs::BuildNamespaceError> {
    // Convert (the transformed) program_input_dict to namespace.
    //
    // The namespace is flattened as much as is possible, up until any paths listed in
    // `dont_flatten_past` (past which no flattening happens).
    //
    // For example, a dictionary that contains a dictionary at "data" that contains one directory
    // at "foo" should add a directory to the namespace at "/data/foo", not a directory at "/data".
    //
    // Alternatively if a dictionary contains a dictionary at "svc" that contains a directory
    // connector at "foo.bar" and `dont_flatten_past` contains "svc", then a dictionary gets added
    // to the namespace at "/svc", not a directory connector at "/svc/foo.bar".
    for (key, value) in program_input_dict.enumerate() {
        let new_prefix = NamespacePath::new(format!("{prefix}/{key}")).unwrap();
        match value {
            Capability::Dictionary(d) => {
                if dont_flatten_past.contains(&new_prefix) {
                    namespace.add_entry(Capability::Dictionary(d), &new_prefix)?;
                } else {
                    program_input_dict_to_namespace(
                        &format!("{prefix}/{key}"),
                        namespace,
                        &d,
                        dont_flatten_past.clone(),
                    )?;
                }
            }
            cap @ Capability::DirConnector(_) => {
                namespace.add_entry(cap, &new_prefix)?;
            }
            cap @ Capability::DirConnectorRouter(_) => {
                namespace.add_entry(cap, &new_prefix)?;
            }
            cap @ Capability::DictionaryRouter(_) => {
                namespace.add_entry(cap, &new_prefix)?;
            }
            cap => {
                namespace.add_object(cap, &Path::new(format!("{prefix}/{key}")).unwrap())?;
            }
        }
    }

    Ok(())
}

fn not_found_logging(component: &Arc<ComponentInstance>) -> UnboundedSender<String> {
    let (sender, mut receiver) = unbounded();
    let component_for_logger: WeakComponentInstance = component.as_weak();

    component.execution_scope.spawn(async move {
        while let Some(path) = receiver.next().await {
            match component_for_logger.upgrade() {
                Ok(target) => {
                    target
                        .log(
                            log::Level::Warn,
                            format!(
                                "No capability available at path {} for component {}, \
                             verify the component has the proper `use` declaration.",
                                path, target.moniker
                            ),
                            &[],
                        )
                        .await;
                }
                Err(_) => {}
            }
        }
    });

    sender
}
