// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements timer multiplexing for both the boot and the UTC timeline.
//!
//! Provides a [Heap] of timers, each of which can be relative to either
//! the boot timeline or the UTC timeline. The complication is that the
//! timeline relationship may change during the lifetime of the system, and
//! UTC deadlines may shift. This heap implementation ensures that any such
//! changes are properly accounted for.
//!
//! The user must provide a
//! [fuchsia_runtime::UtcClockTransform], which allows the heap invariants
//! to be maintained.

use fidl;
use fidl_fuchsia_time_alarms as fta;
use fuchsia_async as fasync;
use fuchsia_runtime as fxr;
use fuchsia_trace as trace;
use log::error;
use std::cell::RefCell;
use std::cmp;
use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::rc::Rc;
use time_pretty::{format_duration, format_timer};
use zx;

pub(crate) trait Responder: std::fmt::Debug {
    fn send(
        &self,
        alarm_id: &str,
        result: Result<fidl::EventPair, fta::WakeAlarmsError>,
    ) -> Option<Result<(), fidl::Error>>;
}

impl Responder for RefCell<Option<fta::WakeAlarmsSetAndWaitResponder>> {
    fn send(
        &self,
        _alarm_id: &str,
        result: Result<fidl::EventPair, fta::WakeAlarmsError>,
    ) -> Option<Result<(), fidl::Error>> {
        self.borrow_mut().take().map(|responder| responder.send(result))
    }
}

impl Responder for RefCell<Option<fidl::endpoints::ClientEnd<fta::NotifierMarker>>> {
    fn send(
        &self,
        alarm_id: &str,
        result: Result<fidl::EventPair, fta::WakeAlarmsError>,
    ) -> Option<Result<(), fidl::Error>> {
        self.borrow_mut().take().map(|notifier| {
            let proxy = notifier.into_proxy();
            match result {
                Ok(keep_alive) => proxy.notify(alarm_id, keep_alive),
                Err(e) => proxy.notify_error(alarm_id, e),
            }
        })
    }
}

impl Responder for RefCell<Option<fta::WakeAlarmsSetAndWaitUtcResponder>> {
    fn send(
        &self,
        _alarm_id: &str,
        result: Result<fidl::EventPair, fta::WakeAlarmsError>,
    ) -> Option<Result<(), fidl::Error>> {
        self.borrow_mut().take().map(|responder| responder.send(result))
    }
}

// Converts a UTC instant into a boot instant using `transform`, using the approach that never
// panics.
fn safe_utc_to_boot(
    instant: &fxr::UtcInstant,
    transform: &fxr::UtcClockTransform,
) -> fasync::BootInstant {
    if transform.rate.synthetic_ticks == 0 || transform.rate.reference_ticks == 0 {
        // If the time does not tick, any UTC instant is forever in the future.
        zx::BootInstant::INFINITE.into()
    } else {
        transform.apply_inverse(*instant).into()
    }
}

/// A representation of deadlines with different reference points.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Deadline {
    /// A deadline that is fixed on a boot timeline.
    Boot(fasync::BootInstant),
    /// A deadline on a UTC timeline.
    ///
    /// This deadline's mapping to the boot timeline can change over the lifetime of the Deadline.
    /// However, the `Utc` deadline must not contain this mapping to ensure compatibility with
    /// `Ord`.
    Utc(fxr::UtcInstant),
}

impl std::fmt::Display for Deadline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Deadline::Boot(d) => {
                write!(f, "{}", format_timer((*d).into()))
            }
            Deadline::Utc(d) => {
                write!(f, "UTC[{}]", format_timer((*d).into()))
            }
        }
    }
}

impl Deadline {
    /// Get the deadline represented as a timestamp on the boot timeline.
    pub fn as_boot(&self, t: &fxr::UtcClockTransform) -> fasync::BootInstant {
        match self {
            Deadline::Boot(d) => *d,
            Deadline::Utc(d) => safe_utc_to_boot(d, t),
        }
    }
}

/// A representation of the state of a single Timer.
#[derive(Debug)]
pub(crate) struct Node {
    // The Node's unique identifier.
    node_id: Id,
    /// The responder that is blocked until the timer expires.  Used to notify
    /// the alarms subsystem client when this alarm expires.
    responder: Rc<dyn Responder>,
    /// The deadline at which the timer expires.
    deadline: Deadline,
    /// Needed for `std::cmp` traits, so that we can plug the `Node` into `BinaryHeap`.
    transform: Rc<RefCell<fxr::UtcClockTransform>>,
}

