// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::future::poll_fn;
use futures::{Future, FutureExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use crate::error::Result;
use crate::parser::Mutability;
use crate::value::{Value, ValueExt};

/// Indicates how to map slots from one frame to another when the first frame
/// contains the scope where a function or other capturing body was defined, and
/// the second frame contains the body of that function, and captures variables
/// from the first.
#[derive(Copy, Clone)]
pub(crate) struct CaptureMapEntry {
    pub slot_from: usize,
    pub slot_to: usize,
    pub mutability: Mutability,
}

/// A value stored in a [`Frame`]. Frame values have the potential to be
/// futures, and we await them automatically, just in time.
enum FrameValue {
    /// The value is ready. This `FrameValue` can be read synchronously.
    Ready(Result<Value>),
    /// The value is deferred. Something elsewhere in the interpreter is still
    /// filling out this field. If we've tried to read this value, we should put
    /// a waker in here and wait for the value to become ready.
    Waiting(Vec<Waker>),
}

impl FrameValue {
    /// Create a new `FrameValue` in the ready state, wrapped in an
    /// `Arc<Mutex<..>>` and ready for use.
    fn from_value_sync(value: Result<Value>) -> Arc<Mutex<FrameValue>> {
        Arc::new(Mutex::new(FrameValue::Ready(value)))
    }

    /// Create a new `FrameValue` in the waiting state, wrapped in an
    /// `Arc<Mutex<..>>` and ready for use.
    fn waiting_sync() -> Arc<Mutex<FrameValue>> {
        Arc::new(Mutex::new(FrameValue::Waiting(Vec::new())))
    }

    /// Get the current value. If the value isn't ready, await until it is.
    pub fn get(slot: Arc<Mutex<FrameValue>>) -> impl Future<Output = Result<Value>> + 'static {
        async move {
            let got = poll_fn(|ctx| {
                let mut slot = slot.lock().unwrap();

                match &mut *slot {
                    FrameValue::Ready(Ok(result)) => Poll::Ready(Ok(result.duplicate())),
                    FrameValue::Ready(Err(e)) => Poll::Ready(Err(e.clone())),
                    FrameValue::Waiting(wakers) => {
                        wakers.push(ctx.waker().clone());
                        Poll::Pending
                    }
                }
            })
            .await?;
            Ok(got)
        }
    }
}

/// A map of global variables.
///
/// The compiler erases variable names in favor of integer slot numbers at run
/// time. Only top-level globals need to keep the names around as every time we
/// execute a new command we re-run the compiler, so we get new slot
/// assignments. That is why this can't just be a [`Frame`].
#[derive(Default)]
pub struct GlobalVariables {
    entries: HashMap<String, (Arc<Mutex<Arc<Mutex<FrameValue>>>>, Mutability)>,
}

