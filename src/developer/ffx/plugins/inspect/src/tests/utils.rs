// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use diagnostics_data::{
    DiagnosticsHierarchy, InspectData, InspectDataBuilder, InspectHandleName, Property, Timestamp,
};
use errors as _;
use fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy;
use fdomain_fuchsia_diagnostics::{
    ClientSelectorConfiguration, DataType, Format, StreamMode, StreamParameters,
};
use fdomain_fuchsia_diagnostics_host::{ArchiveAccessorProxy, ArchiveAccessorRequest};
use ffx_writer as _;
use futures::AsyncWriteExt;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Default)]
pub struct FakeArchiveIteratorResponse {
    value: String,
}

impl FakeArchiveIteratorResponse {
    pub fn new_with_value(value: String) -> Self {
        FakeArchiveIteratorResponse { value, ..Default::default() }
    }
}

pub fn setup_fake_accessor_provider(
    mut server_end: fdomain_client::Socket,
    responses: Rc<Vec<FakeArchiveIteratorResponse>>,
) -> Result<()> {
    fuchsia_async::Task::local(async move {
        if responses.is_empty() {
            return;
        }
        assert_eq!(responses.len(), 1);
        server_end.write_all(responses[0].value.as_bytes()).await.unwrap();
    })
    .detach();
    Ok(())
}

pub struct FakeAccessorData {
    parameters: StreamParameters,
    responses: Rc<Vec<FakeArchiveIteratorResponse>>,
}

impl FakeAccessorData {
    pub fn new(
        parameters: StreamParameters,
        responses: Rc<Vec<FakeArchiveIteratorResponse>>,
    ) -> Self {
        FakeAccessorData { parameters, responses }
    }
}

pub fn setup_fake_archive_accessor(
    client: Arc<fdomain_client::Client>,
    expected_data: Vec<FakeAccessorData>,
) -> ArchiveAccessorProxy {
    let proxy =
        target_holders::fake_proxy::<ArchiveAccessorProxy>(client.clone(), move |req| match req {
            ArchiveAccessorRequest::StreamDiagnostics { parameters, stream, responder } => {
                for data in expected_data.iter() {
                    if data.parameters == parameters {
                        setup_fake_accessor_provider(stream, data.responses.clone()).unwrap();
                        responder.send().expect("should send");
                        return;
                    }
                }
                unreachable!(
                    "{:#?} did not match any expected parameters: {:#?}",
                    parameters,
                    expected_data.iter().map(|d| d.parameters.clone()).collect::<Vec<_>>()
                );
            }
            _ => unreachable!("We don't expect any other call"),
        });
    proxy
}

pub fn make_inspects_for_lifecycle() -> Vec<InspectData> {
    let fake_name = "fake-name";
    vec![
        make_inspect("test/moniker1", 1, 20, fake_name),
        make_inspect("test/moniker1", 2, 30, fake_name),
        make_inspect("test/moniker3", 3, 3, fake_name),
    ]
}

// `components` are component monikers that should report as existing in the resultant RealmQuery.
// This will make them appear in fuzzy searches using RemoteControlProxy/RealmQuery
pub fn setup_fake_rcs(
    client: Arc<fdomain_client::Client>,
    components: Vec<&str>,
) -> RemoteControlProxy {
    let config = testing_lib::FakeRcsConfig {
        components: components.iter().map(|s| s.to_string()).collect(),
        identify_host_response: None,
        capability_handlers: std::collections::HashMap::new(),
        identify_host_handler: None,
    };
    testing_lib::setup_fake_rcs(client, config)
}

pub fn make_inspect_with_length(moniker: &str, timestamp: i64, len: usize) -> InspectData {
    make_inspect(moniker, timestamp, len, "fake-name")
}

pub fn make_inspect(moniker: &str, timestamp: i64, len: usize, tree_name: &str) -> InspectData {
    let long_string = std::iter::repeat("a").take(len).collect::<String>();
    let hierarchy = DiagnosticsHierarchy::new(
        String::from("name"),
        vec![Property::String(format!("hello_{}", timestamp), long_string)],
        vec![],
    );
    InspectDataBuilder::new(
        moniker.try_into().unwrap(),
        format!("fake-url://{}", moniker),
        Timestamp::from_nanos(timestamp),
    )
    .with_hierarchy(hierarchy)
    .with_name(InspectHandleName::name(tree_name))
    .build()
}

pub fn make_inspects() -> Vec<InspectData> {
    let fake_name = "fake-name";
    vec![
        make_inspect("test/moniker1", 1, 20, fake_name),
        make_inspect("test/moniker2", 2, 10, fake_name),
        make_inspect("test/moniker3", 3, 30, fake_name),
        make_inspect("test/moniker1", 20, 3, fake_name),
    ]
}

pub fn inspect_accessor_data(
    client_selector_configuration: ClientSelectorConfiguration,
    inspects: Vec<InspectData>,
) -> FakeAccessorData {
    let params = fdomain_fuchsia_diagnostics::StreamParameters {
        stream_mode: Some(StreamMode::Snapshot),
        data_type: Some(DataType::Inspect),
        format: Some(Format::Json),
        client_selector_configuration: Some(client_selector_configuration),
        ..Default::default()
    };
    let value = serde_json::to_string(&inspects).unwrap();
    let expected_responses = Rc::new(vec![FakeArchiveIteratorResponse::new_with_value(value)]);
    FakeAccessorData::new(params, expected_responses)
}

pub fn get_empty_value_json() -> serde_json::Value {
    serde_json::json!([])
}

pub fn get_v1_json_dump() -> serde_json::Value {
    serde_json::json!(
        [
            {
                "data_source":"Inspect",
                "metadata":{
                    "name":"fuchsia.inspect.Tree",
                    "component_url": "fuchsia-pkg://fuchsia.com/account#meta/account_manager",
                    "timestamp":0
                },
                "moniker":"realm1/realm2/session5/account_manager",
                "payload":{
                    "root": {
                        "accounts": {
                            "active": 0,
                            "total": 0
                        },
                        "auth_providers": {
                            "types": "google"
                        },
                        "listeners": {
                            "active": 1,
                            "events": 0,
                            "total_opened": 1
                        }
                    }
                },
                "version":1
            }
        ]
    )
}

pub fn get_v1_single_value_json() -> serde_json::Value {
    serde_json::json!(
        [
            {
                "data_source":"Inspect",
                "metadata":{
                    "name":"fuchsia.inspect.Tree",
                    "component_url": "fuchsia-pkg://fuchsia.com/account#meta/account_manager",
                    "timestamp":0
                },
                "moniker":"realm1/realm2/session5/account_manager",
                "payload":{
                    "root": {
                        "accounts": {
                            "active": 0
                        }
                    }
                },
                "version":1
            }
        ]
    )
}
