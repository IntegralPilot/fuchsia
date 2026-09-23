// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fake network definitions to place core contexts in a network together.

use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::fmt::Debug;
use core::hash::Hash;
use core::time::Duration;
use netstack3_hashmap::HashMap;

use packet::Buf;

use crate::InstantContext as _;
use crate::testutil::{
    FakeInstant, FakeTimerId, InstantAndData, WithFakeFrameContext, WithFakeTimerContext,
};

/// A fake network, composed of many `FakeCoreCtx`s.
///
/// Provides a utility to have many contexts keyed by `CtxId` that can
/// exchange frames.
pub struct FakeNetwork<Spec: FakeNetworkSpec, CtxId, Links> {
    links: Links,
    current_time: FakeInstant,
    pending_frames: BinaryHeap<PendingFrame<CtxId, Spec::RecvMeta>>,
    // Declare `contexts` last to ensure that it is dropped last. See
    // https://doc.rust-lang.org/std/ops/trait.Drop.html#drop-order for
    // details.
    contexts: HashMap<CtxId, Spec::Context>,
}

/// The data associated with a [`FakeNetwork`]'s pending frame.
#[derive(Debug)]
pub struct PendingFrameData<CtxId, Meta> {
    /// The frame's destination context ID.
    pub dst_context: CtxId,
    /// The associated frame metadata.
    pub meta: Meta,
    /// Frame contents.
    pub frame: Vec<u8>,
}

/// A [`PendingFrameData`] and the instant it was sent.
pub type PendingFrame<CtxId, Meta> = InstantAndData<PendingFrameData<CtxId, Meta>>;

/// A network spec that defines a [`FakeNetwork`].
pub trait FakeNetworkSpec: Sized {
    /// The context type, which represents a node in the network.
    type Context: WithFakeTimerContext<Self::TimerId>;
    /// The type of timer IDs handled by [`Self::Context`].
    type TimerId: Clone;
    /// The type of metadata associated with frames sent by this nodes in the
    /// network.
    type SendMeta;
    /// The type of metadata associated with frames received by nodes in the
    /// network.
    type RecvMeta;

    /// Handles a single received frame by `ctx`.
    fn handle_frame(ctx: &mut Self::Context, recv: Self::RecvMeta, data: Buf<Vec<u8>>);
    /// Handles a single timer id in `ctx`.
    ///
    /// `dispatch` is the timer's dispatch identifier, i.e., an implementer of
    /// `HandleableTimer`. `timer` is the unique timer identifier for the fake
    /// timer that fired.
    fn handle_timer(ctx: &mut Self::Context, dispatch: Self::TimerId, timer: FakeTimerId);
    /// Processes any context-internal queues, returning `true` if any work
    /// was done.
    ///
    /// This is used to drive queued frames that may be sitting inside the
    /// context and invisible to the [`FakeNetwork`].
    fn process_queues(ctx: &mut Self::Context) -> bool;

    /// Extracts accesses to fake frames from [`Self::Context`].
    fn fake_frames(ctx: &mut Self::Context) -> &mut impl WithFakeFrameContext<Self::SendMeta>;

    /// Creates a new fake network from this spec.
    fn new_network<CtxId, Links, I>(contexts: I, links: Links) -> FakeNetwork<Self, CtxId, Links>
    where
        CtxId: Eq + Hash + Copy + Debug,
        I: IntoIterator<Item = (CtxId, Self::Context)>,
        Links: FakeNetworkLinks<Self::SendMeta, Self::RecvMeta, CtxId>,
    {
        FakeNetwork::new(contexts, links)
    }
}

/// A set of links in a `FakeNetwork`.
///
/// A `FakeNetworkLinks` represents the set of links in a `FakeNetwork`.
/// It exposes the link information by providing the ability to map from a
/// frame's sending metadata - including its context, local state, and
/// `SendMeta` - to the set of appropriate receivers, each represented by
/// a context ID, receive metadata, and latency.
pub trait FakeNetworkLinks<SendMeta, RecvMeta, CtxId> {
    /// Maps a link from the send metadata of the context to the receive
    /// metadata.
    fn map_link(&self, ctx: CtxId, meta: SendMeta) -> Vec<(CtxId, RecvMeta, Option<Duration>)>;
}

