// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::hanging_get::client::HangingGetStream;
use bt_common::PeerId;
use fidl_fuchsia_bluetooth_le as fidl_le;
use futures::stream::Stream;
use futures::{Future, StreamExt, TryStreamExt};
use std::pin::Pin;

#[cfg(test)]
use crate::to_fidl_uuid;
use crate::{to_fidl_peer_id, to_gatt_scan_data};

fn to_gatt_sync_error(err: fidl_le::PeriodicAdvertisingSyncError) -> bt_gatt::types::Error {
    let gatt_err = match err {
        fidl_le::PeriodicAdvertisingSyncError::InitialSynchronizationFailed => {
            bt_gatt::periodic_advertising::Error::SyncEstablishFailed
        }
        fidl_le::PeriodicAdvertisingSyncError::SynchronizationLost => {
            bt_gatt::periodic_advertising::Error::SyncLost
        }
        _ => bt_gatt::periodic_advertising::Error::Io,
    };
    bt_gatt::types::Error::Other(Box::new(gatt_err))
}

fn to_gatt_phy(phy: fidl_le::PhysicalLayer) -> bt_common::core::Phy {
    match phy {
        fidl_le::PhysicalLayer::Le1M => bt_common::core::Phy::Le1m,
        fidl_le::PhysicalLayer::Le2M => bt_common::core::Phy::Le2m,
        fidl_le::PhysicalLayer::LeCoded => bt_common::core::Phy::LeCoded,
        _ => bt_common::core::Phy::Le1m,
    }
}

fn to_gatt_big_info(
    info: &fidl_le::BroadcastIsochronousGroupInfo,
) -> bt_gatt::periodic_advertising::BroadcastIsochronousGroupInfo {
    bt_gatt::periodic_advertising::BroadcastIsochronousGroupInfo {
        streams_count: info.streams_count.unwrap_or(0),
        sdu_interval: 0, // TODO(https://fxbug.dev/429213165): Missing in FIDL
        max_sdu_size: info.max_sdu_size.unwrap_or(0),
        phy: info.phy.map(to_gatt_phy).unwrap_or(bt_common::core::Phy::Le1m),
        encryption: info.encryption.unwrap_or(false),
    }
}

fn to_gatt_periodic_advertising_report(
    report: fidl_le::PeriodicAdvertisingReport,
) -> bt_gatt::periodic_advertising::PeriodicAdvertisingReport {
    bt_gatt::periodic_advertising::PeriodicAdvertisingReport {
        rssi: report.rssi.unwrap_or(0),
        data: report.data.map(to_gatt_scan_data).unwrap_or_default(),
        event_counter: report.event_counter,
        subevent: report.subevent,
        timestamp: report.timestamp.unwrap_or(0),
    }
}

fn to_gatt_sync_report(
    report: fidl_le::SyncReport,
) -> bt_gatt::Result<Option<bt_gatt::periodic_advertising::SyncReport>> {
    match report {
        fidl_le::SyncReport::PeriodicAdvertisingReport(r) => {
            Ok(Some(bt_gatt::periodic_advertising::SyncReport::PeriodicAdvertisingReport(
                to_gatt_periodic_advertising_report(r),
            )))
        }
        fidl_le::SyncReport::BroadcastIsochronousGroupInfoReport(r) => {
            match &r.info {
                Some(info) => {
                    let info = to_gatt_big_info(info);
                    Ok(Some(bt_gatt::periodic_advertising::SyncReport::BroadcastIsochronousGroupInfoReport(
                        bt_gatt::periodic_advertising::BroadcastIsochronousGroupInfoReport {
                            info,
                            timestamp: r.timestamp.unwrap_or(0),
                        },
                    )))
                }
                None => Ok(None),
            }
        }
        _ => Err(bt_gatt::types::Error::Other(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unknown SyncReport variant",
        )))),
    }
}

