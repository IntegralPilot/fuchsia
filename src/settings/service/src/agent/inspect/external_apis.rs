// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The external_apis mod defines the [ExternalApisInspectAgent], which is responsible for recording
//! external API requests and responses to Inspect. Since API usages might happen before agent
//! lifecycle states are communicated (due to agent priority ordering), the
//! [ExternalApisInspectAgent] begins listening to requests immediately after creation.
//!
//! Example Inspect structure:
//!
//! ```text
//! {
//!   "fuchsia.external.FakeAPI": {
//!     "pending_calls": {
//!       "00000000000000000005": {
//!         request: "set_manual_brightness(0.7)",
//!         response: "None",
//!         request_timestamp: "19.002716",
//!         response_timestamp: "None",
//!       },
//!     },
//!     "calls": {
//!       "00000000000000000002": {
//!         request: "set_manual_brightness(0.6)",
//!         response: "Ok(None)",
//!         request_timestamp: "18.293864",
//!         response_timestamp: "18.466811",
//!       },
//!       "00000000000000000004": {
//!         request: "set_manual_brightness(0.8)",
//!         response: "Ok(None)",
//!         request_timestamp: "18.788366",
//!         response_timestamp: "18.915355",
//!       },
//!     },
//!   },
//!   ...
//! }
//! ```

use fuchsia_async as fasync;
use fuchsia_inspect::{Node, component};
use fuchsia_inspect_derive::{IValue, Inspect, WithInspect};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
#[cfg(test)]
use futures::channel::mpsc::UnboundedSender;
use settings_common::service_context::ExternalServiceEvent;
use settings_common::trace;
use settings_inspect_utils::managed_inspect_map::ManagedInspectMap;
use settings_inspect_utils::managed_inspect_queue::ManagedInspectQueue;

/// The key for the queue for completed calls per protocol.
const COMPLETED_CALLS_KEY: &str = "completed_calls";

/// The key for the queue for pending calls per protocol.
const PENDING_CALLS_KEY: &str = "pending_calls";

/// The maximum number of recently completed calls that will be kept in
/// inspect per protocol.
const MAX_COMPLETED_CALLS: usize = 10;

/// The maximum number of still pending calls that will be kept in
/// inspect per protocol.
const MAX_PENDING_CALLS: usize = 10;

// TODO(https://fxbug.dev/42060063): Explore reducing size of keys in inspect.
#[derive(Debug, Default, Inspect)]
struct ExternalApiCallInfo {
    /// Node of this info.
    inspect_node: Node,

    /// The request sent via the external API.
    request: IValue<String>,

    /// The response received by the external API.
    response: IValue<String>,

    /// The timestamp at which the request was sent.
    request_timestamp: IValue<String>,

    /// The timestamp at which the response was received.
    response_timestamp: IValue<String>,
}

impl ExternalApiCallInfo {
    fn new(
        request: &str,
        response: &str,
        request_timestamp: &str,
        response_timestamp: &str,
    ) -> Self {
        let mut info = Self::default();
        info.request.iset(request.to_string());
        info.response.iset(response.to_string());
        info.request_timestamp.iset(request_timestamp.to_string());
        info.response_timestamp.iset(response_timestamp.to_string());
        info
    }
}

#[derive(Default, Inspect)]
struct ExternalApiCallsWrapper {
    inspect_node: Node,
    /// The number of total calls that have been made on this protocol.
    count: IValue<u64>,
    /// The most recent pending and completed calls per-protocol.
    calls: ManagedInspectMap<ManagedInspectQueue<ExternalApiCallInfo>>,
    /// The external api event counts.
    event_counts: ManagedInspectMap<IValue<u64>>,
}

