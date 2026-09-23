// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_metrics::MetricsLogger;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_inspect::{self as inspect, NumericProperty};
use fuchsia_inspect_derive::Inspect;
use fuchsia_sync::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::packets::{BrowseLevel, MediaPlayerItem};
use fuchsia_bluetooth::profile::avrcp::{AvrcpControllerFeatures, AvrcpTargetFeatures};

pub const METRICS_NODE_NAME: &str = "metrics";

impl From<BrowseLevel>
    for bt_metrics::AvrcpTargetDistinctPeerPlayerCapabilitiesMetricDimensionBrowsing
{
    fn from(src: BrowseLevel) -> Self {
        match src {
            BrowseLevel::NotBrowsable => Self::Unsupported,
            BrowseLevel::OnlyBrowsableWhenAddressed => Self::SupportedWhenAddressed,
            BrowseLevel::Browsable => Self::Supported,
        }
    }
}

/// Cumulative metrics node for the supported features of discovered peers.
#[derive(Default, Inspect)]
struct PeerSupportMetrics {
    target_peers_supporting_browsing: inspect::UintProperty,
    target_peers_supporting_cover_art: inspect::UintProperty,
    controller_peers_supporting_browsing: inspect::UintProperty,
    controller_peers_supporting_cover_art: inspect::UintProperty,

    /// Note: The distinct_* metrics are not holistic measures. Because these values are not
    /// persisted across reboots, they should not be used for more than informational purposes
    /// about a specific period of time.
    distinct_target_peers_supporting_browsing: inspect::UintProperty,
    distinct_target_peers_supporting_cover_art: inspect::UintProperty,
    // If a target peer has any players that support browsing without the
    // `OnlyBrowsableWhenAddressed` feature, this should be incremented by 1.
    distinct_target_peers_with_players_support_browsing: inspect::UintProperty,
    // If a target peer has any players that support browsing with `OnlyBrowsableWhenAddressed`
    // feature, this should be incremented by 1.
    distinct_target_peers_with_players_only_browsable_when_addressed: inspect::UintProperty,
    // If a target peer has no players that support browsing in any way, this should be incremented by 1.
    distinct_target_peers_with_no_players_support_browsing: inspect::UintProperty,
    distinct_controller_peers_supporting_browsing: inspect::UintProperty,
    distinct_controller_peers_supporting_cover_art: inspect::UintProperty,

    /// Internal collections to track uniqueness. These aren't included in the inspect
    /// representation.
    #[inspect(skip)]
    tg_browse_peers: HashSet<PeerId>,
    #[inspect(skip)]
    tg_players_highest_browse_level_peers: HashMap<PeerId, BrowseLevel>,
    #[inspect(skip)]
    tg_cover_art_peers: HashSet<PeerId>,
    #[inspect(skip)]
    ct_browse_peers: HashSet<PeerId>,
    #[inspect(skip)]
    ct_cover_art_peers: HashSet<PeerId>,
}

#[derive(Default, Inspect)]
struct MetricsNodeInner {
    /// Total number of connection errors.
    connection_errors: inspect::UintProperty,

    /// Total number of control connections.
    control_connections: inspect::UintProperty,

    /// Total number of browse connections.
    browse_connections: inspect::UintProperty,

    /// Total number of unique peers discovered since the last reboot.
    distinct_peers: inspect::UintProperty,
    #[inspect(skip)]
    distinct_peers_set: HashSet<PeerId>,

    /// Total number of control channel connection collisions. Namely, when an inbound
    /// and outbound connection occur at roughly the same time.
    control_channel_collisions: inspect::UintProperty,

    /// Total number of browse channel connection collisions. Namely, when an inbound
    /// and outbound connection occur at roughly the same time.
    browse_channel_collisions: inspect::UintProperty,

    /// Metrics for features supported by discovered peers.
    support_node: PeerSupportMetrics,

    /// The inspect node for this object.
    inspect_node: inspect::Node,
}

impl MetricsNodeInner {
    /// Checks if the `id` is a newly discovered peer and updates the metric count.
    fn check_distinct_peer(&mut self, id: PeerId) {
        if self.distinct_peers_set.insert(id) {
            let _ = self.distinct_peers.add(1);
        }
    }

    fn controller_supporting_browsing(&mut self, id: PeerId) {
        let _ = self.support_node.controller_peers_supporting_browsing.add(1);
        if self.support_node.ct_browse_peers.insert(id) {
            let _ = self.support_node.distinct_controller_peers_supporting_browsing.add(1);
        }
    }