fn create_sync_stream(
    sync_proxy: fidl_le::PeriodicAdvertisingSyncProxy,
    event_stream: fidl_le::PeriodicAdvertisingSyncEventStream,
) -> <PeriodicAdvertising as bt_gatt::periodic_advertising::PeriodicAdvertising>::SyncStream {
    let hanging_get_stream =
        HangingGetStream::new_eager(sync_proxy, |p| p.watch_advertising_report());

    let reports_stream = hanging_get_stream
        .map_err(|e| bt_gatt::types::Error::Other(Box::new(e)))
        .map(|res| match res {
            Ok(response) => {
                let reports = response.reports.unwrap_or_default();
                let gatt_reports: Vec<bt_gatt::Result<bt_gatt::periodic_advertising::SyncReport>> =
                    reports
                        .into_iter()
                        .filter_map(|r| match to_gatt_sync_report(r) {
                            Ok(Some(report)) => Some(Ok(report)),
                            Ok(None) => None,
                            Err(e) => Some(Err(e)),
                        })
                        .collect();
                futures::stream::iter(gatt_reports)
            }
            Err(e) => futures::stream::iter(vec![Err(e)]),
        })
        .flatten();

    let event_errors = event_stream.filter_map(|event_res| {
        let res = match event_res {
            Ok(fidl_le::PeriodicAdvertisingSyncEvent::OnError { error }) => {
                Some(Err(to_gatt_sync_error(error)))
            }
            Ok(_) => Some(Err(bt_gatt::types::Error::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected event after establishment",
            ))))),
            Err(e) => Some(Err(bt_gatt::types::Error::Other(Box::new(e)))),
        };
        futures::future::ready(res)
    });

    let merged_stream = futures::stream::select(reports_stream, event_errors);

    Box::pin(merged_stream)
}

#[derive(Clone)]
pub struct PeriodicAdvertising {
    pub(crate) proxy: fidl_le::CentralProxy,
}

impl bt_gatt::periodic_advertising::PeriodicAdvertising for PeriodicAdvertising {
    type SyncFut = Pin<Box<dyn Future<Output = bt_gatt::Result<Self::SyncStream>> + Send>>;
    type SyncStream = Pin<
        Box<
            dyn Stream<Item = bt_gatt::Result<bt_gatt::periodic_advertising::SyncReport>>
                + Send
                + 'static,
        >,
    >;