impl GlobalVariables {
    /// Get the names of the variables in this namespace. Filters out variables
    /// whose values are not ready yet, and variables with values for which the
    /// given function returns `false`.
    pub fn names<'a>(
        &'a self,
        filter: impl Fn(&Value) -> bool + 'a,
    ) -> impl std::iter::Iterator<Item = &String> {
        self.entries
            .iter()
            .filter(move |(_, x)| match &*x.0.lock().unwrap().lock().unwrap() {
                FrameValue::Ready(Ok(x)) => filter(x),
                _ => false,
            })
            .map(|x| x.0)
    }

    /// Merge another [`GlobalVariables`] with this one. Overwrite keys in this
    /// instance in case of collisions.
    pub fn merge(&mut self, other: GlobalVariables) {
        self.entries.extend(other.entries.into_iter());
    }

    /// Create a new frame of the given `size` containing these global variables
    /// where needed. See [`apply_to_frame`].
    pub fn as_frame(&self, size: usize, mapping: impl FnMut(&str) -> Option<usize>) -> Frame {
        let mut frame = Frame::new(size);
        self.apply_to_frame(&mut frame, mapping);
        frame
    }

    /// Fill [`Frame`] with the values of some of the global variables in this
    /// structure. Overwrite existing values where necessary. The `mapping`
    /// function can dictate what IDs the values receive, and can return `None`
    /// to skip certain values. The slots in the resulting frame capture from
    /// this structure, meaning you can assign to the values in the frame and
    /// update this structure.
    pub fn apply_to_frame(
        &self,
        frame: &mut Frame,
        mut mapping: impl FnMut(&str) -> Option<usize>,
    ) {
        for (id, (entry, is_const)) in self.entries.iter().filter_map(|(name, value)| {
            mapping(name).map(|id| (id, (Arc::clone(&value.0), value.1)))
        }) {
            frame.slots[id] = Slot::Captured(entry, is_const);
        }
    }

    /// If a variable with the given name is defined in this structure, update
    /// its mutability and returned. If it is not defined, define it and use the
    /// `value` function to givei t a value.
    pub fn ensure_defined(
        &mut self,
        name: impl Into<String>,
        value: impl FnOnce() -> Result<Value>,
        mutability: Mutability,
    ) {
        self.entries.entry(name.into()).and_modify(|entry| entry.1 = mutability).or_insert_with(
            || (Arc::new(Mutex::new(Arc::new(Mutex::new(FrameValue::Ready(value()))))), mutability),
        );
    }

    /// Define a new global variable.
    pub fn define(
        &mut self,
        name: impl Into<String>,
        value: Result<Value>,
        mutability: Mutability,
    ) {
        self.entries.insert(
            name.into(),
            (Arc::new(Mutex::new(Arc::new(Mutex::new(FrameValue::Ready(value))))), mutability),
        );
    }

    /// Get the value of a global variable.
    pub fn get(&self, name: &str) -> impl Future<Output = Option<Result<Value>>> + 'static {
        let fut =
            self.entries.get(name).map(|slot| FrameValue::get(Arc::clone(&slot.0.lock().unwrap())));

        async move { if let Some(fut) = fut { Some(fut.await) } else { None } }
    }

    /// Convert this set of global variables to a `Value::Object` where the keys
    /// are variable names and the values are the values of those variables.
    pub async fn to_object(self) -> Result<Value> {
        let mut ret = Vec::with_capacity(self.entries.len());
        for (name, (value, _)) in self.entries.into_iter() {
            let value = Arc::clone(&value.lock().unwrap());
            ret.push((name, FrameValue::get(value).await?));
        }
        Ok(Value::Object(ret))
    }
}

/// A slot in a [`Frame`]. Each slot in a frame has a numerical ID and
/// represents the value behind a single variable. Slots are either normal or
/// "captured" as in closure capture, where captured slots have to be more
/// indirected with more synchronization because two concurrent tasks could
/// observe the variable being mutated at the same time.
#[derive(Clone)]
enum Slot {
    Normal(Arc<Mutex<FrameValue>>),
    Captured(Arc<Mutex<Arc<Mutex<FrameValue>>>>, Mutability),
}

impl Slot {
    /// If this slot is a normal slot, turn it in to a captured slot, then
    /// return a copy of it. This is how we initiate the capturing of a variable
    /// in one scope by another scope.
    fn capture(&mut self, mutability: Mutability) -> Slot {
        if let Slot::Normal(slot) = self {
            let new = Slot::Captured(Arc::new(Mutex::new(Arc::clone(slot))), mutability);
            *self = new
        }

        self.clone()
    }
}

impl Default for Slot {
    fn default() -> Slot {
        Slot::Normal(FrameValue::from_value_sync(Ok(Value::Null)))
    }
}

/// A frame represents all the variables allocated in a function and can be
/// thought of more or less like a stack frame. The compiler maps variable names
/// to numbers, which index the array of slots in the frame.
pub struct Frame {
    slots: Vec<Slot>,
}

impl Frame {
    /// Create a new frame with the given number of slots.
    pub fn new(slots: usize) -> Frame {
        Frame { slots: std::iter::repeat_with(Slot::default).take(slots).collect() }
    }

