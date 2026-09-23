// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::hanging_get::error::HangingGetServerError;
use core::hash::Hash;
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// A broker of updates to some state, using the hanging get pattern.
///
/// The broker stores a state, and mediates state changes between
/// [Publisher]s and [Subscriber]s.  Any [Publisher] can change the state at any
/// time.  When the state is changed, all [Subscriber]s are notified with the new
/// state.
///
/// To follow the hanging get pattern, when a [Subscriber] is first created,
/// the current state is sent immediately.  Subsequent updates to the same
/// subscriber are only sent when the state is modified.
///
/// * Use [HangingGet::new] to create a new broker for a single state item.
/// * Use [HangingGet::new_publisher] to create an object that allows you to
///   update the state.
/// * Use [HangingGet::new_subscriber] to create an object that monitors for
///   updates to the state.
///
/// ## Type parameters
///
/// * `S`: the type of the stored hanging get state.  This is the state that gets
///   communicated to subscribers.
/// * `O`: The type of the object used to notify of state change.
/// * `F`: The type of a function used to send the new state content to an instance
///   of `O`.  `F` gets passed the content of the new state, the object that it
///   needs to notify, and is expected to return `true` if the notification was
///   a success; otherwise it must return `false`.
pub struct HangingGet<S, O, F: Fn(&S, O) -> bool> {
    inner: Arc<Mutex<HangingGetInner<S, subscriber_key::Key, O, F>>>,
    /// A [subscriber_key::Key] held by the broker to track the next unique key that the broker can
    /// assign to a [Subscriber].
    subscriber_key_generator: subscriber_key::Generator,
}

impl<S, O, F> HangingGet<S, O, F>
where
    F: Fn(&S, O) -> bool,
{
    /// Create a new broker.
    ///
    /// ## Args:
    ///
    /// * `state` is the initial state of the [HangingGet].
    /// * `notify` is a function to notify observers of state of the state change.
    pub fn new(state: S, notify: F) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HangingGetInner::new(Some(state), notify))),
            subscriber_key_generator: subscriber_key::Generator::default(),
        }
    }

    /// Create a new broker, but delays any subscribers from being notified until the value is
    /// initialized.
    ///
    /// ## Args:
    ///
    /// * `notify` is a function to notify observers of state of the state change.
    ///
    /// # Disclaimer:
    /// This initialier is more prone to cause hangs if code is not properly setup. This
    /// should only be used for patterns where the is no useful default state.
    pub fn new_unknown_state(notify: F) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HangingGetInner::new(None, notify))),
            subscriber_key_generator: subscriber_key::Generator::default(),
        }
    }

    /// Create a new [Publisher] that can make atomic updates to the state value.
    pub fn new_publisher(&self) -> Publisher<S, O, F> {
        Publisher { inner: self.inner.clone() }
    }

    /// Create a new [Subscriber] that represents a single hanging get client.
    ///
    /// The newly-created subscriber will be notified with the current state
    /// immediately.  After the first notification, the subscriber will be
    /// notified only if the state changes.
    pub fn new_subscriber(&mut self) -> Subscriber<S, O, F> {
        Subscriber { inner: self.inner.clone(), key: self.subscriber_key_generator.next().unwrap() }
    }
}

/// A `Subscriber` can be used to register observation requests with the `HangingGet`.
/// These will be notified when the state changes or immediately the first time
/// a `Subscriber` registers an observation.
///
/// ## Type parameters
///
/// See [HangingGet] for the explanation of the type parameters `S`, `O`, `F`.
pub struct Subscriber<S, O, F: Fn(&S, O) -> bool> {
    inner: Arc<Mutex<HangingGetInner<S, subscriber_key::Key, O, F>>>,
    key: subscriber_key::Key,
}