/// The [SettingTypeUsageInspectAgent] is responsible for listening to requests to external
/// APIs and recording their requests and responses to Inspect.
pub(crate) struct ExternalApiInspectAgent {
    /// Map from the API call type to its most recent calls.
    ///
    /// Example structure:
    /// ```text
    /// {
    ///   "fuchsia.ui.brightness.Control": {
    ///     "count": 6,
    ///     "calls": {
    ///       "pending_calls": {
    ///         "00000000000000000006": {
    ///           request: "set_manual_brightness(0.7)",
    ///           response: "None",
    ///           request_timestamp: "19.002716",
    ///           response_timestamp: "None",
    ///         },
    ///       }],
    ///       "completed_calls": [{
    ///         "00000000000000000003": {
    ///           request: "set_manual_brightness(0.6)",
    ///           response: "Ok(None)",
    ///           request_timestamp: "18.293864",
    ///           response_timestamp: "18.466811",
    ///         },
    ///         "00000000000000000005": {
    ///           request: "set_manual_brightness(0.8)",
    ///           response: "Ok(None)",
    ///           request_timestamp: "18.788366",
    ///           response_timestamp: "18.915355",
    ///         },
    ///       },
    ///     },
    ///     "event_counts": {
    ///       "Connect": 1,
    ///       "ApiCall": 3,
    ///       "ApiResponse": 2,
    ///     }
    ///   },
    ///   ...
    /// }
    /// ```
    api_calls: ManagedInspectMap<ExternalApiCallsWrapper>,
    event_rx: Option<UnboundedReceiver<ExternalServiceEvent>>,
    #[cfg(test)]
    done_tx: Option<UnboundedSender<()>>,
}

impl ExternalApiInspectAgent {
    pub fn new(event_rx: UnboundedReceiver<ExternalServiceEvent>) -> Self {
        Self::create_with_node(
            event_rx,
            component::inspector().root().create_child("external_apis"),
            #[cfg(test)]
            None,
        )
    }

    /// Creates the `ExternalApiInspectAgent` with the Inspect `node`.
    fn create_with_node(
        event_rx: UnboundedReceiver<ExternalServiceEvent>,
        node: Node,
        #[cfg(test)] done_tx: Option<UnboundedSender<()>>,
    ) -> Self {
        ExternalApiInspectAgent {
            api_calls: ManagedInspectMap::<ExternalApiCallsWrapper>::with_node(node),
            event_rx: Some(event_rx),
            #[cfg(test)]
            done_tx,
        }
    }

    pub fn initialize(mut self) {
        fasync::Task::local({
            async move {
                let id = fuchsia_trace::Id::new();
                trace!(id, "external_api_inspect_agent");
                let mut event_rx = self.event_rx.take().unwrap();
                while let Some(event) = event_rx.next().await {
                    self.process_direct_event(event);
                    #[cfg(test)]
                    if let Some(done_tx) = &self.done_tx {
                        let _ = done_tx.unbounded_send(());
                    }
                }
            }
        })
        .detach();
    }

    fn process_direct_event(&mut self, event: ExternalServiceEvent) {
        match event {
            ExternalServiceEvent::Created(protocol, timestamp) => {
                let count = self.get_count(protocol) + 1;
                let info = ExternalApiCallInfo::new("connect", "none", "none", &timestamp);
                self.add_info(protocol, COMPLETED_CALLS_KEY, "Created", info, count);
            }
            ExternalServiceEvent::ApiCall(protocol, request, timestamp) => {
                let count = self.get_count(protocol) + 1;
                let info = ExternalApiCallInfo::new(&request, "none", &timestamp, "none");
                self.add_info(protocol, PENDING_CALLS_KEY, "ApiCall", info, count);
            }
            ExternalServiceEvent::ApiResponse(
                protocol,
                response,
                request,
                request_timestamp,
                response_timestamp,
            ) => {
                let count = self.get_count(protocol) + 1;
                let info = ExternalApiCallInfo::new(
                    &request,
                    &response,
                    &request_timestamp,
                    &response_timestamp,
                );
                self.remove_pending(protocol, &info);
                self.add_info(protocol, COMPLETED_CALLS_KEY, "ApiResponse", info, count);
            }
            ExternalServiceEvent::ApiError(
                protocol,
                error,
                request,
                request_timestamp,
                error_timestamp,
            ) => {
                let count = self.get_count(protocol) + 1;
                let info = ExternalApiCallInfo::new(
                    &request,
                    &error,
                    &request_timestamp,
                    &error_timestamp,
                );
                self.remove_pending(protocol, &info);
                self.add_info(protocol, COMPLETED_CALLS_KEY, "ApiError", info, count);
            }
            ExternalServiceEvent::Closed(
                protocol,
                request,
                request_timestamp,
                response_timestamp,
            ) => {
                let count = self.get_count(protocol) + 1;
                let info = ExternalApiCallInfo::new(
                    &request,
                    "closed",
                    &request_timestamp,
                    &response_timestamp,
                );
                self.remove_pending(protocol, &info);
                self.add_info(protocol, COMPLETED_CALLS_KEY, "Closed", info, count);
            }
        }
    }

