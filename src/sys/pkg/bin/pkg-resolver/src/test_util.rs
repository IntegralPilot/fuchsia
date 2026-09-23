// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cobalt_client::traits::AsEventCodes;
use fidl_contrib::protocol_connector::ProtocolSender;
use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use futures::channel::mpsc;

pub(crate) fn get_fake_cobalt_sender() -> (ProtocolSender<MetricEvent>, mpsc::Receiver<MetricEvent>)
{
    let (sender, cobalt_receiver) = mpsc::channel(1);
    (ProtocolSender::new(sender), cobalt_receiver)
}

pub(crate) fn verify_cobalt_emits_event(
    cobalt_receiver: &mut mpsc::Receiver<MetricEvent>,
    metric_id: u32,
    expected_event_codes: impl AsEventCodes,
) {
    assert_eq!(
        cobalt_receiver.try_next().unwrap().unwrap(),
        MetricEvent {
            metric_id,
            event_codes: expected_event_codes.as_event_codes(),
            payload: MetricEventPayload::Count(1),
        }
    );
}
