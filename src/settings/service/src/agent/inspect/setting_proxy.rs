// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The inspect mod defines the [SettingProxyInspectAgent], which is responsible for logging
//! the contents of requests and responses, as well as timestamps and counts to Inspect. Since this
//! activity might happen before agent lifecycle states are communicated (due to agent priority
//! ordering), the [SettingProxyInspectAgent] begins listening to requests immediately after
//! creation.
//!
//! [SettingProxyInspectAgent]: inspect::SettingProxyInspectAgent

use crate::clock;
use fuchsia_async as fasync;
use fuchsia_inspect::{self as inspect, NumericProperty, component};
use fuchsia_inspect_derive::{IValue, Inspect};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
#[cfg(test)]
use futures::channel::mpsc::UnboundedSender;
use settings_common::inspect::event::{Direction, ResponseType, UsageEvent};
use settings_common::trace;
use settings_inspect_utils::joinable_inspect_vecdeque::JoinableInspectVecDeque;
use settings_inspect_utils::managed_inspect_map::ManagedInspectMap;
use settings_inspect_utils::managed_inspect_queue::ManagedInspectQueue;

/// The maximum number of pending requests to store in inspect per setting. There should generally
/// be fairly few of these unless a setting is changing rapidly, so a slightly larger size allows us
/// to avoid dropping requests.
const MAX_PENDING_REQUESTS: usize = 20;

/// The maximum number of unique request + response pairs to store per request type in each setting.
const MAX_REQUEST_RESPONSE_PAIRS: usize = 10;

/// The maximum number of request or response timestamps to store per request + response pair.
const MAX_REQUEST_RESPONSE_TIMESTAMPS: usize = 10;

/// Name of the top-level node under root used to store the contents of requests and responses.
const REQUEST_RESPONSE_NODE_NAME: &str = "requests_and_responses";

/// Name of the top-level node under root used to store request counts.
const RESPONSE_COUNTS_NODE_NAME: &str = "response_counts";

#[derive(Default, Inspect)]
/// Information about response counts to be written to inspect.
struct SettingTypeResponseCountInfo {
    /// Map from the name of the ResponseType variant to a ResponseCountInfo that holds the number
    /// of occurrences of that response.
    #[inspect(forward)]
    response_counts_by_type: ManagedInspectMap<ResponseTypeCount>,
}

#[derive(Default, Inspect)]
/// Information about the number of responses of a given response type.
struct ResponseTypeCount {
    count: inspect::UintProperty,
    inspect_node: inspect::Node,
}

/// Inspect information on the requests and responses of one setting.
#[derive(Inspect)]
struct SettingTypeRequestResponseInfo {
    /// Map from request type to a map containing [RequestResponsePairInfo].
    ///
    /// The first-level map's keys are the enum variant names from [Request::to_inspect], to allow
    /// different request types to be recorded separately. The second-level map's keys are the
    /// concatenation of the debug representation of the request and response and holds up to
    /// [REQUEST_RESPONSE_COUNT] of the most recently seen unique request + response pairs.
    #[inspect(rename = "requests_and_responses")]
    requests_and_responses_by_type: ManagedInspectMap<ManagedInspectMap<RequestResponsePairInfo>>,

    /// Queue of pending requests that have been sent but have not received a response yet.
    pending_requests: ManagedInspectQueue<PendingRequestInspectInfo>,

    /// Inspect node to which this setting's data is written.
    inspect_node: inspect::Node,

    /// Incrementing count for all requests of this setting type.
    ///
    /// The same counter is used across all request types to easily see the order that requests
    /// occurred in.
    #[inspect(skip)]
    count: u64,
}

impl SettingTypeRequestResponseInfo {
    fn new() -> Self {
        Self {
            requests_and_responses_by_type: Default::default(),
            pending_requests: ManagedInspectQueue::<PendingRequestInspectInfo>::new(
                MAX_PENDING_REQUESTS,
            ),
            inspect_node: Default::default(),
            count: 0,
        }
    }
}

/// Information to be written to inspect about a request that has not yet received a response.
#[derive(Debug, Default, Inspect)]
struct PendingRequestInspectInfo {
    /// Debug string representation of the request.
    request: IValue<String>,

