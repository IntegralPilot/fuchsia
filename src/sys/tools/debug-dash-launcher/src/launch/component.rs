// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{layout, socket};
use fidl::endpoints::{ClientEnd, create_proxy};
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_dash as fdash;
use fidl_fuchsia_dash::LauncherError;
use fidl_fuchsia_hardware_pty as pty;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_component::client::connect_to_protocol;
use log::warn;
use moniker::Moniker;

pub async fn explore_over_socket(
    moniker: &str,
    socket: zx::Socket,
    tool_urls: Vec<String>,
    command: Option<String>,
    ns_layout: fdash::DashNamespaceLayout,
) -> Result<(zx::Process, zx::Job), LauncherError> {
    let pty = socket::spawn_pty_forwarder(socket).await?;
    explore_over_pty(moniker, pty, tool_urls, command, ns_layout).await
}

pub async fn explore_over_pty(
    moniker: &str,
    pty: ClientEnd<pty::DeviceMarker>,
    tool_urls: Vec<String>,
    command: Option<String>,
    ns_layout: fdash::DashNamespaceLayout,
) -> Result<(zx::Process, zx::Job), LauncherError> {
    let (stdin, stdout, stderr) = super::split_pty_into_handles(pty)?;
    explore_over_handles(moniker, stdin, stdout, stderr, tool_urls, command, ns_layout).await
}

async fn explore_over_handles(
    moniker: &str,
    stdin: zx::NullableHandle,
    stdout: zx::NullableHandle,
    stderr: zx::NullableHandle,
    tool_urls: Vec<String>,
    command: Option<String>,
    ns_layout: fdash::DashNamespaceLayout,
) -> Result<(zx::Process, zx::Job), LauncherError> {
    // Get all directories needed to launch dash successfully.
    let query =
        connect_to_protocol::<fsys::RealmQueryMarker>().map_err(|_| LauncherError::RealmQuery)?;

    let moniker = Moniker::parse_str(moniker).map_err(|_| LauncherError::BadMoniker)?;
    let moniker = moniker.to_string();

    let out_dir = open_outgoing_dir(&query, &moniker).await?;
    let runtime_dir = open_runtime_dir(&query, &moniker).await?;
    let exposed_dir = open_exposed_dir(&query, &moniker).await?;
    let ns_entries = construct_namespace(&query, &moniker).await?;

    // Add all the necessary entries, except for the tools, into the dash namespace.
    let name_infos = match ns_layout {
        fdash::DashNamespaceLayout::NestAllInstanceDirs => {
            // Add a custom `/svc` directory to dash that contains `fuchsia.process.Launcher` and
            // `fuchsia.process.Resolver`.
            let svc_dir = layout::serve_process_launcher_and_resolver_svc_dir()?;

            layout::nest_all_instance_dirs(ns_entries, exposed_dir, svc_dir, out_dir, runtime_dir)
        }
        fdash::DashNamespaceLayout::InstanceNamespaceIsRoot => {
            layout::instance_namespace_is_root(ns_entries).await
        }
    };

    // Set a name for the dash process of the component we're exploring that is easy to find. If
    // moniker is `core/foo`, process name is `sh-core-foo`.
    let process_name =
        if moniker == "." { "sh".to_string() } else { format!("sh-{}", moniker.replace('/', "-")) };

    super::explore_over_handles(super::ExploreArgs {
        stdin,
        stdout,
        stderr,
        tool_urls,
        command,
        name_infos,
        process_name,
        package_resolver: &mut crate::package_resolver::PackageResolver::new(
            fdash::FuchsiaPkgResolver::Full,
        ),
        moniker: Some(moniker),
    })
    .await
}

async fn open_outgoing_dir(
    query: &fsys::RealmQueryProxy,
    moniker: &str,
) -> Result<Option<fio::DirectoryProxy>, LauncherError> {
    let (dir, server_end) = create_proxy::<fio::DirectoryMarker>();
    let result = query
        .open_directory(&moniker, fsys::OpenDirType::OutgoingDir, server_end)
        .await
        .map_err(|error| {
        warn!(moniker:%, error:%; "FIDL call failed to open outgoing dir");
        LauncherError::RealmQuery
    })?;
    match result {
        Ok(()) => Ok(Some(dir)),
        Err(fsys::OpenError::InstanceNotRunning) | Err(fsys::OpenError::NoSuchDir) => Ok(None),
        Err(fsys::OpenError::BadMoniker) => Err(LauncherError::BadMoniker),
        Err(fsys::OpenError::InstanceNotResolved) => Err(LauncherError::InstanceNotResolved),
        Err(fsys::OpenError::InstanceNotFound) => Err(LauncherError::InstanceNotFound),
        Err(error) => {
            warn!(moniker:%, error:?; "RealmQuery returned error opening outgoing dir");
            Err(LauncherError::RealmQuery)
        }
    }
}

