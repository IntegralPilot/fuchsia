// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_bluetooth_snoop::{PacketFormat, SnoopPacket as FidlSnoopPacket};
use fidl_fuchsia_hardware_bluetooth::{
    PacketDirection as Direction, SnoopEvent, SnoopEventStream, SnoopMarker,
    SnoopOnObservePacketRequest, SnoopPacket as HardwareSnoopPacket, SnoopProxy,
};
use futures::{Stream, StreamExt, ready};
use log::warn;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::bounded_queue::{CreatedAt, SizeOf};

pub struct SnoopPacket {
    pub is_received: bool,
    pub format: PacketFormat,
    pub timestamp: zx::MonotonicInstant,
    pub original_len: usize,
    pub payload: Vec<u8>,
}

impl SnoopPacket {
    pub fn new(
        is_received: bool,
        format: PacketFormat,
        timestamp: zx::MonotonicInstant,
        payload: Vec<u8>,
    ) -> Self {
        Self { is_received, format, timestamp, original_len: payload.len(), payload }
    }

    /// Create a FidlSnoopPacket
    pub fn to_fidl(&self) -> FidlSnoopPacket {
        FidlSnoopPacket {
            is_received: Some(self.is_received),
            format: Some(self.format),
            timestamp: Some(self.timestamp.into_nanos()),
            length: Some(self.original_len as u32),
            data: Some(self.payload.clone()),
            ..Default::default()
        }
    }
}

impl SizeOf for SnoopPacket {
    fn size_of(&self) -> usize {
        std::mem::size_of::<Self>() + self.payload.len()
    }
}

impl CreatedAt for SnoopPacket {
    fn created_at(&self) -> Duration {
        Duration::from_nanos(self.timestamp.into_nanos() as u64)
    }
}

/// A Snooper provides a `Stream` associated with the snoop channel for a single HCI device. This
/// stream can be polled for packets.
pub(crate) struct Snooper {
    pub device_name: String,
    pub proxy: SnoopProxy,
    pub event_stream: SnoopEventStream,
    pub is_terminated: bool,
}

impl fmt::Debug for Snooper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snooper")
            .field("device_name", &self.device_name)
            .field("is_terminated", &self.is_terminated)
            .finish_non_exhaustive()
    }
}

impl Snooper {
    pub(crate) async fn from_vendor(
        vendor: &fidl_fuchsia_hardware_bluetooth::VendorProxy,
        path: &str,
    ) -> Result<Snooper, Error> {
        let snoop_client = vendor
            .open_snoop()
            .await?
            .map_err(|e| format_err!("Failed opening Snoop with {e:?}"))?;

        Ok(Snooper::from_client(snoop_client, path))
    }

    pub fn from_client(client: ClientEnd<SnoopMarker>, path: &str) -> Snooper {
        let device_name = path.to_owned();
        let proxy = client.into_proxy();
        let event_stream = proxy.take_event_stream();
        Snooper { device_name, proxy, event_stream, is_terminated: false }
    }
}

impl TryFrom<SnoopOnObservePacketRequest> for SnoopPacket {
    type Error = Error;

    fn try_from(value: SnoopOnObservePacketRequest) -> Result<Self, Self::Error> {
        let time = fuchsia_async::MonotonicInstant::now().into();
        let SnoopOnObservePacketRequest {
            packet: Some(packet), direction: Some(direction), ..
        } = value
        else {
            return Err(format_err!("Missing required fields"));
        };
        let packet_format;
        let buf = match packet {
            HardwareSnoopPacket::Event(buf) => {
                packet_format = PacketFormat::Event;
                buf
            }
            HardwareSnoopPacket::Command(buf) => {
                packet_format = PacketFormat::Command;
                buf
            }
            HardwareSnoopPacket::Acl(buf) => {
                packet_format = PacketFormat::AclData;
                buf
            }
            HardwareSnoopPacket::Sco(buf) => {
                packet_format = PacketFormat::SynchronousData;
                buf
            }
            HardwareSnoopPacket::Iso(buf) => {
                packet_format = PacketFormat::IsoData;
                buf
            }
            _ => return Err(format_err!("Unknown packet type")),
        };
        let is_received = direction == Direction::ControllerToHost;
        return Ok(SnoopPacket::new(is_received, packet_format, time, buf));
    }
}

