// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::run_command;
use crate::tests::utils::{
    FakeAccessorData, FakeArchiveIteratorResponse, make_inspects_for_lifecycle,
    setup_fake_archive_accessor, setup_fake_rcs,
};

use fdomain_fuchsia_diagnostics::{
    ClientSelectorConfiguration, DataType, StreamMode, StreamParameters,
};
use ffx_writer::{Format, MachineWriter, TestBuffers};
use iquery_fdomain::commands::ListCommand;
use std::rc::Rc;

#[fuchsia::test]
async fn test_list_empty() {
    let params = StreamParameters {
        stream_mode: Some(StreamMode::Snapshot),
        data_type: Some(DataType::Inspect),
        format: Some(fdomain_fuchsia_diagnostics::Format::Json),
        client_selector_configuration: Some(ClientSelectorConfiguration::SelectAll(true)),
        ..Default::default()
    };
    let expected_responses = Rc::new(vec![]);
    let test_buffers = TestBuffers::default();
    let mut writer = MachineWriter::new_test(Some(Format::Json), &test_buffers);
    let cmd = ListCommand { component: None, with_url: false, accessor: None };
    let client = fdomain_local::local_client_empty();
    let rcs_proxy = setup_fake_rcs(client.clone(), vec![]);
    let accessor_proxy = setup_fake_archive_accessor(
        client,
        vec![FakeAccessorData::new(params, expected_responses.clone())],
    );
    run_command(rcs_proxy, accessor_proxy, ListCommand::from(cmd), &mut writer).await.unwrap();

    let output = test_buffers.into_stdout_str();
    assert_eq!(output.trim_end(), String::from("[]"));
}

#[fuchsia::test]
async fn test_list_with_data() {
    let params = StreamParameters {
        stream_mode: Some(StreamMode::Snapshot),
        data_type: Some(DataType::Inspect),
        format: Some(fdomain_fuchsia_diagnostics::Format::Json),
        client_selector_configuration: Some(ClientSelectorConfiguration::SelectAll(true)),
        ..Default::default()
    };
    let lifecycles = make_inspects_for_lifecycle();
    let value = serde_json::to_string(&lifecycles).unwrap();
    let expected_responses = Rc::new(vec![FakeArchiveIteratorResponse::new_with_value(value)]);
    let test_buffers = TestBuffers::default();
    let mut writer = MachineWriter::new_test(Some(Format::Json), &test_buffers);
    let cmd = ListCommand { component: None, with_url: false, accessor: None };
    let client = fdomain_local::local_client_empty();
    let rcs_proxy = setup_fake_rcs(client.clone(), vec![]);
    let accessor_proxy = setup_fake_archive_accessor(
        client,
        vec![FakeAccessorData::new(params, expected_responses.clone())],
    );
    run_command(rcs_proxy, accessor_proxy, ListCommand::from(cmd), &mut writer).await.unwrap();

    let expected =
        serde_json::to_string(&vec![String::from("test/moniker1"), String::from("test/moniker3")])
            .unwrap();
    let output = test_buffers.into_stdout_str();
    assert_eq!(output.trim_end(), expected);
}

#[fuchsia::test]
async fn test_list_with_data_with_url() {
    let params = StreamParameters {
        stream_mode: Some(StreamMode::Snapshot),
        data_type: Some(DataType::Inspect),
        format: Some(fdomain_fuchsia_diagnostics::Format::Json),
        client_selector_configuration: Some(ClientSelectorConfiguration::SelectAll(true)),
        ..Default::default()
    };
    let lifecycles = make_inspects_for_lifecycle();
    let value = serde_json::to_string(&lifecycles).unwrap();
    let expected_responses = Rc::new(vec![FakeArchiveIteratorResponse::new_with_value(value)]);
    let test_buffers = TestBuffers::default();
    let mut writer = MachineWriter::new_test(Some(Format::Json), &test_buffers);
    let cmd = ListCommand { component: None, with_url: true, accessor: None };
    let client = fdomain_local::local_client_empty();
    let rcs_proxy = setup_fake_rcs(client.clone(), vec![]);
    let accessor_proxy = setup_fake_archive_accessor(
        client,
        vec![FakeAccessorData::new(params, expected_responses.clone())],
    );
    run_command(rcs_proxy, accessor_proxy, ListCommand::from(cmd), &mut writer).await.unwrap();

    let expected = serde_json::to_string(&vec![
        iquery_fdomain::commands::MonikerWithUrl {
            moniker: String::from("test/moniker1"),
            component_url: String::from("fake-url://test/moniker1"),
        },
        iquery_fdomain::commands::MonikerWithUrl {
            moniker: String::from("test/moniker3"),
            component_url: String::from("fake-url://test/moniker3"),
        },
    ])
    .unwrap();
    let output = test_buffers.into_stdout_str();
    assert_eq!(output.trim_end(), expected);
}
