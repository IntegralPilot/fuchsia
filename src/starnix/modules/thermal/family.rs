// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::thermal_zone::{SensorProps, ThermalZone};
use async_trait::async_trait;
use fuchsia_async as fasync;
use futures::channel::mpsc::UnboundedSender;
use futures::{Future, StreamExt};
use linux_uapi::{
    NLM_F_CAPPED, THERMAL_GENL_EVENT_GROUP_NAME, THERMAL_GENL_FAMILY_NAME,
    THERMAL_GENL_SAMPLING_GROUP_NAME,
};
use netlink::NETLINK_LOG_TAG;
use netlink::messaging::Sender;
use netlink_packet_core::{
    ErrorMessage, NETLINK_HEADER_LEN, NetlinkHeader, NetlinkMessage, NetlinkPayload,
};
use netlink_packet_generic::GenlMessage;
use netlink_packet_utils::traits::Emitable;
use starnix_core::vfs::socket::{GenericMessage, GenericNetlinkFamily};
use starnix_logging::{log_debug, log_error, track_stub};
use starnix_uapi::EOPNOTSUPP;
use std::collections::HashMap;
use std::num::NonZero;
use thermal_netlink::{
    GenlThermal, GenlThermalCmd, GenlThermalPayload, ThermalAttr, celsius_to_millicelsius,
};

const SAMPLING_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

async fn run_samplers(
    sensor_map: HashMap<SensorProps, ThermalZone>,
    sampling_sender: mpmc::Sender<Vec<ThermalAttr>>,
) {
    futures::stream::iter(sensor_map.iter())
        .for_each_concurrent(None, |(_, thermal_zone)| {
            let sampling_sender = sampling_sender.clone();
            async move {
                loop {
                    fasync::Timer::new(SAMPLING_DELAY).await;
                    let result = thermal_zone.proxy.get_temperature_celsius().await;

                    let temp_c = match result {
                        Ok((status, temp_c)) => {
                            if let Err(e) = zx::Status::ok(status) {
                                log_error!(
                                    "get_temperature_celsius driver returned error: {:?}",
                                    e
                                );
                                continue;
                            }
                            temp_c
                        }
                        Err(e) => {
                            log_error!("Failed get_temperature_celcius request: {:?}", e);
                            continue;
                        }
                    };

                    sampling_sender
                        .send(vec![
                            ThermalAttr::ThermalZoneId(thermal_zone.id),
                            ThermalAttr::ThermalZoneTemp(celsius_to_millicelsius(temp_c) as u32),
                        ])
                        .await;
                }
            }
        })
        .await;
}

#[derive(Clone)]
pub struct ThermalFamily {
    sampling_sender: mpmc::Sender<Vec<ThermalAttr>>,
}

impl ThermalFamily {
    /// Returns a newly created [`ThermalFamily`] and its asynchronous worker.
    ///
    /// Callers are responsible for polling the worker [`Future`], which drives
    /// the Netlink implementation's asynchronous work. The worker will never
    /// complete.
    pub fn new(
        sensor_map: HashMap<SensorProps, ThermalZone>,
    ) -> (Self, impl Future<Output = ()> + Send) {
        let sampling_sender = mpmc::Sender::default();
        let thermal_netlink_family_worker = run_samplers(sensor_map, sampling_sender.clone());
        (Self { sampling_sender }, thermal_netlink_family_worker)
    }
}

#[async_trait]
impl<S: Sender<GenericMessage>> GenericNetlinkFamily<S> for ThermalFamily {
    fn name(&self) -> String {
        THERMAL_GENL_FAMILY_NAME.to_str().expect("family name is not valid UTF-8.").to_string()
    }

    fn multicast_groups(&self) -> Vec<String> {
        // These constants use UTF-8 compatible characters, so it should be safe
        // to unwrap them without checking.
        vec![
            THERMAL_GENL_SAMPLING_GROUP_NAME
                .to_str()
                .expect("group name is not valid UTF-8.")
                .to_string(),
            THERMAL_GENL_EVENT_GROUP_NAME
                .to_str()
                .expect("group name is not valid UTF-8.")
                .to_string(),
        ]
    }

    async fn stream_multicast_messages(
        &self,
        group: String,
        assigned_family_id: u16,
        message_sink: UnboundedSender<NetlinkMessage<GenericMessage>>,
    ) {
        match std::ffi::CString::new(group) {
            Ok(group) => {
                if *group == *THERMAL_GENL_SAMPLING_GROUP_NAME {
                    let mut samples = self.sampling_sender.new_receiver();

                    while let Some(sample) = samples.next().await {
                        let mut header = NetlinkHeader::default();
                        header.message_type = assigned_family_id;

                        let thermal_msg = GenlMessage::from_payload(GenlThermal {
                            family_id: assigned_family_id,
                            payload: GenlThermalPayload {
                                cmd: GenlThermalCmd::ThermalGenlSamplingTemp,
                                nlas: sample,
                            },
                        });
                        let mut buffer = vec![0u8; thermal_msg.buffer_len()];
                        thermal_msg.emit(&mut buffer);

                        let mut msg = NetlinkMessage::new(
                            header,
                            NetlinkPayload::InnerMessage(GenericMessage::Other {
                                family: assigned_family_id,
                                payload: buffer,
                            }),
                        );
                        msg.finalize();

                        if let Err(e) = message_sink.unbounded_send(msg) {
                            log_error!(
                                tag = NETLINK_LOG_TAG;
                                "Failed to send sampling message: {:?}: {:?}",
                                e,
                                assigned_family_id,
                            );

                            if e.is_disconnected() {
                                return;
                            }
                        }
                    }
                } else {
                    // Netlink expects this to never return. Normally,
                    // it would handle multicast messages, but since this is a no-op
                    // implementation it intentionally hangs forever so the protocol
                    // is kept alive.
                    track_stub!(
                        TODO("https://fxbug.dev/419596416"),
                        "THERMAL_GENL_EVENT_GROUP_NAME stream_multicast_messages"
                    );
                    std::future::pending::<()>().await;
                }
            }
            Err(e) => {
                log_error!("Failed to convert group to CString: {:?}", e);
            }
        }
    }

    async fn handle_message(
        &self,
        mut netlink_header: NetlinkHeader,
        payload: Vec<u8>,
        sender: &mut S,
    ) {
        track_stub!(TODO("https://fxbug.dev/419596416"), "ThermalFamily handle_message");
        log_debug!(
            tag = NETLINK_LOG_TAG;
            "Received thermal netlink protocol message: {:?}  -- {:?}",
            netlink_header,
            payload
        );

        // Message handling is not implemented so just return an error.
        let mut buffer = [0; NETLINK_HEADER_LEN];
        netlink_header.emit(&mut buffer[..NETLINK_HEADER_LEN]);
        let mut error = ErrorMessage::default();
        error.code = NonZero::new(-(EOPNOTSUPP as i32));
        error.header = buffer.to_vec();
        netlink_header.flags = NLM_F_CAPPED as u16;
        let mut netlink_message = NetlinkMessage::new(netlink_header, NetlinkPayload::Error(error));
        netlink_message.finalize();
        sender.send(netlink_message, None);
    }
}
