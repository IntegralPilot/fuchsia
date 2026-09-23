// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use fidl_fuchsia_hardware_bluetooth::{
    HciTransportEvent, HciTransportEventStream, HciTransportProxy, ReceivedPacket, SentPacket,
    ServiceMarker,
};
use fuchsia_async::{DurationExt, MonotonicDuration, TimeoutExt};
use fuchsia_component::client::Service;
use futures::{Stream, StreamExt, ready};
use std::convert::TryFrom as _;
use std::fmt;
use std::pin::Pin;
use std::task::Poll;

use crate::types::{
    EventPacketType, InquiryResult, StatusCode, decode_opcode, parse_inquiry_result,
};

const OPEN_SERVICE_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(5);

/// A CommandChannel provides a `Stream` associated with the control channel for a single HCI device.
/// This stream can be polled for `EventPacket`s coming off the channel.
pub struct CommandChannel {
    pub proxy: HciTransportProxy,
    event_stream: HciTransportEventStream,
    is_terminated: bool,
}

impl CommandChannel {
    /// Create a new CommandChannel. This opens a new command channel, returning
    /// an error if the device doesn't exist or the channel cannot be created.
    pub async fn new() -> Result<CommandChannel, Error> {
        let proxy = open_hci_transport().await?;
        CommandChannel::from_proxy(proxy)
    }

    /// Take a HciTransportProxy and wrap it in a `CommandChannel`.
    fn from_proxy(proxy: HciTransportProxy) -> Result<CommandChannel, Error> {
        let event_stream = proxy.take_event_stream();
        Ok(CommandChannel { proxy, event_stream, is_terminated: false })
    }

    pub async fn send_command_packet(&self, buf: Vec<u8>) -> Result<(), Error> {
        self.proxy.send_(&SentPacket::Command(buf)).await?;
        Ok(())
    }
}

impl Unpin for CommandChannel {}
impl Stream for CommandChannel {
    type Item = EventPacket;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.is_terminated {
            return Poll::Ready(None);
        }
        loop {
            let result = ready!(self.event_stream.poll_next_unpin(cx));
            let Some(Ok(packet)) = result else {
                self.is_terminated = true;
                return Poll::Ready(None);
            };
            match packet {
                HciTransportEvent::OnReceive { payload } => {
                    if self.proxy.ack_receive().is_err() {
                        self.is_terminated = true;
                        return Poll::Ready(None);
                    }
                    let ReceivedPacket::Event(event) = payload else {
                        continue;
                    };
                    return Poll::Ready(Some(EventPacket::new(event)));
                }
                _ => continue,
            }
        }
    }
}

/// Event packets from the HCI device.
#[derive(Debug)]
pub enum EventPacket {
    CommandComplete { opcode: u16, payload: Vec<u8> },
    CommandStatus { opcode: u16, status_code: StatusCode, payload: Vec<u8> },
    InquiryResult { results: Vec<InquiryResult>, payload: Vec<u8> },
    InquiryComplete { status_code: StatusCode, payload: Vec<u8> },
    Unknown { payload: Vec<u8> },
}

const HCI_COMMAND_COMPLETE_MIN: usize = 5;
const HCI_COMMAND_STATUS_MIN: usize = 6;
const HCI_INQUIRY_RESULT_MIN: usize = 3;
const HCI_INQUIRY_COMPLETE_MIN: usize = 3;