    /// Retrieves the total call count for the given `protocol`. Implicitly
    /// calls `ensure_protocol_exists`.
    fn get_count(&mut self, protocol: &str) -> u64 {
        self.ensure_protocol_exists(protocol);
        *self.api_calls.get(protocol).expect("Wrapper should exist").count
    }

    /// Ensures that an entry exists for the given `protocol`, adding a new one if
    /// it does not yet exist.
    fn ensure_protocol_exists(&mut self, protocol: &str) {
        let _ = self
            .api_calls
            .get_or_insert_with(protocol.to_string(), ExternalApiCallsWrapper::default);
    }

    /// Ensures that an entry exists for the given `protocol`, and `queue_key` adding a
    /// new queue of max size `queue_size` if one does not yet exist. Implicitly calls
    /// `ensure_protocol_exists`.
    fn ensure_queue_exists(&mut self, protocol: &str, queue_key: &'static str, queue_size: usize) {
        self.ensure_protocol_exists(protocol);

        let protocol_map = self.api_calls.get_mut(protocol).expect("Protocol entry should exist");
        let _ = protocol_map.calls.get_or_insert_with(queue_key.to_string(), || {
            ManagedInspectQueue::<ExternalApiCallInfo>::new(queue_size)
        });
    }

    /// Inserts the given `info` into the entry at `protocol` and `queue_key`, incrementing
    /// the total call count to the protocol's wrapper entry.
    fn add_info(
        &mut self,
        protocol: &str,
        queue_key: &'static str,
        event_type: &str,
        info: ExternalApiCallInfo,
        count: u64,
    ) {
        self.ensure_queue_exists(
            protocol,
            queue_key,
            if queue_key == COMPLETED_CALLS_KEY { MAX_COMPLETED_CALLS } else { MAX_PENDING_CALLS },
        );
        let wrapper = self.api_calls.get_mut(protocol).expect("Protocol entry should exist");
        {
            let mut wrapper_guard = wrapper.count.as_mut();
            *wrapper_guard += 1;
        }
        let event_count =
            wrapper.event_counts.get_or_insert_with(event_type.to_string(), || IValue::new(0));
        {
            let mut event_count_guard = event_count.as_mut();
            *event_count_guard += 1;
        }

        let queue = wrapper.calls.get_mut(queue_key).expect("Queue should exist");
        let key = format!("{count:020}");
        queue.push(
            &key,
            info.with_inspect(queue.inspect_node(), &key)
                .expect("Failed to create ExternalApiCallInfo node"),
        );
    }