    fn controller_supporting_cover_art(&mut self, id: PeerId) {
        let _ = self.support_node.controller_peers_supporting_cover_art.add(1);
        if self.support_node.ct_cover_art_peers.insert(id) {
            let _ = self.support_node.distinct_controller_peers_supporting_cover_art.add(1);
        }
    }

    fn target_supporting_browsing(&mut self, id: PeerId) {
        let _ = self.support_node.target_peers_supporting_browsing.add(1);
        if self.support_node.tg_browse_peers.insert(id) {
            let _ = self.support_node.distinct_target_peers_supporting_browsing.add(1);
        }
    }

    fn target_supporting_cover_art(&mut self, id: PeerId) {
        let _ = self.support_node.target_peers_supporting_cover_art.add(1);
        if self.support_node.tg_cover_art_peers.insert(id) {
            let _ = self.support_node.distinct_target_peers_supporting_cover_art.add(1);
        }
    }

    /// Returns true if the current support level is different from previously
    /// logged value to indicate highest support level update for this peer.
    /// Otherwise, return false.
    fn target_players_support_browsing(&mut self, id: PeerId, support_level: BrowseLevel) -> bool {
        let prev_val =
            self.support_node.tg_players_highest_browse_level_peers.insert(id, support_level);
        // Only increment metrics for peer player browsability if we have not
        // yet or if the new highest support level is different from previously
        // seen value.
        if prev_val.is_none() || prev_val.unwrap() != support_level {
            match support_level {
                BrowseLevel::Browsable => {
                    let _ = self
                        .support_node
                        .distinct_target_peers_with_players_support_browsing
                        .add(1);
                }
                BrowseLevel::OnlyBrowsableWhenAddressed => {
                    let _ = self
                        .support_node
                        .distinct_target_peers_with_players_only_browsable_when_addressed
                        .add(1);
                }
                BrowseLevel::NotBrowsable => {
                    let _ = self
                        .support_node
                        .distinct_target_peers_with_no_players_support_browsing
                        .add(1);
                }
            };
            return true;
        }
        return false;
    }
}

/// An object, backed by inspect, used to track cumulative metrics for the AVRCP component.
#[derive(Clone, Default, Inspect)]
pub struct MetricsNode {
    #[inspect(forward)]
    inner: Arc<Mutex<MetricsNodeInner>>,
    /// Logger for reporting component metrics to Cobalt.
    metrics_logger: MetricsLogger,
}

impl MetricsNode {
    pub fn new_peer(&self, id: PeerId) {
        self.inner.lock().check_distinct_peer(id);
    }

    pub fn connection_error(&self) {
        let _ = self.inner.lock().connection_errors.add(1);
    }

    pub fn control_connection(&self) {
        let _ = self.inner.lock().control_connections.add(1);
    }

    pub fn browse_connection(&self) {
        let _ = self.inner.lock().browse_connections.add(1);
    }

    pub fn control_collision(&self) {
        let _ = self.inner.lock().control_channel_collisions.add(1);
    }

    pub fn browse_collision(&self) {
        let _ = self.inner.lock().browse_channel_collisions.add(1);
    }

    pub fn with_cobalt_logger(mut self, metrics_logger: MetricsLogger) -> Self {
        self.metrics_logger = metrics_logger;
        self
    }

    /// A peer supporting the controller role is discovered.
    pub fn controller_features(&self, id: PeerId, features: AvrcpControllerFeatures) {
        let mut inner = self.inner.lock();
        if features.contains(AvrcpControllerFeatures::SUPPORTSBROWSING) {
            inner.controller_supporting_browsing(id);
        }

        if features.supports_cover_art() {
            inner.controller_supporting_cover_art(id);
        }
    }

    /// A peer supporting the target role is discovered.
    pub fn target_features(&self, id: PeerId, features: AvrcpTargetFeatures) {
        let mut inner = self.inner.lock();
        if features.contains(AvrcpTargetFeatures::SUPPORTSBROWSING) {
            inner.target_supporting_browsing(id);
        }

        if features.contains(AvrcpTargetFeatures::SUPPORTSCOVERART) {
            inner.target_supporting_cover_art(id);
        }
    }

