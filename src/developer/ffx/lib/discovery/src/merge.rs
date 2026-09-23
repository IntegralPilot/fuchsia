// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{TargetEvent, TargetHandle, TargetState};

#[derive(Default)]
pub(crate) struct TargetSet {
    targets: Vec<TargetHandle>,
}

fn is_same_target(th1: &TargetHandle, th2: &TargetHandle) -> bool {
    let (are_same, reason) = match (&th1.state, &th2.state) {
        (
            TargetState::Product { addrs: addrs1, serial: serial1 },
            TargetState::Product { addrs: addrs2, serial: serial2 },
        ) =>
        // Start with serials. If they both have one, let's assume we're dealing with the same target.
        {
            if let (Some(s1), Some(s2)) = (serial1, serial2)
                && (!s1.is_empty() && !s2.is_empty())
            {
                if s1 == s2 {
                    (true, "products: same serial")
                } else {
                    (false, "products: different serials")
                }
            } else {
                // If at least one doesn't have a serial, then just check the addresses. It's possible that
                // in an update we got a new address, so just check for intersection.
                // Note that we are _not_ checking the name -- there's no particular reason to think that
                // identical names means identical targets.
                if addrs1.iter().any(|a| addrs2.contains(a)) {
                    (true, "products: common address")
                } else {
                    (false, "products: different serials and addresses")
                }
            }
        }
        (TargetState::Fastboot(fb1), TargetState::Fastboot(fb2)) => {
            // Check the serial _and_ the connection
            if fb1.serial_number == fb2.serial_number {
                if fb1.connection_state == fb2.connection_state {
                    (true, "fastboot: same serials and state")
                } else {
                    (false, "fastboot: same serials but different state")
                }
            } else {
                (false, "fastboot: different serials")
            }
        }
        // We're not going to worry about Zedboot devices, and all other possibilities (devices are in an
        // Unknown state, or in different states), we'll consider as distinct, just in case.
        _ => (false, "neither both-product nor both-fastboot"),
    };
    log::trace!("targets {th1:?}, {th2:?} are same: {are_same}: {reason}");
    are_same
}

impl TargetSet {
    pub(crate) fn new() -> Self {
        Self { targets: vec![] }
    }
    pub(crate) fn into_targets(self) -> Vec<TargetHandle> {
        self.targets
    }

    pub(crate) fn process_event(&mut self, event: TargetEvent) {
        match event {
            TargetEvent::Added(target_handle) => self.process_add(target_handle),
            TargetEvent::Removed(target_handle) => self.process_remove(target_handle),
        }
    }

    fn process_add(&mut self, added_th: TargetHandle) {
        for t in &mut self.targets {
            if is_same_target(&t, &added_th) {
                *t = added_th;
                return;
            }
        }
        // Note that one can imagine a constraint that no two targets in self.targets match with
        // is_same_target(): but imagine adding th1 with addr A, then th2 with addr B, then th1 gets
        // updated with addrs [A, B]. Now th1 and th2 _would_ match. The complicated version of this
        // problem involves merging targets when we discover there is a match. To keep things simple
        // we won't worry about that for now.
        self.targets.push(added_th);
    }

    fn process_remove(&mut self, removed_th: TargetHandle) {
        self.targets.retain(|th| !is_same_target(th, &removed_th))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FastbootConnectionState, FastbootTargetState};
    use addr::TargetAddr;
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    fn create_product_target(
        nodename: Option<&str>,
        addrs: Vec<&str>,
        serial: Option<&str>,
    ) -> TargetHandle {
        TargetHandle {
            node_name: nodename.map(String::from),
            state: TargetState::Product {
                addrs: addrs.into_iter().map(|s| TargetAddr::from_str(s).unwrap()).collect(),
                serial: serial.map(String::from),
            },
            manual: false,
        }
    }

    fn create_fastboot_target(serial: &str, state: FastbootConnectionState) -> TargetHandle {
        TargetHandle {
            node_name: None,
            state: TargetState::Fastboot(FastbootTargetState {
                serial_number: serial.to_string(),
                connection_state: state,
            }),
            manual: false,
        }
    }

    #[fuchsia::test]
    fn test_is_same_target_product_by_serial() {
        let t1 = create_product_target(Some("a"), vec!["[::1]:22"], Some("123"));
        let t2 = create_product_target(Some("b"), vec!["[::2]:22"], Some("123"));
        assert!(is_same_target(&t1, &t2));
    }

    #[fuchsia::test]
    fn test_is_same_target_product_by_addr() {
        let t1 = create_product_target(Some("a"), vec!["[::1]:22", "[::3]:22"], None);
        let t2 = create_product_target(Some("b"), vec!["[::2]:22", "[::1]:22"], None);
        assert!(is_same_target(&t1, &t2));
    }