    /// Bring captures from the given capture set into this frame.
    pub fn apply_capture(&mut self, capture_set: &CaptureSet) {
        for (id, slot) in capture_set.slots.iter() {
            self.slots[*id] = slot.clone()
        }
    }

    /// Assign an immediately available value to the given slot.
    ///
    /// # Panics
    ///
    /// For captured slots we have runtime knowledge of whether the slot backs a
    /// constant declaration, and will panic if it does.
    pub fn assign(&mut self, slot_num: usize, value: Result<Value>) {
        self.assign_inner(slot_num, value, false);
    }

    /// Assign an immediately available value to the given slot. Ignores whether
    /// the value is constant.
    pub fn assign_ignore_const(&mut self, slot_num: usize, value: Result<Value>) {
        self.assign_inner(slot_num, value, true);
    }

    /// Shared implementation of [`assign`] and [`assign_ignore_const`]
    fn assign_inner(&mut self, slot_num: usize, value: Result<Value>, ignore_const: bool) {
        let value = FrameValue::from_value_sync(value);
        match &mut self.slots[slot_num] {
            Slot::Normal(slot) => {
                *slot = value;
            }
            Slot::Captured(slot, mutability) => {
                assert!(
                    !mutability.is_constant() || ignore_const,
                    "Assigned to constant when interpreter expected it was impossible"
                );
                *slot.lock().unwrap() = value;
            }
        }
    }

    /// Assign a value that will be produced by the given future to the given
    /// slot. The returned future will poll the passed future and then update
    /// the slot when that future completes. Consequently the returned future
    /// must be polled to completion or the slot will never become ready, and
    /// anything reading from it will poll forever.
    ///
    /// If skip_if_const is true, this will return `None` if the slot is
    /// constant and cannot be assigned. Note we only know this if the slot is a
    /// captured slot. For normal slots the compiler is expected to prevent
    /// writing at compile time.
    fn assign_future_inner(
        &mut self,
        slot_num: usize,
        fut: impl Future<Output = Result<Value>> + Send + 'static,
        skip_if_const: bool,
    ) -> Option<impl Future<Output = ()> + 'static> {
        let ns_value = FrameValue::waiting_sync();
        match &mut self.slots[slot_num] {
            Slot::Normal(slot) => {
                *slot = Arc::clone(&ns_value);
            }
            Slot::Captured(slot, mutability) => {
                if mutability.is_constant() && skip_if_const {
                    return None;
                } else {
                    *slot.lock().unwrap() = Arc::clone(&ns_value);
                }
            }
        }
        Some(fut.then(move |value| async move {
            let FrameValue::Waiting(wakers) =
                std::mem::replace(&mut *ns_value.lock().unwrap(), FrameValue::Ready(value))
            else {
                panic!("Value state set twice");
            };
            wakers.into_iter().for_each(Waker::wake);
        }))
    }

    /// Assign a value that will be produced by the given future to the given
    /// slot. The returned future will poll the passed future and then update
    /// the slot when that future completes. Consequently the returned future
    /// must be polled to completion or the slot will never become ready, and
    /// anything reading from it will poll forever.
    ///
    /// This will return `None` if the slot is constant and cannot be assigned.
    /// Note we only know this if the slot is a captured slot. For normal slots
    /// the compiler is expected to prevent writing at compile time.
    pub fn assign_future_if_not_const(
        &mut self,
        slot_num: usize,
        fut: impl Future<Output = Result<Value>> + Send + 'static,
    ) -> Option<impl Future<Output = ()> + 'static> {
        self.assign_future_inner(slot_num, fut, true)
    }

    /// Assign a value that will be produced by the given future to the given
    /// slot. The returned future will poll the passed future and then update
    /// the slot when that future completes. Consequently the returned future
    /// must be polled to completion or the slot will never become ready, and
    /// anything reading from it will poll forever.
    ///
    /// This will always modify the slot even if we have runtime knowledge
    /// suggesting the slot is associated with a constant declaration.
    pub fn assign_future_ignore_const(
        &mut self,
        slot_num: usize,
        fut: impl Future<Output = Result<Value>> + Send + 'static,
    ) -> impl Future<Output = ()> + 'static {
        self.assign_future_inner(slot_num, fut, false).unwrap()
    }

    /// Get the value in the given slot.
    pub fn get(&self, slot_num: usize) -> impl Future<Output = Result<Value>> + 'static {
        match &self.slots[slot_num] {
            Slot::Normal(slot) => FrameValue::get(Arc::clone(&slot)),
            Slot::Captured(slot, _) => FrameValue::get(Arc::clone(&slot.lock().unwrap())),
        }
    }
}