impl EventPacket {
    pub fn new(payload: Vec<u8>) -> EventPacket {
        assert!(!payload.is_empty(), "HCI Packet is empty");

        match EventPacketType::try_from(payload[0]) {
            Ok(EventPacketType::CommandComplete) => {
                if payload.len() < HCI_COMMAND_COMPLETE_MIN {
                    return EventPacket::Unknown { payload };
                }
                let opcode = decode_opcode(&payload[3..=4]);
                EventPacket::CommandComplete { opcode, payload }
            }
            Ok(EventPacketType::CommandStatus) => {
                if payload.len() < HCI_COMMAND_STATUS_MIN {
                    return EventPacket::Unknown { payload };
                }
                let opcode = decode_opcode(&payload[4..=5]);
                let status_code =
                    StatusCode::try_from(payload[2]).unwrap_or(StatusCode::UnknownCommand);
                EventPacket::CommandStatus { opcode, status_code, payload }
            }
            Ok(EventPacketType::InquiryResult) => {
                if payload.len() < HCI_INQUIRY_RESULT_MIN {
                    return EventPacket::Unknown { payload };
                }
                let results = parse_inquiry_result(&payload[..]);
                EventPacket::InquiryResult { results, payload }
            }
            Ok(EventPacketType::InquiryComplete) => {
                if payload.len() < HCI_INQUIRY_COMPLETE_MIN {
                    return EventPacket::Unknown { payload };
                }
                let status_code =
                    StatusCode::try_from(payload[2]).unwrap_or(StatusCode::UnknownCommand);
                EventPacket::InquiryComplete { status_code, payload }
            }
            Err(_) => EventPacket::Unknown { payload },
        }
    }

    /// Returns a human readable representation of the event.
    pub fn decode(&self) -> String {
        match self {
            EventPacket::CommandComplete { opcode, .. } => {
                format!("HCI_Command_Complete Opcode: {} Payload: {}", opcode, self)
            }
            EventPacket::CommandStatus { opcode, status_code, .. } => format!(
                "HCI_Command_Status Opcode: {} Status: {} Payload: {}",
                opcode,
                status_code.name(),
                self
            ),
            EventPacket::InquiryResult { results, .. } => {
                format!("HCI_Inquiry_Result results: {:?}", results)
            }
            EventPacket::InquiryComplete { status_code, .. } => {
                format!("HCI_Inquiry_Complete Status: {}", status_code.name())
            }
            EventPacket::Unknown { payload } => {
                format!("Unknown Event: 0x{} Payload: {}", hex::encode(&[payload[0]]), self)
            }
        }
    }

    pub fn payload(&self) -> &Vec<u8> {
        match self {
            EventPacket::CommandComplete { payload, .. } => payload,
            EventPacket::CommandStatus { payload, .. } => payload,
            EventPacket::InquiryResult { payload, .. } => payload,
            EventPacket::InquiryComplete { payload, .. } => payload,
            EventPacket::Unknown { payload, .. } => payload,
        }
    }

    pub fn opcode(&self) -> Option<u16> {
        match self {
            EventPacket::CommandComplete { opcode, .. } => Some(*opcode),
            EventPacket::CommandStatus { opcode, .. } => Some(*opcode),
            _ => None,
        }
    }
}

impl fmt::Display for EventPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.payload()))
    }
}

async fn open_hci_transport() -> Result<HciTransportProxy, Error> {
    let vendor_proxy = Service::open(ServiceMarker)?
        .watch_for_any()
        .on_timeout(OPEN_SERVICE_TIMEOUT.after_now(), || {
            Err(anyhow::anyhow!("Timeout waiting for Service"))
        })
        .await
        .context("Failed to find any fuchsia.hardware.bluetooth.Service instance")?
        .connect_to_vendor()
        .context("Failed to connect to vendor member")?;

    let proxy = vendor_proxy
        .open_hci_transport()
        .await?
        .map_err(|_| format_err!("open_hci_transport failed"))?
        .into_proxy();
    Ok(proxy)
}