impl Stream for Snooper {
    type Item = (String, SnoopPacket);

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.is_terminated {
            return Poll::Ready(None);
        }
        loop {
            let result = ready!(self.event_stream.poll_next_unpin(cx)).transpose();
            if let Err(e) = result {
                warn!("error polling SnoopRequestStream: {e:?}");
                self.is_terminated = true;
                return Poll::Ready(None);
            }
            let Ok(Some(req)) = result else {
                self.is_terminated = true;
                return Poll::Ready(None);
            };

            match req {
                SnoopEvent::OnObservePacket { payload } => {
                    let Some(sequence) = payload.sequence else {
                        warn!("ObservePacket missing sequence number");
                        continue;
                    };
                    let result = self.proxy.acknowledge_packets(sequence);
                    if let Err(err) = result {
                        warn!("acknowledge_packets error: {:?}", err);
                        self.is_terminated = true;
                        return Poll::Ready(None);
                    }
                    let packet_result = SnoopPacket::try_from(payload);
                    match packet_result {
                        Ok(packet) => {
                            return Poll::Ready(Some((self.device_name.clone(), packet)));
                        }
                        Err(err) => {
                            warn!("ObservePacket parse error: {:?}", err);
                            self.is_terminated = true;
                            return Poll::Ready(None);
                        }
                    }
                }
                _ => continue,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_utils::PollExt;
    use fidl::endpoints::RequestStream;
    use fidl_fuchsia_hardware_bluetooth::{SnoopRequest, VendorMarker as HardwareVendorMarker};
    use futures::{StreamExt, poll};

    #[fuchsia::test(allow_stalls = false)]
    async fn test_from_proxy() {
        let (client, _stream) = fidl::endpoints::create_request_stream::<SnoopMarker>();
        let snooper = Snooper::from_client(client, "c");
        assert_eq!(snooper.device_name, "c");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_try_from() {
        let req = SnoopOnObservePacketRequest {
            sequence: Some(0),
            direction: Some(Direction::ControllerToHost),
            packet: Some(HardwareSnoopPacket::Event(vec![])),
            ..Default::default()
        };
        let pkt = SnoopPacket::try_from(req).unwrap();
        assert!(pkt.is_received);
        assert!(pkt.payload.is_empty());
        assert_eq!(pkt.format, PacketFormat::Event);

        let req = SnoopOnObservePacketRequest {
            sequence: Some(0),
            direction: Some(Direction::HostToController),
            packet: Some(HardwareSnoopPacket::Acl(vec![0, 1, 2])),
            ..Default::default()
        };
        let pkt = SnoopPacket::try_from(req).unwrap();
        assert!(!pkt.is_received);
        assert_eq!(pkt.payload, vec![0, 1, 2]);
        assert_eq!(pkt.format, PacketFormat::AclData);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_try_from_missing_payload() {
        let req = SnoopOnObservePacketRequest {
            sequence: Some(0),
            direction: Some(Direction::ControllerToHost),
            packet: None,
            ..Default::default()
        };
        let result = SnoopPacket::try_from(req);
        assert!(result.is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_try_from_missing_direction() {
        let req = SnoopOnObservePacketRequest {
            sequence: Some(0),
            direction: None,
            packet: Some(HardwareSnoopPacket::Event(vec![0, 1, 2])),
            ..Default::default()
        };
        let result = SnoopPacket::try_from(req);
        assert!(result.is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_snoop_stream() {
        let (snoop_client, mut req_stream) =
            fidl::endpoints::create_request_stream::<SnoopMarker>();
        let snoop_control = req_stream.control_handle();
        let mut snooper = Snooper::from_client(snoop_client, "c");
        let req_0 = SnoopOnObservePacketRequest {
            sequence: Some(0),
            direction: Some(Direction::ControllerToHost),
            packet: Some(HardwareSnoopPacket::Event(vec![0, 1, 2])),
            ..Default::default()
        };
        let req_1 = SnoopOnObservePacketRequest {
            sequence: Some(1),
            direction: Some(Direction::HostToController),
            packet: Some(HardwareSnoopPacket::Command(vec![3, 4, 5])),
            ..Default::default()
        };
        snoop_control.send_on_observe_packet(&req_0).unwrap();
        snoop_control.send_on_observe_packet(&req_1).unwrap();

        assert_eq!(snooper.next().await.unwrap().1.payload, vec![0, 1, 2]);
        assert_eq!(snooper.next().await.unwrap().1.payload, vec![3, 4, 5]);

        poll!(snooper.next()).expect_pending("pending item_3");

        match req_stream.next().await {
            Some(Ok(SnoopRequest::AcknowledgePackets { sequence, .. })) => {
                assert_eq!(sequence, 0);
            }
            _ => panic!("failed to send OnAcknowledgePackets"),
        }
        match req_stream.next().await {
            Some(Ok(SnoopRequest::AcknowledgePackets { sequence, .. })) => {
                assert_eq!(sequence, 1);
            }
            _ => panic!("failed to send OnAcknowledgePackets"),
        }
        poll!(req_stream.next()).expect_pending("pending req_3");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_snoop_stream_missing_sequence() {
        let (snoop_client, snoop_stream) = fidl::endpoints::create_request_stream::<SnoopMarker>();
        let snoop_control = snoop_stream.control_handle();
        let mut snooper = Snooper::from_client(snoop_client, "c");

        let req_0 = SnoopOnObservePacketRequest {
            sequence: None, // Missing sequence!
            direction: Some(Direction::ControllerToHost),
            packet: Some(HardwareSnoopPacket::Event(vec![0, 1, 2])),
            ..Default::default()
        };
        snoop_control.send_on_observe_packet(&req_0).unwrap();
        poll!(snooper.next()).expect_pending("pending item");

        let req_1 = SnoopOnObservePacketRequest {
            sequence: Some(1),
            direction: Some(Direction::HostToController),
            packet: Some(HardwareSnoopPacket::Command(vec![3, 4, 5])),
            ..Default::default()
        };
        snoop_control.send_on_observe_packet(&req_1).unwrap();
        assert!(snooper.next().await.is_some());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_snoop_stream_lifecycle() {
        let (snoop_client, snoop_server) = fidl::endpoints::create_endpoints::<SnoopMarker>();
        let mut snooper = Snooper::from_client(snoop_client, "c");

        poll!(snooper.next()).expect_pending("pending item");

        drop(snoop_server);
        assert!(snooper.next().await.is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_from_vendor_success() {
        let (vendor_proxy, mut vendor_stream) =
            fidl::endpoints::create_proxy_and_stream::<HardwareVendorMarker>();

        let path = "test_dev";
        let from_vendor_fut = Snooper::from_vendor(&vendor_proxy, path);

        let stream_fut = async move {
            let Some(Ok(fidl_fuchsia_hardware_bluetooth::VendorRequest::OpenSnoop { responder })) =
                vendor_stream.next().await
            else {
                panic!("Expected OpenSnoop");
            };
            let (snoop_client, _snoop_stream) = fidl::endpoints::create_endpoints::<SnoopMarker>();
            responder.send(Ok(snoop_client)).unwrap();
        };

        let (res, _) = futures::future::join(from_vendor_fut, stream_fut).await;
        let snooper = res.expect("from_vendor should succeed");
        assert_eq!(snooper.device_name, path);
    }
}
