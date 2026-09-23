// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::pin::Pin;
use futures::stream::{
    abortable, AbortHandle, FusedStream, FuturesUnordered, SelectAll, Stream, StreamExt,
};
use futures::FutureExt;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;

use bt_bap::types::{BroadcastAudioSourceEndpoint, BroadcastId};
use bt_common::core::{AdvertisingSetId, PeriodicAdvertisingInterval};
use bt_common::packet_encoding::Decodable;
use bt_common::packet_encoding::Error as PacketError;
use bt_common::PeerId;
use bt_gatt::central::{AdvertisingDatum, ScanResult};
use bt_gatt::periodic_advertising::{PeriodicAdvertising, SyncConfiguration, SyncReport};
use bt_gatt::GattTypes;

use crate::assistant::{
    DiscoveredBroadcastSources, Error, BASIC_AUDIO_ANNOUNCEMENT_SERVICE,
    BROADCAST_AUDIO_ANNOUNCEMENT_SERVICE,
};
use crate::types::BroadcastSource;

#[derive(Debug)]
pub enum Event {
    FoundBroadcastSource {
        peer: PeerId,
        advertising_sid: AdvertisingSetId,
        source: BroadcastSource,
    },
    CouldNotParseAdvertisingData {
        peer: PeerId,
        error: PacketError,
    },
}

type PeriodicAdvertisingSyncResult<T> = Result<
    <<T as GattTypes>::PeriodicAdvertising as PeriodicAdvertising>::SyncStream,
    bt_gatt::types::Error,
>;

type PeriodicAdvertisingFuture<T> =
    Pin<Box<dyn Future<Output = (PeerId, u8, PeriodicAdvertisingSyncResult<T>)>>>;

type PeriodicAdvertisingStream =
    Pin<Box<dyn Stream<Item = (PeerId, u8, Result<SyncReport, bt_gatt::types::Error>)>>>;

/// A stream of discovered broadcast sources.
/// This stream polls the scan results from GATT client to discover
/// available broadcast sources.
pub struct EventStream<T: bt_gatt::GattTypes> {
    scan_result_stream: Pin<Box<<T as bt_gatt::GattTypes>::ScanResultStream>>,
    terminated: bool,

    broadcast_sources: Arc<DiscoveredBroadcastSources>,
    broadcast_source_scan_started: Arc<AtomicBool>,

    periodic_advertising: Option<T::PeriodicAdvertising>,

    establishing_periodic_advertising_syncs: FuturesUnordered<PeriodicAdvertisingFuture<T>>,
    active_periodic_advertising_sync_streams: SelectAll<PeriodicAdvertisingStream>,
    active_syncs: HashMap<(PeerId, u8), Option<AbortHandle>>,
}

impl<T: bt_gatt::GattTypes> Unpin for EventStream<T> {}

