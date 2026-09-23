// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use crate::ot::{BorderAgentEphemeralKeyState, create_ephemeral_key};
use anyhow::Error;
use async_trait::async_trait;
use core::future::ready;
use fidl_fuchsia_lowpan_experimental::{
    AddressOrigin, HistoryTrackerNeighborEvent, HistoryTrackerNetDataEvent,
    HistoryTrackerRouterEvent,
};
use lowpan_driver_common::lowpan_fidl::*;
use lowpan_driver_common::{AsyncConditionWait, Driver as LowpanDriver};
use openthread::ot::SrpServerLeaseInfo;
use otsys::OT_BORDER_AGENT_MAX_EPHEMERAL_KEY_TIMEOUT;
use std::net::Ipv6Addr;
use zx::IoErrorKindExt;

const EPSKC_PORT: u16 = 61632;
const ROUTER_ID_OFFSET: u8 = 10;

/// Helpers for API-related tasks.
impl<OT: Send, NI, BI: Send> OtDriver<OT, NI, BI> {
    /// Helper function for methods that return streams. Allows you
    /// to have an initialization method that returns a lock which can be
    /// held while another stream is running.
    pub(super) fn start_ongoing_stream_process<'a, R, FInit, SStream, L>(
        &'a self,
        init_task: FInit,
        stream: SStream,
        timeout: fasync::MonotonicInstant,
    ) -> BoxStream<'a, ZxResult<R>>
    where
        R: Send + 'a,
        FInit: Send + Future<Output = Result<L, Error>> + 'a,
        SStream: Send + Stream<Item = Result<R, Error>> + 'a,
        L: Send + 'a,
    {
        enum InternalState<'a, R, L> {
            Init(BoxFuture<'a, ZxResult<L>>, BoxStream<'a, ZxResult<R>>),
            Running(L, BoxStream<'a, ZxResult<R>>),
            Done,
        }

        let init_task = init_task
            .map_err(|e| ZxStatus::from(ErrorAdapter(e)))
            .on_timeout(fasync::MonotonicInstant::after(DEFAULT_TIMEOUT), || {
                Err(ZxStatus::TIMED_OUT)
            });

        let stream = stream.map_err(|e| ZxStatus::from(ErrorAdapter(e)));

        futures::stream::unfold(
            InternalState::Init(init_task.boxed(), stream.boxed()),
            move |mut last_state: InternalState<'_, R, L>| async move {
                last_state = match last_state {
                    InternalState::Init(init_task, stream) => {
                        debug!(tag = "api"; "ongoing_stream_process: Initializing. . .");
                        match init_task.await {
                            Ok(lock) => {
                                debug!(tag = "api"; "ongoing_stream_process: Initialized.");
                                InternalState::Running(lock, stream)
                            }
                            Err(err) => {
                                debug!(
                                    tag = "api";
                                    "ongoing_stream_process: Initialization failed: {:?}", err
                                );
                                return Some((Err(err), InternalState::Done));
                            }
                        }
                    }
                    last_state => last_state,
                };

                if let InternalState::Running(lock, mut stream) = last_state {
                    debug!(tag = "api"; "ongoing_stream_process: getting next");
                    if let Some(next) = stream
                        .next()
                        .on_timeout(timeout, move || {
                            error!(tag = "api"; "ongoing_stream_process: Timeout");
                            Some(Err(ZxStatus::TIMED_OUT))
                        })
                        .await
                    {
                        return Some((next, InternalState::Running(lock, stream)));
                    }
                }

                debug!(tag = "api"; "ongoing_stream_process: Done");

                None
            },
        )
        .boxed()
    }
}