/// A set of slots which have been captured from a given frame, and presumably
/// will be presented in a different frame later on.
#[derive(Clone)]
pub struct CaptureSet {
    slots: Vec<(usize, Slot)>,
}

impl CaptureSet {
    /// Construct a new capture set.
    pub(crate) fn new<'a>(
        frame: &mut Frame,
        entries: impl IntoIterator<Item = &'a CaptureMapEntry>,
    ) -> Self {
        CaptureSet {
            slots: entries
                .into_iter()
                .map(|CaptureMapEntry { slot_from, slot_to, mutability }| {
                    (*slot_to, frame.slots[*slot_from].capture(*mutability))
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::future::Either;
    use std::pin::pin;

    #[fuchsia::test]
    async fn capture() {
        let mut frame_a = Frame::new(2);
        let mut frame_b = Frame::new(2);

        frame_a.assign(0, Ok(Value::U8(1)));
        frame_a.assign(1, Ok(Value::U8(2)));

        let captures = CaptureSet::new(
            &mut frame_a,
            [
                &CaptureMapEntry { slot_from: 0, slot_to: 1, mutability: Mutability::Mutable },
                &CaptureMapEntry { slot_from: 1, slot_to: 0, mutability: Mutability::Mutable },
            ],
        );
        frame_b.apply_capture(&captures);

        assert!(matches!(frame_b.get(0).await.unwrap(), Value::U8(2)));
        assert!(matches!(frame_b.get(1).await.unwrap(), Value::U8(1)));

        frame_b.assign(0, Ok(Value::U8(3)));

        assert!(matches!(frame_a.get(1).await.unwrap(), Value::U8(3)));
    }

    #[fuchsia::test]
    async fn assign_future() {
        let mut frame = Frame::new(1);

        let fut = frame.assign_future_ignore_const(0, async move {
            fuchsia_async::Timer::new(std::time::Duration::from_millis(500)).await;
            Ok(Value::U8(42))
        });

        let got = match futures::future::select(pin!(frame.get(0)), pin!(fut)).await {
            Either::Left((v, _)) => v,
            Either::Right(((), f)) => f.await,
        };

        assert!(matches!(got.unwrap(), Value::U8(42)));
    }

    #[fuchsia::test]
    async fn globals() {
        let mut globals = GlobalVariables::default();

        globals.define("foo", Ok(Value::U8(1)), Mutability::Constant);
        globals.define("bar", Ok(Value::U8(2)), Mutability::Mutable);
        globals.ensure_defined("baz", || Ok(Value::U8(3)), Mutability::Constant);
        globals.ensure_defined("foo", || unreachable!(), Mutability::Mutable);

        let mut frame = globals.as_frame(2, |var| {
            if var == "foo" {
                Some(1)
            } else if var == "baz" {
                Some(0)
            } else {
                None
            }
        });

        assert!(matches!(frame.get(0).await.unwrap(), Value::U8(3)));
        assert!(matches!(frame.get(1).await.unwrap(), Value::U8(1)));
        if let Some(x) = frame.assign_future_if_not_const(0, async { Ok(Value::U8(5)) }) {
            x.await;
        }
        if let Some(x) = frame.assign_future_if_not_const(1, async { Ok(Value::U8(5)) }) {
            x.await;
        }

        assert!(matches!(globals.get("foo").await.unwrap().unwrap(), Value::U8(5)));
        assert!(matches!(globals.get("baz").await.unwrap().unwrap(), Value::U8(3)));
    }
}
