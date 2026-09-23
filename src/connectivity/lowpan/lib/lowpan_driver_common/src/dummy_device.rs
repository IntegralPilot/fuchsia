// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! LoWPAN Dummy Driver

use super::prelude_internal::*;

use crate::lowpan_fidl::*;
use crate::Driver;
use core::future::ready;
use futures::stream::BoxStream;
use futures::FutureExt;
use zx_status;

/// A dummy LoWPAN Driver implementation, for testing.
#[derive(Debug, Copy, Clone, Default)]
pub struct DummyDevice {}

#[async_trait::async_trait]
impl Driver for DummyDevice {
    async fn provision_network(&self, params: ProvisioningParams) -> ZxResult<()> {
        info!("Got provision command: {:?}", params);
        Ok(())
    }

    async fn leave_network(&self) -> ZxResult<()> {
        info!("Got leave command");
        Ok(())
    }

    async fn reset(&self) -> ZxResult<()> {
        info!("Got reset command");
        Ok(())
    }

    async fn set_active(&self, active: bool) -> ZxResult<()> {
        info!("Got set active command: {:?}", active);
        Ok(())
    }

    async fn get_supported_network_types(&self) -> ZxResult<Vec<String>> {
        info!("Got get_supported_network_types command");

        Ok(vec!["network_type_0".to_string(), "network_type_1".to_string()])
    }

    async fn get_supported_channels(&self) -> ZxResult<Vec<ChannelInfo>> {
        info!("Got get_supported_channels command");
        let channel_info = ChannelInfo {
            id: Some("id".to_string()),
            index: Some(20),
            max_transmit_power_dbm: Some(-100),
            spectrum_center_frequency_hz: Some(2450000000),
            spectrum_bandwidth_hz: Some(2000000),
            masked_by_regulatory_domain: Some(false),
            ..Default::default()
        };
        Ok(vec![channel_info])
    }

    fn form_network(
        &self,
        params: ProvisioningParams,
    ) -> BoxStream<'_, ZxResult<Result<ProvisioningProgress, ProvisionError>>> {
        info!("Got form command: {:?}", params);