impl Drop for Node {
    // If the Node was evicted without having expired, notify the other
    // end that the timer has been canceled.
    fn drop(&mut self) {
        // If the Node is dropped, notify the client that may have
        // been waiting. We can not drop a responder, because that kills
        // the FIDL connection.
        if let Some(Err(e)) =
            self.responder.send(self.node_id.alarm(), Err(fta::WakeAlarmsError::Dropped))
        {
            error!("could not drop responder: {:?}", e);
        }
    }
}

impl Node {
    /// Create a new Node with an absolute deadline tied to the boot timeline.
    fn new_boot(
        deadline: fasync::BootInstant,
        alarm_id: String,
        conn_id: zx::Koid,
        responder: Rc<dyn Responder>,
        transform: Rc<RefCell<fxr::UtcClockTransform>>,
    ) -> Self {
        Self {
            node_id: Id { alarm_id, conn: conn_id },
            responder,
            deadline: Deadline::Boot(deadline),
            transform,
        }
    }

    /// Create a new Node with an absolute deadline tied to the UTC timeline.
    fn new_utc(
        deadline: fxr::UtcInstant,
        alarm_id: String,
        conn_id: zx::Koid,
        responder: Rc<dyn Responder>,
        transform: Rc<RefCell<fxr::UtcClockTransform>>,
    ) -> Self {
        Self {
            node_id: Id { alarm_id, conn: conn_id },
            responder,
            deadline: Deadline::Utc(deadline),
            transform,
        }
    }

    pub fn get_deadline(&self) -> &Deadline {
        &self.deadline
    }

    pub fn get_boot_deadline(&self) -> fasync::BootInstant {
        self.get_deadline().as_boot(&*self.transform.borrow())
    }

    pub fn id(&self) -> &Id {
        &self.node_id
    }

    pub fn get_responder(&self) -> Rc<dyn Responder> {
        self.responder.clone()
    }
}

impl std::cmp::Eq for Node {}

impl std::cmp::PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        let transform = &*self.transform.borrow();
        self.deadline.as_boot(transform) == other.deadline.as_boot(transform)
            && self.id() == other.id()
    }
}

impl std::cmp::PartialOrd for Node {
    /// Order by deadline first, but timers with same deadline are ordered
    /// by respective IDs to avoid ordering nondeterminism.
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Node {
    /// Compares two [Node]s, by "which is sooner".
    ///
    /// Ties are broken by alarm ID, then by connection ID.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let transform = &*self.transform.borrow();
        let other_boot = other.deadline.as_boot(transform);
        let self_boot = self.deadline.as_boot(transform);
        let ordering = other_boot.cmp(&self_boot);
        if ordering == std::cmp::Ordering::Equal {
            let ordering = self.id().alarm().cmp(other.id().alarm());
            if ordering == std::cmp::Ordering::Equal {
                self.id().conn.cmp(&other.id().conn)
            } else {
                ordering
            }
        } else {
            ordering
        }
    }
}

/// The unique alarm ID associated with a timer.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) struct Id {
    alarm_id: String,
    /// Connection identifier, unique per each FIDL connection.
    pub conn: zx::Koid,
}

impl Id {
    pub fn new(alarm: String, conn: zx::Koid) -> Self {
        Self { alarm_id: alarm, conn }
    }

    /// Connection-unique alarm ID.
    pub fn alarm(&self) -> &str {
        &self.alarm_id[..]
    }
}

// Compute a trace ID for a given alarm ID. This identifier is used across
// processes for tracking the alarm's lifetime.
pub fn get_trace_id(alarm_id: &str) -> trace::Id {
    if let Some(rest) = alarm_id.strip_prefix("starnix:Koid(") {
        if let Some((koid_str, _)) = rest.split_once(')') {
            if let Ok(trace_id) = koid_str.parse::<u64>() {
                return trace_id.into();
            }
        }
    }

    // For now, other components don't have a specific way to get the trace id.
    0.into()
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TimerId[alarm_id:{},conn_id:{:?}]", self.alarm(), self.conn)
    }
}

fn dump_nodes_as_strings(now: fasync::BootInstant, nodes: &BinaryHeap<Node>) -> Vec<String> {
    nodes
        .iter()
        .map(|node| (node.get_boot_deadline().clone(), node.id().alarm_id.clone()))
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(k, v)| {
            let remaining = k - now;
            format!(
                "Timeout: {} => timer_id: {}, remaining: {}",
                format_timer(k.into()),
                v,
                format_duration(remaining.into())
            )
        })
        .collect::<Vec<_>>()
}

