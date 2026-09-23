// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{LinkState, Proto};
use fuchsia_inspect::Node;
use fuchsia_inspect_contrib::inspect_log;
use fuchsia_inspect_contrib::nodes::BoundedListNode;

// Keep only the 50 most recent events.
const INSPECT_LOG_WINDOW_SIZE: usize = 50;

/// Maintains the Inspect information.
pub(crate) struct InspectInfo {
    _node: Node,
    v4: BoundedListNode,
    v6: BoundedListNode,
}

impl InspectInfo {
    /// Create inspect node `id` rooted at `n`.
    pub(crate) fn new(n: &Node, id: &str, name: &str) -> Self {
        let node = n.create_child(id);
        node.record_string("name", name);
        let mut v4 = BoundedListNode::new(node.create_child("IPv4"), INSPECT_LOG_WINDOW_SIZE);
        inspect_log!(v4, state: "None");
        let mut v6 = BoundedListNode::new(node.create_child("IPv6"), INSPECT_LOG_WINDOW_SIZE);
        inspect_log!(v6, state: "None");

        InspectInfo { _node: node, v4, v6 }
    }
    pub(crate) fn log_link_state(&mut self, proto: Proto, link_state: LinkState) {
        match proto {
            Proto::IPv4 => inspect_log!(self.v4, state: format!("{:?}", link_state)),
            Proto::IPv6 => inspect_log!(self.v6, state: format!("{:?}", link_state)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_inspect::Inspector;

    #[test]
    fn test_log_state() {
        let mut executor = fuchsia_async::TestExecutor::new();
        let inspector = Inspector::default();
        let mut i = InspectInfo::new(inspector.root(), "id", "myname");
        assert_data_tree!(@executor executor, inspector, root: contains {
            id: {
                name:"myname",
                IPv4:{"0": contains {
                    state: "None"
                }
                },
                IPv6:{"0": contains {
                    state: "None"
                }
                }
            }
        });

        i.log_link_state(Proto::IPv4, LinkState::Internet);
        assert_data_tree!(@executor executor, inspector, root: contains {
            id: {
                name:"myname",
                IPv4:{"0": contains {
                    state: "None"
                },
                "1": contains {
                    state: "Internet"
                }
                },
                IPv6:{"0": contains {
                    state: "None"
                }
                }
            }
        });
        i.log_link_state(Proto::IPv4, LinkState::Gateway);
        i.log_link_state(Proto::IPv6, LinkState::Local);
        assert_data_tree!(@executor executor, inspector, root: contains {
            id: {
                name:"myname",
                IPv4:{"0": contains {
                    state: "None"
                },
                "1": contains {
                    state: "Internet"
                },
                "2": contains {
                    state: "Gateway"
                }
                },
                IPv6:{"0": contains {
                    state: "None"
                },
                "1": contains {
                    state: "Local"
                }
                }
            }
        });
    }
}
