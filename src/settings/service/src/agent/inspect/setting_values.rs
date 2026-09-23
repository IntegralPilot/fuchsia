// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::clock;
use fuchsia_async as fasync;
use fuchsia_inspect::{self as inspect, Property, StringProperty, component};
use fuchsia_inspect_derive::{Inspect, WithInspect};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
#[cfg(test)]
use futures::channel::mpsc::UnboundedSender;
use settings_inspect_utils::managed_inspect_map::ManagedInspectMap;

const INSPECT_NODE_NAME: &str = "setting_values";
const SETTING_TYPE_INSPECT_NODE_NAME: &str = "setting_types";

/// Information about the setting types available in the settings service.
///
/// Inspect nodes are not used, but need to be held as they're deleted from inspect once they go
/// out of scope.
#[derive(Debug, Default, Inspect)]
struct SettingTypesInspectInfo {
    inspect_node: inspect::Node,
    value: inspect::StringProperty,
}

impl SettingTypesInspectInfo {
    fn new(value: String, node: &inspect::Node, key: &str) -> Self {
        let info = Self::default()
            .with_inspect(node, key)
            .expect("Failed to create SettingTypesInspectInfo node");
        info.value.set(&value);
        info
    }
}

/// Information about a setting to be written to inspect.
///
/// Inspect nodes are not used, but need to be held as they're deleted from inspect once they go
/// out of scope.
#[derive(Default, Inspect)]
struct SettingValuesInspectInfo {
    /// Node of this info.
    inspect_node: inspect::Node,

    /// Debug string representation of the value of this setting.
    value: StringProperty,

    /// Milliseconds since Unix epoch that this setting's value was changed.
    timestamp: StringProperty,
}

pub struct AgentSetup {
    agent: SettingValuesInspectAgent,
    event_rx: UnboundedReceiver<(&'static str, String)>,
}

impl AgentSetup {
    pub fn initialize(self, #[cfg(test)] mut channel_done_tx: Option<UnboundedSender<()>>) {
        let AgentSetup { mut agent, mut event_rx } = self;
        fasync::Task::local(async move {
            while let Some((setting_name, setting_str)) = event_rx.next().await {
                agent.write_raw_setting_to_inspect(setting_name, setting_str).await;
                #[cfg(test)]
                {
                    let _ = channel_done_tx.as_mut().map(|tx| tx.start_send(()));
                }
            }
        })
        .detach();
    }
}

/// An agent that listens in on messages between the proxy and setting handlers to record the
/// values of all settings to inspect.
pub(crate) struct SettingValuesInspectAgent {
    setting_values: ManagedInspectMap<SettingValuesInspectInfo>,
    _setting_types_inspect_info: SettingTypesInspectInfo,
}

impl SettingValuesInspectAgent {
    // Returns Self along with receiver.
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new(
        settings: Vec<String>,
        rx: UnboundedReceiver<(&'static str, String)>,
    ) -> Option<AgentSetup> {
        Self::create_with_node(
            settings,
            component::inspector().root().create_child(INSPECT_NODE_NAME),
            rx,
            None,
        )
    }

    /// Create an agent to listen in on all messages between Proxy and setting
    /// handlers. Agent starts immediately without calling invocation, but
    /// acknowledges the invocation payload to let the Authority know the agent
    /// starts properly.
    fn create_with_node(
        mut settings: Vec<String>,
        inspect_node: inspect::Node,
        event_rx: UnboundedReceiver<(&'static str, String)>,
        custom_inspector: Option<&inspect::Inspector>,
    ) -> Option<AgentSetup> {
        let inspector = custom_inspector.unwrap_or_else(|| component::inspector());

        // Add inspect node for the setting types.
        settings.sort();
        let setting_types_value = format!("{settings:?}");
        let setting_types_inspect_info = SettingTypesInspectInfo::new(
            setting_types_value,
            inspector.root(),
            SETTING_TYPE_INSPECT_NODE_NAME,
        );

        let agent = Self {
            setting_values: ManagedInspectMap::<SettingValuesInspectInfo>::with_node(inspect_node),
            _setting_types_inspect_info: setting_types_inspect_info,
        };

        Some(AgentSetup { agent, event_rx })
    }

    /// Writes a setting value to inspect.
    async fn write_raw_setting_to_inspect(&mut self, setting_name: &'static str, value: String) {
        let timestamp = clock::inspect_format_now();

        let key_str = setting_name.to_string();
        let setting_values = self.setting_values.get_mut(&key_str);

        if let Some(setting_values_info) = setting_values {
            // Value already known, just update its fields.
            setting_values_info.timestamp.set(&timestamp);
            setting_values_info.value.set(&value);
        } else {
            // Setting value not recorded yet, create a new inspect node.
            let inspect_info =
                self.setting_values.get_or_insert_with(key_str, SettingValuesInspectInfo::default);
            inspect_info.timestamp.set(&timestamp);
            inspect_info.value.set(&value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use futures::channel::mpsc;
    use zx::MonotonicInstant;

    // Verifies that inspect agent intercepts setting change events and writes the setting value
    // to inspect.
    #[fuchsia::test(allow_stalls = false)]
    async fn test_write_inspect_on_changed() {
        // Set the clock so that timestamps will always be 0.
        clock::mock::set(MonotonicInstant::from_nanos(0));

        let inspector = inspect::Inspector::default();
        let inspect_node = inspector.root().create_child(INSPECT_NODE_NAME);

        let (mut tx, rx) = mpsc::unbounded();
        let agent_setup = SettingValuesInspectAgent::create_with_node(
            vec!["Unknown".to_string()],
            inspect_node,
            rx,
            Some(&inspector),
        )
        .expect("agent missing inspect");

        let (done_tx, mut done_rx) = mpsc::unbounded();
        agent_setup.initialize(Some(done_tx));

        // Inspect agent should not report any setting values.
        assert_data_tree!(inspector, root: {
            setting_types: {
                "value": "[\"Unknown\"]",
            },
            setting_values: {
            }
        });

        tx.start_send(("Unknown", "UnknownInfo(true)".to_string())).expect("sending via channel");
        let () = done_rx.next().await.expect("should have processed event");

        // Inspect agent writes value to inspect.
        assert_data_tree!(inspector, root: {
            setting_types: {
                "value": "[\"Unknown\"]",
            },
            setting_values: {
                "Unknown": {
                    value: "UnknownInfo(true)",
                    timestamp: "0.000000000",
                }
            }
        });
    }
}