impl<SendMeta, RecvMeta, CtxId, F: Fn(CtxId, SendMeta) -> Vec<(CtxId, RecvMeta, Option<Duration>)>>
    FakeNetworkLinks<SendMeta, RecvMeta, CtxId> for F
{
    fn map_link(&self, ctx: CtxId, meta: SendMeta) -> Vec<(CtxId, RecvMeta, Option<Duration>)> {
        (self)(ctx, meta)
    }
}

/// The result of a single step in a `FakeNetwork`
#[derive(Debug)]
pub struct StepResult {
    /// The number of timers fired.
    pub timers_fired: usize,
    /// The number of frames sent.
    pub frames_sent: usize,
    /// The number of contexts that had frames queued in their receive queues.
    pub contexts_with_queued_frames: usize,
}

impl StepResult {
    fn new_idle() -> Self {
        Self { timers_fired: 0, frames_sent: 0, contexts_with_queued_frames: 0 }
    }

    /// Returns `true` if the last step did not perform any operations.
    pub fn is_idle(&self) -> bool {
        return self.timers_fired == 0
            && self.frames_sent == 0
            && self.contexts_with_queued_frames == 0;
    }
}

impl<Spec, CtxId, Links> FakeNetwork<Spec, CtxId, Links>
where
    CtxId: Eq + Hash,
    Spec: FakeNetworkSpec,
{
    /// Retrieves a context named `context`.
    pub fn context<K: Into<CtxId>>(&mut self, context: K) -> &mut Spec::Context {
        self.contexts.get_mut(&context.into()).unwrap()
    }

    /// Calls `f` with a mutable reference to the context named `context`.
    pub fn with_context<K: Into<CtxId>, O, F: FnOnce(&mut Spec::Context) -> O>(
        &mut self,
        context: K,
        f: F,
    ) -> O {
        f(self.context(context))
    }
}