        futures::stream::empty()
            .chain(ready(Ok(Ok(ProvisioningProgress::Progress(0.4)))).into_stream())
            .chain(ready(Ok(Ok(ProvisioningProgress::Progress(0.6)))).into_stream())
            .chain(ready(Ok(Ok(ProvisioningProgress::Identity(params.identity)))).into_stream())
            .boxed()
    }

    fn join_network(
        &self,
        params: JoinParams,
    ) -> BoxStream<'_, ZxResult<Result<ProvisioningProgress, ProvisionError>>> {
        info!("Got join command: {:?}", params);

        futures::stream::empty()
            .chain(ready(Ok(Ok(ProvisioningProgress::Progress(0.5)))).into_stream())
            .chain(
                ready(Ok(Ok(ProvisioningProgress::Identity(Identity {
                    raw_name: Some("MyNet".as_bytes().to_vec()),
                    xpanid: Some([0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77]),
                    net_type: Some(NET_TYPE_THREAD_1_X.to_string()),
                    channel: Some(11),
                    panid: Some(0x1234),
                    ..Default::default()
                }))))
                .into_stream(),
            )
            .boxed()
    }

    async fn get_credential(&self) -> ZxResult<Option<Credential>> {
        info!("Got get credential command");

        let res: Vec<u8> = hex::decode("000102030405060708090a0b0c0d0f")
            .map_err(|_| zx_status::Status::INTERNAL)?
            .to_vec();

        Ok(Some(Credential::NetworkKey(res)))
    }

    async fn get_factory_mac_address(&self) -> ZxResult<MacAddress> {
        info!("Got get_factory_mac_address command");

        Ok(MacAddress { octets: [0, 1, 2, 3, 4, 5, 6, 7] })
    }

    async fn get_current_mac_address(&self) -> ZxResult<MacAddress> {
        info!("Got get_current_mac_address command");

        Ok(MacAddress { octets: [0, 1, 2, 3, 4, 5, 6, 7] })
    }

    fn start_energy_scan(
        &self,
        _params: &EnergyScanParameters,
    ) -> BoxStream<'_, ZxResult<Vec<EnergyScanResult>>> {
        // NOTE: Updates to the returned value may need to be reflected
        //       in `crate::lowpan_device::tests::test_energy_scan`.
        futures::stream::empty()
            .chain(
                ready(vec![EnergyScanResult {
                    channel_index: Some(11),
                    max_rssi: Some(-20),
                    min_rssi: Some(-90),
                    ..Default::default()
                }])
                .into_stream(),
            )
            .chain(ready(vec![]).into_stream())
            .chain(
                ready(vec![
                    EnergyScanResult {
                        channel_index: Some(12),
                        max_rssi: Some(-30),
                        min_rssi: Some(-90),
                        ..Default::default()
                    },
                    EnergyScanResult {
                        channel_index: Some(13),
                        max_rssi: Some(-25),
                        min_rssi: Some(-90),
                        ..Default::default()
                    },
                ])
                .into_stream(),
            )
            .chain(
                ready(vec![
                    EnergyScanResult {
                        channel_index: Some(14),
                        max_rssi: Some(-45),
                        min_rssi: Some(-90),
                        ..Default::default()
                    },
                    EnergyScanResult {
                        channel_index: Some(15),
                        max_rssi: Some(-40),
                        min_rssi: Some(-50),
                        ..Default::default()
                    },
                ])
                .into_stream(),
            )
            .map(Ok)
            .boxed()
    }

    fn start_network_scan(
        &self,
        _params: &NetworkScanParameters,
    ) -> BoxStream<'_, ZxResult<Vec<BeaconInfo>>> {
        // NOTE: Updates to the returned value may need to be reflected
        //       in `crate::lowpan_device::tests::test_network_scan`.
        futures::stream::empty()
            .chain(
                ready(vec![BeaconInfo {
                    identity: Some(Identity {
                        raw_name: Some("MyNet".as_bytes().to_vec()),
                        xpanid: Some([0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77]),
                        net_type: Some(NET_TYPE_THREAD_1_X.to_string()),
                        channel: Some(11),
                        panid: Some(0x1234),
                        ..Default::default()
                    }),
                    rssi: Some(-40),
                    address: Some(MacAddress {
                        octets: [0x02, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05],
                    }),
                    ..Default::default()
                }])
                .into_stream(),
            )
            .chain(ready(vec![]).into_stream())
            .chain(
                ready(vec![
                    BeaconInfo {
                        identity: Some(Identity {
                            raw_name: Some("MyNet".as_bytes().to_vec()),
                            xpanid: Some([0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77]),
                            net_type: Some(NET_TYPE_THREAD_1_X.to_string()),
                            channel: Some(11),
                            panid: Some(0x1234),
                            ..Default::default()
                        }),
                        rssi: Some(-60),
                        address: Some(MacAddress {
                            octets: [0x02, 0x00, 0x00, 0x00, 0x00, 0x03, 0x13, 0x37],
                        }),
                        ..Default::default()
                    },
                    BeaconInfo {
                        identity: Some(Identity {
                            raw_name: Some("MyNet2".as_bytes().to_vec()),
                            xpanid: Some([0xFF, 0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33, 0xFF]),
                            net_type: Some(NET_TYPE_THREAD_1_X.to_string()),
                            channel: Some(12),
                            panid: Some(0x5678),
                            ..Default::default()
                        }),
                        rssi: Some(-26),
                        address: Some(MacAddress {
                            octets: [0x02, 0x00, 0x00, 0x00, 0xde, 0xad, 0xbe, 0xef],
                        }),
                        ..Default::default()
                    },
                ])
                .into_stream(),
            )
            .map(Ok)
            .boxed()
    }

    async fn get_ncp_version(&self) -> ZxResult<String> {
        info!("Got get_ncp_version command");
        Ok("LowpanDummyDriver/0.0".to_string())
    }

    async fn get_current_channel(&self) -> ZxResult<u16> {
        info!("Got get_current_channel command");

        Ok(1)
    }

    async fn get_current_rssi(&self) -> ZxResult<i8> {
        info!("Got get_current_rssi command");

        Ok(0)
    }

    fn watch_device_state(&self) -> BoxStream<'_, ZxResult<DeviceState>> {
        use futures::future::ready;
        use futures::stream::pending;
        let initial = Ok(DeviceState {
            connectivity_state: Some(ConnectivityState::Ready),
            role: Some(Role::Detached),
            ..Default::default()
        });

        ready(initial).into_stream().chain(pending()).boxed()
    }

    fn watch_identity(&self) -> BoxStream<'_, ZxResult<Identity>> {
        use futures::future::ready;
        use futures::stream::pending;
        let initial = Ok(Identity {
            raw_name: Some(b"ABC1234".to_vec()),
            panid: Some(1234),
            ..Default::default()
        });

        ready(initial).into_stream().chain(pending()).boxed()
    }

    async fn get_partition_id(&self) -> ZxResult<u32> {
        info!("Got get_partition_id command");

        Ok(0)
    }

    async fn get_thread_rloc16(&self) -> ZxResult<u16> {
        info!("Got get_thread_rloc16 command");

        Ok(0xffff)
    }

    async fn get_thread_router_id(&self) -> ZxResult<u8> {
        info!("Got get_thread_router_id command");

        Ok(0)
    }

    async fn send_mfg_command(&self, command: &str) -> ZxResult<String> {
        info!("Got send_mfg_command command: {:?}", command);

        Ok("error: The dummy driver currently has no manufacturing commands.".to_string())
    }

    async fn setup_ot_cli(&self, _server_socket: fidl::Socket) -> ZxResult<()> {
        info!("Got setup_ot_cli request");

        Ok(())
    }

    async fn replace_mac_address_filter_settings(
        &self,
        _settings: MacAddressFilterSettings,
    ) -> ZxResult<()> {
        Ok(())
    }

    async fn get_mac_address_filter_settings(&self) -> ZxResult<MacAddressFilterSettings> {
        Ok(MacAddressFilterSettings {
            mode: Some(MacAddressFilterMode::Allow),
            items: Some(vec![MacAddressFilterItem {
                mac_address: Some(MacAddress {
                    octets: [0xFF, 0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33, 0xFF],
                }),
                rssi: Some(8),
                ..Default::default()
            }]),
            ..Default::default()
        })
    }

    async fn get_neighbor_table(&self) -> ZxResult<Vec<NeighborInfo>> {
        return Ok(vec![NeighborInfo {
            mac_address: Some(MacAddress {
                octets: [0xFF, 0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33, 0xFF],
            }),
            short_address: Some(8),
            age: Some(10042934),
            is_child: Some(true),
            link_frame_count: Some(256),
            mgmt_frame_count: Some(128),
            last_rssi_in: Some(-108),
            avg_rssi_in: Some(-12),
            lqi_in: Some(16),
            thread_mode: Some(5),
            ..Default::default()
        }]);
    }

    async fn get_counters(&self) -> ZxResult<AllCounters> {
        return Ok(AllCounters {
            mac_tx: Some(MacCounters {
                total: Some(0),
                unicast: Some(1),
                broadcast: Some(2),
                ack_requested: Some(3),
                acked: Some(4),
                no_ack_requested: Some(5),
                data: Some(6),
                data_poll: Some(7),
                beacon: Some(8),
                beacon_request: Some(9),
                other: Some(10),
                address_filtered: None,
                retries: Some(11),
                direct_max_retry_expiry: Some(15),
                indirect_max_retry_expiry: Some(16),
                dest_addr_filtered: None,
                duplicated: None,
                err_no_frame: None,
                err_unknown_neighbor: None,
                err_invalid_src_addr: None,
                err_sec: None,
                err_fcs: None,
                err_cca: Some(12),
                err_abort: Some(13),
                err_busy_channel: Some(14),
                err_other: None,
                ..Default::default()
            }),
            mac_rx: Some(MacCounters {
                total: Some(100),
                unicast: Some(101),
                broadcast: Some(102),
                ack_requested: None,
                acked: None,
                no_ack_requested: None,
                data: Some(103),
                data_poll: Some(104),
                beacon: Some(105),
                beacon_request: Some(106),
                other: Some(107),
                address_filtered: Some(108),
                retries: None,
                direct_max_retry_expiry: None,
                indirect_max_retry_expiry: None,
                dest_addr_filtered: Some(109),
                duplicated: Some(110),
                err_no_frame: Some(111),
                err_unknown_neighbor: Some(112),
                err_invalid_src_addr: Some(113),
                err_sec: Some(114),
                err_fcs: Some(115),
                err_cca: None,
                err_abort: None,
                err_busy_channel: None,
                err_other: Some(116),
                ..Default::default()
            }),
            coex_tx: Some(CoexCounters {
                requests: Some(200),
                grant_immediate: Some(201),
                grant_wait: Some(202),
                grant_wait_activated: Some(203),
                grant_wait_timeout: Some(204),
                grant_deactivated_during_request: Some(205),
                delayed_grant: Some(206),
                avg_delay_request_to_grant_usec: Some(207),
                ..Default::default()
            }),
            coex_rx: Some(CoexCounters {
                requests: Some(300),
                grant_immediate: Some(301),
                grant_wait: Some(302),
                grant_wait_activated: Some(303),
                grant_wait_timeout: Some(304),
                grant_deactivated_during_request: Some(305),
                delayed_grant: Some(306),
                avg_delay_request_to_grant_usec: Some(307),
                grant_none: Some(308),
                ..Default::default()
            }),
            coex_saturated: Some(false),
            ..Default::default()
        });
    }

    async fn reset_counters(&self) -> ZxResult<AllCounters> {
        return Ok(AllCounters::default());
    }

    async fn register_on_mesh_prefix(&self, _net: OnMeshPrefix) -> ZxResult<()> {
        Ok(())
    }

    async fn unregister_on_mesh_prefix(&self, _net: Ipv6Subnet) -> ZxResult<()> {
        Ok(())
    }

    async fn register_external_route(&self, _net: ExternalRoute) -> ZxResult<()> {
        Ok(())
    }

    async fn unregister_external_route(&self, _net: Ipv6Subnet) -> ZxResult<()> {
        Ok(())
    }

    async fn get_local_on_mesh_prefixes(&self) -> ZxResult<Vec<OnMeshPrefix>> {
        Ok(vec![])
    }

    async fn get_local_external_routes(&self) -> ZxResult<Vec<ExternalRoute>> {
        Ok(vec![])
    }

    async fn make_joinable(&self, _duration: zx::MonotonicDuration, _port: u16) -> ZxResult<()> {
        Ok(())
    }

    async fn get_feature_config(&self) -> ZxResult<FeatureConfig> {
        Ok(FeatureConfig::default())
    }

    async fn update_feature_config(&self, _config: FeatureConfig) -> ZxResult<()> {
        Ok(())
    }

    async fn get_capabilities(&self) -> ZxResult<Capabilities> {
        Ok(Capabilities::default())
    }

    fn start_ephemeral_key(&self, _lifetime: u32) -> ZxResult<Vec<u8>> {
        Ok(vec![])
    }

    fn stop_ephemeral_key(&self, _retain_active_session: bool) -> ZxResult {
        Ok(())
    }
}
