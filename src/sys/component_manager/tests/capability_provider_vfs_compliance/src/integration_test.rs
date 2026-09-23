// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use fuchsia_fs::node::OpenError;
use futures::StreamExt;
use zx_status;

#[fuchsia::test]
async fn component_manager_namespace() {
    let nodes = [
        "/svc/fuchsia.component.Binder",
        "/svc/fuchsia.component.Namespace",
        "/svc/fuchsia.component.Realm",
        "/svc/fuchsia.component.StorageAdmin",
        "/svc/fuchsia.component.runtime.Capabilities",
        "/svc/fuchsia.sys2.LifecycleController",
        "/svc/fuchsia.sys2.RouteValidator",
        "/svc/fuchsia.sys2.RealmQuery",
        "/svc/fuchsia.boot.Arguments",
        "/svc/fuchsia.boot.FactoryItems",
        "/svc/fuchsia.boot.Items",
        "/svc/fuchsia.boot.ReadOnlyLog",
        "/svc/fuchsia.boot.WriteOnlyLog",
        "/svc/fuchsia.inspect.InspectSink",
        "/svc/fuchsia.kernel.CpuResource",
        "/svc/fuchsia.kernel.DebugResource",
        "/svc/fuchsia.kernel.DebuglogResource",
        "/svc/fuchsia.kernel.EnergyInfoResource",
        "/svc/fuchsia.kernel.DebuglogResource",
        "/svc/fuchsia.kernel.HypervisorResource",
        "/svc/fuchsia.kernel.InfoResource",
        "/svc/fuchsia.kernel.IommuResource",
        "/svc/fuchsia.kernel.IrqResource",
        "/svc/fuchsia.kernel.MexecResource",
        "/svc/fuchsia.kernel.MmioResource",
        "/svc/fuchsia.kernel.MsiResource",
        "/svc/fuchsia.kernel.PowerResource",
        "/svc/fuchsia.kernel.ProfileResource",
        "/svc/fuchsia.kernel.RootJob",
        "/svc/fuchsia.kernel.RootJobForInspect",
        "/svc/fuchsia.kernel.Stats",
        "/svc/fuchsia.kernel.VmexResource",
        "/svc/fuchsia.logger.LogSink",
        "/svc/fuchsia.process.Launcher",
        "/svc/fuchsia.sys2.CrashIntrospect",
    ];
    let opens = nodes.iter().map(|node_path| async move {
        assert_matches!(
            validate_open_with_node_reference_and_describe(node_path).await,
            Ok(()),
            "Opening capability: {} with DESCRIBE|NODE_REFERENCE did not produce open stream.",
            node_path
        );
        validate_open_with_extra_path_should_fail(node_path).await;
    });

    let () = futures::future::join_all(opens).await.into_iter().collect();
}

async fn validate_open_with_node_reference_and_describe(path: &str) -> Result<(), OpenError> {
    // The Rust VFS defines the only valid call for DESCRIBE on a service node to be one
    // that includes the NODE_REFERENCE flag. Component framework aims to adhere to the rust
    // VFS implementation of the io protocol.
    // TODO(https://fxbug.dev/42055559): If the rust VFS interpretation of the DESCRIBE
    // flag behavior on service nodes is incorrect, update this call.
    let node = fuchsia_fs::node::open_in_namespace(
        path,
        fio::Flags::PROTOCOL_NODE | fio::Flags::FLAG_SEND_REPRESENTATION,
    )?;

    let mut events = node.take_event_stream();

    match events
        .next()
        .await
        .ok_or(OpenError::OnOpenEventStreamClosed)?
        .map_err(OpenError::OnOpenDecode)?
    {
        fio::NodeEvent::OnOpen_ { s: status, info } => {
            let () = zx_status::Status::ok(status).map_err(OpenError::OpenError)?;
            info.ok_or(OpenError::MissingOnOpenInfo)?;
        }
        fio::NodeEvent::OnRepresentation { payload } => match payload {
            fio::Representation::Node(fio::NodeInfo { .. }) => {}
            fio::Representation::File(fio::FileInfo { .. }) => {}
            fio::Representation::Directory(fio::DirectoryInfo { .. }) => {}
            fio::Representation::Symlink(fio::SymlinkInfo { .. }) => {}
            other => panic!("Compliance test got unexpected OnRepresentation event: {:?}", other),
        },
        fio::NodeEvent::_UnknownEvent { ordinal, .. } => {
            panic!("Compliance test got unexpected unknown: {ordinal:?}")
        }
    }

    Ok(())
}

async fn validate_open_with_extra_path_should_fail(path: &str) {
    let node =
        fuchsia_fs::node::open_in_namespace(&format!("{}/extra", path), fuchsia_fs::PERM_READABLE)
            .unwrap();
    let mut events = node.take_event_stream();
    let event = events.next().await.unwrap();
    event.expect_err("Opening a protocol with a non-empty relative path should fail.");
}
