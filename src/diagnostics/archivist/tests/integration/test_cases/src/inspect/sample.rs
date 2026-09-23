// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::test_topology;
use diagnostics_assertions::assert_json_diff;
use diagnostics_reader::drain_batch_iterator;
use fidl::endpoints::DiscoverableProtocolMarker;
use futures::StreamExt;
use realm_proxy_client::RealmProxyClient;
use std::sync::Arc;
use {fidl_fuchsia_archivist_test as ftest, fidl_fuchsia_diagnostics as fdiagnostics};

async fn connect_sample(realm_proxy: &RealmProxyClient) -> fdiagnostics::SampleProxy {
    realm_proxy
        .connect_to_named_protocol::<fdiagnostics::SampleMarker>(&format!(
            "diagnostics-accessors/{}",
            fdiagnostics::SampleMarker::PROTOCOL_NAME
        ))
        .await
        .expect("connect to fuchsia.diagnostics.Sample")
}

#[fuchsia::test]
async fn sample_components_inspect() {
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(vec![test_topology::PuppetDeclBuilder::new("child").into()]),
        ..Default::default()
    })
    .await
    .unwrap();

    let child_puppet = test_topology::connect_to_puppet(&realm_proxy, "child").await.unwrap();

    let writer = child_puppet
        .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
        .await
        .unwrap()
        .into_proxy();

    writer.set_health_starting_up().await.unwrap();

    let sample_accessor = connect_sample(&realm_proxy).await;

    let (sample_sink_client, sample_sink_server) =
        fidl::endpoints::create_endpoints::<fdiagnostics::SampleSinkMarker>();
    let mut sample_sink_server = sample_sink_server.into_stream();

    let params = fdiagnostics::SampleParameters {
        data: Some(vec![fdiagnostics::SampleDatum {
            selector: Some(fdiagnostics::SelectorArgument::RawSelector(
                "child:root/fuchsia.inspect.Health:status".to_string(),
            )),
            strategy: Some(fdiagnostics::SampleStrategy::OnDiff),
            interval_secs: Some(5),
            ..Default::default()
        }]),
        ..Default::default()
    };

    sample_accessor.set(&params).unwrap();
    sample_accessor.commit(sample_sink_client).await.unwrap().unwrap();

    let fdiagnostics::SampleSinkRequest::OnSampleReadied {
        event:
            fdiagnostics::SampleSinkResult::Ready(fdiagnostics::SampleReady {
                batch_iter: Some(batch_iter),
                ..
            }),
        ..
    } = sample_sink_server.next().await.unwrap().unwrap()
    else {
        panic!("unexpected error");
    };

    let data =
        drain_batch_iterator::<diagnostics_data::InspectData>(Arc::new(batch_iter.into_proxy()))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

    assert_eq!(data.len(), 1);

    assert_json_diff!(data[0].payload.as_ref().unwrap(), root: {
        "fuchsia.inspect.Health": {
            status: "STARTING_UP",
        }
    });

    // trigger a change in the inspect
    writer.set_health_ok().await.unwrap();

    let fdiagnostics::SampleSinkRequest::OnSampleReadied {
        event:
            fdiagnostics::SampleSinkResult::Ready(fdiagnostics::SampleReady {
                batch_iter: Some(batch_iter),
                ..
            }),
        ..
    } = sample_sink_server.next().await.unwrap().unwrap()
    else {
        panic!("unexpected error");
    };

    let data =
        drain_batch_iterator::<diagnostics_data::InspectData>(Arc::new(batch_iter.into_proxy()))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

    assert_eq!(data.len(), 1);

    assert_json_diff!(data[0].payload.as_ref().unwrap(), root: {
        "fuchsia.inspect.Health": {
            status: "OK",
        }
    });
}