/// Contains all the timers known by the alarms subsystem.
///
/// [Heap] can efficiently find a timer with the earliest deadline,
/// and given a cutoff can expire one timer for which the deadline has
/// passed. This allows efficient multiplexing of timers with different
/// deadlines onto a smaller number of hardware wake alarms.
///
/// [Heap] admits scheduling alarms on the boot and UTC timelines, using
/// [new_node_boot] and [new_node_utc] respectively. The deadlines on
/// the boot timeline are fixed. Deadlines on the UTC timeline may
/// move after the fact, as the boot-to-utc clock transform changes during
/// the lifetime of the [Heap]. [Heap] ensures that timers are ordered
/// correctly to match these changes.
pub(crate) struct Heap {
    // Timers attached to the boot timeline.
    //
    // The deadlines of these timers will not change.
    boot_timers: BinaryHeap<Node>,
    // Timers attached to the UTC timeline.
    //
    // Their deadlines mapping to the boot timeline may change if `transform`
    // below gets modified during the lifetime of [Heap]. For example, adding
    // a timer with the deadline of UTC+1h, then moving UTC to UTC+45min must
    // cause the timer to fire in +15min. This can never happen with boot timers.
    //
    // As a consequence, UTC timers can not coexist in the same heap as boot
    // timers, since they could be pushed out or pulled in after having been
    // inserted in the heap. This would require re-heapifying the heap to retain
    // its invariant, which is not efficient.
    //
    // Instead we place UTC timers into a separate heap. Since all UTC timers
    // are affected by `transform` the same way, UTC timers can coexist in
    // the same heap without violating heap ordering despite changes in
    // boot-to-utc mapping.
    //
    // And since there is a constant number of heaps to manage, we can present
    // a heap-like API to the union of the two heaps efficiently.
    utc_timers: BinaryHeap<Node>,
    // IDs of all currently active timers, for indexing purposes. It is always the union of the IDs
    // of timers in `boot_timers` and `utc_timers`.
    active_timer_ids: HashSet<Id>,
    /// Needed for deadline conversions in std::cmp traits. The contents of
    /// `transform` will be mutated by other holders of the shared reference.
    transform: Rc<RefCell<fxr::UtcClockTransform>>,
}

impl std::fmt::Display for Heap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let now = fasync::BootInstant::now();
        let joined_boot = dump_nodes_as_strings(now, &self.boot_timers).join("\n\t");
        let joined_utc = dump_nodes_as_strings(now, &self.utc_timers).join("\n\t");
        write!(f, "Boot:\n\t{}\n\tUTC:\n\t{}", joined_boot, joined_utc)
    }
}

impl Heap {
    /// Creates an empty [Heap].
    pub fn new(transform: Rc<RefCell<fxr::UtcClockTransform>>) -> Self {
        Self {
            boot_timers: BinaryHeap::new(),
            utc_timers: BinaryHeap::new(),
            active_timer_ids: HashSet::new(),
            transform,
        }
    }

    /// Create a new [Node] that can be inserted into the [Heap].
    ///
    /// The `deadline` is tied to the boot timeline.
    pub fn new_node_boot(
        &self,
        deadline: fasync::BootInstant,
        alarm_id: String,
        conn_id: zx::Koid,
        responder: Rc<dyn Responder>,
    ) -> Node {
        Node::new_boot(deadline, alarm_id, conn_id, responder, self.transform.clone())
    }

    /// Create a new [Node] that can be inserted into the [Heap].
    ///
    /// The `deadline` is tied to the UTC timeline.
    pub fn new_node_utc(
        &self,
        deadline: fxr::UtcInstant,
        alarm_id: String,
        conn_id: zx::Koid,
        responder: Rc<dyn Responder>,
    ) -> Node {
        Node::new_utc(deadline, alarm_id, conn_id, responder, self.transform.clone())
    }