pub async fn open_default_device() -> Result<CommandChannel, Error> {
    CommandChannel::new().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_fidl_mocks::hci::HciTransportMock;
    use bt_fidl_mocks::timeout_duration;
    use fidl_fuchsia_hardware_bluetooth::{HciTransportMarker, ReceivedPacket};
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use futures::future::join;

    #[test]
    fn test_command_channel_stream_lifecycle() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, server) = fidl::endpoints::create_proxy::<HciTransportMarker>();
        let (stream, control_handle) = server.into_stream_and_control_handle();
        let mut command_channel = CommandChannel::from_proxy(proxy).unwrap();

        let event = vec![0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
        control_handle.send_on_receive(&ReceivedPacket::Event(event.clone())).unwrap();

        let event = exec.run_until_stalled(&mut command_channel.next());
        assert!(event.is_ready());

        drop(stream);
        drop(control_handle);
        let event = exec.run_until_stalled(&mut command_channel.next());
        let Poll::Ready(None) = event else {
            panic!("Expected None");
        };
    }

    #[test]
    fn test_command_channel() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, server) = fidl::endpoints::create_proxy::<HciTransportMarker>();
        let (stream, control_handle) = server.into_stream_and_control_handle();
        let mut command_channel = CommandChannel::from_proxy(proxy).unwrap();
        let mut mock = HciTransportMock::from_stream(stream, timeout_duration());

        let command = vec![0x03, 0x0c, 0x00];
        let send_fut = command_channel.send_command_packet(command.clone());
        let expect_fut = mock.expect_send(SentPacket::Command(command.clone()));
        let send_poll = exec.run_until_stalled(&mut Box::pin(join(send_fut, expect_fut)));
        let Poll::Ready((send_result, expect_result)) = send_poll else {
            panic!("send future stuck pending");
        };
        let _ = send_result.expect("send failed");
        let _ = expect_result.expect("expect failed");

        let event = vec![0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
        control_handle.send_on_receive(&ReceivedPacket::Event(event.clone())).unwrap();

        let event = exec.run_until_stalled(&mut command_channel.next());
        assert!(event.is_ready());
        match event {
            Poll::Ready(packet) => {
                let pkt = packet.expect("no packet");
                assert_eq!(
                    pkt.payload(),
                    &vec![
                        0x0e, // HCI_COMMAND_COMPLETE
                        0x04, 0x01, //
                        0x03, 0x0c, // opcode (little endian)
                        0x00  // payload len
                    ]
                );
                let opcode = pkt.opcode();
                assert_eq!(opcode, Some(0x0c03));
            }
            _ => panic!("failed to decode packets from stream"),
        }

        let pending_item = exec.run_until_stalled(&mut command_channel.next());
        assert!(pending_item.is_pending());
    }

    #[test]
    fn test_event_packet_decode_cmd_complete() {
        let pkt = EventPacket::new(vec![
            0x0e, 0x04, // HCI_Command_Complete
            0x01, //
            0x03, 0x0c, // opcode (little endian)
            0x00, // payload len
        ]);
        let opcode = pkt.opcode();
        assert_eq!(opcode, Some(0x0c03));
        let pretty_string = pkt.decode();
        assert_eq!(
            pretty_string,
            "HCI_Command_Complete Opcode: 3075 Payload: 0e0401030c00".to_string()
        );
    }

    #[test]
    fn test_event_packet_decode_inquiry() {
        let pkt = EventPacket::new(vec![
            0x02, 0x0f, // HCI_Inquiry_Result
            0x01, // results count
            0x7c, 0x48, 0xc6, 0x8b, 0x42, 0x74, // br_addr
            0x01, // page_scan_repetition_mode:
            0x00, 0x00, // reserved
            0x0c, 0x02, 0x7a, // class of device
            0x45, 0x25, // clock offset
        ]);
        match pkt {
            EventPacket::InquiryResult { results, .. } => {
                assert!(results.len() == 1);
                let result = &results[0];
                assert_eq!(&result.br_addr, &[0x7c, 0x48, 0xc6, 0x8b, 0x42, 0x74]);
                assert_eq!(result.page_scan_repetition_mode, 1);
                assert_eq!(&result.class_of_device, &[0x0c, 0x02, 0x7a]);
                assert_eq!(result.clockoffset, 9541);
            }
            _ => assert!(false, "packet not an inquiry result"),
        }
    }

    #[test]
    fn test_event_packet_decode_inquiry_complete() {
        let pkt = EventPacket::new(vec![
            0x01, 0x0f, // HCI_Inquiry_Complete
            0x00,
        ]);
        match pkt {
            EventPacket::InquiryComplete { status_code, .. } => {
                assert_eq!(status_code, StatusCode::Success);
            }
            _ => assert!(false, "packet not an inquiry complete"),
        }
    }

    #[test]
    fn test_event_packet_decode_unhandled() {
        let pkt = EventPacket::new(vec![0x0e]);
        let e = pkt.decode();
        assert_eq!(format!("{}", e), "Unknown Event: 0x0e Payload: 0e".to_string());
    }
}