impl<Spec, CtxId, Links> FakeNetwork<Spec, CtxId, Links>
where
    Spec: FakeNetworkSpec,
    CtxId: Eq + Hash + Copy + Debug,
    Links: FakeNetworkLinks<Spec::SendMeta, Spec::RecvMeta, CtxId>,
{
    /// Creates a new `FakeNetwork`.
    ///
    /// Creates a new `FakeNetwork` with the collection of `FakeCoreCtx`s in
    /// `contexts`. `Ctx`s are named by type parameter `CtxId`.
    ///
    /// # Panics
    ///
    /// Calls to `new` will panic if given a `FakeCoreCtx` with timer events.
    /// `FakeCoreCtx`s given to `FakeNetwork` **must not** have any timer
    /// events already attached to them, because `FakeNetwork` maintains
    /// all the internal timers in dispatchers in sync to enable synchronous
    /// simulation steps.
    pub fn new<I: IntoIterator<Item = (CtxId, Spec::Context)>>(contexts: I, links: Links) -> Self {
        let mut contexts = contexts.into_iter().collect::<HashMap<_, _>>();
        // Take the current time to be the latest of the times of any of the
        // contexts. This ensures that no context has state which is based
        // on having observed a time in the future, which could cause bugs.
        // For any contexts which have a time further in the past, it will
        // appear as though time has jumped forwards, but that's fine. The
        // only way that this could be a problem would be if a timer were
        // installed which should have fired in the interim (code might
        // become buggy in this case). However, we assert below that no
        // timers are installed.
        let latest_time = contexts
            .iter()
            .map(|(_, ctx)| ctx.with_fake_timer_ctx(|ctx| ctx.instant.time))
            .max()
            // If `max` returns `None`, it means that we were called with no
            // contexts. That's kind of silly, but whatever - arbitrarily
            // choose the current time as the epoch.
            .unwrap_or_else(FakeInstant::default);

        assert!(
            !contexts
                .iter()
                .any(|(_, ctx)| { !ctx.with_fake_timer_ctx(|ctx| ctx.timers.is_empty()) }),
            "can't start network with contexts that already have timers set"
        );

        // Synchronize all contexts' current time to the latest time of any
        // of the contexts. See comment above for more details.
        for (_, ctx) in contexts.iter_mut() {
            ctx.with_fake_timer_ctx_mut(|ctx| ctx.instant.time = latest_time);
        }

        Self { contexts, current_time: latest_time, pending_frames: BinaryHeap::new(), links }
    }

    /// Iterates over pending frames in an arbitrary order.
    pub fn iter_pending_frames(
        &self,
    ) -> impl Iterator<Item = &PendingFrame<CtxId, Spec::RecvMeta>> {
        self.pending_frames.iter()
    }

    /// Asserts no pending frames exist.
    #[track_caller]
    pub fn assert_no_pending_frames(&self)
    where
        Spec::RecvMeta: Debug,
    {
        assert!(self.pending_frames.is_empty(), "pending frames: {:?}", self.pending_frames);
    }

    /// Drops all pending frames; they will not be delivered.
    pub fn drop_pending_frames(&mut self) {
        self.pending_frames.clear();
    }

    /// Performs a single step in network simulation.
    ///
    /// `step` performs a single logical step in the collection of `Ctx`s
    /// held by this `FakeNetwork`. A single step consists of the following
    /// operations:
    ///
    /// - All pending frames, kept in each `FakeCoreCtx`, are mapped to their
    ///   destination context/device pairs and moved to an internal
    ///   collection of pending frames.
    /// - The collection of pending timers and scheduled frames is inspected
    ///   and a simulation time step is retrieved, which will cause a next
    ///   event to trigger. The simulation time is updated to the new time.
    /// - All scheduled frames whose deadline is less than or equal to the
    ///   new simulation time are sent to their destinations, handled using
    ///   `handle_frame`.
    /// - All timer events whose deadline is less than or equal to the new
    ///   simulation time are fired, handled using `handle_timer`.
    ///
    /// If any new events are created during the operation of frames or
    /// timers, they **will not** be taken into account in the current
    /// `step`. That is, `step` collects all the pending events before
    /// dispatching them, ensuring that an infinite loop can't be created as
    /// a side effect of calling `step`.
    ///
    /// The return value of `step` indicates which of the operations were
    /// performed.
    ///
    /// # Panics
    ///
    /// If `FakeNetwork` was set up with a bad `links`, calls to `step` may
    /// panic when trying to route frames to their context/device
    /// destinations.
    pub fn step(&mut self) -> StepResult
    where
        Spec::TimerId: Debug,
    {
        self.step_with(|_, meta, buf| Some((meta, buf)))
    }

    /// Like [`FakeNetwork::step`], but receives a function
    /// `filter_map_frame` that can modify the an inbound frame before
    /// delivery or drop it altogether by returning `None`.
    pub fn step_with<
        F: FnMut(
            &mut Spec::Context,
            Spec::RecvMeta,
            Buf<Vec<u8>>,
        ) -> Option<(Spec::RecvMeta, Buf<Vec<u8>>)>,
    >(
        &mut self,
        filter_map_frame: F,
    ) -> StepResult
    where
        Spec::TimerId: Debug,
    {
        let mut ret = StepResult::new_idle();
        // Drive all queues before checking for the network and time
        // simulation.
        for (_, ctx) in self.contexts.iter_mut() {
            if Spec::process_queues(ctx) {
                ret.contexts_with_queued_frames += 1;
            }
        }

        self.collect_frames();

        let next_step = if let Some(t) = self.next_step() {
            t
        } else {
            return ret;
        };

        // This assertion holds the contract that `next_step` does not
        // return a time in the past.
        assert!(next_step >= self.current_time);

        // Move time forward:
        self.current_time = next_step;
        for (_, ctx) in self.contexts.iter_mut() {
            ctx.with_fake_timer_ctx_mut(|ctx| ctx.instant.time = next_step);
        }

        ret.frames_sent = self.dispatch_pending_frames(filter_map_frame);

        // Dispatch all pending timers.
        for (_, ctx) in self.contexts.iter_mut() {
            // We have to collect the timers before dispatching them, to
            // avoid an infinite loop in case handle_timer schedules another
            // timer for the same or older FakeInstant.
            let mut timers = Vec::<(Spec::TimerId, FakeTimerId)>::new();
            ctx.with_fake_timer_ctx_mut(|ctx| {
                while let Some(InstantAndData(t, timer)) = ctx.timers.peek()
                    && *t <= ctx.now()
                {
                    timers.push((timer.dispatch_id.clone(), timer.timer_id()));
                    assert_ne!(ctx.timers.pop(), None);
                }
            });

            for (dispatch_id, timer_id) in timers {
                Spec::handle_timer(ctx, dispatch_id, timer_id);
                ret.timers_fired += 1;
            }
        }
        ret
    }

    /// Collects and dispatches all pending frames without advancing time or
    /// triggering any timers.
    ///
    /// # Panics
    ///
    /// Panics under the same conditions as [`dispatch_pending_frames`].
    pub fn step_deliver_frames(&mut self) -> StepResult
    where
        Spec::TimerId: Debug,
    {
        self.step_deliver_frames_with(|_, meta, frame| Some((meta, frame)))
    }

    /// Like [`FakeNetwork::step_deliver_frames`], but receives a function
    /// `filter_map_frame` that can modify frames, or drop them if it returns
    /// `None`.
    pub fn step_deliver_frames_with<
        F: FnMut(
            &mut Spec::Context,
            Spec::RecvMeta,
            Buf<Vec<u8>>,
        ) -> Option<(Spec::RecvMeta, Buf<Vec<u8>>)>,
    >(
        &mut self,
        filter_map_frame: F,
    ) -> StepResult
    where
        Spec::TimerId: Debug,
    {
        let mut ret = StepResult::new_idle();
        // Drive all queues before checking for the network simulation.
        for (_, ctx) in self.contexts.iter_mut() {
            if Spec::process_queues(ctx) {
                ret.contexts_with_queued_frames += 1;
            }
        }

        self.collect_frames();
        ret.frames_sent = self.dispatch_pending_frames(filter_map_frame);

        ret
    }

    /// Runs the network until it is starved of events.
    ///
    /// # Panics
    ///
    /// Panics if 1,000,000 steps are performed without becoming idle.
    /// Also panics under the same conditions as [`step`].
    pub fn run_until_idle(&mut self)
    where
        Spec::TimerId: Debug,
    {
        self.run_until_idle_with(|_, meta, frame| Some((meta, frame)))
    }

    /// Like [`FakeNetwork::run_until_idle`] but receives a function
    /// `filter_map_frame` that can modify the an inbound frame before
    /// delivery or drop it altogether by returning `None`.
    pub fn run_until_idle_with<
        F: FnMut(
            &mut Spec::Context,
            Spec::RecvMeta,
            Buf<Vec<u8>>,
        ) -> Option<(Spec::RecvMeta, Buf<Vec<u8>>)>,
    >(
        &mut self,
        mut filter_map_frame: F,
    ) where
        Spec::TimerId: Debug,
    {
        for _ in 0..1_000_000 {
            if self.step_with(&mut filter_map_frame).is_idle() {
                return;
            }
        }
        panic!("FakeNetwork seems to have gotten stuck in a loop.");
    }

    /// Collects all queued frames.
    ///
    /// Collects all pending frames and schedules them for delivery to the
    /// destination context/device based on the result of `links`. The
    /// collected frames are queued for dispatching in the `FakeNetwork`,
    /// ordered by their scheduled delivery time given by the latency result
    /// provided by `links`.
    pub fn collect_frames(&mut self) {
        let all_frames = self.contexts.iter_mut().filter_map(|(n, ctx)| {
            Spec::fake_frames(ctx).with_fake_frame_ctx_mut(|ctx| {
                let frames = ctx.take_frames();
                if frames.is_empty() { None } else { Some((n.clone(), frames)) }
            })
        });

        for (src_context, frames) in all_frames {
            for (send_meta, frame) in frames.into_iter() {
                for (dst_context, recv_meta, latency) in self.links.map_link(src_context, send_meta)
                {
                    self.pending_frames.push(PendingFrame::new(
                        self.current_time + latency.unwrap_or(Duration::from_millis(0)),
                        PendingFrameData { frame: frame.clone(), dst_context, meta: recv_meta },
                    ));
                }
            }
        }
    }

    /// Dispatches scheduled frames that were previously collected with
    /// [`collect_frames`].
    ///
    /// Only frames for which the current deadline is less than or equal to the
    /// current simulation time are delivered.
    ///
    /// # Panics
    ///
    /// If `FakeNetwork` was set up with a bad `links`, calls may panic when
    /// trying to route frames to their context/device destinations.
    pub fn dispatch_pending_frames<
        F: FnMut(
            &mut Spec::Context,
            Spec::RecvMeta,
            Buf<Vec<u8>>,
        ) -> Option<(Spec::RecvMeta, Buf<Vec<u8>>)>,
    >(
        &mut self,
        mut filter_map_frame: F,
    ) -> usize {
        let mut frames_sent = 0;
        while let Some(InstantAndData(t, _)) = self.pending_frames.peek()
            && *t <= self.current_time
        {
            // We can unwrap because we just peeked.
            let PendingFrameData { dst_context, meta, frame } =
                self.pending_frames.pop().unwrap().1;
            let dst_context = self.context(dst_context);
            if let Some((meta, frame)) = filter_map_frame(dst_context, meta, Buf::new(frame, ..)) {
                Spec::handle_frame(dst_context, meta, frame)
            }
            frames_sent += 1;
        }

        frames_sent
    }

    /// Calculates the next `FakeInstant` when events are available.
    ///
    /// Returns the smallest `FakeInstant` greater than or equal to the
    /// current time for which an event is available. If no events are
    /// available, returns `None`.
    pub fn next_step(&self) -> Option<FakeInstant> {
        // Get earliest timer in all contexts.
        let next_timer = self
            .contexts
            .iter()
            .filter_map(|(_, ctx)| {
                ctx.with_fake_timer_ctx(|ctx| match ctx.timers.peek() {
                    Some(tmr) => Some(tmr.0),
                    None => None,
                })
            })
            .min();
        // Get the instant for the next packet.
        let next_packet_due = self.pending_frames.peek().map(|t| t.0);

        // Return the earliest of them both, and protect against returning a
        // time in the past.
        match next_timer {
            Some(t) if next_packet_due.is_some() => Some(t).min(next_packet_due),
            Some(t) => Some(t),
            None => next_packet_due,
        }
        .map(|t| t.max(self.current_time))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::vec;

    use crate::testutil::{FakeFrameCtx, FakeTimerCtx};
    use crate::{SendFrameContext as _, TimerContext as _};

    // Define some fake contexts and links specifically to test the fake
    // network timers implementation.
    #[derive(Default)]
    struct FakeNetworkTestCtx {
        timer_ctx: FakeTimerCtx<u32>,
        frame_ctx: FakeFrameCtx<()>,
        fired_timers: HashMap<u32, usize>,
        frames_received: usize,
    }

    impl FakeNetworkTestCtx {
        #[track_caller]
        fn drain_and_assert_timers(&mut self, iter: impl IntoIterator<Item = (u32, usize)>) {
            for (timer, fire_count) in iter {
                assert_eq!(self.fired_timers.remove(&timer), Some(fire_count), "for timer {timer}");
            }
            assert!(self.fired_timers.is_empty(), "remaining timers: {:?}", self.fired_timers);
        }

        /// Generates an arbitrary request.
        fn request() -> Vec<u8> {
            vec![1, 2, 3, 4]
        }

        /// Generates an arbitrary response.
        fn response() -> Vec<u8> {
            vec![4, 3, 2, 1]
        }
    }

    impl FakeNetworkSpec for FakeNetworkTestCtx {
        type Context = Self;
        type TimerId = u32;
        type SendMeta = ();
        type RecvMeta = ();

        fn handle_frame(ctx: &mut Self, _recv: (), data: Buf<Vec<u8>>) {
            ctx.frames_received += 1;
            // If data is a request, generate a response. This mimics ICMP echo
            // behavior.
            if data.into_inner() == Self::request() {
                ctx.frame_ctx.push((), Self::response())
            }
        }

        fn handle_timer(ctx: &mut Self, dispatch: u32, _: FakeTimerId) {
            *ctx.fired_timers.entry(dispatch).or_insert(0) += 1;
        }

        fn process_queues(_ctx: &mut Self) -> bool {
            false
        }

        fn fake_frames(ctx: &mut Self) -> &mut impl WithFakeFrameContext<Self::SendMeta> {
            ctx
        }
    }

    impl WithFakeFrameContext<()> for FakeNetworkTestCtx {
        fn with_fake_frame_ctx_mut<O, F: FnOnce(&mut FakeFrameCtx<()>) -> O>(&mut self, f: F) -> O {
            f(&mut self.frame_ctx)
        }
    }

    impl WithFakeTimerContext<u32> for FakeNetworkTestCtx {
        fn with_fake_timer_ctx<O, F: FnOnce(&FakeTimerCtx<u32>) -> O>(&self, f: F) -> O {
            f(&self.timer_ctx)
        }

        fn with_fake_timer_ctx_mut<O, F: FnOnce(&mut FakeTimerCtx<u32>) -> O>(
            &mut self,
            f: F,
        ) -> O {
            f(&mut self.timer_ctx)
        }
    }

    fn new_fake_network_with_latency(
        latency: Option<Duration>,
    ) -> FakeNetwork<FakeNetworkTestCtx, i32, impl FakeNetworkLinks<(), (), i32>> {
        FakeNetwork::new(
            [(1, FakeNetworkTestCtx::default()), (2, FakeNetworkTestCtx::default())],
            move |id, ()| {
                vec![(
                    match id {
                        1 => 2,
                        2 => 1,
                        _ => unreachable!(),
                    },
                    (),
                    latency,
                )]
            },
        )
    }

    #[test]
    fn timers() {
        let mut net = new_fake_network_with_latency(None);

        let (mut t1, mut t4, mut t5) =
            net.with_context(1, |FakeNetworkTestCtx { timer_ctx, .. }| {
                (timer_ctx.new_timer(1), timer_ctx.new_timer(4), timer_ctx.new_timer(5))
            });

        net.with_context(1, |FakeNetworkTestCtx { timer_ctx, .. }| {
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(1), &mut t1), None);
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(4), &mut t4), None);
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(5), &mut t5), None);
        });

        let (mut t2, mut t3, mut t6) =
            net.with_context(2, |FakeNetworkTestCtx { timer_ctx, .. }| {
                (timer_ctx.new_timer(2), timer_ctx.new_timer(3), timer_ctx.new_timer(6))
            });

        net.with_context(2, |FakeNetworkTestCtx { timer_ctx, .. }| {
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(2), &mut t2), None);
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(3), &mut t3), None);
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(5), &mut t6), None);
        });

        // No timers fired before.
        net.context(1).drain_and_assert_timers([]);
        net.context(2).drain_and_assert_timers([]);
        assert_eq!(net.step().timers_fired, 1);
        // Only timer in context 1 should have fired.
        net.context(1).drain_and_assert_timers([(1, 1)]);
        net.context(2).drain_and_assert_timers([]);
        assert_eq!(net.step().timers_fired, 1);
        // Only timer in context 2 should have fired.
        net.context(1).drain_and_assert_timers([]);
        net.context(2).drain_and_assert_timers([(2, 1)]);
        assert_eq!(net.step().timers_fired, 1);
        // Only timer in context 2 should have fired.
        net.context(1).drain_and_assert_timers([]);
        net.context(2).drain_and_assert_timers([(3, 1)]);
        assert_eq!(net.step().timers_fired, 1);
        // Only timer in context 1 should have fired.
        net.context(1).drain_and_assert_timers([(4, 1)]);
        net.context(2).drain_and_assert_timers([]);
        assert_eq!(net.step().timers_fired, 2);
        // Both timers have fired at the same time.
        net.context(1).drain_and_assert_timers([(5, 1)]);
        net.context(2).drain_and_assert_timers([(6, 1)]);

        assert!(net.step().is_idle());
        // Check that current time on contexts tick together.
        let t1 = net.with_context(1, |FakeNetworkTestCtx { timer_ctx, .. }| timer_ctx.now());
        let t2 = net.with_context(2, |FakeNetworkTestCtx { timer_ctx, .. }| timer_ctx.now());
        assert_eq!(t1, t2);
    }

    #[test]
    fn until_idle() {
        let mut net = new_fake_network_with_latency(None);

        let mut t1 =
            net.with_context(1, |FakeNetworkTestCtx { timer_ctx, .. }| timer_ctx.new_timer(1));
        net.with_context(1, |FakeNetworkTestCtx { timer_ctx, .. }| {
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(1), &mut t1), None);
        });

        let (mut t2, mut t3) = net.with_context(2, |FakeNetworkTestCtx { timer_ctx, .. }| {
            (timer_ctx.new_timer(2), timer_ctx.new_timer(3))
        });
        net.with_context(2, |FakeNetworkTestCtx { timer_ctx, .. }| {
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(2), &mut t2), None);
            assert_eq!(timer_ctx.schedule_timer(Duration::from_secs(3), &mut t3), None);
        });

        while !net.step().is_idle() && net.context(1).fired_timers.len() < 1
            || net.context(2).fired_timers.len() < 1
        {}
        // Assert that we stopped before all times were fired, meaning we can
        // step again.
        assert_eq!(net.step().timers_fired, 1);
    }

    #[test]
    fn delayed_packets() {
        // Create a network that takes 5ms to get any packet to go through.
        let mut net = new_fake_network_with_latency(Some(Duration::from_millis(5)));

        // 1 sends 2 a request and schedules a timer.
        let mut t11 =
            net.with_context(1, |FakeNetworkTestCtx { timer_ctx, .. }| timer_ctx.new_timer(1));
        net.with_context(1, |FakeNetworkTestCtx { frame_ctx, timer_ctx, .. }| {
            frame_ctx.push((), FakeNetworkTestCtx::request());
            assert_eq!(timer_ctx.schedule_timer(Duration::from_millis(3), &mut t11), None);
        });
        // 2 schedules some timers.
        let (mut t21, mut t22) = net.with_context(2, |FakeNetworkTestCtx { timer_ctx, .. }| {
            (timer_ctx.new_timer(1), timer_ctx.new_timer(2))
        });
        net.with_context(2, |FakeNetworkTestCtx { timer_ctx, .. }| {
            assert_eq!(timer_ctx.schedule_timer(Duration::from_millis(7), &mut t22), None);
            assert_eq!(timer_ctx.schedule_timer(Duration::from_millis(10), &mut t21), None);
        });

        // Order of expected events is as follows:
        // - ctx1's timer expires at t = 3
        // - ctx2 receives ctx1's packet at t = 5
        // - ctx2's timer expires at t = 7
        // - ctx1 receives ctx2's response and ctx2's last timer fires at t = 10

        let assert_full_state = |net: &mut FakeNetwork<FakeNetworkTestCtx, _, _>,
                                 ctx1_timers,
                                 ctx2_timers,
                                 ctx2_frames,
                                 ctx1_frames| {
            let ctx1 = net.context(1);
            assert_eq!(ctx1.fired_timers.len(), ctx1_timers);
            assert_eq!(ctx1.frames_received, ctx1_frames);
            let ctx2 = net.context(2);
            assert_eq!(ctx2.fired_timers.len(), ctx2_timers);
            assert_eq!(ctx2.frames_received, ctx2_frames);
        };

        assert_eq!(net.step().timers_fired, 1);
        assert_full_state(&mut net, 1, 0, 0, 0);
        assert_eq!(net.step().frames_sent, 1);
        assert_full_state(&mut net, 1, 0, 1, 0);
        assert_eq!(net.step().timers_fired, 1);
        assert_full_state(&mut net, 1, 1, 1, 0);
        let step = net.step();
        assert_eq!(step.frames_sent, 1);
        assert_eq!(step.timers_fired, 1);
        assert_full_state(&mut net, 1, 2, 1, 1);

        // Should've starved all events.
        assert!(net.step().is_idle());
    }

    #[test]
    fn fake_network_transmits_packets() {
        let mut net = new_fake_network_with_latency(None);

        // Send a frame from 1 to 2.
        net.with_context(1, |FakeNetworkTestCtx { frame_ctx, .. }| {
            frame_ctx.send_frame(&mut (), (), Buf::new(FakeNetworkTestCtx::request(), ..)).unwrap();
        });

        // Send from 1 to 2.
        assert_eq!(net.step().frames_sent, 1);
        // Respond from 2 to 1.
        assert_eq!(net.step().frames_sent, 1);
        // Should've starved all events.
        assert!(net.step().is_idle());
    }

    #[test]
    fn send_to_many() {
        let mut net = FakeNetworkTestCtx::new_network(
            [
                (1, FakeNetworkTestCtx::default()),
                (2, FakeNetworkTestCtx::default()),
                (3, FakeNetworkTestCtx::default()),
            ],
            |id, ()| match id {
                // 1 sends to both 2 and 3.
                1 => vec![(2, (), None), (3, (), None)],
                // 2 only sends to 1.
                2 => vec![(1, (), None)],
                // 3 doesn't send anything.
                3 => vec![],
                _ => unreachable!(),
            },
        );
        net.assert_no_pending_frames();

        // 2 and 3 should get any packet sent by 1.
        net.with_context(1, |FakeNetworkTestCtx { frame_ctx, .. }| {
            frame_ctx.send_frame(&mut (), (), Buf::new(vec![], ..)).unwrap();
        });
        net.collect_frames();
        assert_eq!(net.iter_pending_frames().count(), 2);
        assert!(net.iter_pending_frames().any(|InstantAndData(_, x)| x.dst_context == 2));
        assert!(net.iter_pending_frames().any(|InstantAndData(_, x)| x.dst_context == 3));
        net.drop_pending_frames();

        // Only 1 should get packets sent by 2.
        net.with_context(2, |FakeNetworkTestCtx { frame_ctx, .. }| {
            frame_ctx.send_frame(&mut (), (), Buf::new(vec![], ..)).unwrap();
        });
        net.collect_frames();
        assert_eq!(net.iter_pending_frames().count(), 1);
        assert!(net.iter_pending_frames().any(|InstantAndData(_, x)| x.dst_context == 1));
        net.drop_pending_frames();

        // No one receives packets sent by 3.
        net.with_context(3, |FakeNetworkTestCtx { frame_ctx, .. }| {
            frame_ctx.send_frame(&mut (), (), Buf::new(vec![], ..)).unwrap();
        });
        net.collect_frames();
        net.assert_no_pending_frames();

        // Because we didn't run the simulation, no one actually received any of
        // these frames they were always in the pending queue.
        for i in 1..=3 {
            assert_eq!(net.context(i).frames_received, 0, "context: {i}");
        }
    }
}
