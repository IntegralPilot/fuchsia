// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use fidl_fuchsia_driver_development as fdd;
use futures::StreamExt;
use futures::channel::mpsc;
use std::collections::{HashMap, HashSet};

// Wait for the events from the |nodes| to be received. Updates the entries to be Some.
pub async fn wait_for_nodes(
    nodes: &mut HashMap<String, Option<Option<u64>>>,
    receiver: &mut mpsc::Receiver<(String, String)>,
) -> Result<()> {
    while nodes.values().any(|&x| x.is_none()) {
        let (from_node, _) = receiver.next().await.ok_or_else(|| anyhow!("Receiver failed"))?;
        if !nodes.contains_key(&from_node) {
            return Err(anyhow!("Couldn't find node '{}' in 'nodes'.", from_node.to_string()));
        }
        nodes.entry(from_node).and_modify(|x| {
            *x = Some(None);
        });
    }

    Ok(())
}

// Validates the host koids given the device infos.
// Performs the following:
// - Stores the host koid for nodes in changed_or_new as those are expected to be new/changed.
// - Validate bound nodes are not in should_not_exist
// - Validate bound nodes have the same koid as the most recent item in previous.
// - Validate that items in changed_or_new have been changed from the most recent item in previous
//   if they are not new.
pub async fn validate_host_koids(
    test_stage_name: &str,
    device_infos: Vec<fdd::NodeInfo>,
    changed_or_new: &mut HashMap<String, Option<Option<u64>>>,
    previous: Vec<&HashMap<String, Option<Option<u64>>>>,
    should_not_exist: Option<&HashSet<String>>,
) -> Result<()> {
    for dev in &device_infos {
        let key = dev.moniker.clone().unwrap().split(".").last().unwrap().to_string();

        // Items in changed_or_new are expected to be different so just save that info and move on.
        if changed_or_new.contains_key(&key) {
            changed_or_new.entry(key).and_modify(|x| {
                *x = Some(dev.driver_host_koid);
            });

            continue;
        }

        // Skip comparison and should_not_exist check as the koid is not valid when its unbound.
        if matches!(dev.bound_driver_url.as_deref(), Some("unbound")) {
            continue;
        }

        // Error if the item is in should not exist.
        if let Some(should_not_exist_value) = &should_not_exist {
            if should_not_exist_value.contains(&key) {
                return Err(anyhow!(
                    "Found node that should not exist after {}: '{}'.",
                    test_stage_name,
                    key
                ));
            }
        }

        // Go through the previous items (which should come in from most to least recent order)
        // and make sure this matches the most recent instance.
        for prev in &previous {
            if let Some(prev_koid) = prev.get(&key) {
                match prev_koid {
                    Some(prev_koid_value) => {
                        if *prev_koid_value != dev.driver_host_koid {
                            return Err(anyhow!(
                                "koid should not have changed for node '{}' after {}.",
                                key,
                                test_stage_name
                            ));
                        }

                        break;
                    }
                    None => {
                        // This is not possible as things are today.
                        // The values in the entries cannot be None since in wait_for_nodes we have
                        // waited for all of them to not be None.
                        return Err(anyhow!("prev koid not available after."));
                    }
                }
            }
        }
    }

    // Now we can make sure those items in changed_or_new are different than their most recent
    // previous item. Skipping ones that are not seen in previous.
    for (key, changed_or_new_node_entry) in changed_or_new {
        // First find the most recent previous koid if one exists.
        let mut koid_before: Option<&Option<u64>> = None;
        for prev in &previous {
            if let Some(prev_koid) = prev.get(key) {
                match prev_koid {
                    Some(prev_koid_value) => {
                        koid_before = Some(prev_koid_value);
                        break;
                    }
                    None => {
                        // This is not possible as things are today.
                        // The values in the entries cannot be None since in wait_for_nodes we have
                        // waited for all of them to not be None.
                        return Err(anyhow!("previous map entry cannot have None outer option."));
                    }
                }
            }
        }

        // Now compare it with current to make sure it is different.
        match koid_before {
            Some(koid_before) => match koid_before {
                Some(koid_before) => match changed_or_new_node_entry {
                    Some(koid_after) => match koid_after {
                        Some(koid_after) => {
                            // The node had a koid in both current and previous. Ensure that the
                            // koid value is different.
                            if koid_before == koid_after {
                                return Err(anyhow!(
                                    "koid should have changed for node '{}' after {}.",
                                    key,
                                    test_stage_name
                                ));
                            }
                        }
                        None => {
                            // The current node doesn't contain a koid.
                            // This can happen if the node is a composite parent, or if it is unbound.
                            continue;
                        }
                    },
                    None => {
                        // This is not possible as things are today.
                        // The values in the entries cannot be None since in wait_for_nodes we have
                        // waited for all of them to not be None.
                        return Err(anyhow!("changed_or_new_node entry cannot be None."));
                    }
                },
                None => {
                    // This node existed in a previous item, but it didn't contain a koid.
                    // This can happen if the node was a composite parent, or if it was unbound.
                    continue;
                }
            },
            None => {
                // This is a new node and it doesn't exist in previous.
                continue;
            }
        }
    }

    Ok(())
}