impl<S, O, F> Subscriber<S, O, F>
where
    F: Fn(&S, O) -> bool,
{
    /// Register a new observation.
    ///
    /// Errors occur when:
    /// * A Subscriber attempts to register an observation when there is already an outstanding
    ///   observation waiting on updates.
    ///
    /// Returns `observation` on error.
    pub fn register(&self, observation: O) -> Result<(), HangingGetServerError> {
        self.register2(observation).map_err(|_| HangingGetServerError::MultipleObservers)
    }
    /// Register a new observation.
    ///
    /// Errors occur when:
    /// * A Subscriber attempts to register an observation when there is already an outstanding
    ///   observation waiting on updates.
    ///
    /// Returns `observation` on error.
    pub fn register2(&self, observation: O) -> Result<(), O> {
        self.inner.lock().subscribe(self.key, observation)
    }
}

impl<S, O, F> Drop for Subscriber<S, O, F>
where
    F: Fn(&S, O) -> bool,
{
    fn drop(&mut self) {
        self.inner.lock().unsubscribe(self.key);
    }
}

/// A `Publisher` is used to make changes to the state contained within a `HangingGet`.
/// It is designed to be cheaply cloned and `Send`.
///
/// ## Type parameters
///
/// See [HangingGet] for the explanation of the generic types `S`, `O`, `F`.
pub struct Publisher<S, O, F: Fn(&S, O) -> bool> {
    inner: Arc<Mutex<HangingGetInner<S, subscriber_key::Key, O, F>>>,
}

impl<S, O, F: Fn(&S, O) -> bool> Clone for Publisher<S, O, F> {
    /// Clones this [Publisher].
    ///
    /// It is cheap to clone a [Publisher].
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<S, O, F> Publisher<S, O, F>
where
    F: Fn(&S, O) -> bool,
{
    /// Set the stored state to `S`. Subscribers will be updated.
    pub fn set(&self, state: S) {
        self.inner.lock().set(state)
    }

    /// Pass a function to the hanging get that can update the hanging get state in place.
    ///
    /// Any subscriber that has registered an observer will immediately be notified of the
    /// update.
    ///
    /// ## Type parameters
    ///
    /// * `UpdateFn`: an update function: gets passed the new state, and returns `true`
    ///   if the state has been updated with success.
    pub fn update<UpdateFn>(&self, update: UpdateFn)
    where
        UpdateFn: FnOnce(&mut Option<S>) -> bool,
    {
        self.inner.lock().update(update)
    }
}

/// A [HangingGetInner] object manages some internal state `S` and notifies observers of type `O`
/// when their view of the state is outdated.
///
/// While it _can_ be used directly, most API consumers will want to use the higher level
/// [HangingGet] object. `HangingGet` and its companion types provide `Send`
/// for use from multiple threads or async tasks.
///
/// ## Type parameters
///
/// * `K` - the Key by which Observers are identified.
/// * For other type args see [HangingGet].
pub struct HangingGetInner<S, K, O, F: Fn(&S, O) -> bool> {
    state: Option<S>,
    notify: F,
    observers: HashMap<K, Window<O>>,
}

impl<S, K, O, F> HangingGetInner<S, K, O, F>
where
    K: Eq + Hash,
    F: Fn(&S, O) -> bool,
{
    fn notify_all(&mut self) {
        for window in self.observers.values_mut() {
            window.notify(&self.notify, self.state.as_ref().unwrap());
        }
    }

    /// Create a new `HangingGetInner`.
    /// `state` is the initial state of the HangingGetInner
    /// `notify` is a `Fn` that is used to notify observers of state.
    pub fn new(state: Option<S>, notify: F) -> Self {
        Self { state, notify, observers: HashMap::new() }
    }

    /// Set the internal state value to `state` and notify all observers.
    /// Note that notification is not conditional on the _value_ set by calling the `set` function.
    /// Notification will occur regardless of whether the `state` value differs from the value
    /// currently stored by `HangingGetInner`.
    pub fn set(&mut self, state: S) {
        self.state = Some(state);
        self.notify_all();
    }

    /// Modify the internal state in-place using the `state_update` function. Notify all
    /// observers if `state_update` returns true.
    pub fn update(&mut self, state_update: impl FnOnce(&mut Option<S>) -> bool) {
        if state_update(&mut self.state) {
            self.notify_all();
        }
    }

    /// Register an observer as a subscriber of the state.
    ///
    /// Observers are grouped by key and all observers will the same key are assumed to have
    /// received the latest state update. If an observer with a previously unseen key subscribes,
    /// it is immediately notified to the stated. If an observer with a known key subscribes, it
    /// will only be notified when the state is updated since last sent to an observer with the same
    /// key. All unresolved observers will be resolved to the same value immediately after the state
    /// is updated. If there is no stored state, then the notification will be delayed until an
    /// update is made.
    ///
    /// Returns `observer` on error.
    pub fn subscribe(&mut self, key: K, observer: O) -> Result<(), O> {
        let entry = self.observers.entry(key).or_insert_with(Window::new);
        entry.observe(observer, &self.notify, self.state.as_ref())
    }

    /// Deregister all observers that subscribed with `key`. If an observer is subsequently
    /// subscribed with the same `key` value, it will be treated as a previously unseen `key`.
    pub fn unsubscribe(&mut self, key: K) {
        drop(self.observers.remove(&key));
    }
}

/// Window tracks all observers for a given `key` and whether their view of the state is
/// `dirty` or not.
struct Window<O> {
    dirty: bool,
    observer: Option<O>,
}

impl<O> Window<O> {
    /// Create a new `Window` without an `observer` and an initial `dirty` value of `true`.
    pub fn new() -> Self {
        Window { dirty: true, observer: None }
    }