    /// The request type of the request, from [Request::to_inspect]. Used to bucket by request type
    /// when recording responses.
    #[inspect(skip)]
    request_type: String,

    /// Time this request was sent, in milliseconds of uptime. Uses the system monotonic clock
    /// (zx_clock_get_monotonic).
    timestamp: IValue<String>,

    /// Request count within the setting for this request.
    #[inspect(skip)]
    count: u64,

    /// Inspect node this request will be recorded at.
    inspect_node: inspect::Node,
}

/// Information about a request and response pair to be written to inspect.
///
/// Timestamps are recorded upon receiving a response, so [request_timestamp] and
/// [response_timestamp] will always be the same length and the timestamps at index N of each array
/// belong to the same request + response round trip.
#[derive(Default, Inspect)]
struct RequestResponsePairInfo {
    /// Debug string representation of the request.
    request: IValue<String>,

    /// Debug string representation of the response.
    response: IValue<String>,

    /// Request count of the most recently received request + response.
    #[inspect(skip)]
    request_count: u64,

    /// List of timestamps at which this request was seen.
    ///
    /// Timestamps are recorded as milliseconds of system uptime. Uses the system monotonic clock
    /// (zx_clock_get_monotonic).
    request_timestamps: IValue<JoinableInspectVecDeque>,

    /// List of timestamps at which this response was seen.
    ///
    /// Timestamps are recorded as milliseconds of system uptime. Uses the system monotonic clock
    /// (zx_clock_get_monotonic).
    response_timestamps: IValue<JoinableInspectVecDeque>,

    /// Inspect node at which this info is stored.
    inspect_node: inspect::Node,
}

impl RequestResponsePairInfo {
    fn new(request: String, response: String, count: u64) -> Self {
        Self {
            request: IValue::new(request),
            response: IValue::new(response),
            request_count: count,
            request_timestamps: Default::default(),
            response_timestamps: Default::default(),
            inspect_node: Default::default(),
        }
    }
}

/// The SettingProxyInspectAgent is responsible for listening to requests to the setting
/// handlers and recording the requests and responses to Inspect.
pub(crate) struct SettingProxyInspectAgent {
    /// Response type accumulation info.
    response_counts: ManagedInspectMap<SettingTypeResponseCountInfo>,

    /// Information for each setting on requests and responses.
    setting_request_response_info: ManagedInspectMap<SettingTypeRequestResponseInfo>,

    usage_rx: Option<UnboundedReceiver<UsageEvent>>,

    #[cfg(test)]
    done_tx: Option<UnboundedSender<()>>,
}

impl SettingProxyInspectAgent {
    pub fn new(rx: UnboundedReceiver<UsageEvent>) -> Self {
        Self::create_with_node(
            rx,
            component::inspector().root().create_child(REQUEST_RESPONSE_NODE_NAME),
            component::inspector().root().create_child(RESPONSE_COUNTS_NODE_NAME),
            #[cfg(test)]
            None,
        )
    }

    fn create_with_node(
        rx: UnboundedReceiver<UsageEvent>,
        request_response_inspect_node: inspect::Node,
        response_counts_node: inspect::Node,
        #[cfg(test)] done_tx: Option<UnboundedSender<()>>,
    ) -> Self {
        SettingProxyInspectAgent {
            response_counts: ManagedInspectMap::<SettingTypeResponseCountInfo>::with_node(
                response_counts_node,
            ),
            setting_request_response_info:
                ManagedInspectMap::<SettingTypeRequestResponseInfo>::with_node(
                    request_response_inspect_node,
                ),
            usage_rx: Some(rx),
            #[cfg(test)]
            done_tx,
        }
    }

    pub fn initialize(mut self) {
        fasync::Task::local(async move {
            let id = fuchsia_trace::Id::new();
            trace!(id, "setting_proxy_inspect_agent");
            let mut usage_rx = self.usage_rx.take().unwrap();
            while let Some(usage_event) = usage_rx.next().await {
                if let Direction::Request(_) = usage_event.direction {
                    self.process_usage_event(usage_event);
                } else {
                    self.process_usage_response_event(usage_event);
                }
                #[cfg(test)]
                if let Some(done_tx) = &self.done_tx {
                    let _ = done_tx.unbounded_send(());
                }
            }
        })
        .detach();
    }

