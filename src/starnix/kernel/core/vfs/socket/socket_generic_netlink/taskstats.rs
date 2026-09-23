// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;
use linux_uapi::{TASKSTATS_TYPE_NULL, TASKSTATS_VERSION};
use netlink::messaging::Sender;
use netlink_packet_core::{
    ErrorMessage, NETLINK_HEADER_LEN, NetlinkHeader, NetlinkMessage, NetlinkPayload,
};
use netlink_packet_generic::GenlFamily;
use netlink_packet_utils::Emitable;
use netlink_packet_utils::nla::Nla;
use starnix_logging::track_stub;

use super::{GenericMessage, GenericNetlinkFamily};

#[derive(Clone)]
pub struct TaskstatsFamily {}

impl TaskstatsFamily {
    pub fn new() -> Self {
        Self {}
    }
}

struct TaskstatMsg {
    cmd: u8,
    nlas: Vec<TaskstatNla>,
}

impl GenlFamily for TaskstatMsg {
    fn family_name() -> &'static str {
        "TASKSTATS"
    }

    fn command(&self) -> u8 {
        self.cmd
    }

    fn version(&self) -> u8 {
        // This is safe because 14 < 255
        TASKSTATS_VERSION as u8
    }
}

fn ref_cast<T>(vec: &Vec<T>) -> &[T] {
    &vec
}

impl Emitable for TaskstatMsg {
    fn buffer_len(&self) -> usize {
        1 + ref_cast(&self.nlas).buffer_len()
    }

    fn emit(&self, buffer: &mut [u8]) {
        buffer[0] = self.cmd;
        ref_cast(&self.nlas).emit(&mut buffer[1..]);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TaskstatNla {
    /// End of task information list
    Null,
}

impl Nla for TaskstatNla {
    fn value_len(&self) -> usize {
        0
    }

    fn kind(&self) -> u16 {
        // This is safe because 6 < 65535.
        TASKSTATS_TYPE_NULL as u16
    }

    fn emit_value(&self, _buffer: &mut [u8]) {
        // This is a no-op because it's a "null" value,
        // meaning it contains no value.
    }
}

#[async_trait]
impl<S: Sender<GenericMessage>> GenericNetlinkFamily<S> for TaskstatsFamily {
    fn name(&self) -> String {
        "TASKSTATS".into()
    }

    fn multicast_groups(&self) -> Vec<String> {
        vec!["PIDS".into()]
    }

    async fn stream_multicast_messages(
        &self,
        _group: String,
        _assigned_family_id: u16,
        _message_sink: UnboundedSender<NetlinkMessage<GenericMessage>>,
    ) {
        // Netlink expects this to never return. Normally,
        // it would handle multicast messages, but since this is a no-op
        // implementation it intentionally hangs forever so the protocol
        // is kept alive.
        std::future::pending::<()>().await;
    }

    async fn handle_message(
        &self,
        netlink_header: NetlinkHeader,
        payload: Vec<u8>,
        sender: &mut S,
    ) {
        track_stub!(TODO("https://fxbug.dev/339675153"), "proper taskstats implementation");
        // Send a no-op response
        let msg = TaskstatMsg {
            cmd: payload.get(0).copied().unwrap_or(0),
            nlas: vec![TaskstatNla::Null],
        };
        let mut buffer = vec![0; msg.buffer_len()];
        msg.emit(&mut buffer);
        let mut msg = NetlinkMessage::new(
            netlink_header,
            NetlinkPayload::InnerMessage(GenericMessage::Other {
                family: netlink_header.message_type,
                payload: buffer,
            }),
        );
        msg.finalize();
        sender.send(msg, None);
        // Ack the message
        let mut buffer = [0; NETLINK_HEADER_LEN];
        netlink_header.emit(&mut buffer[..NETLINK_HEADER_LEN]);
        let mut ack = ErrorMessage::default();
        // Netlink uses an error payload with no error code to indicate a
        // successful ack.
        ack.code = None;
        ack.header = buffer.to_vec();
        let mut netlink_message = NetlinkMessage::new(netlink_header, NetlinkPayload::Error(ack));
        netlink_message.finalize();
        sender.send(netlink_message, None);
    }
}