/// API-related tasks. Implementation of [`lowpan_driver_common::Driver`].
#[async_trait]
impl<OT, NI, BI> LowpanDriver for OtDriver<OT, NI, BI>
where
    OT: Send + ot::InstanceInterface,
    NI: NetworkInterface,
    BI: BackboneInterface,
{
    async fn provision_network(&self, params: ProvisioningParams) -> ZxResult<()> {
        info!(tag = "api"; "Got \"provision network\" request");
        debug!(tag = "api"; "provision command: {:?}", params);

        // Wait until we are not busy.
        self.wait_for_state(|x| !x.is_busy()).await;

        if params.identity.raw_name.is_none() {
            // We must at least have the network name specified.
            return Err(ZxStatus::INVALID_ARGS);
        }

        if let Some(ref net_type) = params.identity.net_type {
            if !self.is_net_type_supported(net_type.as_str()) {
                error!(
                    tag = "api";
                    "Network type {:?} is not supported by this interface.", net_type
                );
                return Err(ZxStatus::NOT_SUPPORTED);
            }
        };

        let task = async {
            let mut dataset = ot::OperationalDataset::empty();
            let driver_state = self.driver_state.lock();
            let ot_instance = &driver_state.ot_instance;

            // Start with a new blank dataset.
            ot_instance.dataset_create_new_network(&mut dataset)?;

            // Update that dataset with the provisioning parameters.
            dataset.update_from(&params)?;

            // Update OpenThread with the new dataset.
            ot_instance.dataset_set_active(&dataset)?;

            if !ot_instance.is_commissioned() {
                return Err(format_err!(
                    "Set all of the parameters, but we aren't commissioned yet"
                ));
            }

            Ok(())
        };

        self.apply_standard_combinators(task.boxed()).await
    }

    async fn leave_network(&self) -> ZxResult<()> {
        info!(tag = "api"; "Got leave command");

        let task = async {
            {
                let driver_state = self.driver_state.lock();
                let ot_instance = &driver_state.ot_instance;

                ot_instance.thread_set_enabled(false)?;
                ot_instance.ip6_set_enabled(false)?;
                ot_instance.dataset_set_active(&ot::OperationalDataset::empty())?;
                ot_instance.erase_persistent_info()?;

                if ot_instance.is_commissioned() {
                    return Err(format_err!("Unable to fully clear dataset"));
                }
            }

            // Go ahead and make sure that the connectivity state is
            // updated quickly so that we don't cause problems later on.
            self.update_connectivity_state();

            Ok(())
        };

        self.apply_standard_combinators(task.boxed()).await
    }

    async fn set_active(&self, enabled: bool) -> ZxResult<()> {
        info!(tag = "api"; "Got set active command: {:?}", enabled);

        // Wait until we are not busy.
        self.wait_for_state(|x| !x.is_busy()).await;

        self.apply_standard_combinators(self.net_if.set_enabled(enabled).boxed()).await?;

        self.wait_for_state(|x| x.is_active() == enabled).await;

        Ok(())
    }

    async fn get_supported_network_types(&self) -> ZxResult<Vec<String>> {
        // We only support Thread networks.
        Ok(vec![NET_TYPE_THREAD_1_X.to_string()])
    }

    async fn get_supported_channels(&self) -> ZxResult<Vec<ChannelInfo>> {
        let supported_channel_mask =
            self.driver_state.lock().ot_instance.get_supported_channel_mask();

        Ok(supported_channel_mask
            .into_iter()
            .map(|x| ChannelInfo {
                // TODO: Actually calculate all of the fields for channel info struct
                id: Some(x.to_string()),
                index: Some(u16::from(x)),
                masked_by_regulatory_domain: Some(false),
                ..Default::default()
            })
            .collect())
    }

    fn watch_device_state(&self) -> BoxStream<'_, ZxResult<DeviceState>> {
        futures::stream::unfold(
            None,
            move |last_state: Option<(DeviceState, AsyncConditionWait<'_>)>| {
                async move {
                    let mut snapshot;
                    if let Some((last_state, mut condition)) = last_state {
                        // The first item has already been emitted by the stream, so
                        // we need to wait for changes before we emit more.
                        loop {
                            // This loop is where our stream waits for
                            // the next change to the device state.

                            // Wait for the driver state change condition to unblock.
                            condition.await;

                            // Set up the condition for the next iteration.
                            condition = self.driver_state_change.wait();

                            // Wait until we are ready.
                            self.wait_for_state(DriverState::is_initialized).await;

                            snapshot = self.driver_state.lock().get_current_device_state();
                            if snapshot != last_state {
                                break;
                            }
                        }

                        // We start out with our "delta" being a clone of the
                        // current device state. We will then selectively clear
                        // the fields it contains so that only fields that have
                        // changed are represented.
                        let mut delta = snapshot.clone();

                        if last_state.connectivity_state == snapshot.connectivity_state {
                            delta.connectivity_state = None;
                        }

                        if last_state.role == snapshot.role {
                            delta.role = None;
                        }

                        Some((Ok(delta), Some((snapshot, condition))))
                    } else {
                        // This is the first item being emitted from the stream,
                        // so we end up emitting the current device state and
                        // setting ourselves up for the next iteration.
                        let condition = self.driver_state_change.wait();
                        snapshot = self.driver_state.lock().get_current_device_state();
                        Some((Ok(snapshot.clone()), Some((snapshot, condition))))
                    }
                }
            },
        )
        .boxed()
    }

    fn watch_identity(&self) -> BoxStream<'_, ZxResult<Identity>> {
        futures::stream::unfold(
            None,
            move |last_state: Option<(Identity, AsyncConditionWait<'_>)>| {
                async move {
                    let mut snapshot;
                    if let Some((last_state, mut condition)) = last_state {
                        // The first copy of the identity has already been emitted
                        // by the stream, so we need to wait for changes before we emit more.
                        loop {
                            // This loop is where our stream waits for
                            // the next change to the identity.

                            // Wait for the driver state change condition to unblock.
                            condition.await;

                            // Set up the condition for the next iteration.
                            condition = self.driver_state_change.wait();

                            // Wait until we are ready.
                            self.wait_for_state(DriverState::is_initialized).await;

                            // Grab our identity snapshot and make sure it is actually different.
                            snapshot = self.driver_state.lock().get_current_identity();
                            if snapshot != last_state {
                                break;
                            }
                        }
                        Some((Ok(snapshot.clone()), Some((snapshot, condition))))
                    } else {
                        // This is the first item being emitted from the stream,
                        // so we end up emitting the current identity and
                        // setting ourselves up for the next iteration.
                        let condition = self.driver_state_change.wait();
                        snapshot = self.driver_state.lock().get_current_identity();
                        Some((Ok(snapshot.clone()), Some((snapshot, condition))))
                    }
                }
            },
        )
        .boxed()
    }

    fn form_network(
        &self,
        params: ProvisioningParams,
    ) -> BoxStream<'_, ZxResult<Result<ProvisioningProgress, ProvisionError>>> {
        info!(tag = "api"; "Got \"form network\" request");
        debug!(tag = "api"; "form command: {:?}", params);

        ready(Err(ZxStatus::NOT_SUPPORTED)).into_stream().boxed()
    }

    fn join_network(
        &self,
        params: JoinParams,
    ) -> BoxStream<'_, ZxResult<Result<ProvisioningProgress, ProvisionError>>> {
        info!(tag = "api"; "Got \"join network\" request");
        debug!(tag = "api"; "join command: {:?}", params);

        match params {
            JoinParams::JoinerParameter(joiner_params) => self.joiner_start(joiner_params),
            _ => {
                error!("join network: provision params not yet supported");
                ready(Err(ZxStatus::INVALID_ARGS)).into_stream().boxed()
            }
        }
    }

    async fn get_credential(&self) -> ZxResult<Option<Credential>> {
        info!(tag = "api"; "Got get credential command");
        let driver_state = self.driver_state.lock();
        let ot_instance = &driver_state.ot_instance;
        let mut operational_dataset = Default::default();

        ot_instance.dataset_get_active(&mut operational_dataset)?;

        Ok(operational_dataset
            .get_network_key()
            .map(ot::NetworkKey::to_vec)
            .map(Credential::NetworkKey))
    }

    fn start_energy_scan(
        &self,
        params: &EnergyScanParameters,
    ) -> BoxStream<'_, ZxResult<Vec<fidl_fuchsia_lowpan_device::EnergyScanResult>>> {
        info!(tag = "api"; "Got energy scan command: {:?}", params);

        let driver_state = self.driver_state.lock();
        let ot_instance = &driver_state.ot_instance;

        let all_channels = ot_instance.get_supported_channel_mask();

        let channels = if let Some(channels) = params.channels.as_ref() {
            ot::ChannelMask::try_from(channels)
        } else {
            Ok(all_channels)
        };

        let dwell_time_ms: u64 = params.dwell_time_ms.unwrap_or(DEFAULT_SCAN_DWELL_TIME_MS).into();

        let dwell_time = std::time::Duration::from_millis(dwell_time_ms);

        let timeout = fasync::MonotonicInstant::after(
            zx::MonotonicDuration::from_millis(
                (dwell_time_ms * all_channels.len() as u64).try_into().unwrap(),
            ) + SCAN_EXTRA_TIMEOUT,
        );

        use futures::channel::mpsc;
        let (sender, receiver) = mpsc::unbounded();

        let init_task = async move {
            // Wait until we are not busy.
            self.wait_for_state(|x| !x.is_busy()).await;

            self.driver_state.lock().ot_instance.start_energy_scan(
                channels?,
                dwell_time,
                move |x| {
                    trace!(tag = "api"; "energy_scan_callback: Got result {:?}", x);
                    if let Some(x) = x {
                        if sender.unbounded_send(x.clone()).is_err() {
                            // If this is an error then that just means the
                            // other end has been dropped. We really don't care,
                            // not even worth logging.
                        }
                    } else {
                        trace!(tag = "api"; "energy_scan_callback: Closing scan stream");
                        sender.close_channel();

                        // Make sure the rest of the state machine knows we finished scanning.
                        self.driver_state_change.trigger();
                    }
                },
            )?;

            // Make sure the rest of the state machine recognizes that we are scanning.
            self.driver_state_change.trigger();

            Ok(())
        };

        let stream = receiver.map(|x| {
            Ok(vec![EnergyScanResult {
                channel_index: Some(x.channel().into()),
                max_rssi: Some(x.max_rssi().into()),
                ..Default::default()
            }])
        });

        self.start_ongoing_stream_process(init_task, stream, timeout)
    }

    fn start_network_scan(
        &self,
        params: &NetworkScanParameters,
    ) -> BoxStream<'_, ZxResult<Vec<BeaconInfo>>> {
        info!(tag = "api"; "Got network scan command: {:?}", params);

        let driver_state = self.driver_state.lock();
        let ot_instance = &driver_state.ot_instance;

        let all_channels = ot_instance.get_supported_channel_mask();

        let channels = if let Some(channels) = params.channels.as_ref() {
            ot::ChannelMask::try_from(channels)
        } else {
            Ok(all_channels)
        };

        let dwell_time_ms: u64 = DEFAULT_SCAN_DWELL_TIME_MS.into();

        let dwell_time = std::time::Duration::from_millis(dwell_time_ms);

        let timeout = fasync::MonotonicInstant::after(
            zx::MonotonicDuration::from_millis(
                (dwell_time_ms * all_channels.len() as u64).try_into().unwrap(),
            ) + SCAN_EXTRA_TIMEOUT,
        );

        use futures::channel::mpsc;
        let (sender, receiver) = mpsc::unbounded();

        let init_task = async move {
            // Wait until we are not busy.
            self.wait_for_state(|x| !x.is_busy()).await;

            self.driver_state.lock().ot_instance.start_active_scan(
                channels?,
                dwell_time,
                move |x| {
                    trace!(tag = "api"; "active_scan_callback: Got result {:?}", x);
                    if let Some(x) = x {
                        if sender.unbounded_send(x.clone()).is_err() {
                            // If this is an error then that just means the
                            // other end has been dropped. We really don't care,
                            // not even worth logging.
                        }
                    } else {
                        trace!(tag = "api"; "active_scan_callback: Closing scan stream");
                        sender.close_channel();

                        // Make sure the rest of the state machine knows we finished scanning.
                        self.driver_state_change.trigger();
                    }
                },
            )?;

            // Make sure the rest of the state machine recognizes that we are scanning.
            self.driver_state_change.trigger();

            Ok(())
        };

        let stream = receiver.map(|x| Ok(vec![x.into_ext()]));

        self.start_ongoing_stream_process(init_task, stream, timeout)
    }

    async fn reset(&self) -> ZxResult<()> {
        warn!(tag = "api"; "Got API request to reset");
        self.driver_state.lock().ot_instance.reset();
        Ok(())
    }

    async fn get_factory_mac_address(&self) -> ZxResult<MacAddress> {
        let octets =
            self.driver_state.lock().ot_instance.get_factory_assigned_ieee_eui_64().into_array();
        Ok(MacAddress { octets })
    }

    async fn get_current_mac_address(&self) -> ZxResult<MacAddress> {
        let octets = self.driver_state.lock().ot_instance.get_extended_address().into_array();
        Ok(MacAddress { octets })
    }

    async fn get_ncp_version(&self) -> ZxResult<String> {
        Ok(ot::get_version_string().to_string())
    }

    async fn get_current_channel(&self) -> ZxResult<u16> {
        Ok(self.driver_state.lock().ot_instance.get_channel() as u16)
    }

    async fn get_current_rssi(&self) -> ZxResult<i8> {
        Ok(self.driver_state.lock().ot_instance.get_rssi())
    }

    async fn get_partition_id(&self) -> ZxResult<u32> {
        Ok(self.driver_state.lock().ot_instance.get_partition_id())
    }

    async fn get_thread_rloc16(&self) -> ZxResult<u16> {
        Ok(self.driver_state.lock().ot_instance.get_rloc16())
    }

    async fn get_thread_router_id(&self) -> ZxResult<u8> {
        self.get_thread_rloc16().await.map(ot::rloc16_to_router_id)
    }

    async fn send_mfg_command(&self, command: &str) -> ZxResult<String> {
        // For this method we are sending manufacturing commands to the normal
        // OpenThread CLI interface one at a time
        const WAIT_FOR_RESPONSE_TIMEOUT: zx::MonotonicDuration =
            zx::MonotonicDuration::from_seconds(120);

        info!(tag = "api"; "CLI command: {:?}", command);

        let mut cmd = std::string::String::from(command);
        cmd.push('\n');

        let (server_socket_fidl, client_socket_fidl) = fidl::Socket::create_stream();
        self.setup_ot_cli(server_socket_fidl).await?;
        let mut client_socket = fuchsia_async::Socket::from_socket(client_socket_fidl);

        // Flush out any previous response. If we don't do this then we might get
        // unexpected text at the top of the command output, which would be confusing.
        let mut inbound_buffer: Vec<u8> = Vec::new();

        client_socket.read_datagram(&mut inbound_buffer).now_or_never();
        client_socket.write_all(cmd.as_bytes()).await.map_err(|e| e.kind().to_status())?;
        let fut = async {
            loop {
                client_socket.read_datagram(&mut inbound_buffer).await?;
                if let Ok(output) = std::str::from_utf8(&inbound_buffer) {
                    if output.ends_with("Done\r\n")
                        || output.starts_with("Error ")
                        || output.contains("\r\nError ")
                    {
                        // Break early if we are done or there was an error
                        break;
                    }
                }
            }
            Ok(())
        };

        fut.on_timeout(fasync::MonotonicInstant::after(WAIT_FOR_RESPONSE_TIMEOUT), || {
            error!(tag = "api"; "Timeout");
            Err(ZxStatus::TIMED_OUT)
        })
        .await?;

        match std::str::from_utf8(&inbound_buffer) {
            Ok(result) => Ok(result.to_string()),
            Err(_) => Ok("Error: invalid UTF-8 string".to_string()),
        }
    }

    async fn setup_ot_cli(&self, server_socket: fidl::Socket) -> ZxResult<()> {
        info!(tag = "api"; "Got \"setup OT CLI\" request");
        let driver_state = self.driver_state.lock();
        let ot_instance = &driver_state.ot_instance;
        let ot_ctl = &driver_state.ot_ctl;
        ot_ctl
            .replace_client_socket(fuchsia_async::Socket::from_socket(server_socket), ot_instance);
        Ok(())
    }

    async fn replace_mac_address_filter_settings(
        &self,
        _settings: MacAddressFilterSettings,
    ) -> ZxResult<()> {
        return Err(ZxStatus::NOT_SUPPORTED);
    }

    async fn get_mac_address_filter_settings(&self) -> ZxResult<MacAddressFilterSettings> {
        return Err(ZxStatus::NOT_SUPPORTED);
    }

    #[allow(clippy::useless_conversion)]
    async fn get_neighbor_table(&self) -> ZxResult<Vec<NeighborInfo>> {
        Ok(self
            .driver_state
            .lock()
            .ot_instance
            .iter_neighbor_info()
            .map(|x| NeighborInfo {
                mac_address: Some(MacAddress { octets: x.ext_address().into_array() }),
                short_address: Some(x.rloc16()),
                age: Some(
                    fuchsia_async::MonotonicDuration::from_seconds(x.age().into())
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                ),
                is_child: Some(x.is_child()),
                link_frame_count: Some(x.link_frame_counter()),
                mgmt_frame_count: Some(x.mle_frame_counter()),
                last_rssi_in: Some(x.last_rssi() as i32),
                avg_rssi_in: Some(x.average_rssi()),
                lqi_in: Some(x.lqi_in()),
                conntime: Some(
                    fuchsia_async::MonotonicDuration::from_seconds(x.conntime().into())
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                ),
                ..Default::default()
            })
            .collect::<Vec<_>>())
    }

    async fn get_counters(&self) -> ZxResult<AllCounters> {
        let mut ret = AllCounters::default();
        let driver_state = self.driver_state.lock();

        ret.update_from(driver_state.ot_instance.link_get_counters());
        ret.update_from(driver_state.ot_instance.get_ip6_counters());
        ret.update_from(driver_state.ot_instance.get_mle_counters());

        if let Ok(coex_metrics) = driver_state.ot_instance.get_coex_metrics() {
            ret.update_from(&coex_metrics);
        }

        if let Some(counters) = driver_state.ot_instance.border_agent_get_counters() {
            ret.update_from(&counters)
        }

        Ok(ret)
    }

    async fn reset_counters(&self) -> ZxResult<AllCounters> {
        return Err(ZxStatus::NOT_SUPPORTED);
    }

    async fn register_on_mesh_prefix(&self, net: OnMeshPrefix) -> ZxResult<()> {
        info!(tag = "api"; "Got \"register on mesh prefix\" request");
        let prefix = if let Some(subnet) = net.subnet {
            Ok(ot::Ip6Prefix::new(subnet.addr.addr, subnet.prefix_len))
        } else {
            Err(ZxStatus::INVALID_ARGS)
        }?;

        let mut omp = ot::BorderRouterConfig::from_prefix(prefix);

        omp.set_on_mesh(true);

        omp.set_default_route_preference(
            net.default_route_preference.map(ot::RoutePreference::from_ext),
        );

        if let Some(x) = net.slaac_preferred {
            omp.set_preferred(x);
        }

        if let Some(x) = net.slaac_valid {
            omp.set_slaac(x);
        }

        omp.set_stable(net.stable.unwrap_or(true));

        Ok(self.driver_state.lock().ot_instance.add_on_mesh_prefix(&omp).map_err(|e| {
            warn!(tag = "api"; "register_on_mesh_prefix: Error: {:?}", e);
            ZxStatus::from(ErrorAdapter(e))
        })?)
    }

    async fn unregister_on_mesh_prefix(
        &self,
        subnet: fidl_fuchsia_net::Ipv6AddressWithPrefix,
    ) -> ZxResult<()> {
        info!(tag = "api"; "Got \"unregister on mesh prefix\" request");
        let prefix = ot::Ip6Prefix::new(subnet.addr.addr, subnet.prefix_len);

        Ok(self.driver_state.lock().ot_instance.remove_on_mesh_prefix(&prefix).map_err(|e| {
            warn!(tag = "api"; "unregister_on_mesh_prefix: Error: {:?}", e);
            ZxStatus::from(ErrorAdapter(e))
        })?)
    }

    async fn register_external_route(&self, net: ExternalRoute) -> ZxResult<()> {
        info!(tag = "api"; "Got \"register external route\" request");
        let prefix = if let Some(subnet) = net.subnet {
            Ok(ot::Ip6Prefix::new(subnet.addr.addr, subnet.prefix_len))
        } else {
            Err(ZxStatus::INVALID_ARGS)
        }?;

        let mut er = ot::ExternalRouteConfig::from_prefix(prefix);

        if let Some(route_preference) = net.route_preference {
            er.set_route_preference(route_preference.into_ext());
        }

        if let Some(stable) = net.stable {
            er.set_stable(stable);
        }

        Ok(self.driver_state.lock().ot_instance.add_external_route(&er).map_err(|e| {
            warn!(tag = "api"; "register_external_route: Error: {:?}", e);
            ZxStatus::from(ErrorAdapter(e))
        })?)
    }

    async fn unregister_external_route(
        &self,
        subnet: fidl_fuchsia_net::Ipv6AddressWithPrefix,
    ) -> ZxResult<()> {
        info!(tag = "api"; "Got \"unregister external route\" request");
        let prefix = ot::Ip6Prefix::new(subnet.addr.addr, subnet.prefix_len);

        Ok(self.driver_state.lock().ot_instance.remove_external_route(&prefix).map_err(|e| {
            warn!(tag = "api"; "unregister_external_route: Error: {:?}", e);
            ZxStatus::from(ErrorAdapter(e))
        })?)
    }

    async fn get_local_on_mesh_prefixes(
        &self,
    ) -> ZxResult<Vec<lowpan_driver_common::lowpan_fidl::OnMeshPrefix>> {
        Ok(self
            .driver_state
            .lock()
            .ot_instance
            .iter_local_on_mesh_prefixes()
            .map(OnMeshPrefix::from_ext)
            .collect::<Vec<_>>())
    }

    async fn get_local_external_routes(
        &self,
    ) -> ZxResult<Vec<lowpan_driver_common::lowpan_fidl::ExternalRoute>> {
        Ok(self
            .driver_state
            .lock()
            .ot_instance
            .iter_local_external_routes()
            .map(ExternalRoute::from_ext)
            .collect::<Vec<_>>())
    }

    async fn make_joinable(&self, _duration: zx::MonotonicDuration, _port: u16) -> ZxResult<()> {
        warn!(tag = "api"; "make_joinable: NOT_SUPPORTED");
        return Err(ZxStatus::NOT_SUPPORTED);
    }

    async fn get_active_dataset_tlvs(&self) -> ZxResult<Vec<u8>> {
        self.driver_state
            .lock()
            .ot_instance
            .dataset_get_active_tlvs()
            .map(Vec::<u8>::from)
            .or_else(|e| match e {
                ot::Error::NotFound => Ok(vec![]),
                err => Err(err),
            })
            .map_err(|e| {
                warn!(tag = "api"; "get_active_dataset_tlvs: Error: {:?}", e);
                ZxStatus::from(ErrorAdapter(e))
            })
    }

    async fn set_active_dataset_tlvs(&self, dataset: &[u8]) -> ZxResult {
        info!(tag = "api"; "Got \"set active dataset\" request, dataset len:{}", dataset.len());
        let dataset = ot::OperationalDatasetTlvs::try_from_slice(dataset).map_err(|e| {
            warn!(tag = "api"; "set_active_dataset_tlvs: Error: {:?}", e);
            ZxStatus::from(ErrorAdapter(e))
        })?;

        self.driver_state.lock().ot_instance.dataset_set_active_tlvs(&dataset).map_err(|e| {
            warn!(tag = "api"; "set_active_dataset_tlvs: Error: {:?}", e);
            ZxStatus::from(ErrorAdapter(e))
        })
    }

    async fn attach_all_nodes_to(&self, dataset_raw: &[u8]) -> ZxResult<i64> {
        info!(
            tag = "api";
            "Got \"attach all nodes to\" request, raw dataset len:{}",
            dataset_raw.len()
        );
        const DELAY_TIMER_MS: u32 = 300 * 1000;

        let dataset_tlvs =
            ot::OperationalDatasetTlvs::try_from_slice(dataset_raw).map_err(|e| {
                warn!(tag = "api"; "attach_all_nodes_to: WrongSize");
                ZxStatus::from(ErrorAdapter(e))
            })?;

        let mut dataset = dataset_tlvs.try_to_dataset().map_err(|e| {
            warn!(tag = "api"; "attach_all_nodes_to: Unable to parse: {e:?}");
            ZxStatus::from(ErrorAdapter(e))
        })?;

        if !dataset.is_complete() {
            warn!(tag = "api"; "attach_all_nodes_to: Given dataset not complete");
            debug!(tag = "api"; "attach_all_nodes_to: Dataset: {:?}", dataset);
            return Err(ZxStatus::INVALID_ARGS);
        }

        if dataset.get_pending_timestamp().is_some() {
            warn!(tag = "api"; "attach_all_nodes_to: Dataset contains pending timestamp");
            debug!(tag = "api"; "attach_all_nodes_to: Dataset: {:?}", dataset);
            return Err(ZxStatus::INVALID_ARGS);
        }

        if dataset.get_delay().is_some() {
            warn!(tag = "api"; "attach_all_nodes_to: Dataset contains delay timer");
            debug!(tag = "api"; "attach_all_nodes_to: Dataset: {:?}", dataset);
            return Err(ZxStatus::INVALID_ARGS);
        }

        let future = {
            let driver_state = self.driver_state.lock();

            if !driver_state.is_active() {
                warn!(tag = "api"; "attach_all_nodes_to: Driver is not active, cannot continue.");
                return Err(ZxStatus::BAD_STATE);
            }

            if !driver_state.is_ready() {
                // If we aren't ready then we can just set
                // the active TLVs and be done with it.
                return driver_state
                    .ot_instance
                    .dataset_set_active_tlvs(&dataset_tlvs)
                    .map_err(|e| {
                        warn!(tag = "api"; "attach_all_nodes_to: Error: {:?}", e);
                        ZxStatus::from(ErrorAdapter(e))
                    })
                    .map(|()| 0i64);
            }

            dataset.clear();
            dataset.set_pending_timestamp(Some(ot::Timestamp::now()));
            dataset.set_delay(Some(DELAY_TIMER_MS));

            // Transition all devices over to the new dataset.
            driver_state.ot_instance.dataset_send_mgmt_pending_set_async(dataset, dataset_raw)
        };

        future
            .map(|result| match result {
                Ok(Ok(())) => Ok(i64::from(DELAY_TIMER_MS)),
                Ok(Err(e)) => {
                    warn!(tag = "api"; "attach_all_nodes_to: Error: {:?}", e);
                    Err(ZxStatus::from(ErrorAdapter(e)))
                }
                Err(e) => {
                    warn!(tag = "api"; "attach_all_nodes_to: Error: {:?}", e);
                    Err(ZxStatus::from(ErrorAdapter(e)))
                }
            })
            .on_timeout(fasync::MonotonicInstant::after(DEFAULT_TIMEOUT), || {
                error!(tag = "api"; "attach_all_nodes_to: Timeout");
                Err(ZxStatus::TIMED_OUT)
            })
            .await
    }

    async fn meshcop_update_txt_entries(&self, txt_entries: Vec<(String, Vec<u8>)>) -> ZxResult {
        info!(
            tag = "api";
            "Got \"meshcop update txt entries\" request, txt entries size:{}",
            txt_entries.len()
        );

        *self.border_agent_vendor_txt_entries.lock().await = txt_entries;
        self.driver_state.lock().border_agent.trigger_service_update();

        Ok(())
    }

    /// Returns telemetry information of the device.
    async fn get_telemetry(&self) -> ZxResult<Telemetry> {
        let driver_state = self.driver_state.lock();

        let ot = &driver_state.ot_instance;
        let buffer_info = ot.get_buffer_info();

        // Compute total lease times and host/service fresh counts.
        let mut hosts_registration = SrpServerRegistration {
            deleted_count: Some(0),
            fresh_count: Some(0),
            lease_time_total: Some(0),
            key_lease_time_total: Some(0),
            remaining_lease_time_total: Some(0),
            remaining_key_lease_time_total: Some(0),
            ..Default::default()
        };
        let mut services_registration = SrpServerRegistration {
            deleted_count: Some(0),
            fresh_count: Some(0),
            lease_time_total: Some(0),
            key_lease_time_total: Some(0),
            remaining_lease_time_total: Some(0),
            remaining_key_lease_time_total: Some(0),
            ..Default::default()
        };
        let mut srp_server_services: Vec<fidl_fuchsia_lowpan_experimental::SrpServerService> =
            Vec::new();
        let mut srp_server_hosts: Vec<fidl_fuchsia_lowpan_experimental::SrpServerHost> = Vec::new();
        for srp_host in ot.srp_server_hosts() {
            if srp_host.is_deleted() {
                *hosts_registration.deleted_count.get_or_insert(0) += 1;
            } else {
                *hosts_registration.fresh_count.get_or_insert(0) += 1;
                let mut lease_info = SrpServerLeaseInfo::default();
                srp_host.get_lease_info(&mut lease_info);
                *hosts_registration.lease_time_total.get_or_insert(0) +=
                    lease_info.lease().into_nanos();
                *hosts_registration.key_lease_time_total.get_or_insert(0) +=
                    lease_info.key_lease().into_nanos();
                *hosts_registration.remaining_lease_time_total.get_or_insert(0) +=
                    lease_info.remaining_lease().into_nanos();
                *hosts_registration.remaining_key_lease_time_total.get_or_insert(0) +=
                    lease_info.remaining_key_lease().into_nanos();
            }
            // SRP host dump info.
            let host = fidl_fuchsia_lowpan_experimental::SrpServerHost {
                name: Some(match srp_host.full_name_cstr().to_str() {
                    Ok(name) => name.to_string(),
                    Err(e) => {
                        warn!(tag = "api"; "fail to convert the UTF-8 host name {:?}.", e);
                        format!("invalid host name: {:?}", srp_host.full_name_cstr())
                    }
                }),
                deleted: Some(srp_host.is_deleted()),
                addresses: Some(
                    srp_host
                        .addresses()
                        .iter()
                        .take(16) // Limit the number of address to 16 per the FIDL definition.
                        .map(|addr| fidl_fuchsia_net::Ipv6Address { addr: addr.octets() })
                        .collect::<Vec<_>>(),
                ),
                ..Default::default()
            };
            // Ensure the number of registered host dump entries does not exceed the limit (64).
            if srp_server_hosts.len() <= 64 {
                srp_server_hosts.push(host.clone());
            }

            for srp_service in srp_host.services() {
                let mut service = fidl_fuchsia_lowpan_experimental::SrpServerService::default();
                if srp_service.is_deleted() {
                    *services_registration.deleted_count.get_or_insert(0) += 1;
                    service.deleted = Some(true);
                } else {
                    *services_registration.fresh_count.get_or_insert(0) += 1;
                    let mut lease_info = SrpServerLeaseInfo::default();
                    srp_service.get_lease_info(&mut lease_info);
                    *services_registration.lease_time_total.get_or_insert(0) +=
                        lease_info.lease().into_nanos();
                    *services_registration.key_lease_time_total.get_or_insert(0) +=
                        lease_info.key_lease().into_nanos();
                    *services_registration.remaining_lease_time_total.get_or_insert(0) +=
                        lease_info.remaining_lease().into_nanos();
                    *services_registration.remaining_key_lease_time_total.get_or_insert(0) +=
                        lease_info.remaining_key_lease().into_nanos();
                    // SRP service dump info.
                    service.instance_name = Some(match srp_service.instance_name_cstr().to_str() {
                        Ok(name) => name.to_string(),
                        Err(e) => {
                            warn!(tag = "api"; "fail to convert the UTF-8 instance name {:?}.", e);
                            format!("invalid instance name: {:?}", srp_service.instance_name_cstr())
                        }
                    });
                    service.deleted = Some(false);
                    service.subtypes = Some(
                        srp_service
                            .subtypes()
                            .take(6) // Limit the number of subtypes to 6 per the FIDL definition.
                            .filter_map(|service_name| {
                                match ot.parse_label_from_subtype_service_name(service_name) {
                                    Ok(label) => match label.to_str() {
                                        Ok(s) => Some(s.to_string()),
                                        Err(e) => {
                                            warn!(
                                                "Parsed subtype label is not valid UTF-8: {:?}",
                                                e
                                            );
                                            Some(format!("invalid subtype: {:?}", label))
                                        }
                                    },
                                    Err(e) => {
                                        warn!(
                                            "Failed to parse subtype label from {:?}: {:?}",
                                            service_name, e
                                        );
                                        None
                                    }
                                }
                            })
                            .collect(),
                    );
                    service.port = Some(srp_service.port());
                    service.priority = Some(srp_service.priority());
                    service.weight = Some(srp_service.weight());
                    service.ttl = Some(srp_service.ttl().into_nanos());
                    service.lease = Some(lease_info.lease().into_nanos());
                    service.key_lease = Some(lease_info.key_lease().into_nanos());
                    service.txt_data = Some(
                        srp_service
                            .txt_entries()
                            .filter_map(|entry| {
                                let txt = match entry {
                                    Ok(e) => e,
                                    Err(err) => {
                                        warn!("Error iterating TXT entries: {:?}", err);
                                        return None;
                                    }
                                };
                                let key = txt.key()?;
                                let value = txt.value()?;
                                // Ensure the key length does not exceed 64 bytes and the value
                                // length does not exceed 253 bytes (DNS-SD/SRP limits).
                                if key.len() <= 64 && value.len() <= 253 {
                                    Some(fidl_fuchsia_lowpan_experimental::DnsTxtEntry {
                                        key: Some(key.to_string()),
                                        value: Some(value.to_vec()),
                                        ..Default::default()
                                    })
                                } else {
                                    warn!(
                                        "Skipping TXT entry: key length ({}) or value length ({}) exceeds limits.",
                                        key.len(),
                                        value.len()
                                    );
                                    None
                                }
                            })
                            .collect(),
                    );
                    service.host = Some(host.clone());
                }
                srp_server_services.push(service);
            }
        }

        // Get NAT64 telemetry
        let nat64_mappings = ot
            .nat64_get_address_mapping_iterator()
            .map(|mapping| fidl_fuchsia_lowpan_experimental::Nat64Mapping {
                mapping_id: Some(mapping.get_mapping_id()),
                ip4_addr: Some(mapping.get_ipv4_addr().octets().into()),
                ip6_addr: Some(mapping.get_ipv6_addr().octets().into()),
                remaining_time_ms: Some(mapping.get_remaining_time_ms()),
                counters: Some((&mapping.get_protocol_counters()).into_ext()),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        let nat64_error_counter = ot.nat64_get_error_counters();
        let nat64_error_counter_4_to_6 = nat64_error_counter.get_counter_4_to_6();
        let nat64_error_counter_6_to_4 = nat64_error_counter.get_counter_6_to_4();

        let nat64_info = fidl_fuchsia_lowpan_experimental::Nat64Info {
            nat64_state: Some(fidl_fuchsia_lowpan_experimental::BorderRoutingNat64State {
                prefix_manager_state: Some((&ot.nat64_get_prefix_manager_state()).into_ext()),
                translator_state: Some((&ot.nat64_get_translator_state()).into_ext()),
                ..Default::default()
            }),
            nat64_mappings: Some(nat64_mappings),
            nat64_error_counters: Some(fidl_fuchsia_lowpan_experimental::Nat64ErrorCounters {
                unknown: Some(fidl_fuchsia_lowpan_experimental::Nat64PacketCounters {
                    ipv4_to_ipv6_packets: Some(
                        nat64_error_counter_4_to_6[otsys::OT_NAT64_DROP_REASON_UNKNOWN as usize],
                    ),
                    ipv6_to_ipv4_packets: Some(
                        nat64_error_counter_6_to_4[otsys::OT_NAT64_DROP_REASON_UNKNOWN as usize],
                    ),
                    ..Default::default()
                }),
                illegal_packet: Some(fidl_fuchsia_lowpan_experimental::Nat64PacketCounters {
                    ipv4_to_ipv6_packets: Some(
                        nat64_error_counter_4_to_6
                            [otsys::OT_NAT64_DROP_REASON_ILLEGAL_PACKET as usize],
                    ),
                    ipv6_to_ipv4_packets: Some(
                        nat64_error_counter_6_to_4
                            [otsys::OT_NAT64_DROP_REASON_ILLEGAL_PACKET as usize],
                    ),
                    ..Default::default()
                }),
                unsupported_protocol: Some(fidl_fuchsia_lowpan_experimental::Nat64PacketCounters {
                    ipv4_to_ipv6_packets: Some(
                        nat64_error_counter_4_to_6
                            [otsys::OT_NAT64_DROP_REASON_UNSUPPORTED_PROTO as usize],
                    ),
                    ipv6_to_ipv4_packets: Some(
                        nat64_error_counter_6_to_4
                            [otsys::OT_NAT64_DROP_REASON_UNSUPPORTED_PROTO as usize],
                    ),
                    ..Default::default()
                }),
                no_mapping: Some(fidl_fuchsia_lowpan_experimental::Nat64PacketCounters {
                    ipv4_to_ipv6_packets: Some(
                        nat64_error_counter_4_to_6[otsys::OT_NAT64_DROP_REASON_NO_MAPPING as usize],
                    ),
                    ipv6_to_ipv4_packets: Some(
                        nat64_error_counter_6_to_4[otsys::OT_NAT64_DROP_REASON_NO_MAPPING as usize],
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            nat64_protocol_counters: Some((&ot.nat64_get_counters()).into_ext()),
            ..Default::default()
        };

        // Get link metrics manager related fields
        let neighbor_ext_addrs =
            ot.iter_neighbor_info().map(|x| x.ext_address()).collect::<Vec<_>>();

        let link_metrics_entries: Vec<fidl_fuchsia_lowpan_experimental::LinkMetricsEntry> =
            neighbor_ext_addrs
                .iter()
                .filter_map(|x| {
                    if let Ok(y) = ot.link_metrics_manager_get_metrics_value_by_ext_addr(x) {
                        Some(fidl_fuchsia_lowpan_experimental::LinkMetricsEntry {
                            link_margin: Some(y.link_margin()),
                            rssi: Some(y.rssi()),
                            extended_address: Some(x.into_array().to_vec()),
                            ..Default::default()
                        })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

        // Gets the multi radio link information associated with a neighbor.
        let multiradio_neighbor_info: Vec<
            fidl_fuchsia_lowpan_experimental::MultiRadioNeighborInfo,
        > = ot
            .iter_neighbor_info()
            .map(|x| {
                let mut neighbor =
                    fidl_fuchsia_lowpan_experimental::MultiRadioNeighborInfo::default();

                neighbor.extended_address = Some(x.ext_address().into_array().to_vec());
                neighbor.thread_rloc = Some(x.rloc16());

                let mut radio_links: Vec<fidl_fuchsia_lowpan_experimental::RadioLinkInfo> =
                    Vec::new();

                // Limit the number of radio link entries to 5 per the FIDL definition,
                // so far there are only two links: '15.4' and 'TREL'.
                if let Ok(y) = ot.multi_radio_get_neighbor_info(&x.ext_address()) {
                    if y.is_ieee_802_15_4_supported() && radio_links.len() <= 5 {
                        radio_links.push(fidl_fuchsia_lowpan_experimental::RadioLinkInfo {
                            link_type: Some("15.4".to_string()),
                            preference: Some(y.ieee_802_15_4_preference()),
                            ..Default::default()
                        });
                    }
                    if y.is_trel_supported() && radio_links.len() <= 5 {
                        radio_links.push(fidl_fuchsia_lowpan_experimental::RadioLinkInfo {
                            link_type: Some("TREL".to_string()),
                            preference: Some(y.trel_preference()),
                            ..Default::default()
                        });
                    }
                }

                neighbor.radio_link_info = Some(radio_links);

                neighbor
            })
            .collect::<Vec<_>>();

        // Get border agent counters.
        let border_agent_counters =
            driver_state.ot_instance.border_agent_get_counters().as_ref().map(|counters| {
                fidl_fuchsia_lowpan_device::BorderAgentCounters::from_ext(counters)
            });

        // Get whether multi-ail (Adjacent Infrastructure Link) scenario is detected or not.
        let multi_ail_detected = driver_state.ot_instance.border_routing_is_multi_ail_detected();

        // Get the extended pan id of current Thread network.
        let extended_pan_id = u64::from_ne_bytes(ot.get_extended_pan_id().into_array());

        // Get the list of peer BRs found in Network Data entries.
        let border_routing_peers = ot
            .border_routing_peer_get_iterator()
            .take(64) // Limit the number of peers to 64 per the FIDL definition.
            .map(|peer| fidl_fuchsia_lowpan_experimental::BorderRoutingPeer {
                thread_rloc: Some(peer.rloc16().into()),
                age: Some(
                    fuchsia_async::MonotonicDuration::from_seconds(peer.age().into())
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                ),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        // Get the list of discovered routers by Border Routing Manager on the infrastructure link.
        let border_routing_routers = ot
            .border_routing_router_get_iterator()
            .take(64) // Limit the number of routers to 64 per the FIDL definition.
            .map(|router| fidl_fuchsia_lowpan_experimental::BorderRoutingRouter {
                address: Some(Ipv6Addr::from(router.address().octets()).to_string()),
                duration_since_last_update: Some(
                    fuchsia_async::MonotonicDuration::from_millis(
                        router.msec_since_last_update().into(),
                    )
                    .into_nanos()
                    .try_into()
                    .unwrap(),
                ),
                age: Some(
                    fuchsia_async::MonotonicDuration::from_seconds(router.age().into())
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                ),
                managed_address_config_flag: Some(router.managed_address_config_flag()),
                other_config_flag: Some(router.other_config_flag()),
                snac_router_flag: Some(router.snac_router_flag()),
                is_local_device: Some(router.is_local_device()),
                is_reachable: Some(router.is_reachable()),
                is_peer_br: Some(router.is_peer_br()),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        // Get the list of discovered prefixes by Border Routing Manager on the infrastructure link.
        let border_routing_prefixes = ot
            .border_routing_prefix_table_get_iterator()
            .take(fidl_fuchsia_lowpan_experimental::MAX_NEIGHBOR_INSPECT_ENTRIES as usize)
            .map(|prefix| fidl_fuchsia_lowpan_experimental::BorderRoutingPrefixTable {
                router: Some(fidl_fuchsia_lowpan_experimental::BorderRoutingRouter {
                    address: Some(Ipv6Addr::from(prefix.router().address().octets()).to_string()),
                    duration_since_last_update: Some(
                        fuchsia_async::MonotonicDuration::from_millis(
                            prefix.router().msec_since_last_update().into(),
                        )
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                    ),
                    age: Some(
                        fuchsia_async::MonotonicDuration::from_seconds(
                            prefix.router().age().into(),
                        )
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                    ),
                    managed_address_config_flag: Some(
                        prefix.router().managed_address_config_flag(),
                    ),
                    other_config_flag: Some(prefix.router().other_config_flag()),
                    snac_router_flag: Some(prefix.router().snac_router_flag()),
                    is_local_device: Some(prefix.router().is_local_device()),
                    is_reachable: Some(prefix.router().is_reachable()),
                    is_peer_br: Some(prefix.router().is_peer_br()),
                    ..Default::default()
                }),
                prefix: Some(prefix.prefix().to_string()),
                is_on_link: Some(prefix.is_on_link()),
                duration_since_last_update: Some(
                    fuchsia_async::MonotonicDuration::from_millis(
                        prefix.msec_since_last_update().into(),
                    )
                    .into_nanos()
                    .try_into()
                    .unwrap(),
                ),
                valid_lifetime: Some(prefix.valid_lifetime()),
                preference: Some(prefix.route_preference() as i8),
                preferred_lifetime: Some(prefix.preferred_lifetime()),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        // Get the list of discovered Recursive DNS Server (RDNSS) addresses by Border Routing
        // Manager on the infrastructure link.
        let border_routing_rdnsses = ot
            .border_routing_rdnss_get_iterator()
            .take(fidl_fuchsia_lowpan_experimental::MAX_NEIGHBOR_INSPECT_ENTRIES as usize)
            .map(|rdnss| fidl_fuchsia_lowpan_experimental::BorderRoutingRdnss {
                router: Some(fidl_fuchsia_lowpan_experimental::BorderRoutingRouter {
                    address: Some(Ipv6Addr::from(rdnss.router().address().octets()).to_string()),
                    duration_since_last_update: Some(
                        fuchsia_async::MonotonicDuration::from_millis(
                            rdnss.router().msec_since_last_update().into(),
                        )
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                    ),
                    age: Some(
                        fuchsia_async::MonotonicDuration::from_seconds(rdnss.router().age().into())
                            .into_nanos()
                            .try_into()
                            .unwrap(),
                    ),
                    managed_address_config_flag: Some(rdnss.router().managed_address_config_flag()),
                    other_config_flag: Some(rdnss.router().other_config_flag()),
                    snac_router_flag: Some(rdnss.router().snac_router_flag()),
                    is_local_device: Some(rdnss.router().is_local_device()),
                    is_reachable: Some(rdnss.router().is_reachable()),
                    is_peer_br: Some(rdnss.router().is_peer_br()),
                    ..Default::default()
                }),
                address: Some(fidl_fuchsia_net::Ipv6Address { addr: rdnss.address().octets() }),
                duration_since_last_update: Some(
                    fuchsia_async::MonotonicDuration::from_millis(
                        rdnss.msec_since_last_update().into(),
                    )
                    .into_nanos()
                    .try_into()
                    .unwrap(),
                ),
                lifetime: Some(rdnss.lifetime()),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        // Get the active dataset.
        let mut active_dataset = fidl_fuchsia_lowpan_experimental::OperationalDataset::default();
        let mut operational_dataset = Default::default();
        match ot.dataset_get_active(&mut operational_dataset) {
            Ok(_) => {
                if let Some(x) = operational_dataset.get_active_timestamp() {
                    active_dataset.active_timestamp = Some(
                        fuchsia_async::MonotonicDuration::from_seconds(x.as_secs() as i64)
                            .into_nanos()
                            .try_into()
                            .unwrap(),
                    );
                }
                if let Some(x) = operational_dataset.get_network_name() {
                    active_dataset.network_name = Some(x.to_vec());
                }
                if let Some(x) = operational_dataset.get_extended_pan_id() {
                    active_dataset.extended_pan_id = Some(x.to_vec());
                }
                if let Some(x) = operational_dataset.get_mesh_local_prefix() {
                    active_dataset.mesh_local_prefix = Some(x.octets().to_vec());
                }
                if let Some(x) = operational_dataset.get_pan_id() {
                    active_dataset.pan_id = Some(x.into());
                }
                if let Some(x) = operational_dataset.get_channel() {
                    active_dataset.channel = Some(x.into());
                }
                if let Some(x) = operational_dataset.get_channel_mask() {
                    active_dataset.channel_mask = Some(x.into());
                }
                if let Some(x) = operational_dataset.get_security_policy() {
                    let mut policy = fidl_fuchsia_lowpan_experimental::SecurityPolicy::default();
                    policy.rotation_time = Some(x.get_rotation_time_in_hours());
                    policy.obtain_network_key_enabled = Some(x.is_obtain_network_key_enabled());
                    policy.native_commissioning_enabled = Some(x.is_native_commissioning_enabled());
                    policy.routers_enabled = Some(x.is_routers_enabled());
                    policy.external_commissioning_enabled =
                        Some(x.is_external_commissioning_enabled());
                    policy.autonomous_enrollment_enabled =
                        Some(x.is_autonomous_enrollment_enabled());
                    policy.network_key_provisioning_enabled =
                        Some(x.is_network_key_provisioning_enabled());
                    policy.toble_link_enabled = Some(x.is_toble_link_enabled());
                    policy.nonccm_routers_enabled = Some(x.is_non_ccm_routers_enabled());
                    policy.version_threshold_for_routing =
                        Some(x.get_version_threshold_for_routing());
                    active_dataset.security_policy = Some(policy);
                }
                // TODO: implement the "Wake-up channel".
            }
            Err(e) => {
                warn!("Could not retrieve active dataset: {:?}", e);
            }
        }

        // Get the information regarding all active routers within the Thread network.
        let max_router_id = ot.get_max_router_id();
        let mut router_info: Vec<fidl_fuchsia_lowpan_experimental::RouterInfo> = Vec::new();
        for i in 0..max_router_id {
            if let Ok(x) = ot.get_router_info(i.into()) {
                // Limit the number of routers to 64 per the FIDL definition.
                if router_info.len() > 64 {
                    break;
                }
                router_info.push(fidl_fuchsia_lowpan_experimental::RouterInfo {
                    extended_address: Some(x.get_ext_address().into_array().to_vec()),
                    thread_rloc: Some(x.get_rloc16()),
                    router_id: Some(x.get_router_id()),
                    next_hop: Some(x.get_next_hop()),
                    path_cost: Some(x.get_path_cost()),
                    link_quality_in: Some(x.get_link_quality_in()),
                    link_quality_out: Some(x.get_link_quality_out()),
                    age: Some(
                        fuchsia_async::MonotonicDuration::from_seconds(x.get_age().into())
                            .into_nanos()
                            .try_into()
                            .unwrap(),
                    ),
                    link_established: Some(x.get_link_established()),
                    ..Default::default()
                });
            }
        }

        // Get the full network data from received from the Leader.
        let on_mesh_prefixes = ot
            .iter_on_mesh_prefixes()
            .take(32) // Limit the number of routers to 32 per the FIDL definition.
            .map(|config| fidl_fuchsia_lowpan_experimental::BorderRouterConfig {
                prefix: Some(config.prefix().to_string()),
                preference: Some(config.preference() as i8),
                preferred: Some(config.is_preferred()),
                slaac: Some(config.is_slaac()),
                dhcp: Some(config.is_dhcp()),
                configure: Some(config.is_configure()),
                default_route: Some(config.is_default_route()),
                on_mesh: Some(config.is_on_mesh()),
                stable: Some(config.is_stable()),
                nd_dns: Some(config.is_nd_dns()),
                dp: Some(config.is_domain_prefix()),
                rloc16: Some(config.rloc16()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let external_routes = ot
            .iter_external_routes()
            .take(32) // Limit the number of routers to 32 per the FIDL definition.
            .map(|route| fidl_fuchsia_lowpan_experimental::ExternalRouteConfig {
                prefix: Some(route.prefix().to_string()),
                rloc16: Some(route.rloc16()),
                preference: Some(route.route_preference() as i8),
                nat64: Some(route.is_nat64()),
                stable: Some(route.is_stable()),
                next_hop_is_this_device: Some(route.is_next_hop_this_device()),
                adv_pio: Some(route.is_adv_pio()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let services = ot
            .iter_services()
            .take(32) // Limit the number of routers to 32 per the FIDL definition.
            .map(|service| fidl_fuchsia_lowpan_experimental::ServiceConfig {
                service_id: Some(service.service_id()),
                enterprise_number: Some(service.enterprise_number()),
                service_data_length: Some(service.service_data_len()),
                service_data: Some(service.service_data().to_vec()),
                server_config: Some(fidl_fuchsia_lowpan_experimental::ServerConfig {
                    stable: Some(service.server_config().is_stable()),
                    server_data_length: Some(service.server_config().server_data_len()),
                    server_data: Some(service.server_config().server_data().to_vec()),
                    rloc16: Some(service.server_config().rloc16()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let lowpan_contexts_info = ot
            .iter_lowpan_contexts_info()
            .take(32) // Limit the number of routers to 32 per the FIDL definition.
            .map(|info| fidl_fuchsia_lowpan_experimental::LowpanContextInfo {
                context_id: Some(info.context_id()),
                compress_flag: Some(info.compress_flag()),
                stable: Some(info.is_stable()),
                prefix: Some(info.prefix().to_string()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let mut commissioning_dataset = Default::default();
        ot.net_data_get_commissioning_dataset(&mut commissioning_dataset);

        // Get the history tracker related information.
        let net_info_history = ot
            .history_tracker_net_info_history_get_iterator()
            .take(fidl_fuchsia_lowpan_experimental::MAX_THREAD_NET_INFO_HISTORY_ENTRIES as usize)
            .map(|(info, entry_age)| fidl_fuchsia_lowpan_experimental::ThreadNetworkInfoEntry {
                age: Some(
                    fuchsia_async::MonotonicDuration::from_millis(entry_age.into())
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                ),
                role: Some(match info.role() {
                    ot::DeviceRole::Disabled => Role::Detached,
                    ot::DeviceRole::Detached => Role::Detached,
                    ot::DeviceRole::Child => Role::EndDevice,
                    ot::DeviceRole::Router => Role::Router,
                    ot::DeviceRole::Leader => Role::Leader,
                }),
                mode: Some(fidl_fuchsia_lowpan_experimental::ThreadLinkMode {
                    rx_on_when_idle: Some(info.mode().rx_on_while_idle()),
                    device_type: Some(info.mode().is_ftd()),
                    network_data: Some(info.mode().full_network_data()),
                    ..Default::default()
                }),
                rloc16: Some(info.rloc16()),
                partition_id: Some(info.partition_id()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let neighbor_info_history = ot
            .history_tracker_neighbor_history_get_iterator()
            .take(fidl_fuchsia_lowpan_experimental::MAX_THREAD_NEIGHBOR_HISTORY_ENTRIES as usize)
            .map(|(neighbor, entry_age)| {
                fidl_fuchsia_lowpan_experimental::ThreadNeighborInfoEntry {
                    age: Some(
                        fuchsia_async::MonotonicDuration::from_millis(entry_age.into())
                            .into_nanos()
                            .try_into()
                            .unwrap(),
                    ),
                    is_child: Some(neighbor.is_child()),
                    event: Some(match neighbor.event() {
                        openthread::ot::HistoryTrackerNeighborEvent::Added => {
                            HistoryTrackerNeighborEvent::Added
                        }
                        openthread::ot::HistoryTrackerNeighborEvent::Removed => {
                            HistoryTrackerNeighborEvent::Removed
                        }
                        openthread::ot::HistoryTrackerNeighborEvent::Changed => {
                            HistoryTrackerNeighborEvent::Changed
                        }
                        openthread::ot::HistoryTrackerNeighborEvent::Restoring => {
                            HistoryTrackerNeighborEvent::Restoring
                        }
                    }),
                    extended_address: Some(neighbor.ext_address().into_array().to_vec()),
                    rloc16: Some(neighbor.rloc16()),
                    mode: Some(fidl_fuchsia_lowpan_experimental::ThreadLinkMode {
                        rx_on_when_idle: Some(neighbor.rx_on_while_idle()),
                        device_type: Some(neighbor.full_thread_device()),
                        network_data: Some(neighbor.full_network_data()),
                        ..Default::default()
                    }),
                    avg_rssi: Some(neighbor.avg_rssi().into()),
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
        let router_info_history = ot
            .history_tracker_router_history_get_iterator()
            .take(fidl_fuchsia_lowpan_experimental::MAX_THREAD_ROUTER_HISTORY_ENTRIES as usize)
            .map(|(router, entry_age)| fidl_fuchsia_lowpan_experimental::ThreadRouterInfoEntry {
                age: Some(
                    fuchsia_async::MonotonicDuration::from_millis(entry_age.into())
                        .into_nanos()
                        .try_into()
                        // SAFE: `entry_age` is a u32 representing milliseconds, which safely
                        // fits within the destination i64 without any risk of overflowing.
                        .unwrap(),
                ),
                event: Some(match router.event() {
                    openthread::ot::HistoryTrackerRouterEvent::Added => {
                        HistoryTrackerRouterEvent::Added
                    }
                    openthread::ot::HistoryTrackerRouterEvent::Removed => {
                        HistoryTrackerRouterEvent::Removed
                    }
                    openthread::ot::HistoryTrackerRouterEvent::NextHopChanged => {
                        HistoryTrackerRouterEvent::NextHopChanged
                    }
                    openthread::ot::HistoryTrackerRouterEvent::PathCostChanged => {
                        HistoryTrackerRouterEvent::PathCostChanged
                    }
                }),
                router_id: Some(router.router_id()),
                router_rloc16: Some((router.router_id() as u16) << ROUTER_ID_OFFSET),
                next_hop_id: Some(router.next_hop()),
                next_hop_rloc16: Some((router.next_hop() as u16) << ROUTER_ID_OFFSET),
                old_path_cost: Some(router.old_path_cost()),
                new_path_cost: Some(router.path_cost()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let prefix_info_history = ot
            .history_tracker_on_mesh_prefix_history_get_iterator()
            .take(
                fidl_fuchsia_lowpan_experimental::MAX_THREAD_NET_DATA_PREFIX_HISTORY_ENTRIES
                    as usize,
            )
            .map(|(prefix, entry_age)| {
                fidl_fuchsia_lowpan_experimental::ThreadNetDataPrefixInfoEntry {
                    age: Some(
                        fuchsia_async::MonotonicDuration::from_millis(entry_age.into())
                            .into_nanos()
                            .try_into()
                            .unwrap(),
                    ),
                    event: Some(match prefix.event() {
                        openthread::ot::HistoryTrackerNetDataEvent::Added => {
                            HistoryTrackerNetDataEvent::Added
                        }
                        openthread::ot::HistoryTrackerNetDataEvent::Removed => {
                            HistoryTrackerNetDataEvent::Removed
                        }
                    }),
                    on_mesh_prefix: Some(fidl_fuchsia_lowpan_experimental::BorderRouterConfig {
                        prefix: Some(prefix.prefix().prefix().to_string()),
                        preference: Some(prefix.prefix().preference() as i8),
                        preferred: Some(prefix.prefix().is_preferred()),
                        slaac: Some(prefix.prefix().is_slaac()),
                        dhcp: Some(prefix.prefix().is_dhcp()),
                        configure: Some(prefix.prefix().is_configure()),
                        default_route: Some(prefix.prefix().is_default_route()),
                        on_mesh: Some(prefix.prefix().is_on_mesh()),
                        stable: Some(prefix.prefix().is_stable()),
                        nd_dns: Some(prefix.prefix().is_nd_dns()),
                        dp: Some(prefix.prefix().is_domain_prefix()),
                        rloc16: Some(prefix.prefix().rloc16()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
        let route_info_history = ot
            .history_tracker_external_route_history_get_iterator()
            .take(
                fidl_fuchsia_lowpan_experimental::MAX_THREAD_NET_DATA_ROUTE_HISTORY_ENTRIES
                    as usize,
            )
            .map(|(route, entry_age)| {
                fidl_fuchsia_lowpan_experimental::ThreadNetDataRouteInfoEntry {
                    age: Some(
                        fuchsia_async::MonotonicDuration::from_millis(entry_age.into())
                            .into_nanos()
                            .try_into()
                            .unwrap(),
                    ),
                    event: Some(match route.event() {
                        openthread::ot::HistoryTrackerNetDataEvent::Added => {
                            HistoryTrackerNetDataEvent::Added
                        }
                        openthread::ot::HistoryTrackerNetDataEvent::Removed => {
                            HistoryTrackerNetDataEvent::Removed
                        }
                    }),
                    external_route: Some(fidl_fuchsia_lowpan_experimental::ExternalRouteConfig {
                        prefix: Some(route.route().prefix().to_string()),
                        rloc16: Some(route.route().rloc16()),
                        preference: Some(route.route().route_preference() as i8),
                        nat64: Some(route.route().is_nat64()),
                        stable: Some(route.route().is_stable()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();

        // Get the list of IPv6 unicast addresses assigned to the Thread interface.
        let ipaddrs = ot
            .ip6_get_unicast_addresses()
            .take(fidl_fuchsia_lowpan_experimental::MAX_IPV6_UNICAST_ADDRS as usize)
            .map(|addr| fidl_fuchsia_lowpan_experimental::NetifAddress {
                address: Some(fidl_fuchsia_net::Ipv6Address { addr: addr.addr().octets() }),
                prefix_length: Some(addr.prefix_len()),
                origin: match addr.address_origin() {
                    openthread::ot::AddressOrigin::THREAD => Some(AddressOrigin::Thread),
                    openthread::ot::AddressOrigin::SLAAC => Some(AddressOrigin::Slaac),
                    openthread::ot::AddressOrigin::DHCPV6 => Some(AddressOrigin::Dhcpv6),
                    openthread::ot::AddressOrigin::MANUAL => Some(AddressOrigin::Manual),
                    _ => None,
                },
                preferred: Some(addr.is_preferred()),
                valid: Some(addr.is_valid()),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        // Get the list of IPv6 multicast addresses assigned to the Thread interface.
        let ipmaddrs = ot
            .ip6_get_multicast_addresses()
            .take(fidl_fuchsia_lowpan_experimental::MAX_IPV6_MULTICAST_ADDRS as usize)
            .map(|addr| fidl_fuchsia_net::Ipv6Address { addr: addr.addr().octets() })
            .collect::<Vec<_>>();

        // Get the list of TREL peers of the Thread border router.
        let trel_peers = ot
            .trel_peer_get_iterator()
            .take(fidl_fuchsia_lowpan_experimental::MAX_NEIGHBOR_INSPECT_ENTRIES as usize)
            .map(|peer| fidl_fuchsia_lowpan_experimental::TrelPeer {
                extended_address: Some(peer.ext_address().into_array().to_vec()),
                extended_pan_id: Some(peer.ext_pan_id().into_array().to_vec()),
                sock_address: Some(peer.sock_address().to_string()),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        // Get the list of UDP Sockets.
        let netstat = ot
            .udp_get_sockets()
            .take(fidl_fuchsia_lowpan_experimental::MAX_UDP_SOCKETS as usize)
            .map(|socket| fidl_fuchsia_lowpan_experimental::UdpSocket {
                sock_name: Some(socket.sock_name().to_string()),
                peer_name: Some(socket.peer_name().to_string()),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        // Get the EID-to-RLOC cache entries.
        let eid_cache_entries = ot
            .iter_cache_entry_info()
            .take(fidl_fuchsia_lowpan_experimental::MAX_NEIGHBOR_INSPECT_ENTRIES as usize)
            .map(|entry| fidl_fuchsia_lowpan_experimental::EidCacheEntry {
                target: Some(fidl_fuchsia_net::Ipv6Address { addr: entry.target().octets() }),
                rloc16: Some(entry.rloc16()),
                state: entry.state().into_ext(),
                can_evict: Some(entry.can_evict()),
                ramp_down: Some(entry.ramp_down()),
                valid_last_trans: Some(entry.valid_last_trans()),
                last_trans_time: Some(
                    fuchsia_async::MonotonicDuration::from_seconds(entry.last_trans_time().into())
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                ),
                mesh_local_eid: Some(fidl_fuchsia_net::Ipv6Address {
                    addr: entry.mesh_local_eid().octets(),
                }),
                timeout: Some(
                    fuchsia_async::MonotonicDuration::from_seconds(entry.timeout().into())
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                ),
                retry_delay: Some(
                    fuchsia_async::MonotonicDuration::from_seconds(entry.retry_delay().into())
                        .into_nanos()
                        .try_into()
                        .unwrap(),
                ),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        Ok(Telemetry {
            rssi: Some(ot.get_rssi()),
            partition_id: Some(ot.get_partition_id()),
            stack_version: Some(ot::get_version_string().to_string()),
            rcp_version: Some(ot.radio_get_version_string().to_string()),
            thread_link_mode: Some(ot.get_link_mode().bits()),
            thread_rloc: Some(ot.get_rloc16()),
            thread_router_id: Some(ot::rloc16_to_router_id(ot.get_rloc16())),
            thread_network_data_version: Some(ot.net_data_get_version()),
            thread_stable_network_data_version: Some(ot.net_data_get_stable_version()),
            channel_index: Some(ot.get_channel().into()),
            tx_power: ot.get_transmit_power().ok(),
            thread_network_data: ot.net_data_as_vec(false).ok(),
            thread_stable_network_data: ot.net_data_as_vec(true).ok(),
            thread_border_routing_counters: Some(ot.ip6_get_border_routing_counters().into_ext()),
            srp_server_info: Some(SrpServerInfo {
                state: Some(ot.srp_server_get_state().into_ext()),
                port: match ot.srp_server_get_state().into_ext() {
                    SrpServerState::Disabled => None,
                    _ => Some(ot.srp_server_get_port()),
                },
                address_mode: Some(ot.srp_server_get_address_mode().into_ext()),
                response_counters: Some(ot.srp_server_get_response_counters().into_ext()),
                hosts_registration: Some(hosts_registration),
                services_registration: Some(services_registration),
                hosts: Some(srp_server_hosts),
                services: Some(srp_server_services),
                ..Default::default()
            }),
            dnssd_counters: Some(ot.dnssd_get_counters().into_ext()),
            leader_data: Some((&ot.get_leader_data().ok().unwrap_or_default()).into_ext()),
            uptime: Some(ot.get_uptime().into_nanos()),
            trel_counters: ot.trel_get_counters().map(|x| x.into_ext()),
            trel_peers_info: Some(fidl_fuchsia_lowpan_experimental::TrelPeersInfo {
                num_trel_peers: Some(ot.trel_get_number_of_peers()),
                trel_peers: Some(trel_peers),
                ..Default::default()
            }),
            nat64_info: Some(nat64_info),
            upstream_dns_info: Some(fidl_fuchsia_lowpan_experimental::UpstreamDnsInfo {
                upstream_dns_query_state: match ot.dnssd_upstream_query_is_enabled() {
                    true => Some(fidl_fuchsia_lowpan_experimental::UpstreamDnsQueryState::UpstreamdnsQueryStateEnabled),
                    false => Some(fidl_fuchsia_lowpan_experimental::UpstreamDnsQueryState::UpstreamdnsQueryStateDisabled),
                },
                ..Default::default()
            }),
            dhcp6pd_info: Some(fidl_fuchsia_lowpan_experimental::Dhcp6PdInfo {
                dhcp6pd_state: Some((&ot.border_routing_dhcp6_pd_get_state()).into_ext()),
                pd_processed_ra_info: Some((&ot.border_routing_get_pd_processed_ra_info()).into_ext()),
                hashed_pd_prefix: None,
                ..Default::default()
            }),
            link_metrics_entries: Some(link_metrics_entries),
            border_agent_counters,
            multi_ail_detected: Some(multi_ail_detected),
            extended_pan_id: Some(extended_pan_id),
            border_routing_peers: Some(border_routing_peers),
            border_routing_routers: Some(border_routing_routers),
            border_routing_prefixes: Some(border_routing_prefixes),
            border_routing_rdnsses: Some(border_routing_rdnsses),
            active_dataset: Some(active_dataset),
            multiradio_neighbor_info: Some(multiradio_neighbor_info),
            router_info: Some(router_info),
            network_data: Some(fidl_fuchsia_lowpan_experimental::NetworkData {
                on_mesh_prefixes: Some(on_mesh_prefixes),
                external_routes: Some(external_routes),
                services: Some(services),
                contexts: Some(lowpan_contexts_info),
                commissioning_dataset: Some(fidl_fuchsia_lowpan_experimental::CommissioningDataset {
                    locator: Some(commissioning_dataset.locator()),
                    session_id: Some(commissioning_dataset.session_id()),
                    steering_data: Some(commissioning_dataset.steering_data().to_vec()),
                    joiner_udp_port: Some(commissioning_dataset.joiner_udp_port()),
                    is_locator_set: Some(commissioning_dataset.is_locator_set()),
                    is_session_id_set: Some(commissioning_dataset.is_session_id_set()),
                    is_steering_data_set: Some(commissioning_dataset.is_steering_data_set()),
                    is_joiner_udp_port_set: Some(commissioning_dataset.is_joiner_udp_port_set()),
                    has_extra_tlv: Some(commissioning_dataset.has_extra_tlv()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            history_report: Some(fidl_fuchsia_lowpan_experimental::ThreadHistoryReport {
                net_info_history: Some(net_info_history),
                neighbor_info_history: Some(neighbor_info_history),
                router_info_history: Some(router_info_history),
                prefix_info_history: Some(prefix_info_history),
                route_info_history: Some(route_info_history),
                ..Default::default()
            }),
            ipaddrs: Some(ipaddrs),
            ipmaddrs: Some(ipmaddrs),
            netstat: Some(netstat),
            csl_info: Some(fidl_fuchsia_lowpan_experimental::CslInfo {
                csl_accuracy: Some(ot.get_csl_accuracy()),
                csl_uncertainty: Some(ot.get_csl_uncertainty()),
                ..Default::default()
            }),
            buffer_info: Some((&buffer_info).into_ext()),
            eid_cache_entries: Some(eid_cache_entries),
            cca_threshold: ot.get_cca_threshold().ok(),
            ..Default::default()
        })
    }

    async fn get_feature_config(&self) -> ZxResult<FeatureConfig> {
        let driver_state = self.driver_state.lock();

        let (detailed_logging_enabled, detailed_logging_level) =
            driver_state.detailed_logging.process_detailed_logging_get();
        Ok(FeatureConfig {
            trel_enabled: Some(driver_state.is_trel_enabled()),
            detailed_logging_enabled: Some(detailed_logging_enabled),
            detailed_logging_level: Some(detailed_logging_level.into()),
            dhcpv6_pd_enabled: Some(driver_state.is_dhcpv6_pd_enabled()),
            dns_upstream_query_enabled: Some(
                driver_state.ot_instance.dnssd_upstream_query_is_enabled(),
            ),
            link_metrics_manager_enabled: Some(
                driver_state.ot_instance.link_metrics_manager_is_enabled(),
            ),
            epskc_enabled: Some(driver_state.is_epskc_enabled()),
            ..Default::default()
        })
    }

    async fn update_feature_config(&self, config: FeatureConfig) -> ZxResult<()> {
        info!(tag = "api"; "Got \"update feature config\" request");
        let mut driver_state = self.driver_state.lock();

        if let Some(trel_enabled) = config.trel_enabled {
            driver_state.set_trel_enabled(trel_enabled);
        }

        if let Some(nat64_enabled) = config.nat64_enabled {
            driver_state.ot_instance.nat64_set_enabled(nat64_enabled);
        }

        if let Some(dhcpv6_pd_enabled) = config.dhcpv6_pd_enabled {
            driver_state.set_dhcpv6_pd_enabled(dhcpv6_pd_enabled);
        }

        if let Some(dns_upstream_query_enabled) = config.dns_upstream_query_enabled {
            driver_state.ot_instance.dnssd_upstream_query_set_enabled(dns_upstream_query_enabled);
        }

        if let Some(link_metrics_manager_enabled) = config.link_metrics_manager_enabled {
            driver_state.ot_instance.link_metrics_manager_set_enabled(link_metrics_manager_enabled);
        }

        if let Some(epskc_enabled) = config.epskc_enabled {
            driver_state.set_epskc_enabled(epskc_enabled);
        }

        if let Err(e) = driver_state.detailed_logging.process_detailed_logging_set(
            config.detailed_logging_enabled,
            config.detailed_logging_level.map(|level| level.into()),
        ) {
            warn!("error in process_detailed_logging_set: {:?}", e);
        };

        Ok(())
    }

    async fn get_capabilities(&self) -> ZxResult<Capabilities> {
        // The values are hardcoded here intentionally as we currently have
        // conflicts with configs defined in openthread. Once we resolve that
        // and figure out a long term plan we will use variables tied to the
        // openthread config. Also the config values in openthread are defined at
        // compile time, so they never change for a given software version.

        Ok(Capabilities {
            nat64: Some(true),
            dhcpv6_pd: Some(true),
            epskc: Some(true),
            ..Default::default()
        })
    }

    fn start_ephemeral_key(&self, lifetime: u32) -> ZxResult<Vec<u8>> {
        let driver_state = self.driver_state.lock();
        let ot_instance = &driver_state.ot_instance;

        // Verify ePSKc is enabled.
        if ot_instance.border_agent_ephemeral_key_get_state()
            == BorderAgentEphemeralKeyState::Disabled
        {
            return Err(zx::Status::BAD_STATE);
        }

        // Verify that the timeout is withing the allowed amount of time.
        if lifetime > OT_BORDER_AGENT_MAX_EPHEMERAL_KEY_TIMEOUT {
            return Err(zx::Status::INVALID_ARGS);
        }

        // Generate ephemeral key.
        let key = create_ephemeral_key().map_err(|e| {
            warn!(tag = "api"; "Ephemeral key generation failed; {:?}", e);
            zx::Status::INTERNAL
        })?;

        // Start using the key.
        ot_instance
            .border_agent_ephemeral_key_start(key.as_c_str(), lifetime, EPSKC_PORT)
            .map_err(|e| {
                warn!("Ephemeral key start failed: {:?}", e);
                zx::Status::INTERNAL
            })?;

        Ok(key.into())
    }

    fn stop_ephemeral_key(&self, retain_active_session: bool) -> ZxResult {
        let driver_state = self.driver_state.lock();
        let ot_instance = &driver_state.ot_instance;

        let curr_state = ot_instance.border_agent_ephemeral_key_get_state();

        match curr_state {
            BorderAgentEphemeralKeyState::Disabled => Err(zx::Status::BAD_STATE),
            BorderAgentEphemeralKeyState::Stopped => Ok(()),
            BorderAgentEphemeralKeyState::Started => {
                ot_instance.border_agent_ephemeral_key_stop();
                Ok(())
            }
            BorderAgentEphemeralKeyState::Connected | BorderAgentEphemeralKeyState::Accepted => {
                if !retain_active_session {
                    ot_instance.border_agent_ephemeral_key_stop();
                    Ok(())
                } else {
                    Err(zx::Status::BAD_STATE)
                }
            }
        }
    }
}