    /// Register a new observer. The observer will be notified immediately if the `Window`
    /// has a dirty view of the state. The observer will be stored for future notification
    /// if the `Window` does not have a dirty view.
    ///
    /// Returns `Err(observer)` if the window already has an observer.
    pub fn observe<S>(
        &mut self,
        observer: O,
        f: impl Fn(&S, O) -> bool,
        current_state: Option<&S>,
    ) -> Result<(), O> {
        if self.observer.is_some() {
            return Err(observer);
        }
        self.observer = Some(observer);
        if let Some(current_state) = current_state {
            if self.dirty {
                self.notify(f, current_state);
            }
        }
        Ok(())
    }

    /// Notify the observer _if and only if_ the `Window` has a dirty view of `state`.
    /// If an observer was present and notified, the `Window` no longer has a dirty view
    /// after this method returns.
    pub fn notify<S>(&mut self, f: impl Fn(&S, O) -> bool, state: &S) {
        match self.observer.take() {
            Some(observer) => {
                if f(state, observer) {
                    self.dirty = false;
                }
            }
            None => self.dirty = true,
        }
    }
}

/// Submodule used to keep the internals of `Key`s and `Generator`s inaccessable.
mod subscriber_key {
    /// Manages the creation and distribution of unique `Key`s
    pub struct Generator {
        next: Key,
    }

    impl Default for Generator {
        fn default() -> Self {
            Self { next: Key(0) }
        }
    }

    impl Generator {
        /// Get a unique key.
        /// Returns `None` if 2^64-2 keys have already been generated.
        pub fn next(&mut self) -> Option<Key> {
            let key = self.next.clone();
            if let Some(next) = self.next.0.checked_add(1) {
                self.next.0 = next;
                Some(key)
            } else {
                None
            }
        }
    }