    fn process_usage_event(&mut self, event: UsageEvent) {
        let timestamp = clock::inspect_format_now();

        // Get or create the info for this setting type.
        let request_response_info = self
            .setting_request_response_info
            .get_or_insert_with(event.setting.to_string(), SettingTypeRequestResponseInfo::new);

        request_response_info.count += 1;

        let Direction::Request(request) = event.direction else {
            panic!("Call process_usage_event with response!");
        };
        let pending_request_info = PendingRequestInspectInfo {
            request: request.into(),
            request_type: format!("{:?}", event.request_type),
            timestamp: timestamp.into(),
            count: event.id,
            inspect_node: inspect::Node::default(),
        };

        let count_key = format!("{:020}", event.id);
        request_response_info.pending_requests.push(&count_key, pending_request_info);
    }

    fn process_usage_response_event(&mut self, event: UsageEvent) {
        let setting_type_str = event.setting.to_string();
        let timestamp = clock::inspect_format_now();
        let Direction::Response(response, response_type) = event.direction else {
            panic!("Called process_usage_response_event with a request!");
        };

        // Update the response counter.
        self.increment_response_count(setting_type_str.clone(), response_type);

        // Find the inspect data for this setting. This should always be present as it's created
        // upon receiving a request, which should happen before the response is recorded.
        let condensed_setting_type_info = self
            .setting_request_response_info
            .map_mut()
            .get_mut(&setting_type_str)
            .expect("Missing info for request");

        let pending_requests = &mut condensed_setting_type_info.pending_requests;

        // Find the position of the pending request with the same request count and remove it. This
        // should generally be the first pending request in the queue if requests are being answered
        // in order.
        let position = match pending_requests.iter_mut().position(|info| info.count == event.id) {
            Some(position) => position,
            None => {
                // We may be unable to find a matching request if requests are piling up faster than
                // responses, as the number of pending requests is limited.
                return;
            }
        };
        let pending =
            pending_requests.items_mut().remove(position).expect("Failed to find pending item");

        // Find the info for this particular request type.
        let request_type_info_map = condensed_setting_type_info
            .requests_and_responses_by_type
            .get_or_insert_with(pending.request_type, || {
                ManagedInspectMap::<RequestResponsePairInfo>::default()
            });

        // Request and response pairs are keyed by the concatenation of the request and response,
        // which uniquely identifies them within a setting.
        let map_key = format!("{:?}{:?}", pending.request, response);

        // Find this request + response pair in the map and remove it, if it's present. While the
        // map key is the request + response concatenated, the key displayed in inspect is the
        // newest request count for that pair. We remove the map entry if it exists so that we can
        // re-insert to update the key displayed in inspect.
        let removed_info = request_type_info_map.map_mut().remove(&map_key);

        let mut info = removed_info.unwrap_or_else(|| {
            RequestResponsePairInfo::new(pending.request.into_inner(), response, pending.count)
        });
        {
            // Update the request and response timestamps. We have borrow from the IValues with
            // as_mut and drop the variables after this scope ends so that the IValues will know to
            // update the values in inspect.
            let mut_requests = &mut info.request_timestamps.as_mut().0;
            let mut_responses = &mut info.response_timestamps.as_mut().0;

            mut_requests.push_back(pending.timestamp.into_inner());
            mut_responses.push_back(timestamp);

            // If there are too many timestamps, remove earlier ones.
            if mut_requests.len() > MAX_REQUEST_RESPONSE_TIMESTAMPS {
                let _ = mut_requests.pop_front();
            }
            if mut_responses.len() > MAX_REQUEST_RESPONSE_TIMESTAMPS {
                let _ = mut_responses.pop_front();
            }
        }

        // Insert into the map, but display the key in inspect as the request count.
        let count_key = format!("{:020}", pending.count);
        let _ = request_type_info_map.insert_with_property_name(map_key, count_key, info);

        // If there are too many entries, find and remove the oldest.
        let num_request_response_pairs = request_type_info_map.map().len();
        if num_request_response_pairs > MAX_REQUEST_RESPONSE_PAIRS {
            // Find the item with the lowest request count, which means it was the oldest request
            // received.
            let mut lowest_count: u64 = u64::MAX;
            let mut lowest_key: Option<String> = None;
            for (key, inspect_info) in request_type_info_map.map() {
                if inspect_info.request_count < lowest_count {
                    lowest_count = inspect_info.request_count;
                    lowest_key = Some(key.clone());
                }
            }

            if let Some(key_to_remove) = lowest_key {
                let _ = request_type_info_map.map_mut().remove(&key_to_remove);
            }
        }
    }

