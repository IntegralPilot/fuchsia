// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The usage_counts mod defines the [SettingTypeUsageInspectAgent], which is responsible for counting
//! relevant API usages to Inspect. Since API usages might happen before agent lifecycle states are
//! communicated (due to agent priority ordering), the [SettingTypeUsageInspectAgent] begins
//! listening to requests immediately after creation.
//!

use fuchsia_async as fasync;
use fuchsia_inspect::{self as inspect, component};
use fuchsia_inspect_derive::Inspect;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
#[cfg(test)]
use futures::channel::mpsc::UnboundedSender;
use inspect::NumericProperty;
use settings_common::inspect::event::{Direction, UsageEvent};
use settings_common::trace;
use settings_inspect_utils::managed_inspect_map::ManagedInspectMap;
use std::collections::HashMap;

/// Information about a setting type usage count to be written to inspect.
struct SettingTypeUsageInspectInfo {
    /// Map from the name of the Request variant to its calling counts.
    requests_by_type: ManagedInspectMap<UsageInfo>,
}

impl SettingTypeUsageInspectInfo {
    fn new(parent: &inspect::Node, setting_type_str: &str) -> Self {
        Self {
            requests_by_type: ManagedInspectMap::<UsageInfo>::with_node(
                parent.create_child(setting_type_str),
            ),
        }
    }
}

#[derive(Default, Inspect)]
struct UsageInfo {
    /// Node of this info.
    inspect_node: inspect::Node,

    /// Call counts of the current API.
    count: inspect::IntProperty,
}

/// The SettingTypeUsageInspectAgent is responsible for listening to requests to the setting
/// handlers and recording the related API usage counts to Inspect.
pub(crate) struct SettingTypeUsageInspectAgent {
    /// Node of this info.
    inspect_node: inspect::Node,

    /// Mapping from SettingType key to api usage counts.
    api_call_counts: HashMap<String, SettingTypeUsageInspectInfo>,

    usage_rx: Option<UnboundedReceiver<UsageEvent>>,

    #[cfg(test)]
    done_tx: Option<UnboundedSender<()>>,
}

impl SettingTypeUsageInspectAgent {
    pub fn new(rx: UnboundedReceiver<UsageEvent>) -> Self {
        Self::create_with_node(
            rx,
            component::inspector().root().create_child("api_usage_counts"),
            #[cfg(test)]
            None,
        )
    }

    fn create_with_node(
        rx: UnboundedReceiver<UsageEvent>,
        node: inspect::Node,
        #[cfg(test)] done_tx: Option<UnboundedSender<()>>,
    ) -> Self {
        SettingTypeUsageInspectAgent {
            inspect_node: node,
            api_call_counts: HashMap::new(),
            usage_rx: Some(rx),
            #[cfg(test)]
            done_tx,
        }
    }

    pub fn initialize(mut self) {
        fasync::Task::local({
            async move {
                let id = fuchsia_trace::Id::new();
                trace!(id, "usage_counts_inspect_agent");
                let mut usage_rx = self.usage_rx.take().unwrap();

                while let Some(usage_event) = usage_rx.next().await {
                    self.process_usage_event(usage_event);
                    #[cfg(test)]
                    if let Some(done_tx) = &self.done_tx {
                        let _ = done_tx.unbounded_send(());
                    }
                }
            }
        })
        .detach();
    }

    fn process_usage_event(&mut self, event: UsageEvent) {
        // We only need to track incoming requests.
        if let Direction::Response(..) = event.direction {
            return;
        }

        let inspect_node = &self.inspect_node;
        let setting_type_info = self
            .api_call_counts
            .entry(event.setting.to_string())
            .or_insert_with(|| SettingTypeUsageInspectInfo::new(inspect_node, event.setting));

        let key = event.request_type;
        let usage = setting_type_info
            .requests_by_type
            .get_or_insert_with(format!("{key:?}"), UsageInfo::default);
        let _ = usage.count.add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use futures::channel::mpsc;
    use settings_common::inspect::event::{Direction, RequestType, ResponseType};

    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect() {
        let inspector = inspect::Inspector::default();
        let inspect_node = inspector.root().create_child("api_usage_counts");

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = SettingTypeUsageInspectAgent::create_with_node(rx, inspect_node, Some(done_tx));
        agent.initialize();

        // Send a few requests to make sure they get written to inspect properly.
        let mut request_event = UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Request(
                "SetDisplayInfo{auto_brightness: Some(false)}".to_string(),
            ),
            id: 0,
        };
        let mut response_event = UsageEvent {
            setting: "Display",
            request_type: RequestType::Set,
            direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
            id: 0,
        };

        let _ = tx.unbounded_send(request_event.clone());
        let _ = done_rx.next().await;

        let _ = tx.unbounded_send(response_event.clone());
        let _ = done_rx.next().await;

        request_event.id = 1;
        response_event.id = 1;

        let _ = tx.unbounded_send(request_event);
        let _ = done_rx.next().await;

        let _ = tx.unbounded_send(response_event);
        let _ = done_rx.next().await;

        for i in 0..100 {
            let _ = tx.unbounded_send(UsageEvent {
                setting: "Intl",
                request_type: RequestType::Set,
                direction: Direction::Request(
                    "SetIntlInfo{ \
                        locales: Some([LocaleId { id: \"en-US\" }]), \
                        temperature_unit: Some(Celsius), \
                        time_zone_id: Some(\"UTC\"), \
                        hour_cycle: None \
                    }"
                    .to_string(),
                ),
                id: i + 2,
            });
            let _ = done_rx.next().await;

            let _ = tx.unbounded_send(UsageEvent {
                setting: "Intl",
                request_type: RequestType::Set,
                direction: Direction::Response("Ok(None)".to_string(), ResponseType::OkNone),
                id: i + 2,
            });
            let _ = done_rx.next().await;
        }

        assert_data_tree!(inspector, root: {
            api_usage_counts: {
                "Display": {
                    "Set": {
                        count: 2i64,
                    },
                },
                "Intl": {
                    "Set": {
                       count: 100i64
                    },
                }
            },
        });
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_inspect_mixed_request_types() {
        let inspector = inspect::Inspector::default();
        let inspect_node = inspector.root().create_child("api_usage_counts");

        let (tx, rx) = mpsc::unbounded();
        let (done_tx, mut done_rx) = mpsc::unbounded();
        let agent = SettingTypeUsageInspectAgent::create_with_node(rx, inspect_node, Some(done_tx));
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

        assert_data_tree!(inspector, root: {
            api_usage_counts: {
                "Display": {
                    "Set": {
                        count: 2i64,
                    },
                    "Get": {
                        count: 2i64,
                    },
                },
            }
        });
    }
}