    /// An internal per-subscriber key that is intended to be unique.
    #[derive(PartialEq, Eq, Hash, Debug, Clone, Copy)]
    pub struct Key(u64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PollExt;
    use crate::hanging_get::test_util::TestObserver;
    use fuchsia_async as fasync;
    use futures::channel::oneshot;

    #[test]
    fn subscriber_key_generator_creates_unique_keys() {
        let mut generator = subscriber_key::Generator::default();
        let key1 = generator.next();
        let key2 = generator.next();
        assert!(key1 != key2);
    }

    #[test]
    fn window_add_first_observer_notifies() {
        let state = 0;
        let mut window = Window::new();
        window
            .observe(TestObserver::expect_value(state), TestObserver::observe, Some(&state))
            .unwrap();
    }

    #[test]
    fn window_none_state_does_not_notify() {
        let mut window = Window::new();
        window
            .observe::<i32>(TestObserver::expect_no_value(), TestObserver::observe, None)
            .unwrap();
    }

    #[test]
    fn window_add_second_observer_does_not_notify() {
        let state = 0;
        let mut window = Window::new();
        window
            .observe(TestObserver::expect_value(state), TestObserver::observe, Some(&state))
            .unwrap();

        // Second observer added without updating the value
        window
            .observe(TestObserver::expect_no_value(), TestObserver::observe, Some(&state))
            .unwrap();
    }

    #[test]
    fn window_add_second_observer_notifies_after_notify_call() {
        let mut state = 0;
        let mut window = Window::new();
        window
            .observe(TestObserver::expect_value(state), TestObserver::observe, Some(&state))
            .unwrap();

        state = 1;
        window.notify(TestObserver::observe, &state);

        // Second observer added without updating the value
        window
            .observe(TestObserver::expect_value(state), TestObserver::observe, Some(&state))
            .unwrap();
    }

    #[test]
    fn window_add_multiple_observers_are_notified() {
        let mut state = 0;
        let mut window = Window::new();
        window
            .observe(TestObserver::expect_value(state), TestObserver::observe, Some(&state))
            .unwrap();

        // Second observer added without updating the value
        let o1 = TestObserver::expect_value(1);
        let o2 = TestObserver::expect_no_value();
        window.observe(o1.clone(), TestObserver::observe, Some(&state)).unwrap();
        let result = window.observe(o2.clone(), TestObserver::observe, Some(&state));
        assert_eq!(result, Err(o2));
        assert!(!o1.has_value());
        state = 1;
        window.notify(TestObserver::observe, &state);
    }

    #[test]
    fn window_dirty_flag_state() {
        let state = 0;
        let mut window = Window::new();
        let o = TestObserver::expect_value(state);
        window.observe(o, TestObserver::observe, Some(&state)).unwrap();
        assert!(window.observer.is_none());
        assert!(!window.dirty);
        window.notify(TestObserver::observe, &state);
        assert!(window.dirty);
        let o = TestObserver::expect_value(state);
        window.observe(o, TestObserver::observe, Some(&state)).unwrap();
        assert!(!window.dirty);
    }

    #[test]
    fn window_dirty_flag_respects_consumed_flag() {
        let state = 0;
        let mut window = Window::new();

        let o = TestObserver::expect_value(state);
        window.observe(o, TestObserver::observe_incomplete, Some(&state)).unwrap();
        assert!(window.dirty);
    }

    #[test]
    fn hanging_get_inner_subscribe() {
        let mut hanging = HangingGetInner::new(Some(0), TestObserver::observe);
        let o = TestObserver::expect_value(0);
        assert!(!o.has_value());
        hanging.subscribe(0, o.clone()).unwrap();
    }

    #[test]
    fn hanging_get_inner_subscribe_then_set() {
        let mut hanging = HangingGetInner::new(Some(0), TestObserver::observe);
        let o = TestObserver::expect_value(0);
        hanging.subscribe(0, o.clone()).unwrap();

        // No change without a new subscription
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_subscribe_twice_then_set() {
        let mut hanging = HangingGetInner::new(Some(0), TestObserver::observe);
        hanging.subscribe(0, TestObserver::expect_value(0)).unwrap();

        hanging.subscribe(0, TestObserver::expect_value(1)).unwrap();
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_subscribe_multiple_then_set() {
        let mut hanging = HangingGetInner::new(Some(0), TestObserver::observe);
        hanging.subscribe(0, TestObserver::expect_value(0)).unwrap();

        // A second subscription with the same client key should not be notified
        let o2 = TestObserver::expect_value(1);
        hanging.subscribe(0, o2.clone()).unwrap();
        assert!(!o2.has_value());

        // A third subscription will queue up along the other waiting observer
        let _ = hanging.subscribe(0, TestObserver::expect_no_value()).unwrap_err();

        // Set should notify all observers to the change
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_subscribe_with_two_clients_then_set() {
        let mut hanging = HangingGetInner::new(Some(0), TestObserver::observe);
        hanging.subscribe(0, TestObserver::expect_value(0)).unwrap();
        hanging.subscribe(0, TestObserver::expect_value(1)).unwrap();
        hanging.subscribe(1, TestObserver::expect_value(0)).unwrap();
        hanging.subscribe(1, TestObserver::expect_value(1)).unwrap();
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_unsubscribe() {
        let mut hanging = HangingGetInner::new(Some(0), TestObserver::observe);
        hanging.subscribe(0, TestObserver::expect_value(0)).unwrap();
        hanging.subscribe(0, TestObserver::expect_no_value()).unwrap();
        hanging.unsubscribe(0);
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_unsubscribe_one_of_many() {
        let mut hanging = HangingGetInner::new(Some(0), TestObserver::observe);

        hanging.subscribe(0, TestObserver::expect_value(0)).unwrap();
        hanging.subscribe(0, TestObserver::expect_no_value()).unwrap();
        hanging.subscribe(1, TestObserver::expect_value(0)).unwrap();
        hanging.subscribe(1, TestObserver::expect_no_value()).unwrap();

        // Unsubscribe one of the two observers
        hanging.unsubscribe(0);
        assert!(!hanging.observers.contains_key(&0));
        assert!(hanging.observers.contains_key(&1));
    }

    #[test]
    fn hanging_get_inner_delayed_subscribe() {
        let mut hanging = HangingGetInner::new(None, TestObserver::<u8>::observe);
        let o = TestObserver::expect_no_value();
        assert!(!o.has_value());
        hanging.subscribe(0, o.clone()).unwrap();
    }

    #[test]
    fn hanging_get_inner_delayed_subscribe_then_set() {
        let mut hanging = HangingGetInner::new(None, TestObserver::observe);
        let o = TestObserver::expect_value(1);
        hanging.subscribe(0, o.clone()).unwrap();

        // Initial value now set.
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_delayed_subscribe_twice_then_set() {
        let mut hanging = HangingGetInner::new(None, TestObserver::observe);
        hanging.subscribe(0, TestObserver::expect_value(1)).unwrap();

        // Since the first result is delayed, subscribing again is an error.
        let o = TestObserver::expect_no_value();
        let result = hanging.subscribe(0, o.clone());
        assert_eq!(result, Err(o));
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_delayed_subscribe_multiple_then_set() {
        let mut hanging = HangingGetInner::new(None, TestObserver::observe);
        hanging.subscribe(0, TestObserver::expect_value(1)).unwrap();

        // A second subscription with the same client key should fail while the subscription hasn't
        // been notified
        let o2 = TestObserver::expect_no_value();
        let result = hanging.subscribe(0, o2.clone());
        assert_eq!(result, Err(o2.clone()));
        assert!(!o2.has_value());

        // A third subscription will also fail for the same reason.
        let o3 = TestObserver::expect_no_value();
        let result = hanging.subscribe(0, o3.clone());
        assert_eq!(result, Err(o3));

        // Set should notify all observers to the change
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_delayed_subscribe_with_two_clients_then_set() {
        let mut hanging = HangingGetInner::new(None, TestObserver::observe);
        hanging.subscribe(0, TestObserver::expect_value(1)).unwrap();
        let o = TestObserver::expect_no_value();
        let result = hanging.subscribe(0, o.clone());
        assert_eq!(result, Err(o));
        hanging.subscribe(1, TestObserver::expect_value(1)).unwrap();
        let o2 = TestObserver::expect_no_value();
        let result = hanging.subscribe(1, o2.clone());
        assert_eq!(result, Err(o2));
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_delayed_unsubscribe() {
        let mut hanging = HangingGetInner::new(None, TestObserver::observe);
        hanging.subscribe(0, TestObserver::expect_no_value()).unwrap();
        let o = TestObserver::expect_no_value();
        let result = hanging.subscribe(0, o.clone());
        assert_eq!(result, Err(o));
        hanging.unsubscribe(0);
        hanging.set(1);
    }

    #[test]
    fn hanging_get_inner_delayed_unsubscribe_one_of_many() {
        let mut hanging = HangingGetInner::new(None, TestObserver::observe);

        hanging.subscribe(0, TestObserver::<i32>::expect_no_value()).unwrap();
        let o = TestObserver::expect_no_value();
        let result = hanging.subscribe(0, o.clone());
        assert_eq!(result, Err(o));
        hanging.subscribe(1, TestObserver::expect_no_value()).unwrap();
        let o2 = TestObserver::expect_no_value();
        let result = hanging.subscribe(1, o2.clone());
        assert_eq!(result, Err(o2));

        // Unsubscribe one of the two observers
        hanging.unsubscribe(0);
        assert!(!hanging.observers.contains_key(&0));
        assert!(hanging.observers.contains_key(&1));
    }

    #[test]
    fn sync_pub_sub_updates_and_observes() {
        let mut ex = fasync::TestExecutor::new();
        let mut broker = HangingGet::new(0i32, |s, o: oneshot::Sender<_>| {
            o.send(s.clone()).map(|()| true).unwrap()
        });
        let publisher = broker.new_publisher();
        let subscriber = broker.new_subscriber();

        // Initial observation is immediate
        let (sender, mut receiver) = oneshot::channel();
        subscriber.register(sender).unwrap();
        let observation =
            ex.run_until_stalled(&mut receiver).expect("received initial observation");
        assert_eq!(observation, Ok(0));

        // Subsequent observations do not happen until after an update
        let (sender, mut receiver) = oneshot::channel();
        subscriber.register(sender).unwrap();
        assert!(ex.run_until_stalled(&mut receiver).is_pending());

        publisher.set(1);

        let observation =
            ex.run_until_stalled(&mut receiver).expect("received subsequent observation");
        assert_eq!(observation, Ok(1));
    }

    #[test]
    fn sync_pub_sub_multiple_subscribers() {
        let mut ex = fasync::TestExecutor::new();
        let mut broker = HangingGet::new(0i32, |s, o: oneshot::Sender<_>| {
            o.send(s.clone()).map(|()| true).unwrap()
        });
        let publisher = broker.new_publisher();

        let sub1 = broker.new_subscriber();
        let sub2 = broker.new_subscriber();

        // Initial observation for subscribers is immediate
        let (sender, mut receiver) = oneshot::channel();
        sub1.register(sender).unwrap();
        let observation =
            ex.run_until_stalled(&mut receiver).expect("received initial observation");
        assert_eq!(observation, Ok(0));

        let (sender, mut receiver) = oneshot::channel();
        sub2.register(sender).unwrap();
        let observation =
            ex.run_until_stalled(&mut receiver).expect("received initial observation");
        assert_eq!(observation, Ok(0));

        // Subsequent observations do not happen until after an update
        let (sender, mut recv1) = oneshot::channel();
        sub1.register(sender).unwrap();
        assert!(ex.run_until_stalled(&mut recv1).is_pending());

        let (sender, mut recv2) = oneshot::channel();
        sub2.register(sender).unwrap();
        assert!(ex.run_until_stalled(&mut recv2).is_pending());

        publisher.set(1);
        let obs1 =
            ex.run_until_stalled(&mut recv1).expect("receiver 1 received subsequent observation");
        assert_eq!(obs1, Ok(1));
        let obs2 =
            ex.run_until_stalled(&mut recv2).expect("receiver 2 received subsequent observation");
        assert_eq!(obs2, Ok(1));
    }

    #[test]
    fn sync_pub_sub_delayed_updates_and_observes() {
        let mut ex = fasync::TestExecutor::new();
        let mut broker = HangingGet::<i32, _, _>::new_unknown_state(|s, o: oneshot::Sender<_>| {
            o.send(s.clone()).map(|()| true).unwrap()
        });
        let publisher = broker.new_publisher();
        let subscriber = broker.new_subscriber();

        // Initial observation is delayed due to lack of initial state.
        let (sender, mut recv1) = oneshot::channel();
        subscriber.register(sender).unwrap();
        assert!(ex.run_until_stalled(&mut recv1).is_pending());

        // Subsequent registration fails since the original registration is still pending its
        // observation.
        let (sender, mut receiver) = oneshot::channel();
        assert!(subscriber.register(sender).is_err());
        assert!(ex.run_until_stalled(&mut receiver).expect("sender closed").is_err());

        // Initial observation received now that the initial value is set.
        publisher.set(1);

        let observation =
            ex.run_until_stalled(&mut recv1).expect("received subsequent observation");
        assert_eq!(observation, Ok(1));
    }

    #[test]
    fn sync_pub_sub_delayed_multiple_subscribers() {
        let mut ex = fasync::TestExecutor::new();
        let mut broker = HangingGet::<i32, _, _>::new_unknown_state(|s, o: oneshot::Sender<_>| {
            o.send(s.clone()).map(|()| true).unwrap()
        });
        let publisher = broker.new_publisher();

        let sub1 = broker.new_subscriber();
        let sub2 = broker.new_subscriber();

        // Initial observation for subscribers delayed due to lack of initial state.
        let (sender, mut recv1) = oneshot::channel();
        sub1.register(sender).unwrap();
        assert!(ex.run_until_stalled(&mut recv1).is_pending());

        let (sender, mut recv2) = oneshot::channel();
        sub2.register(sender).unwrap();
        assert!(ex.run_until_stalled(&mut recv2).is_pending());

        // Subsequent registrations fails since the original registrations are still pending their
        // observations.
        let (sender, mut recv3) = oneshot::channel();
        assert!(sub1.register(sender).is_err());
        assert!(ex.run_until_stalled(&mut recv3).expect("sender 3 closed").is_err());

        let (sender, mut recv4) = oneshot::channel();
        assert!(sub2.register(sender).is_err());
        assert!(ex.run_until_stalled(&mut recv4).expect("sender 4 closed").is_err());

        // Initial observations received now that initial value is set.
        publisher.set(1);
        let obs1 =
            ex.run_until_stalled(&mut recv1).expect("receiver 1 received subsequent observation");
        assert_eq!(obs1, Ok(1));
        let obs2 =
            ex.run_until_stalled(&mut recv2).expect("receiver 2 received subsequent observation");
        assert_eq!(obs2, Ok(1));
    }

    #[test]
    fn subscriber_drop_unsubscribes() {
        let mut broker = HangingGet::new(0i32, |s, o: oneshot::Sender<_>| {
            o.send(s.clone()).map(|()| true).unwrap()
        });
        let subscriber = broker.new_subscriber();
        assert_eq!(broker.inner.lock().observers.len(), 0);

        let (sender, _receiver) = oneshot::channel();
        subscriber.register(sender).unwrap();
        assert_eq!(broker.inner.lock().observers.len(), 1);

        drop(subscriber);
        assert_eq!(broker.inner.lock().observers.len(), 0);
    }
}
