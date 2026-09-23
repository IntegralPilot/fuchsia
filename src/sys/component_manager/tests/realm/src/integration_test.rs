// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::create_proxy;
use fidl_fidl_examples_routing_echo::EchoMarker;
use fuchsia_async::DurationExt;
use fuchsia_component::client::*;
use {fidl_fuchsia_component_decl as fcdecl, fidl_fuchsia_sys2 as fsys, fuchsia_async as fasync};

async fn get_manifest(query: &fsys::RealmQueryProxy, moniker: &str) -> fcdecl::Component {
    let iterator = query.get_resolved_declaration(moniker).await.unwrap().unwrap();
    let iterator = iterator.into_proxy();

    let mut bytes = vec![];

    loop {
        let mut batch = iterator.next().await.unwrap();
        if batch.is_empty() {
            break;
        }
        bytes.append(&mut batch);
    }

    fidl::unpersist::<fcdecl::Component>(&bytes).unwrap()
}

#[fuchsia::test]
pub async fn get_instance_self() {
    let query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();

    let instance = query.get_instance("./").await.unwrap().unwrap();
    let url = instance.url.unwrap();
    assert!(
        url.starts_with("fuchsia-pkg://fuchsia.com/realm_integration_tests")
            || url.starts_with("realm_integration_tests-ctf"),
        "{url}"
    );
    assert!(url.ends_with("#meta/realm_integration_test.cm"), "{url}");

    let resolved = instance.resolved_info.unwrap();
    let execution = resolved.execution_info.unwrap();
    execution.start_reason.unwrap();
}

#[fuchsia::test]
pub async fn get_manifest_self() {
    let query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();

    let decl = get_manifest(&query, "./").await;

    let program = decl.program.unwrap();
    program.runner.unwrap();

    let uses = decl.uses.unwrap();
    let exposes = decl.exposes.unwrap();
    // Extra use (for debugdata) is possible on coverage builds.
    assert!(uses.len() == 4 || uses.len() == 5, "{uses:?}");
    assert_eq!(exposes.len(), 1);
}

#[fuchsia::test]
pub async fn get_structured_config_self() {
    let query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();

    let err = query.get_structured_config("./").await.unwrap().unwrap_err();

    assert_eq!(err, fsys::GetStructuredConfigError::NoConfig);
}