    fn increment_response_count(&mut self, setting_type_str: String, response_type: ResponseType) {
        // Get the response count info for the setting type, creating a new info object
        // if it doesn't exist in the map yet.
        let response_count_info = self
            .response_counts
            .get_or_insert_with(setting_type_str, SettingTypeResponseCountInfo::default);

        // Get the count for the response type, creating a new count if it doesn't exist
        // in the map yet, then increment the response count
        let response_count = response_count_info
            .response_counts_by_type
            .get_or_insert_with(format!("{response_type:?}"), ResponseTypeCount::default);
        let _ = response_count.count.add(1u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{TreeAssertion, assert_data_tree};
    use futures::channel::mpsc;
    use settings_common::inspect::event::RequestType;
    use zx::MonotonicInstant;

    // Verifies that request + response pairs with the same value and request type are grouped
    // together.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_grouped_responses() {
        // Set the clock so that timestamps can be controlled.
        clock::mock::set(MonotonicInstant::from_nanos(0));

        let inspector = inspect::Inspector::default();
        let condense_node = inspector.root().create_child(REQUEST_RESPONSE_NODE_NAME);
        let response_counts_node = inspector.root().create_child(RESPONSE_COUNTS_NODE_NAME);

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = SettingProxyInspectAgent::create_with_node(
            rx,
            condense_node,
            response_counts_node,
            Some(done_tx),
        );
        agent.initialize();

        // Send a request to turn off auto brightness.
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetDisplayInfo{auto_brightness: Some(false)}".to_string(),
            ),
            id: 0,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 0,
        });
        let _ = done_rx.next().await;

        // Increment clock and send a request to turn on auto brightness.
        clock::mock::set(MonotonicInstant::from_nanos(100));
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetDisplayInfo{auto_brightness: Some(true)}".to_string(),
            ),
            id: 1,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 1,
        });
        let _ = done_rx.next().await;

        // Increment clock and send the same request as the first one. The two should be grouped
        // together.
        clock::mock::set(MonotonicInstant::from_nanos(200));
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetDisplayInfo{auto_brightness: Some(false)}".to_string(),
            ),
            id: 2,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 2,
        });
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: contains {
            requests_and_responses: {
                "Display": {
                    "pending_requests": {},
                    "requests_and_responses": {
                        "Set": {
                            "00000000000000000001": {
                                "request": "SetDisplayInfo{auto_brightness: Some(true)}",
                                "request_timestamps": "0.000000100",
                                "response": "Ok(None)",
                                "response_timestamps": "0.000000100"
                            },
                            "00000000000000000002": {
                                "request": "SetDisplayInfo{auto_brightness: Some(false)}",
                                "request_timestamps": "0.000000000,0.000000200",
                                "response": "Ok(None)",
                                "response_timestamps": "0.000000000,0.000000200"
                            }
                        }
                    }
                }
            },
        });
    }

    // Test that multiple requests of different request types for the same setting records the
    // correct inspect data.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_mixed_request_types() {
        // Set the clock so that timestamps can be controlled.
        clock::mock::set(MonotonicInstant::from_nanos(0));

        let inspector = inspect::Inspector::default();
        let condense_node = inspector.root().create_child(REQUEST_RESPONSE_NODE_NAME);
        let response_counts_node = inspector.root().create_child(RESPONSE_COUNTS_NODE_NAME);

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = SettingProxyInspectAgent::create_with_node(
            rx,
            condense_node,
            response_counts_node,
            Some(done_tx),
        );
        agent.initialize();

        // Interlace different request types to make sure the counter is correct.
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetDisplayInfo{auto_brightness: Some(false)}".to_string(),
            ),
            id: 0,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 0,
        });
        let _ = done_rx.next().await;

        // Set to a different time so that a response can correctly link to its request.
        clock::mock::set(MonotonicInstant::from_nanos(100));
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Get,
            direction: Direction::Request("WatchDisplayInfo".to_string()),
            id: 1,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Get,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 1,
        });
        let _ = done_rx.next().await;

        // Set to a different time so that a response can correctly link to its request.
        clock::mock::set(MonotonicInstant::from_nanos(200));
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetDisplayInfo{auto_brightness: Some(true)}".to_string(),
            ),
            id: 2,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 2,
        });
        let _ = done_rx.next().await;

        clock::mock::set(MonotonicInstant::from_nanos(300));
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Get,
            direction: Direction::Request("WatchDisplayInfo".to_string()),
            id: 3,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Get,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 3,
        });
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: contains {
            requests_and_responses: {
                "Display": {
                    "pending_requests": {},
                    "requests_and_responses": {
                        "Get": {
                            "00000000000000000003": {
                                "request": "WatchDisplayInfo",
                                "request_timestamps": "0.000000100,0.000000300",
                                "response": "Ok(None)",
                                "response_timestamps": "0.000000100,0.000000300"
                            }
                        },
                        "Set": {
                            "00000000000000000000": {
                                "request": "SetDisplayInfo{auto_brightness: Some(false)}",
                                  "request_timestamps": "0.000000000",
                                  "response": "Ok(None)",
                                  "response_timestamps": "0.000000000"
                            },
                            "00000000000000000002": {
                                "request": "SetDisplayInfo{auto_brightness: Some(true)}",
                                "request_timestamps": "0.000000200",
                                "response": "Ok(None)",
                                "response_timestamps": "0.000000200"
                            }
                        }
                    }
                }
            },
            response_counts: {
                "Display": {
                    "OkNone": {
                        count: 4u64,
                    }
                },
            },
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_pending_request() {
        // Set the clock so that timestamps can be controlled.
        clock::mock::set(MonotonicInstant::from_nanos(0));

        let inspector = inspect::Inspector::default();
        let condense_node = inspector.root().create_child(REQUEST_RESPONSE_NODE_NAME);
        let response_counts_node = inspector.root().create_child(RESPONSE_COUNTS_NODE_NAME);

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = SettingProxyInspectAgent::create_with_node(
            rx,
            condense_node,
            response_counts_node,
            Some(done_tx),
        );
        agent.initialize();

        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetDisplayInfo{auto_brightness: Some(false)}".to_string(),
            ),
            id: 0,
        });
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: contains {
            requests_and_responses: {
                "Display": {
                    "pending_requests": {
                        "00000000000000000000": {
                            "request": "SetDisplayInfo{auto_brightness: Some(false)}",
                            "timestamp": "0.000000000",
                        }
                    },
                    "requests_and_responses": {}
                }
            },
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_response_counts_inspect() {
        // Set the clock so that timestamps can be controlled.
        clock::mock::set(MonotonicInstant::from_nanos(0));

        let inspector = inspect::Inspector::default();
        let condense_node = inspector.root().create_child(REQUEST_RESPONSE_NODE_NAME);
        let response_counts_node = inspector.root().create_child(RESPONSE_COUNTS_NODE_NAME);

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = SettingProxyInspectAgent::create_with_node(
            rx,
            condense_node,
            response_counts_node,
            Some(done_tx),
        );
        agent.initialize();
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetDisplayInfo{auto_brightness:Some(false)}".to_string(),
            ),
            id: 0,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 0,
        });
        let _ = done_rx.next().await;

        clock::mock::set(MonotonicInstant::from_nanos(100));
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Get,
            direction: Direction::Request("WatchDisplayInfo".to_string()),
            id: 1,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Get,
            direction: Direction::Response("Ok(Some())".to_string(), ResponseType::OkNone),
            id: 1,
        });
        let _ = done_rx.next().await;

        clock::mock::set(MonotonicInstant::from_nanos(200));
        // For direction arg.
        #[allow(clippy::literal_string_with_formatting_args)]
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request("SetDisplayInfo{auto_brightness:None}".to_string()),
            id: 2,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 2,
        });
        let _ = done_rx.next().await;

        clock::mock::set(MonotonicInstant::from_nanos(300));
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Get,
            direction: Direction::Request("WatchDisplayInfo".to_string()),
            id: 3,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Display",
            request_type: RequestType::Get,
            direction: Direction::Response("Ok(Some())".to_string(), ResponseType::OkNone),
            id: 3,
        });
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: contains {
            response_counts: {
                "Display": {
                    "OkNone": {
                        count: 4u64,
                    },
                },
            },
        });
    }

    // Verifies that old requests are dropped after MAX_REQUEST_RESPONSE_PAIRS are received for a
    // given request + response pair.
    #[fuchsia::test(allow_stalls = false)]
    async fn inspect_queue_test() {
        // Set the clock so that timestamps will always be 0.
        clock::mock::set(MonotonicInstant::from_nanos(0));
        let inspector = inspect::Inspector::default();
        let condense_node = inspector.root().create_child(REQUEST_RESPONSE_NODE_NAME);
        let response_counts_node = inspector.root().create_child(RESPONSE_COUNTS_NODE_NAME);

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = SettingProxyInspectAgent::create_with_node(
            rx,
            condense_node,
            response_counts_node,
            Some(done_tx),
        );
        agent.initialize();

        let _ = tx.unbounded_send(UsageEvent {
            setting: "Intl",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetIntlInfo { \
                    locales: Some([LocaleId { id: \"en-US\" }]), \
                    temperature_unit: Some(Celsius), \
                    time_zone_id: Some(\"UTC\"), \
                    hour_cycle: None \
                }"
                .to_string(),
            ),
            id: 0,
        });
        let _ = done_rx.next().await;
        let _ = tx.unbounded_send(UsageEvent {
            setting: "Intl",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 0,
        });
        let _ = done_rx.next().await;

        // Send one more than the max requests to make sure they get pushed off the end of the
        // queue. The requests must have different values to avoid getting grouped together.
        for i in 0..MAX_REQUEST_RESPONSE_PAIRS + 1 {
            let _ = tx.unbounded_send(UsageEvent {
                setting: "Display",
                request_type: RequestType::Set,
                direction: Direction::Request(format!(
                    "SetDisplayInfo{{manual_brightness_value: Some({})}}",
                    (i as f32) / 100f32
                )),
                id: i as u64,
            });
            let _ = done_rx.next().await;
            let _ = tx.unbounded_send(UsageEvent {
                setting: "Display",
                request_type: RequestType::Set,
                direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
                id: i as u64,
            });
            let _ = done_rx.next().await;
        }

        // Ensures we have INSPECT_REQUESTS_COUNT items and that the queue dropped the earliest one
        // when hitting the limit.
        fn display_subtree_assertion() -> TreeAssertion {
            let mut tree_assertion = TreeAssertion::new("Display", false);
            let mut request_response_assertion = TreeAssertion::new("requests_and_responses", true);
            let mut request_assertion = TreeAssertion::new("Set", true);

            for i in 1..MAX_REQUEST_RESPONSE_PAIRS + 1 {
                // We don't need to set clock here since we don't do exact match.
                request_assertion
                    .add_child_assertion(TreeAssertion::new(&format!("{i:020}"), false));
            }
            request_response_assertion.add_child_assertion(request_assertion);
            tree_assertion.add_child_assertion(request_response_assertion);
            tree_assertion
        }

        assert_data_tree!(inspector, root: contains {
            requests_and_responses: {
                display_subtree_assertion(),
                "Intl": {
                    "pending_requests": {},
                    "requests_and_responses": {
                        "Set": {
                            "00000000000000000000": {
                                "request": "SetIntlInfo { \
                                    locales: Some([LocaleId { id: \"en-US\" }]), \
                                    temperature_unit: Some(Celsius), \
                                    time_zone_id: Some(\"UTC\"), \
                                    hour_cycle: None \
                                }",
                                "request_timestamps": "0.000000000",
                                "response": "Ok(None)",
                                "response_timestamps": "0.000000000"
                            }
                        }
                    }
                }
            },
        });
    }
}