async fn open_runtime_dir(
    query: &fsys::RealmQueryProxy,
    moniker: &str,
) -> Result<Option<fio::DirectoryProxy>, LauncherError> {
    let (dir, server_end) = create_proxy::<fio::DirectoryMarker>();
    let result = query
        .open_directory(&moniker, fsys::OpenDirType::RuntimeDir, server_end)
        .await
        .map_err(|error| {
            warn!(moniker:%, error:%; "FIDL call failed to open runtime dir");
            LauncherError::RealmQuery
        })?;
    match result {
        Ok(()) => Ok(Some(dir)),
        Err(fsys::OpenError::InstanceNotRunning) | Err(fsys::OpenError::NoSuchDir) => Ok(None),
        Err(fsys::OpenError::BadMoniker) => Err(LauncherError::BadMoniker),
        Err(fsys::OpenError::InstanceNotResolved) => Err(LauncherError::InstanceNotResolved),
        Err(fsys::OpenError::InstanceNotFound) => Err(LauncherError::InstanceNotFound),
        Err(error) => {
            warn!(moniker:%, error:?; "RealmQuery returned error opening runtime dir");
            Err(LauncherError::RealmQuery)
        }
    }
}

async fn open_exposed_dir(
    query: &fsys::RealmQueryProxy,
    moniker: &str,
) -> Result<fio::DirectoryProxy, LauncherError> {
    let (dir, server_end) = create_proxy::<fio::DirectoryMarker>();
    let result = query
        .open_directory(&moniker, fsys::OpenDirType::ExposedDir, server_end)
        .await
        .map_err(|error| {
            warn!(moniker:%, error:%; "FIDL call failed to open exposed dir");
            LauncherError::RealmQuery
        })?;
    match result {
        Ok(()) => Ok(dir),
        Err(fsys::OpenError::BadMoniker) => Err(LauncherError::BadMoniker),
        Err(fsys::OpenError::InstanceNotResolved) => Err(LauncherError::InstanceNotResolved),
        Err(fsys::OpenError::InstanceNotFound) => Err(LauncherError::InstanceNotFound),
        Err(error) => {
            warn!(moniker:%, error:?; "RealmQuery returned error opening exposed dir");
            Err(LauncherError::RealmQuery)
        }
    }
}

async fn construct_namespace(
    query: &fsys::RealmQueryProxy,
    moniker: &str,
) -> Result<Vec<fcrunner::ComponentNamespaceEntry>, LauncherError> {
    let result =
        query.construct_namespace(&moniker).await.map_err(|_| LauncherError::RealmQuery)?;
    match result {
        Ok(ns_entries) => Ok(ns_entries),
        Err(fsys::ConstructNamespaceError::BadMoniker) => Err(LauncherError::BadMoniker),
        Err(fsys::ConstructNamespaceError::InstanceNotResolved) => {
            Err(LauncherError::InstanceNotResolved)
        }
        Err(fsys::ConstructNamespaceError::InstanceNotFound) => {
            Err(LauncherError::InstanceNotFound)
        }
        Err(_) => Err(LauncherError::RealmQuery),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;
    use futures::StreamExt;

    fn serve_realm_query(result: fsys::RealmQueryOpenDirectoryResult) -> fsys::RealmQueryProxy {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fsys::RealmQueryMarker>();
        fasync::Task::spawn(async move {
            let mut stream = server_end.into_stream();
            let request = stream.next().await.unwrap().unwrap();
            let (moniker, responder) = match request {
                fsys::RealmQueryRequest::OpenDirectory { moniker, responder, .. } => {
                    (moniker, responder)
                }
                _ => panic!("Unexpected RealmQueryRequest"),
            };
            assert_eq!(moniker, ".");
            responder.send(result).unwrap();
        })
        .detach();
        proxy
    }

    #[fuchsia::test]
    async fn open_success() {
        let query = serve_realm_query(Ok(()));
        open_exposed_dir(&query, ".").await.unwrap();
    }

    #[fuchsia::test]
    async fn instance_not_running() {
        let query = serve_realm_query(Err(fsys::OpenError::InstanceNotRunning));
        assert!(open_outgoing_dir(&query, ".").await.unwrap().is_none());
    }

    #[fuchsia::test]
    async fn no_such_dir() {
        let query = serve_realm_query(Err(fsys::OpenError::NoSuchDir));
        assert!(open_runtime_dir(&query, ".").await.unwrap().is_none());
    }

    #[fuchsia::test]
    async fn error_instance_unresolved() {
        let query = serve_realm_query(Err(fsys::OpenError::InstanceNotResolved));
        let error = open_exposed_dir(&query, ".").await.unwrap_err();
        assert_eq!(error, LauncherError::InstanceNotResolved);
    }

    #[fuchsia::test]
    async fn error_instance_not_found() {
        let query = serve_realm_query(Err(fsys::OpenError::InstanceNotFound));
        let error = open_exposed_dir(&query, ".").await.unwrap_err();
        assert_eq!(error, LauncherError::InstanceNotFound);
    }
}