    fn sync_to_advertising_reports(
        &self,
        peer_id: PeerId,
        adv_sid: u8,
        config: bt_gatt::periodic_advertising::SyncConfiguration,
    ) -> Self::SyncFut {
        let proxy = self.proxy.clone();

        Box::pin(async move {
            let (sync_proxy, server_end) =
                fidl::endpoints::create_proxy::<fidl_le::PeriodicAdvertisingSyncMarker>();

            let fidl_config = fidl_le::PeriodicAdvertisingSyncConfiguration {
                filter_duplicates: Some(config.filter_duplicates),
                ..Default::default()
            };

            proxy
                .sync_to_periodic_advertising(fidl_le::CentralSyncToPeriodicAdvertisingRequest {
                    peer_id: Some(to_fidl_peer_id(&peer_id)),
                    advertising_sid: Some(adv_sid),
                    sync: Some(server_end),
                    config: Some(fidl_config),
                    ..Default::default()
                })
                .map_err(|e| bt_gatt::types::Error::Other(Box::new(e)))?;

            let mut event_stream = sync_proxy.take_event_stream();

            match event_stream.next().await {
                Some(Ok(fidl_le::PeriodicAdvertisingSyncEvent::OnEstablished { .. })) => {
                    let stream = create_sync_stream(sync_proxy, event_stream);
                    Ok(stream)
                }
                Some(Ok(fidl_le::PeriodicAdvertisingSyncEvent::OnError { error })) => {
                    Err(to_gatt_sync_error(error))
                }
                Some(Ok(_)) => Err(bt_gatt::types::Error::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unknown event received during establishment",
                )))),
                Some(Err(e)) => Err(bt_gatt::types::Error::Other(Box::new(e))),
                None => Err(bt_gatt::types::Error::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Event stream closed before establishment",
                )))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_common::Uuid;
    use bt_gatt::central::AdvertisingDatum;

    #[test]
    fn test_to_gatt_sync_error() {
        let err = fidl_le::PeriodicAdvertisingSyncError::InitialSynchronizationFailed;
        let gatt_err = to_gatt_sync_error(err);
        assert!(format!("{gatt_err:?}").contains("SyncEstablishFailed"));

        let err = fidl_le::PeriodicAdvertisingSyncError::SynchronizationLost;
        let gatt_err = to_gatt_sync_error(err);
        assert!(format!("{gatt_err:?}").contains("SyncLost"));

        let err = fidl_le::PeriodicAdvertisingSyncError::NotSupportedLocal;
        let gatt_err = to_gatt_sync_error(err);
        assert!(format!("{gatt_err:?}").contains("Io"));
    }

    #[test]
    fn test_to_gatt_phy() {
        assert_eq!(to_gatt_phy(fidl_le::PhysicalLayer::Le1M), bt_common::core::Phy::Le1m);
        assert_eq!(to_gatt_phy(fidl_le::PhysicalLayer::Le2M), bt_common::core::Phy::Le2m);
        assert_eq!(to_gatt_phy(fidl_le::PhysicalLayer::LeCoded), bt_common::core::Phy::LeCoded);
    }

    #[test]
    fn test_to_gatt_big_info() {
        let fidl_info = fidl_le::BroadcastIsochronousGroupInfo {
            streams_count: Some(5),
            max_sdu_size: Some(200),
            phy: Some(fidl_le::PhysicalLayer::LeCoded),
            encryption: Some(true),
            ..Default::default()
        };
        let gatt_info = to_gatt_big_info(&fidl_info);
        assert_eq!(gatt_info.streams_count, 5);
        assert_eq!(gatt_info.sdu_interval, 0); // Placeholder
        assert_eq!(gatt_info.max_sdu_size, 200);
        assert_eq!(gatt_info.phy, bt_common::core::Phy::LeCoded);
        assert_eq!(gatt_info.encryption, true);
    }

    #[test]
    fn test_to_gatt_scan_data() {
        let service_uuid = Uuid::from_u16(0x1852);
        let scan_data = fidl_le::ScanData {
            tx_power: Some(-15),
            service_uuids: Some(vec![to_fidl_uuid(&service_uuid)]),
            service_data: Some(vec![fidl_le::ServiceData {
                uuid: to_fidl_uuid(&service_uuid),
                data: vec![4, 5, 6],
            }]),
            manufacturer_data: Some(vec![fidl_le::ManufacturerData {
                company_id: 0x00E0,
                data: vec![7, 8],
            }]),
            uris: Some(vec!["https://example.com".to_string()]),
            broadcast_name: Some("My Broadcast".to_string()),
            resolvable_set_identifier: Some([1, 2, 3, 4, 5, 6]),
            ..Default::default()
        };

        let advertised = to_gatt_scan_data(scan_data);
        assert_eq!(advertised.len(), 7);

        assert!(advertised.iter().any(|d| matches!(d, AdvertisingDatum::TxPowerLevel(-15))));

        assert!(advertised.iter().any(|d| match d {
            AdvertisingDatum::Services(uuids) => uuids == &[service_uuid],
            _ => false,
        }));

        assert!(advertised.iter().any(|d| match d {
            AdvertisingDatum::ServiceData(uuid, data) =>
                *uuid == service_uuid && data == &[4, 5, 6],
            _ => false,
        }));

        assert!(advertised.iter().any(|d| match d {
            AdvertisingDatum::ManufacturerData(company_id, data) =>
                *company_id == 0x00E0 && data == &[7, 8],
            _ => false,
        }));

        assert!(advertised.iter().any(|d| match d {
            AdvertisingDatum::Uri(uri) => uri == "https://example.com",
            _ => false,
        }));

        assert!(advertised.iter().any(|d| match d {
            AdvertisingDatum::BroadcastName(name) => name == "My Broadcast",
            _ => false,
        }));

        assert!(advertised.iter().any(|d| match d {
            AdvertisingDatum::ResolvableSetIdentifier(rsi) => rsi == &[1, 2, 3, 4, 5, 6],
            _ => false,
        }));
    }
}