impl<T: bt_gatt::GattTypes> EventStream<T>
where
    <<T as GattTypes>::PeriodicAdvertising as PeriodicAdvertising>::SyncStream: 'static,
    <<T as GattTypes>::PeriodicAdvertising as PeriodicAdvertising>::SyncFut: 'static,
{
    pub(crate) fn new(
        scan_result_stream: T::ScanResultStream,
        periodic_advertising: Option<T::PeriodicAdvertising>,
        broadcast_sources: Arc<DiscoveredBroadcastSources>,
        broadcast_source_scan_started: Arc<AtomicBool>,
    ) -> Self {
        Self {
            scan_result_stream: Box::pin(scan_result_stream),
            terminated: false,
            broadcast_sources,
            broadcast_source_scan_started,
            periodic_advertising,
            establishing_periodic_advertising_syncs: FuturesUnordered::new(),
            active_periodic_advertising_sync_streams: SelectAll::new(),
            active_syncs: HashMap::new(),
        }
    }

    /// Polls the futures currently establishing periodic advertising syncs.
    ///
    /// Returns `Poll::Ready(())` if any future resolved (successfully
    /// establishing a sync or failing), which indicates progress was made.
    /// Returns `Poll::Pending` if no progress was made.
    fn poll_establishing_syncs(&mut self, cx: &mut std::task::Context<'_>) -> Poll<()> {
        if self.establishing_periodic_advertising_syncs.is_terminated() {
            return Poll::Pending;
        }

        match self.establishing_periodic_advertising_syncs.poll_next_unpin(cx) {
            Poll::Ready(Some((peer_id, sid, Ok(stream)))) => {
                self.handle_established_sync(peer_id, sid, stream);
                Poll::Ready(())
            }
            Poll::Ready(Some((peer_id, sid, Err(_)))) => {
                self.active_syncs.remove(&(peer_id, sid));
                Poll::Ready(())
            }
            Poll::Ready(None) | Poll::Pending => Poll::Pending,
        }
    }

    /// Polls the active periodic advertising sync report streams.
    ///
    /// Returns:
    /// - `Poll::Ready(Some(event))` if progress was made and a completed
    ///   `FoundBroadcastSource` event is ready to be returned.
    /// - `Poll::Ready(None)` if progress was made (e.g., a report was processed
    ///   but was incomplete, or a stream failed and was removed), but no event
    ///   is ready yet.
    /// - `Poll::Pending` if no progress was made.
    fn poll_active_syncs(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Option<Event>> {
        if self.active_periodic_advertising_sync_streams.is_terminated() {
            return Poll::Pending;
        }

        match self.active_periodic_advertising_sync_streams.poll_next_unpin(cx) {
            Poll::Ready(Some((peer_id, sid, Ok(report)))) => {
                match self.handle_periodic_advertising_report(peer_id, sid, report) {
                    Some(event) => Poll::Ready(Some(event)),
                    None => Poll::Ready(None), // Progressed, but no event
                }
            }
            Poll::Ready(Some((peer_id, sid, Err(_)))) => {
                self.active_syncs.remove(&(peer_id, sid));
                Poll::Ready(None) // Progressed, but no event
            }
            Poll::Ready(None) | Poll::Pending => Poll::Pending,
        }
    }

    /// Returns the broadcast source if the scanned peer is a broadcast source.
    /// Returns an error if parsing of the scan result data fails and None if
    /// the scanned peer is not a broadcast source.
    fn try_into_broadcast_source(
        scan_result: &ScanResult,
    ) -> Result<Option<BroadcastSource>, PacketError> {
        let mut source = None;
        for datum in &scan_result.advertised {
            match datum {
                AdvertisingDatum::ServiceData(uuid, data)
                    if *uuid == BROADCAST_AUDIO_ANNOUNCEMENT_SERVICE =>
                {
                    let bid = BroadcastId::decode(data.as_slice()).0?;
                    source.get_or_insert(BroadcastSource::default()).with_broadcast_id(bid);
                }
                AdvertisingDatum::BroadcastName(name) => {
                    source
                        .get_or_insert(BroadcastSource::default())
                        .with_broadcast_name(name.clone());
                }
                _ => {}
            }
        }
        if let Some(src) = &mut source {
            src.periodic_advertising_interval =
                scan_result.periodic_advertising_interval.map(PeriodicAdvertisingInterval);
        }
        Ok(source)
    }

    fn handle_established_sync(
        &mut self,
        peer_id: PeerId,
        sid: u8,
        stream: <<T as GattTypes>::PeriodicAdvertising as PeriodicAdvertising>::SyncStream,
    ) {
        let mapped_stream = stream.map(move |report| (peer_id, sid, report));
        let (abortable_stream, abort_handle) = abortable(mapped_stream);

        self.active_periodic_advertising_sync_streams.push(Box::pin(abortable_stream));
        self.active_syncs.insert((peer_id, sid), Some(abort_handle));
    }

    fn handle_periodic_advertising_report(
        &mut self,
        peer_id: PeerId,
        sid: u8,
        report: SyncReport,
    ) -> Option<Event> {
        let SyncReport::PeriodicAdvertisingReport(report) = report else {
            return None;
        };

        let Some(base) = parse_base_from_advertising_data(&report.data) else {
            return None;
        };

        let Ok(advertising_sid) = AdvertisingSetId::try_from(sid) else {
            return None;
        };

        let (broadcast_source, changed) = self.broadcast_sources.merge_broadcast_source_data(
            &(peer_id, advertising_sid),
            &BroadcastSource::default().with_endpoint(base),
        );

        if broadcast_source.is_ready_to_add() && changed {
            if let Some(Some(handle)) =
                self.active_syncs.remove(&(peer_id, advertising_sid.value()))
            {
                handle.abort();
            }
            return Some(Event::FoundBroadcastSource {
                peer: peer_id,
                advertising_sid,
                source: broadcast_source,
            });
        }
        None
    }

    fn handle_scan_result(&mut self, scanned: ScanResult) -> Option<Event> {
        let found_source = match Self::try_into_broadcast_source(&scanned) {
            Ok(Some(src)) => src,
            Ok(None) => return None,
            Err(e) => {
                return Some(Event::CouldNotParseAdvertisingData { peer: scanned.id, error: e })
            }
        };

        let Some(raw_sid) = scanned.advertising_sid else {
            return None;
        };
        let Ok(sid) = AdvertisingSetId::try_from(raw_sid) else {
            return None;
        };

        let (broadcast_source, changed) =
            self.broadcast_sources.merge_broadcast_source_data(&(scanned.id, sid), &found_source);

        if broadcast_source.is_ready_to_add() && changed {
            return Some(Event::FoundBroadcastSource {
                peer: scanned.id,
                advertising_sid: sid,
                source: broadcast_source,
            });
        }

        // See if it's appropriate to establish PA sync for this broadcast source.

        // If we already have the endpoint data (BASE), we don't need to establish a
        // sync.
        if broadcast_source.endpoint.is_some() {
            return None;
        }

        // If the platform doesn't support periodic advertising, we cannot sync.
        let Some(ref pa) = self.periodic_advertising else {
            return None;
        };

        // If we are already actively syncing (or establishing a sync) for this
        // peer/SID, don't start another one.
        let key = (scanned.id, sid.value());
        if self.active_syncs.contains_key(&key) {
            return None;
        }

        self.active_syncs.insert(key, None);
        let fut = pa.sync_to_advertising_reports(
            scanned.id,
            sid.value(),
            SyncConfiguration { filter_duplicates: true },
        );
        let mapped_fut = fut.map(move |res| (scanned.id, sid.value(), res));
        self.establishing_periodic_advertising_syncs.push(Box::pin(mapped_fut));

        None
    }
}

fn parse_base_from_advertising_data(
    data: &[AdvertisingDatum],
) -> Option<BroadcastAudioSourceEndpoint> {
    for datum in data {
        let AdvertisingDatum::ServiceData(uuid, service_data) = datum else {
            continue;
        };
        if *uuid != BASIC_AUDIO_ANNOUNCEMENT_SERVICE {
            continue;
        }
        let (Ok(base), _) = BroadcastAudioSourceEndpoint::decode(service_data) else {
            continue;
        };
        return Some(base);
    }
    None
}

impl<T: bt_gatt::GattTypes> Drop for EventStream<T> {
    fn drop(&mut self) {
        self.broadcast_source_scan_started.store(false, Ordering::Relaxed);
    }
}

impl<T: bt_gatt::GattTypes> FusedStream for EventStream<T>
where
    <<T as GattTypes>::PeriodicAdvertising as PeriodicAdvertising>::SyncStream: 'static,
    <<T as GattTypes>::PeriodicAdvertising as PeriodicAdvertising>::SyncFut: 'static,
{
    fn is_terminated(&self) -> bool {
        self.terminated
    }
}

impl<T: bt_gatt::GattTypes> Stream for EventStream<T>
where
    <<T as GattTypes>::PeriodicAdvertising as PeriodicAdvertising>::SyncStream: 'static,
    <<T as GattTypes>::PeriodicAdvertising as PeriodicAdvertising>::SyncFut: 'static,
{
    type Item = Result<Event, Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.terminated {
            return Poll::Ready(None);
        }

        loop {
            let mut progressed = false;

            if self.poll_establishing_syncs(cx).is_ready() {
                progressed = true;
            }

            match self.poll_active_syncs(cx) {
                Poll::Ready(Some(event)) => return Poll::Ready(Some(Ok(event))),
                Poll::Ready(None) => progressed = true,
                Poll::Pending => {}
            }

            match self.scan_result_stream.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(scanned))) => {
                    progressed = true;
                    if let Some(event) = self.handle_scan_result(scanned) {
                        return Poll::Ready(Some(Ok(event)));
                    }
                }
                Poll::Ready(None | Some(Err(_))) => {
                    self.terminated = true;
                    self.broadcast_source_scan_started.store(false, Ordering::Relaxed);
                    return Poll::Ready(Some(Err(Error::CentralScanTerminated)));
                }
                Poll::Pending => {}
            }

            if !progressed {
                break;
            }
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    use bt_common::core::{AddressType, AdvertisingSetId};
    use bt_gatt::central::{AdvertisingDatum, PeerName};
    use bt_gatt::test_utils::{
        FakePeriodicAdvertising, FakeTypes, ScannedResultStream, ScannedResultStreamController,
    };
    use bt_gatt::types::Error as BtGattError;
    use bt_gatt::types::GattError;

    fn setup_stream(
    ) -> (EventStream<FakeTypes>, ScannedResultStreamController, FakePeriodicAdvertising) {
        let fake_scan_result_stream = ScannedResultStream::new();
        let controller = fake_scan_result_stream.controller();
        let broadcast_sources = DiscoveredBroadcastSources::new();
        let broadcast_source_scan_started = Arc::new(AtomicBool::new(false));
        let pa = FakePeriodicAdvertising::default();

        (
            EventStream::<FakeTypes>::new(
                fake_scan_result_stream,
                Some(pa.clone()),
                broadcast_sources,
                broadcast_source_scan_started,
            ),
            controller,
            pa,
        )
    }

    #[test]
    fn poll_found_broadcast_source_events() {
        let (mut stream, scan_result_controller, pa) = setup_stream();

        // Scanned a broadcast source and its broadcast id.
        let broadcast_source_pid = PeerId(1005);

        scan_result_controller.add_scanned_result(Ok(ScanResult {
            id: broadcast_source_pid,
            connectable: true,
            name: PeerName::Unknown,
            advertised: vec![
                AdvertisingDatum::ServiceData(
                    BROADCAST_AUDIO_ANNOUNCEMENT_SERVICE,
                    vec![0x01, 0x02, 0x03],
                ),
                AdvertisingDatum::BroadcastName("Test Broadcast".to_string()),
            ],
            advertising_sid: Some(1),
            periodic_advertising_interval: Some(0x0100),
        }));

        // Found broadcast source event shouldn't have been sent since braodcast source
        // information isn't complete.
        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        assert!(stream.poll_next_unpin(&mut noop_cx).is_pending());

        // Poll again to let the PA sync transition from Establishing to Established.
        assert!(stream.poll_next_unpin(&mut noop_cx).is_pending());

        // Pretend somehow address, address type were filled out.
        let _ = stream.broadcast_sources.merge_broadcast_source_data(
            &(broadcast_source_pid, AdvertisingSetId::try_from(1).unwrap()),
            &BroadcastSource::default()
                .with_address([1, 2, 3, 4, 5, 6])
                .with_address_type(AddressType::Public),
        );

        // Scanned broadcast source's BASE data:
        #[rustfmt::skip]
        let base_data = vec![
            0x10, 0x20, 0x30, 0x02,               // presentation delay, num of subgroups
            0x01, 0x03, 0x00, 0x00, 0x00, 0x00,   // num of bis, codec id (big #1)
            0x00,                                 // codec specific config len
            0x00,                                 // metadata len,
            0x01, 0x00,                           // bis index, codec specific config len (big #1 / bis #1)
            0x01, 0x02, 0x00, 0x00, 0x00, 0x00,   // num of bis, codec id (big #2)
            0x00,                                 // codec specific config len
            0x00,                                 // metadata len,
            0x01, 0x03, 0x02, 0x05, 0x08,         // bis index, codec specific config len, codec frame blocks LTV (big #2 / bis #2)
        ];

        let advertising_data =
            vec![AdvertisingDatum::ServiceData(BASIC_AUDIO_ANNOUNCEMENT_SERVICE, base_data)];

        // Get the fake periodic advertising sync sender that was registered when we
        // polled the scan result
        let periodic_advertising_sender = pa
            .get_sender(broadcast_source_pid)
            .expect("should have registered periodic advertising sync sender");

        // Send the PA report containing the BASE data
        periodic_advertising_sender
            .unbounded_send(Ok(SyncReport::PeriodicAdvertisingReport(
                bt_gatt::periodic_advertising::PeriodicAdvertisingReport {
                    rssi: -50,
                    data: advertising_data,
                    event_counter: None,
                    subevent: None,
                    timestamp: 0,
                },
            )))
            .unwrap();

        // Expect the stream to send out broadcast source found event since information
        // is complete.
        let Poll::Ready(Some(Ok(event))) = stream.poll_next_unpin(&mut noop_cx) else {
            panic!("should have received event");
        };
        assert_matches!(event, Event::FoundBroadcastSource { peer, advertising_sid, source } => {
            assert_eq!(peer, broadcast_source_pid);
            assert_eq!(advertising_sid, AdvertisingSetId::try_from(1).unwrap());
            assert_eq!(source.periodic_advertising_interval, Some(PeriodicAdvertisingInterval(0x0100)));
            assert_eq!(source.address, Some([1, 2, 3, 4, 5, 6]));
            assert_eq!(source.broadcast_name, Some("Test Broadcast".to_string()));
        });

        // Verify that the PA sync was stopped (removed from active_syncs) to conserve
        // resources
        assert!(stream.active_syncs.is_empty());

        // Subsequent polls should be pending
        assert!(stream.poll_next_unpin(&mut noop_cx).is_pending());
    }

    #[test]
    fn central_scan_stream_terminates() {
        let (mut stream, scan_result_controller, _pa) = setup_stream();

        // Mimick scan error.
        scan_result_controller.add_scanned_result(Err(BtGattError::Gatt(GattError::InvalidPdu)));

        let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        match stream.poll_next_unpin(&mut noop_cx) {
            Poll::Ready(Some(Err(e))) => assert_matches!(e, Error::CentralScanTerminated),
            _ => panic!("should have received central scan terminated error"),
        }

        // Entire stream should have terminated.
        assert_matches!(stream.poll_next_unpin(&mut noop_cx), Poll::Ready(None));
        assert_matches!(stream.is_terminated(), true);
    }
}
