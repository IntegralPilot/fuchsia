// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_bluetooth_a2dp::{AudioModeMarker, AudioModeRequestStream};
use fidl_fuchsia_bluetooth_bredr::ProfileMarker;
use fidl_fuchsia_bluetooth_internal_a2dp::{ControllerMarker, ControllerRequestStream};
use fidl_fuchsia_component::BinderMarker;
use fidl_fuchsia_media::{AudioDeviceEnumeratorMarker, SessionAudioConsumerFactoryMarker};
use fidl_fuchsia_media_sessions2::PublisherMarker;
use fidl_fuchsia_mediacodec::CodecFactoryMarker;
use fidl_fuchsia_metrics::MetricEventLoggerFactoryMarker;
use fidl_fuchsia_power_battery::BatteryManagerMarker;
use fidl_fuchsia_settings::AudioMarker;
use fidl_fuchsia_sysmem2::AllocatorMarker;
use fidl_fuchsia_tracing_provider::RegistryMarker;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStream, TryStreamExt};
use log::info;
use {
    fidl_fuchsia_bluetooth_avdtp_test as fidl_avdtp, fidl_fuchsia_bluetooth_avrcp as fidl_avrcp,
    fuchsia_async as fasync,
};

async fn process_request_stream<S>(mut stream: S, tag: &str)
where
    S: TryStream<Error = fidl::Error> + Unpin,
    <S as TryStream>::Ok: std::fmt::Debug,
{
    info!("Received {} service connection", tag);
    while let Some(request) = stream.try_next().await.expect("serving request stream failed") {
        info!("Received {} service request: {:?}", tag, request);
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    info!("Starting bt-a2dp-topology-fake component...");

    // Set up the outgoing `svc` directory with the services A2DP provides.
    let mut fs = ServiceFs::new();
    let _ = fs
        .dir("svc")
        .add_fidl_service(|stream: AudioModeRequestStream| {
            fasync::Task::local(process_request_stream(stream, AudioModeMarker::PROTOCOL_NAME))
                .detach();
        })
        .add_fidl_service(|stream: fidl_avdtp::PeerManagerRequestStream| {
            fasync::Task::local(process_request_stream(
                stream,
                fidl_avdtp::PeerManagerMarker::PROTOCOL_NAME,
            ))
            .detach();
        })
        .add_fidl_service(|stream: ControllerRequestStream| {
            fasync::Task::local(process_request_stream(stream, ControllerMarker::PROTOCOL_NAME))
                .detach();
        });
    let _ = fs.take_and_serve_directory_handle().expect("Unable to serve ServiceFs requests");
    let service_fs_task = fasync::Task::spawn(fs.collect::<()>());

    // Connect to the services A2DP requires.
    let _avrcp_svc = connect_to_protocol::<fidl_avrcp::PeerManagerMarker>()?;
    let _profile_svc = connect_to_protocol::<ProfileMarker>()?;
    let _cobalt_svc = connect_to_protocol::<MetricEventLoggerFactoryMarker>()?;
    let _audio_device_svc = connect_to_protocol::<AudioDeviceEnumeratorMarker>()?;
    let _session_audio_svc = connect_to_protocol::<SessionAudioConsumerFactoryMarker>()?;
    let _publisher_svc = connect_to_protocol::<PublisherMarker>()?;
    let _codec_factory_svc = connect_to_protocol::<CodecFactoryMarker>()?;
    let _audio_svc = connect_to_protocol::<AudioMarker>()?;
    let _allocator_svc = connect_to_protocol::<AllocatorMarker>()?;
    let _tracing_svc = connect_to_protocol::<RegistryMarker>()?;
    let _battery_manager_svc = connect_to_protocol::<BatteryManagerMarker>()?;
    // A2DP also relies on the `Binder` service which is provided by the component framework; this
    // allows A2DP to start AVRCP-TG;
    let _binder_svc = connect_to_protocol::<BinderMarker>()?;

    service_fs_task.await;
    Ok(())
}