    #[fuchsia::test]
    fn test_is_not_same_target_product_different_serials() {
        let t1 = create_product_target(Some("a"), vec!["[::1]:22"], Some("123"));
        let t2 = create_product_target(Some("a"), vec!["[::1]:22"], Some("456"));
        assert!(!is_same_target(&t1, &t2));
    }

    #[fuchsia::test]
    fn test_is_not_same_target_product_different_addrs() {
        let t1 = create_product_target(Some("a"), vec!["[::1]:22"], None);
        let t2 = create_product_target(Some("b"), vec!["[::2]:22"], None);
        assert!(!is_same_target(&t1, &t2));
    }

    #[fuchsia::test]
    fn test_is_same_target_fastboot() {
        let t1 = create_fastboot_target("123", FastbootConnectionState::Usb);
        let t2 = create_fastboot_target("123", FastbootConnectionState::Usb);
        assert!(is_same_target(&t1, &t2));
    }

    #[fuchsia::test]
    fn test_is_not_same_target_fastboot_different_serials() {
        let t1 = create_fastboot_target("123", FastbootConnectionState::Usb);
        let t2 = create_fastboot_target("456", FastbootConnectionState::Usb);
        assert!(!is_same_target(&t1, &t2));
    }

    #[fuchsia::test]
    fn test_is_not_same_target_fastboot_different_connection() {
        let t1 = create_fastboot_target("123", FastbootConnectionState::Usb);
        let t2 = create_fastboot_target("123", FastbootConnectionState::Tcp(vec![]));
        assert!(!is_same_target(&t1, &t2));
    }

    #[fuchsia::test]
    fn test_is_not_same_target_cross_state() {
        let t1 = create_product_target(Some("a"), vec![], Some("123"));
        let t2 = create_fastboot_target("123", FastbootConnectionState::Usb);
        assert!(!is_same_target(&t1, &t2));
    }

    #[fuchsia::test]
    fn test_target_set_add_new() {
        let mut set = TargetSet::new();
        let t1 = create_product_target(Some("a"), vec!["[::1]:22"], None);
        set.process_event(TargetEvent::Added(t1.clone()));
        assert_eq!(set.targets.len(), 1);
        assert_eq!(set.targets[0], t1);
    }

    #[fuchsia::test]
    fn test_target_set_update_existing() {
        let mut set = TargetSet::new();
        let t1 = create_product_target(Some("a"), vec!["[::1]:22"], Some("123"));
        let t2 = create_product_target(Some("b"), vec!["[::2]:22"], Some("123"));
        set.process_event(TargetEvent::Added(t1));
        assert_eq!(set.targets.len(), 1);
        set.process_event(TargetEvent::Added(t2.clone()));
        assert_eq!(set.targets.len(), 1);
        assert_eq!(set.targets[0], t2);
    }

    #[fuchsia::test]
    fn test_target_set_remove_existing() {
        let mut set = TargetSet::new();
        let t1 = create_product_target(Some("a"), vec!["[::1]:22"], Some("123"));
        set.process_event(TargetEvent::Added(t1.clone()));
        assert_eq!(set.targets.len(), 1);
        set.process_event(TargetEvent::Removed(t1));
        assert!(set.targets.is_empty());
    }

    #[fuchsia::test]
    fn test_target_set_remove_non_existent() {
        let mut set = TargetSet::new();
        let t1 = create_product_target(Some("a"), vec!["[::1]:22"], Some("123"));
        let t2 = create_product_target(Some("b"), vec!["[::2]:22"], Some("456"));
        set.process_event(TargetEvent::Added(t1.clone()));
        assert_eq!(set.targets.len(), 1);
        set.process_event(TargetEvent::Removed(t2));
        assert_eq!(set.targets.len(), 1);
        assert_eq!(set.targets[0], t1);
    }

    #[fuchsia::test]
    fn test_target_set_does_not_merge_on_update() {
        let mut set = TargetSet::new();
        let t1 = create_product_target(Some("a"), vec!["[::1]:22"], None);
        let t2 = create_product_target(Some("b"), vec!["[::2]:22"], None);
        let t1_updated =
            create_product_target(Some("a-updated"), vec!["[::1]:22", "[::2]:22"], None);

        set.process_event(TargetEvent::Added(t1));
        set.process_event(TargetEvent::Added(t2.clone()));
        assert_eq!(set.targets.len(), 2);

        // This should update t1, but not merge it with t2, even though they now
        // share an address.
        set.process_event(TargetEvent::Added(t1_updated.clone()));
        assert_eq!(set.targets.len(), 2);

        // Find t1_updated and t2 in the list and assert they are the only things.
        let mut found_t1 = false;
        let mut found_t2 = false;
        for target in set.targets.iter() {
            if target == &t1_updated {
                found_t1 = true;
            } else if target == &t2 {
                found_t2 = true;
            }
        }
        assert!(found_t1);
        assert!(found_t2);
    }
}
