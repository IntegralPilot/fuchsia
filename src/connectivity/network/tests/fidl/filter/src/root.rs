// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::pin::pin;

use assert_matches::assert_matches;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext::{
    self as fnet_filter_ext, Change, ChangeCommitError, CommitError, Controller, ControllerId,
    Namespace, Resource, ResourceId, Routine, Rule, RuleId,
};
use fidl_fuchsia_net_root as fnet_root;
use fuchsia_async::{DurationExt as _, TimeoutExt as _};
use futures::StreamExt as _;
use netstack_testing_common::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;

use crate::TestValue as _;

#[netstack_test]
async fn create_new_controller_and_reopen(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // The fuchsia.net.root/Filter protocol can be used to create a new namespace
    // controller, the same as fuchsia.net.filter/Control.
    let root = realm.connect_to_protocol::<fnet_root::FilterMarker>().expect("connect to protocol");
    let mut controller = Controller::new_root(&root, &ControllerId(String::from("root")))
        .await
        .expect("open controller through root");
    let controller_id = controller.id().clone();
    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    let state =
        realm.connect_to_protocol::<fnet_filter::StateMarker>().expect("connect to protocol");
    let stream =
        fnet_filter_ext::event_stream_from_state(state.clone()).expect("get filter event stream");
    let mut stream = pin!(stream);
    let mut observed: HashMap<_, _> =
        fnet_filter_ext::get_existing_resources(&mut stream).await.expect("get resources");
    assert_eq!(
        observed,
        HashMap::from([(
            controller_id.clone(),
            resources
                .iter()
                .map(|resource| (resource.id(), resource.clone()))
                .collect::<HashMap<_, _>>(),
        )])
    );

    // However, root connections are auto-detached, i.e. the lifetime of the
    // resources created by the controller is not tied to the lifetime of the
    // channel. Dropping the controller should not result in the resources being
    // removed.
    drop(controller);
    assert_matches!(
        stream.next().on_timeout(ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT.after_now(), || None).await,
        None,
        "changes should not be broadcast until committed"
    );

    // Re-open the controller that we originally created through fuchsia.net.root.
    let mut controller =
        Controller::new_root(&root, &controller_id).await.expect("open controller through root");
    controller
        .push_changes(resources.iter().rev().map(Resource::id).map(Change::Remove).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    fnet_filter_ext::wait_for_condition(&mut stream, &mut observed, |state| {
        state.get(&controller_id).unwrap().is_empty()
    })
    .await
    .expect("wait for resources to be removed");
}

#[netstack_test]
async fn reopen_existing_controller(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Create some resources through a normal (non-root) NamespaceController.
    let control =
        realm.connect_to_protocol::<fnet_filter::ControlMarker>().expect("connect to protocol");
    let mut controller = Controller::new(&control, &ControllerId(String::from("root")))
        .await
        .expect("open normal controller");
    let controller_id = controller.id().clone();
    let resources = [
        Resource::Namespace(Namespace::test_value()),
        Resource::Routine(Routine::test_value()),
        Resource::Rule(Rule::test_value()),
    ];
    controller
        .push_changes(resources.iter().cloned().map(Change::Create).collect())
        .await
        .expect("push changes");
    controller.commit().await.expect("commit pending changes");

    // Now open this controller through fuchsia.net.root, even though the original
    // connection to this controller is still around. We can add and remove
    // resources out from under the other connection.
    let root = realm.connect_to_protocol::<fnet_root::FilterMarker>().expect("connect to protocol");
    let mut root_controller =
        Controller::new_root(&root, &controller_id).await.expect("open controller through root");
    let remove_rule = Change::Remove(ResourceId::Rule(RuleId::test_value()));
    root_controller.push_changes(vec![remove_rule.clone()]).await.expect("push changes");
    root_controller.commit().await.expect("commit pending changes");

    // Removing the resource through the original controller should fail because it
    // was already removed.
    controller.push_changes(vec![remove_rule.clone()]).await.expect("push changes");
    let invalid_changes = assert_matches!(
        controller.commit().await,
        Err(CommitError::ErrorOnChange(changes)) => changes
    );
    assert_eq!(invalid_changes, vec![(remove_rule, ChangeCommitError::RuleNotFound)]);

    // Add the resource back. This should be observable from the root controller,
    // which cannot also add it.
    let add_rule = Change::Create(Resource::Rule(Rule::test_value()));
    controller.push_changes(vec![add_rule.clone()]).await.expect("push changes");
    controller.commit().await.expect("commit pending changes");

    root_controller.push_changes(vec![add_rule.clone()]).await.expect("push changes");
    let invalid_changes = assert_matches!(
        root_controller.commit().await,
        Err(CommitError::ErrorOnChange(changes)) => changes
    );
    assert_eq!(invalid_changes, vec![(add_rule, ChangeCommitError::AlreadyExists)]);
}