    /// Removes the call with the same request timestamp from the `protocol`'s pending
    /// call queue, indicating that the call has completed. Should be called along with
    /// `add_info` to add the completed call.
    fn remove_pending(&mut self, protocol: &str, info: &ExternalApiCallInfo) {
        let wrapper = self.api_calls.get_mut(protocol).expect("Protocol entry should exist");
        let pending_queue =
            wrapper.calls.get_mut(PENDING_CALLS_KEY).expect("Pending queue should exist");
        let req_timestamp = &*info.request_timestamp;
        pending_queue.retain(|pending| &*pending.request_timestamp != req_timestamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect::Inspector;
    use futures::channel::mpsc;

    const MOCK_PROTOCOL_NAME: &str = "fuchsia.external.FakeAPI";

    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_create_connection() {
        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("external_apis");

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = ExternalApiInspectAgent::create_with_node(rx, inspect_node, Some(done_tx));
        agent.initialize();

        let connection_created_event =
            ExternalServiceEvent::Created(MOCK_PROTOCOL_NAME, "0.000000".into());

        let _ = tx.unbounded_send(connection_created_event);
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "completed_calls": {
                            "00000000000000000001": {
                                request: "connect",
                                response: "none",
                                request_timestamp: "none",
                                response_timestamp: "0.000000",
                            },
                        },
                    },
                    "count": 1u64,
                    "event_counts": {
                        "Created": 1u64,
                    },
                },
            },
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_pending() {
        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("external_apis");

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = ExternalApiInspectAgent::create_with_node(rx, inspect_node, Some(done_tx));
        agent.initialize();

        let api_call_event = ExternalServiceEvent::ApiCall(
            MOCK_PROTOCOL_NAME,
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
        );

        let _ = tx.unbounded_send(api_call_event.clone());
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {
                            "00000000000000000001": {
                                request: "set_manual_brightness(0.6)",
                                response: "none",
                                request_timestamp: "0.000000",
                                response_timestamp: "none",
                            },
                        },
                    },
                    "count": 1u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                    },
                },
            },
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_success_response() {
        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("external_apis");

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = ExternalApiInspectAgent::create_with_node(rx, inspect_node, Some(done_tx));
        agent.initialize();

        let api_call_event = ExternalServiceEvent::ApiCall(
            MOCK_PROTOCOL_NAME,
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
        );
        let api_response_event = ExternalServiceEvent::ApiResponse(
            MOCK_PROTOCOL_NAME,
            "Ok(None)".into(),
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
            "0.129987".into(),
        );

        let _ = tx.unbounded_send(api_call_event.clone());
        let _ = done_rx.next().await;
        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {
                            "00000000000000000001": {
                                request: "set_manual_brightness(0.6)",
                                response: "none",
                                request_timestamp: "0.000000",
                                response_timestamp: "none",
                            },
                        },
                    },
                    "count": 1u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                    },
                },
            },
        });

        let _ = tx.unbounded_send(api_response_event.clone());
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {},
                        "completed_calls": {
                            "00000000000000000002": {
                                request: "set_manual_brightness(0.6)",
                                response: "Ok(None)",
                                request_timestamp: "0.000000",
                                response_timestamp: "0.129987",
                            },
                        },
                    },
                    "count": 2u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                        "ApiResponse": 1u64,
                    },
                },
            },
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_error() {
        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("external_apis");

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = ExternalApiInspectAgent::create_with_node(rx, inspect_node, Some(done_tx));
        agent.initialize();

        let api_call_event = ExternalServiceEvent::ApiCall(
            MOCK_PROTOCOL_NAME,
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
        );
        let error_event = ExternalServiceEvent::ApiError(
            MOCK_PROTOCOL_NAME,
            "Err(INTERNAL_ERROR)".into(),
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
            "0.129987".into(),
        );

        let _ = tx.unbounded_send(api_call_event.clone());
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {
                            "00000000000000000001": {
                                request: "set_manual_brightness(0.6)",
                                response: "none",
                                request_timestamp: "0.000000",
                                response_timestamp: "none",
                            },
                        },
                    },
                    "count": 1u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                    },
                },
            },
        });

        let _ = tx.unbounded_send(error_event.clone());
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {},
                        "completed_calls": {
                            "00000000000000000002": {
                                request: "set_manual_brightness(0.6)",
                                response: "Err(INTERNAL_ERROR)",
                                request_timestamp: "0.000000",
                                response_timestamp: "0.129987",
                            },
                        },
                    },
                    "count": 2u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                        "ApiError": 1u64,
                    },
                },
            },
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_channel_closed() {
        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("external_apis");

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = ExternalApiInspectAgent::create_with_node(rx, inspect_node, Some(done_tx));
        agent.initialize();

        let api_call_event = ExternalServiceEvent::ApiCall(
            MOCK_PROTOCOL_NAME,
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
        );
        let closed_event = ExternalServiceEvent::Closed(
            MOCK_PROTOCOL_NAME,
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
            "0.129987".into(),
        );

        let _ = tx.unbounded_send(api_call_event.clone());
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {
                            "00000000000000000001": {
                                request: "set_manual_brightness(0.6)",
                                response: "none",
                                request_timestamp: "0.000000",
                                response_timestamp: "none",
                            },
                        },
                    },
                    "count": 1u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                    },
                },
            },
        });

        let _ = tx.unbounded_send(closed_event.clone());
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {},
                        "completed_calls": {
                            "00000000000000000002": {
                                request: "set_manual_brightness(0.6)",
                                response: "closed",
                                request_timestamp: "0.000000",
                                response_timestamp: "0.129987",
                            },
                        },
                    },
                    "count": 2u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                        "Closed": 1u64,
                    },
                },
            },
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_multiple_requests() {
        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("external_apis");

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = ExternalApiInspectAgent::create_with_node(rx, inspect_node, Some(done_tx));
        agent.initialize();

        let api_call_event = ExternalServiceEvent::ApiCall(
            MOCK_PROTOCOL_NAME,
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
        );
        let api_response_event = ExternalServiceEvent::ApiResponse(
            MOCK_PROTOCOL_NAME,
            "Ok(None)".into(),
            "set_manual_brightness(0.6)".into(),
            "0.000000".into(),
            "0.129987".into(),
        );

        let api_call_event_2 = ExternalServiceEvent::ApiCall(
            MOCK_PROTOCOL_NAME,
            "set_manual_brightness(0.7)".into(),
            "0.139816".into(),
        );
        let api_response_event_2 = ExternalServiceEvent::ApiResponse(
            MOCK_PROTOCOL_NAME,
            "Ok(None)".into(),
            "set_manual_brightness(0.7)".into(),
            "0.139816".into(),
            "0.141235".into(),
        );

        let _ = tx.unbounded_send(api_call_event.clone());
        let _ = done_rx.next().await;
        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {
                            "00000000000000000001": {
                                request: "set_manual_brightness(0.6)",
                                response: "none",
                                request_timestamp: "0.000000",
                                response_timestamp: "none",
                            },
                        },
                    },
                    "count": 1u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                    },
                },
            },
        });

        let _ = tx.unbounded_send(api_response_event.clone());
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {},
                        "completed_calls": {
                            "00000000000000000002": {
                                request: "set_manual_brightness(0.6)",
                                response: "Ok(None)",
                                request_timestamp: "0.000000",
                                response_timestamp: "0.129987",
                            },
                        },
                    },
                    "count": 2u64,
                    "event_counts": {
                        "ApiCall": 1u64,
                        "ApiResponse": 1u64,
                    },
                },
            },
        });

        let _ = tx.unbounded_send(api_call_event_2.clone());
        let _ = done_rx.next().await;
        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {
                            "00000000000000000003": {
                                request: "set_manual_brightness(0.7)",
                                response: "none",
                                request_timestamp: "0.139816",
                                response_timestamp: "none",
                            },
                        },
                        "completed_calls": {
                            "00000000000000000002": {
                                request: "set_manual_brightness(0.6)",
                                response: "Ok(None)",
                                request_timestamp: "0.000000",
                                response_timestamp: "0.129987",
                            },
                        },
                    },
                    "count": 3u64,
                    "event_counts": {
                        "ApiCall": 2u64,
                        "ApiResponse": 1u64,
                    },
                },
            },
        });

        let _ = tx.unbounded_send(api_response_event_2.clone());
        let _ = done_rx.next().await;

        assert_data_tree!(inspector, root: {
            external_apis: {
                "fuchsia.external.FakeAPI": {
                    "calls": {
                        "pending_calls": {},
                        "completed_calls": {
                            "00000000000000000002": {
                                request: "set_manual_brightness(0.6)",
                                response: "Ok(None)",
                                request_timestamp: "0.000000",
                                response_timestamp: "0.129987",
                            },
                            "00000000000000000004": {
                                request: "set_manual_brightness(0.7)",
                                response: "Ok(None)",
                                request_timestamp: "0.139816",
                                response_timestamp: "0.141235",
                            },
                        },
                    },
                    "count": 4u64,
                    "event_counts": {
                        "ApiCall": 2u64,
                        "ApiResponse": 2u64,
                    },
                },
            },
        });
    }
}