    /// A target role peer's players are fetched for feature examination.
    pub fn target_player_features(&self, id: PeerId, players: &[MediaPlayerItem]) {
        if players.len() == 0 {
            return;
        }

        // See AVRCP v1.6.2 section 6.10.2.1 for complete features.
        // Browsing (Octet 7 bit 3).
        // OnlyBrowsableWhenAddressed (Octet 7 bit 7).
        let highest_browse_support = players.iter().map(|p| p.browse_level()).max().unwrap();
        let support_updated: bool;
        {
            let mut inner = self.inner.lock();
            support_updated = inner.target_players_support_browsing(id, highest_browse_support);
        }
        if !support_updated {
            // No need to log to cobalt if the highest browse support level
            // hasn't changed for this peer.
            return;
        }

        // For now we only log for "browsing" dimension (first dimension).
        let browse_cap : bt_metrics::AvrcpTargetDistinctPeerPlayerCapabilitiesMetricDimensionBrowsing = highest_browse_support.into();
        let cobalt_event_codes = [browse_cap as u32, 0, 0];
        self.metrics_logger.log_occurrence(
            bt_metrics::AVRCP_TARGET_DISTINCT_PEER_PLAYER_CAPABILITIES_METRIC_ID,
            cobalt_event_codes.to_vec(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::PlaybackStatus;
    use async_utils::PollExt;
    use bt_metrics::respond_to_metrics_req_for_test;
    use diagnostics_assertions::assert_data_tree;
    use fidl_fuchsia_metrics::{MetricEventLoggerMarker, MetricEventPayload};
    use fuchsia_inspect_derive::WithInspect;
    use futures::stream::StreamExt;
    use {fidl_fuchsia_bluetooth_avrcp as fidl_avrcp, fuchsia_async as fasync};

    #[fuchsia::test]
    async fn multiple_peers_connection_updates_to_shared_node() {
        let inspect = inspect::Inspector::default();

        let metrics = MetricsNode::default().with_inspect(inspect.root(), "metrics").unwrap();

        let (id1, metrics1) = (PeerId(2220), metrics.clone());
        let (id2, metrics2) = (PeerId(7982), metrics.clone());

        // Default inspect tree.
        assert_data_tree!(inspect, root: {
            metrics: contains {
                connection_errors: 0u64,
                control_connections: 0u64,
                browse_connections: 0u64,
                distinct_peers: 0u64,
                control_channel_collisions: 0u64,
                browse_channel_collisions: 0u64,
            }
        });

        // Peer #1 is discovered but encounters a connection error.
        metrics1.new_peer(id1);
        metrics1.connection_error();
        // Peer #2 successfully connects.
        metrics1.new_peer(id2);
        metrics2.control_connection();
        assert_data_tree!(inspect, root: {
            metrics: contains {
                connection_errors: 1u64,
                control_connections: 1u64,
                browse_connections: 0u64,
                distinct_peers: 2u64,
                control_channel_collisions: 0u64,
                browse_channel_collisions: 0u64,
            }
        });

        // Maybe a faulty peer #1 - but eventually connects.
        metrics1.connection_error();
        metrics1.connection_error();
        metrics1.control_connection();
        assert_data_tree!(inspect, root: {
            metrics: contains {
                connection_errors: 3u64,
                control_connections: 2u64,
                browse_connections: 0u64,
                distinct_peers: 2u64,
                control_channel_collisions: 0u64,
                browse_channel_collisions: 0u64,
            }
        });

        // Peer #1 re-connects, also with a browse channel - identifying the peer again
        // does not update the distinct_peers count.
        metrics1.new_peer(id1);
        metrics1.control_collision();
        metrics1.control_connection();
        metrics1.browse_collision();
        metrics1.browse_connection();
        assert_data_tree!(inspect, root: {
            metrics: contains {
                connection_errors: 3u64,
                control_connections: 3u64,
                browse_connections: 1u64,
                distinct_peers: 2u64,
                control_channel_collisions: 1u64,
                browse_channel_collisions: 1u64,
            }
        });
    }

    #[fuchsia::test]
    async fn controller_peers_service_updates() {
        let inspect = inspect::Inspector::default();

        let metrics = MetricsNode::default().with_inspect(inspect.root(), "metrics").unwrap();

        // Peer #1 doesn't support anything.
        let id1 = PeerId(1102);
        let tg_service1 = AvrcpTargetFeatures::empty();
        let ct_service1 = AvrcpControllerFeatures::empty();
        metrics.controller_features(id1, ct_service1);
        metrics.target_features(id1, tg_service1);
        assert_data_tree!(inspect, root: {
            metrics: contains {
                target_peers_supporting_browsing: 0u64,
                distinct_target_peers_supporting_browsing: 0u64,
                target_peers_supporting_cover_art: 0u64,
                distinct_target_peers_supporting_cover_art: 0u64,
                controller_peers_supporting_browsing: 0u64,
                distinct_controller_peers_supporting_browsing: 0u64,
                controller_peers_supporting_cover_art: 0u64,
                distinct_controller_peers_supporting_cover_art: 0u64,
            }
        });

        // Peer #2 supports everything.
        let id2 = PeerId(1102);
        let ct_service2 = AvrcpControllerFeatures::all();
        let tg_service2 = AvrcpTargetFeatures::all();
        metrics.controller_features(id2, ct_service2);
        metrics.target_features(id2, tg_service2);
        assert_data_tree!(inspect, root: {
            metrics: contains {
                target_peers_supporting_browsing: 1u64,
                distinct_target_peers_supporting_browsing: 1u64,
                target_peers_supporting_cover_art: 1u64,
                distinct_target_peers_supporting_cover_art: 1u64,
                controller_peers_supporting_browsing: 1u64,
                distinct_controller_peers_supporting_browsing: 1u64,
                controller_peers_supporting_cover_art: 1u64,
                distinct_controller_peers_supporting_cover_art: 1u64,
            }
        });
        // Peer #2 is re-discovered. Distinct counts shouldn't change.
        metrics.controller_features(id2, ct_service2);
        metrics.target_features(id2, tg_service2);
        assert_data_tree!(inspect, root: {
            metrics: contains {
                target_peers_supporting_browsing: 2u64,
                distinct_target_peers_supporting_browsing: 1u64,
                target_peers_supporting_cover_art: 2u64,
                distinct_target_peers_supporting_cover_art: 1u64,
                controller_peers_supporting_browsing: 2u64,
                distinct_controller_peers_supporting_browsing: 1u64,
                controller_peers_supporting_cover_art: 2u64,
                distinct_controller_peers_supporting_cover_art: 1u64,
            }
        });
    }

    const NOT_BROWSABLE: fidl_avrcp::PlayerFeatureBits = fidl_avrcp::PlayerFeatureBits::empty();
    const BROWSABLE: fidl_avrcp::PlayerFeatureBits = fidl_avrcp::PlayerFeatureBits::BROWSING;
    const BROWSABLE_WHEN_ADDRESSED: fidl_avrcp::PlayerFeatureBits =
        fidl_avrcp::PlayerFeatureBits::from_bits_truncate(
            fidl_avrcp::PlayerFeatureBits::BROWSING.bits()
                | fidl_avrcp::PlayerFeatureBits::ONLY_BROWSABLE_WHEN_ADDRESSED.bits(),
        );

    #[fuchsia::test]
    fn target_peer_players_update() {
        let mut exec = fasync::TestExecutor::new();

        let inspect = inspect::Inspector::default();

        let (proxy, mut receiver) =
            fidl::endpoints::create_proxy_and_stream::<MetricEventLoggerMarker>();

        let metrics = MetricsNode::default()
            .with_inspect(inspect.root(), "metrics")
            .unwrap()
            .with_cobalt_logger(MetricsLogger::from_proxy(proxy));

        // Peer #1 has no players that support browsing in any way.
        let id1 = PeerId(1101);
        metrics.target_player_features(
            id1,
            &[MediaPlayerItem::new(
                1,
                fidl_avrcp::MajorPlayerType::AUDIO.bits(),
                0,
                PlaybackStatus::Stopped,
                [NOT_BROWSABLE.bits(), 0],
                "player 1".to_string(),
            )],
        );
        assert_data_tree!(@executor exec, inspect, root: {
            metrics: contains {
                distinct_target_peers_with_players_support_browsing: 0u64,
                distinct_target_peers_with_players_only_browsable_when_addressed: 0u64,
                distinct_target_peers_with_no_players_support_browsing: 1u64,
            }
        });
        // Cobalt was logged with `Unsupported` value in browse dimension.
        let log_request = exec.run_until_stalled(&mut receiver.next()).expect("should be ready");
        let got = respond_to_metrics_req_for_test(log_request.unwrap().expect("should be ok"));
        assert_eq!(
            bt_metrics::AVRCP_TARGET_DISTINCT_PEER_PLAYER_CAPABILITIES_METRIC_ID,
            got.metric_id
        );
        assert_eq!(vec![1, 0, 0], got.event_codes);
        assert_eq!(MetricEventPayload::Count(1), got.payload);

        // Try logging with 2 non-browsable players.
        let id1 = PeerId(1101);
        metrics.target_player_features(
            id1,
            &[
                MediaPlayerItem::new(
                    1,
                    fidl_avrcp::MajorPlayerType::AUDIO.bits(),
                    0,
                    PlaybackStatus::Stopped,
                    [NOT_BROWSABLE.bits(), 0],
                    "player 1".to_string(),
                ),
                MediaPlayerItem::new(
                    2,
                    fidl_avrcp::MajorPlayerType::AUDIO.bits(),
                    0,
                    PlaybackStatus::Stopped,
                    [NOT_BROWSABLE.bits(), 0],
                    "player 2".to_string(),
                ),
            ],
        );
        // Metrics is not incremented since the highest level of support
        // for peer 1 hasn't changed since last time.
        assert_data_tree!(@executor exec, inspect, root: {
            metrics: contains {
                distinct_target_peers_with_players_support_browsing: 0u64,
                distinct_target_peers_with_players_only_browsable_when_addressed: 0u64,
                distinct_target_peers_with_no_players_support_browsing: 1u64,
            }
        });
        // Cobalt was not logged.
        let _ = exec.run_until_stalled(&mut receiver.next()).expect_pending("should be pending");

        // Peer #2 has one player that supports browsing with
        // OnlyBrowsableWhenAddressed feature bit and another that supports
        // browsing without it.
        let id2 = PeerId(1102);
        metrics.target_player_features(
            id2,
            &[
                MediaPlayerItem::new(
                    1,
                    fidl_avrcp::MajorPlayerType::AUDIO.bits(),
                    0,
                    PlaybackStatus::Stopped,
                    [BROWSABLE_WHEN_ADDRESSED.bits(), 0],
                    "player 1".to_string(),
                ),
                MediaPlayerItem::new(
                    2,
                    fidl_avrcp::MajorPlayerType::AUDIO.bits(),
                    0,
                    PlaybackStatus::Stopped,
                    [BROWSABLE.bits(), 0],
                    "player 2".to_string(),
                ),
            ],
        );
        // `..._support_browsing` metric is incremented since it's the highest
        // support level for player 2.
        assert_data_tree!(@executor exec, inspect, root: {
            metrics: contains {
                distinct_target_peers_with_players_support_browsing: 1u64,
                distinct_target_peers_with_players_only_browsable_when_addressed: 0u64,
                distinct_target_peers_with_no_players_support_browsing: 1u64,
            }
        });
        // Cobalt was logged with `Supported` value in browse dimension.
        let log_request = exec.run_until_stalled(&mut receiver.next()).expect("should be ready");
        let got = respond_to_metrics_req_for_test(log_request.unwrap().expect("should be ok"));
        assert_eq!(
            bt_metrics::AVRCP_TARGET_DISTINCT_PEER_PLAYER_CAPABILITIES_METRIC_ID,
            got.metric_id
        );
        assert_eq!(vec![2, 0, 0], got.event_codes);
        assert_eq!(MetricEventPayload::Count(1), got.payload);

        // Peer #2 available players information changed and the highest level
        // of browsing supported by its players is the one that supports
        // browsing with OnlyBrowsableWhenAddressed feature.
        let id1 = PeerId(1102);
        metrics.target_player_features(
            id1,
            &[
                MediaPlayerItem::new(
                    1,
                    fidl_avrcp::MajorPlayerType::AUDIO.bits(),
                    0,
                    PlaybackStatus::Stopped,
                    [BROWSABLE_WHEN_ADDRESSED.bits(), 0],
                    "player 1".to_string(),
                ),
                MediaPlayerItem::new(
                    2,
                    fidl_avrcp::MajorPlayerType::AUDIO.bits(),
                    0,
                    PlaybackStatus::Stopped,
                    [NOT_BROWSABLE.bits(), 0],
                    "player 2".to_string(),
                ),
            ],
        );
        // `...only_browsable_when_addressed` metric is incremented since the
        // highest support level changed since last time.
        assert_data_tree!(@executor exec, inspect, root: {
            metrics: contains {
                distinct_target_peers_with_players_support_browsing: 1u64,
                distinct_target_peers_with_players_only_browsable_when_addressed: 1u64,
                distinct_target_peers_with_no_players_support_browsing: 1u64,
            }
        });
        // Cobalt was logged with `SupportedWhenAddressed` value in browse dimension.
        let log_request = exec.run_until_stalled(&mut receiver.next()).expect("should be ready");
        let got = respond_to_metrics_req_for_test(log_request.unwrap().expect("should be ok"));
        assert_eq!(
            bt_metrics::AVRCP_TARGET_DISTINCT_PEER_PLAYER_CAPABILITIES_METRIC_ID,
            got.metric_id
        );
        assert_eq!(vec![3, 0, 0], got.event_codes);
        assert_eq!(MetricEventPayload::Count(1), got.payload);
    }
}