#[fuchsia::test]
pub async fn echo_server() {
    let query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();

    let instance = query.get_instance("./echo_server").await.unwrap().unwrap();
    let url = instance.url.unwrap();
    assert_eq!(url, "#meta/echo_server.cm");
    assert!(instance.resolved_info.is_none());

    let err = query.get_resolved_declaration("./echo_server").await.unwrap().unwrap_err();
    assert_eq!(err, fsys::GetDeclarationError::InstanceNotResolved);

    let err = query.get_structured_config("./echo_server").await.unwrap().unwrap_err();
    assert_eq!(err, fsys::GetStructuredConfigError::InstanceNotResolved);

    let (_, server_end) = create_proxy();
    let err = query
        .open_directory("./echo_server", fsys::OpenDirType::PackageDir, server_end)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err, fsys::OpenError::InstanceNotResolved);

    // Now connect to the Echo protocol, thus starting the echo_server
    let echo = connect_to_protocol::<EchoMarker>().unwrap();
    let reply = echo.echo_string(Some("test")).await.unwrap();
    assert_eq!(reply.unwrap(), "test");

    let instance = query.get_instance("./echo_server").await.unwrap().unwrap();
    let resolved = instance.resolved_info.unwrap();
    let resolved_url = resolved.resolved_url.unwrap();
    assert!(
        resolved_url.starts_with("fuchsia-pkg://fuchsia.com/realm_integration_test")
            || resolved_url.starts_with("realm_integration_tests-ctf"),
        "{resolved_url}"
    );
    assert!(resolved_url.ends_with("#meta/echo_server.cm"), "{resolved_url}");

    let execution = resolved.execution_info.unwrap();
    execution.start_reason.unwrap();

    let decl = get_manifest(&query, "./echo_server").await;
    let program = decl.program.unwrap();
    program.runner.unwrap();

    let uses = decl.uses.unwrap();
    let exposes = decl.exposes.unwrap();
    // Extra use (for debugdata) is possible on coverage builds.
    assert!(uses.len() == 1 || uses.len() == 2, "{uses:?}");
    assert_eq!(exposes.len(), 2);

    let (pkg_dir, server_end) = create_proxy();
    query
        .open_directory("./echo_server", fsys::OpenDirType::PackageDir, server_end)
        .await
        .unwrap()
        .unwrap();

    let entries = fuchsia_fs::directory::readdir(&pkg_dir).await.unwrap();
    assert_eq!(
        entries,
        vec![
            fuchsia_fs::directory::DirEntry {
                name: "bin".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory,
            },
            fuchsia_fs::directory::DirEntry {
                name: "data".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory,
            },
            fuchsia_fs::directory::DirEntry {
                name: "lib".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory,
            },
            fuchsia_fs::directory::DirEntry {
                name: "meta".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory,
            }
        ]
    );

    let (exposed_dir, server_end) = create_proxy();
    query
        .open_directory("./echo_server", fsys::OpenDirType::ExposedDir, server_end)
        .await
        .unwrap()
        .unwrap();
    let entries = fuchsia_fs::directory::readdir(&exposed_dir).await.unwrap();
    assert_eq!(
        entries,
        vec![
            fuchsia_fs::directory::DirEntry {
                name: "fidl.examples.routing.echo.Echo".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Service,
            },
            fuchsia_fs::directory::DirEntry {
                name: "fuchsia.component.Binder".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Service,
            },
        ]
    );
    let (ns_dir, server_end) = create_proxy();
    query
        .open_directory("./echo_server", fsys::OpenDirType::NamespaceDir, server_end)
        .await
        .unwrap()
        .unwrap();
    let entries = fuchsia_fs::directory::readdir(&ns_dir).await.unwrap();
    assert_eq!(
        entries,
        vec![
            fuchsia_fs::directory::DirEntry {
                name: "pkg".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory,
            },
            fuchsia_fs::directory::DirEntry {
                name: "svc".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Directory,
            }
        ]
    );

    let svc_dir = fuchsia_fs::directory::open_directory(&ns_dir, "svc", fuchsia_fs::PERM_READABLE)
        .await
        .unwrap();
    let entries = fuchsia_fs::directory::readdir(&svc_dir).await.unwrap();
    // Extra entry (for debugdata) is possible on coverage builds.
    assert!(entries.len() == 1 || entries.len() == 2, "{entries:?}");
    assert!(
        entries.iter().any(|e| *e
            == fuchsia_fs::directory::DirEntry {
                name: "fuchsia.logger.LogSink".to_string(),
                kind: fuchsia_fs::directory::DirentKind::Service,
            }),
        "{entries:?}"
    );

    let (outgoing_dir, server_end) = create_proxy();
    query
        .open_directory("./echo_server", fsys::OpenDirType::OutgoingDir, server_end)
        .await
        .unwrap()
        .unwrap();

    let echo = connect_to_protocol_at_dir_svc::<EchoMarker>(&outgoing_dir).unwrap();
    let reply = echo.echo_string(Some("test")).await.unwrap();
    assert_eq!(reply.unwrap(), "test");

    let (runtime_dir, server_end) = create_proxy();
    query
        .open_directory("./echo_server", fsys::OpenDirType::RuntimeDir, server_end)
        .await
        .unwrap()
        .unwrap();

    // ELF runner doesn't doesn't fully populate the runtime_dir before it begins serving it, so
    // poll in a loop.
    let elf_dir =
        fuchsia_fs::directory::open_directory(&runtime_dir, "elf", fuchsia_fs::PERM_READABLE)
            .await
            .unwrap();
    let expected_entries = vec![
        fuchsia_fs::directory::DirEntry {
            name: "job_id".to_string(),
            kind: fuchsia_fs::directory::DirentKind::File,
        },
        fuchsia_fs::directory::DirEntry {
            name: "process_id".to_string(),
            kind: fuchsia_fs::directory::DirentKind::File,
        },
        fuchsia_fs::directory::DirEntry {
            name: "process_start_time".to_string(),
            kind: fuchsia_fs::directory::DirentKind::File,
        },
    ];
    loop {
        let mut entries = fuchsia_fs::directory::readdir(&elf_dir).await.unwrap();
        if let Some(position) =
            entries.iter().position(|e| e.name == "process_start_time_utc_estimate")
        {
            entries.remove(position);
        }
        if entries.len() >= expected_entries.len() {
            assert_eq!(entries, expected_entries);
            break;
        }
        fasync::Timer::new(zx::MonotonicDuration::from_millis(100).after_now()).await;
    }
}

#[fuchsia::test]
pub async fn will_not_resolve() {
    let query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();

    let instance = query.get_instance("./will_not_resolve").await.unwrap().unwrap();
    let url = instance.url.unwrap();
    assert_eq!(url, "fuchsia-pkg://fake.com");

    assert!(instance.resolved_info.is_none());
}

#[fuchsia::test]
pub async fn get_all_instances() {
    let query = connect_to_protocol::<fsys::RealmQueryMarker>().unwrap();
    let iterator = query.get_all_instances().await.unwrap().unwrap();
    let iterator = iterator.into_proxy();
    let mut instances = vec![];

    loop {
        let mut batch = iterator.next().await.unwrap();
        if batch.is_empty() {
            break;
        }
        instances.append(&mut batch);
    }

    // This component and the two children
    assert_eq!(instances.len(), 3);

    for instance in instances {
        let url = instance.url.unwrap();
        let moniker = instance.moniker.unwrap();
        if url.ends_with("#meta/realm_integration_test.cm") {
            // This component is definitely resolved and started
            assert_eq!(moniker, ".");
            assert!(instance.resolved_info.is_some());
        } else if url.ends_with("#meta/echo_server.cm") {
            // The other test case may start this component so its state is not stable
            assert_eq!(moniker, "echo_server");
        } else if url == "fuchsia-pkg://fake.com" {
            // This component can never be resolved or started
            assert_eq!(moniker, "will_not_resolve");
            assert!(instance.resolved_info.is_none());
        } else {
            panic!("Unknown instance: {}", url);
        }
    }
}