    /// Adds a [Node] to [Heap].
    ///
    /// If the inserted node is identical to an already existing node, then
    /// nothing is changed.  If the deadline is different, then the timer node
    /// is replaced.
    pub fn push(&mut self, new_node: Node) {
        let new_id = new_node.id();
        let new_deadline = new_node.deadline;
        self.active_timer_ids.insert(new_id.clone());
        if let Some(_) = self.active_timer_ids.get(&new_id) {
            // Replace an existing node.
            // The deadline may be pushed out or pulled in.
            self.boot_timers.retain(|t| t.id() != new_node.id());
            self.utc_timers.retain(|t| t.id() != new_node.id());
            match new_deadline {
                Deadline::Boot(_) => {
                    self.boot_timers.push(new_node);
                }
                Deadline::Utc(_) => {
                    self.utc_timers.push(new_node);
                }
            }
        } else {
            // Add a new timer node.
            match new_deadline {
                Deadline::Boot(_) => {
                    self.boot_timers.push(new_node);
                }
                Deadline::Utc(_) => {
                    self.utc_timers.push(new_node);
                }
            }
        }
    }

    /// Finds the node corresponding to the earliest expiring timer, if any.
    pub fn peek_node(&self) -> Option<&Node> {
        let maybe_boot_node = self.boot_timers.peek();
        let boot_deadline =
            maybe_boot_node.map(|node| node.deadline.as_boot(&*node.transform.borrow()));

        let maybe_utc_node = self.utc_timers.peek();
        let utc_deadline =
            maybe_utc_node.map(|node| node.deadline.as_boot(&*node.transform.borrow()));

        match (boot_deadline, utc_deadline) {
            (None, None) => None,
            (Some(_), None) => maybe_boot_node,
            (None, Some(_)) => maybe_utc_node,
            (Some(bd), Some(ud)) => {
                if bd < ud {
                    maybe_boot_node
                } else {
                    maybe_utc_node
                }
            }
        }
    }

    /// Returns the deadline of the proximate timer in [Timers], snapped to
    /// the boot timeline.
    ///
    /// This value is intended for further use in hardware devices, when no
    /// back-references to original deadline is needed.
    pub fn peek_deadline_as_boot(&self) -> Option<fasync::BootInstant> {
        self.peek_node().map(|node| node.deadline.as_boot(&*node.transform.borrow()))
    }

    /// Args:
    /// - `now` is the current time.
    /// - `deadline` is the timer deadline to check for expiry.
    pub fn expired(now: fasync::BootInstant, deadline: fasync::BootInstant) -> bool {
        deadline <= now
    }

    /// Returns true if there are no known timers.
    pub fn is_empty(&self) -> bool {
        let boot_empty = self.boot_timers.is_empty();
        let utc_empty = self.utc_timers.is_empty();
        let empty = self.active_timer_ids.is_empty();

        assert_eq!(
            empty,
            boot_empty && utc_empty,
            "broken invariant: {boot_empty},{utc_empty},{empty}:\n\tactive_timers={:?}\n\tboot_timers={:?}\n\tutc_timers={:?}",
            self.active_timer_ids,
            self.boot_timers,
            self.utc_timers,
        );
        empty
    }

    /// Attempts to expire the earliest timer.
    ///
    /// If a timer is expired, it is removed from [Timers] and returned to the caller. Note that
    /// there may be more timers that need expiring at the provided `reference instant`. To drain
    /// [Timers] of all expired timers, one must repeat the call to this method with the same
    /// value of `reference_instant` until it returns `None`.
    ///
    /// Args:
    /// - `now`: the time instant to compare the stored timers against.  Timers for
    ///   which the deadline has been reached or surpassed are eligible for expiry.
    /// - `utc_transform`: a clock transformation used to line up all the timers
    ///   to the boot timeline, where we decide what to do next.
    pub fn maybe_expire_earliest(&mut self, now: fasync::BootInstant) -> Option<Node> {
        self.peek_node()
            .map(|node| {
                let deadline = node.deadline.clone();
                let transform = node.transform.clone();
                (transform, deadline)
            })
            .map(|(transform, deadline)| {
                if Heap::expired(now, deadline.as_boot(&*transform.borrow())) {
                    match deadline {
                        Deadline::Boot(_) => self.boot_timers.pop().map(|node| {
                            self.active_timer_ids.remove(&node.id());
                            node
                        }),
                        Deadline::Utc(_) => self.utc_timers.pop().map(|node| {
                            self.active_timer_ids.remove(&node.id());
                            node
                        }),
                    }
                } else {
                    None
                }
            })
            .flatten()
    }

    /// Removes an alarm by ID.
    ///
    /// If the alarm matching the ID is the imminent (soonest to trigger), it
    /// it is returned.  If no such alarm, this returns silently. It returns
    /// silently also for non-imminent alarms.
    pub fn remove_by_id(&mut self, timer_id: &Id) -> Option<Node> {
        self.active_timer_ids.remove(timer_id);
        let boot_ret = if let Some(t) = self.boot_timers.peek().map(|node| node.id()) {
            if *t == *timer_id { self.boot_timers.pop() } else { None }
        } else {
            None
        };
        self.boot_timers.retain(|t| *t.id() != *timer_id);
        if boot_ret.is_some() {
            return boot_ret;
        }
        let utc_ret = if let Some(t) = self.utc_timers.peek().map(|node| node.id()) {
            if *t == *timer_id { self.utc_timers.pop() } else { None }
        } else {
            None
        };
        self.utc_timers.retain(|t| *t.id() != *timer_id);
        utc_ret
    }

    /// Returns the number of currently pending timers.
    pub fn timer_count(&self) -> usize {
        let boot_count = self.boot_timers.len();
        let utc_count = self.utc_timers.len();
        let count = self.active_timer_ids.len();
        assert!(
            boot_count + utc_count == count,
            "broken invariant: {boot_count}+{utc_count}=={count}"
        );
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;
    use std::rc::Rc;
    use test_case::test_case;
    use test_util::assert_gt;

    #[derive(Debug)]
    struct FakeResponder {}

    impl Responder for FakeResponder {
        fn send(
            &self,
            _alarm_id: &str,
            _result: Result<fidl::EventPair, fidl_fuchsia_time_alarms::WakeAlarmsError>,
        ) -> Option<Result<(), fidl::Error>> {
            None
        }
    }

    // Make sure the first alarm is always one with a greater koid value.
    #[test_case(
        "alarm1",
        fasync::BootInstant::from_nanos(42),
        "alarm2",
        fasync::BootInstant::from_nanos(42),
        std::cmp::Ordering::Less ; "same deadline tie broken by alarm id"
    )]
    #[test_case(
        "alarm1",
        fasync::BootInstant::from_nanos(42),
        "alarm2",
        fasync::BootInstant::from_nanos(43),
        std::cmp::Ordering::Greater ; "sooner deadline is greater"
    )]
    #[test_case(
        "alarm",
        fasync::BootInstant::from_nanos(42),
        "alarm",
        fasync::BootInstant::from_nanos(42),
        std::cmp::Ordering::Greater ; "same deadline and alarm id tie broken by connection ID"
    )]
    #[test_case(
        "alarm",
        fasync::BootInstant::from_nanos(42),
        "alarm",
        fasync::BootInstant::from_nanos(43),
        std::cmp::Ordering::Greater ; "different deadlines, sooner is 'greater'"
    )]
    fn test_timer_node_comparison_by_alarm_id(
        id1: &str,
        deadline1: fasync::BootInstant,
        id2: &str,
        deadline2: fasync::BootInstant,
        expected: std::cmp::Ordering,
    ) {
        let tr = Rc::new(RefCell::new(new_transform(
            fasync::BootInstant::from_nanos(0),
            fxr::UtcInstant::ZERO,
            1,
            1,
        )));
        let (koid1, koid2) = {
            let koid1 = new_koid();
            let koid2 = new_koid();
            (std::cmp::max(koid1, koid2), std::cmp::min(koid1, koid2))
        };

        assert_gt!(koid1, koid2);

        let one =
            Node::new_boot(deadline1, id1.into(), koid1, Rc::new(FakeResponder {}), tr.clone());
        let other = Node::new_boot(deadline2, id2.into(), koid2, Rc::new(FakeResponder {}), tr);

        assert_eq!(expected, one.cmp(&other), "one={one:?}, other={other:?}");
    }

    // Create a discardable koid for tests.
    fn new_koid() -> zx::Koid {
        zx::Event::create().as_handle_ref().koid().unwrap()
    }

    fn new_transform(
        boot_offset: fasync::BootInstant,
        utc_offset: fxr::UtcInstant,
        boot_ticks: u32,
        utc_ticks: u32,
    ) -> fxr::UtcClockTransform {
        fxr::UtcClockTransform {
            rate: zx::sys::zx_clock_rate_t {
                synthetic_ticks: utc_ticks,
                reference_ticks: boot_ticks,
            },
            reference_offset: boot_offset.into(),
            synthetic_offset: utc_offset.into(),
        }
    }

    #[fuchsia::test]
    async fn test_heap_operations() {
        let tr = Rc::new(RefCell::new(new_transform(
            fasync::BootInstant::from_nanos(100),
            fxr::UtcInstant::from_nanos(100),
            1,
            1,
        )));

        let mut heap = Heap::new(tr.clone());

        // Push one timer node, verify that it's registered.
        let node = heap.new_node_boot(
            fasync::BootInstant::from_nanos(42),
            "alarm1".into(),
            /*conn_id=*/ new_koid(),
            Rc::new(FakeResponder {}),
        );
        assert_eq!(None, heap.peek_node());
        heap.push(node);
        assert_eq!("alarm1", heap.peek_node().unwrap().id().alarm(), "{heap}");

        // Push another node, with a later deadline.
        let node = heap.new_node_boot(
            fasync::BootInstant::from_nanos(45),
            "alarm2".into(),
            /*conn_id=*/ new_koid(),
            Rc::new(FakeResponder {}),
        );
        heap.push(node);
        assert_eq!("alarm1", heap.peek_node().unwrap().id().alarm(), "{heap}");

        // Push another node, with an earlier deadline.
        let node = heap.new_node_boot(
            fasync::BootInstant::from_nanos(41),
            "alarm3".into(),
            /*conn_id=*/ new_koid(),
            Rc::new(FakeResponder {}),
        );
        heap.push(node);
        assert_eq!("alarm3", heap.peek_node().unwrap().id().alarm(), "{heap}");

        // Push a timer on the UTC timeline. For now, the rates are identical,
        // and references are the same.
        let node = heap.new_node_utc(
            fxr::UtcInstant::from_nanos(40),
            "utc_alarm4".into(),
            /*conn_id=*/ new_koid(),
            Rc::new(FakeResponder {}),
        );
        let id = node.id().clone();
        heap.push(node);
        assert_eq!("utc_alarm4", heap.peek_node().unwrap().id().alarm(), "{heap}");

        // Now change the transform to make this alarm not the earliest.
        *tr.borrow_mut() = new_transform(
            fasync::BootInstant::from_nanos(110),
            fxr::UtcInstant::from_nanos(10),
            1,
            1,
        );
        assert_eq!("alarm3", heap.peek_node().unwrap().id().alarm(), "{heap}");

        // Move the transform back. The UTC alarm becomes earliest again.
        *tr.borrow_mut() = new_transform(
            fasync::BootInstant::from_nanos(100),
            fxr::UtcInstant::from_nanos(100),
            1,
            1,
        );
        assert_eq!("utc_alarm4", heap.peek_node().unwrap().id().alarm(), "{heap}");

        // Remove the timer by ID.  The timer should be returned, and the deadline
        // should move to next timer.
        assert!(heap.remove_by_id(&id) != None, "{heap}");
        assert_eq!("alarm3", heap.peek_node().unwrap().id().alarm(), "{heap}");
        assert_eq!(3, heap.timer_count(), "{heap}");
    }

    #[fuchsia::test]
    async fn test_heap_earliest_removal() {
        let tr = Rc::new(RefCell::new(new_transform(
            fasync::BootInstant::from_nanos(100),
            fxr::UtcInstant::from_nanos(100),
            1,
            1,
        )));

        let mut heap = Heap::new(tr.clone());

        // Push one timer node, verify that it's registered.
        let node = heap.new_node_boot(
            fasync::BootInstant::from_nanos(42),
            "alarm".into(),
            /*conn_id=*/ new_koid(),
            Rc::new(FakeResponder {}),
        );
        assert_eq!(None, heap.peek_node());
        heap.push(node);
        assert_eq!("alarm", heap.peek_node().unwrap().id().alarm(), "{heap}");

        let node = heap.new_node_utc(
            fxr::UtcInstant::from_nanos(40),
            "utc_alarm".into(),
            /*conn_id=*/ new_koid(),
            Rc::new(FakeResponder {}),
        );
        heap.push(node);
        assert_eq!("utc_alarm", heap.peek_node().unwrap().id().alarm(), "{heap}");

        let now = fasync::BootInstant::from_nanos(200);
        assert_eq!("utc_alarm", heap.maybe_expire_earliest(now).unwrap().id().alarm(), "{heap}");
        assert_eq!("alarm", heap.maybe_expire_earliest(now).unwrap().id().alarm(), "{heap}");
        assert_eq!(None, heap.maybe_expire_earliest(now));
        assert_eq!(0, heap.timer_count(), "{heap}");
    }
}
