// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! When generating a watcher event, one needs "a list of names" that are then converted into
//! buffers sent to the watchers.  In a sense, an iterator over a list of strings would work, but
//! in order to avoid copying the data around, this namespace provides a more specialized version
//! of this abstraction.

use flex_fuchsia_io as fio;
use static_assertions::const_assert;

/// Watcher event producer, that generates buffers filled with watcher events.  Watchers use this
/// API to obtain buffers that are then sent to the actual watchers.  Every producer may generate
/// multiple events, but they all need to be of the same type, as returned by [`Self::event()`] and
/// [`Self::mask()`] methods.
pub trait EventProducer {
    /// Returns a mask that represents the type of events this producer can generate, as one of the
    /// `flex_fuchsia_io::WatchMask::*` constants.  There might be only one bit set and it should
    /// correspond to the event returned by the [`Self::event()`] method.  It is a duplication, but it
    /// helps the callers that need both masks and event IDs.
    fn mask(&self) -> fio::WatchMask;

    /// Returns an event ID this event producer will use to populate the buffers, as one of the
    /// `flex_fuchsia_io::WatchEvent::*` constants.  Must match what [`Self::mask()`], returns, see
    /// there for details.
    fn event(&self) -> fio::WatchEvent;

    /// Checks if this producer can create another buffer, returning `true` if it can.  This method
    /// does not actually need to construct the buffer just yet, as an optimization if it will not
    /// be needed.
    fn prepare_for_next_buffer(&mut self) -> bool;

    /// Returns a copy of the current buffer prepared by this producer.  This method will be the
    /// one constructing a buffer, if necessary, after a preceding call to
    /// [`Self::prepare_for_next_buffer()`].
    ///
    /// Note that this method will keep returning copies of the same buffer, until
    /// [`Self::prepare_for_next_buffer()`] is not called explicitly.
    fn buffer(&mut self) -> Vec<u8>;
}

/// An [`EventProducer`] that uses a `Vec<String>` with names of the entires to be put into the
/// watcher event.
pub struct StaticVecEventProducer {
    names: Vec<String>,
    next: usize,
    mask: fio::WatchMask,
    event: fio::WatchEvent,
    buffer: Vec<u8>,
}

impl StaticVecEventProducer {
    /// Constructs a new [`EventProducer`] that is producing names form the specified list,
    /// building events of type `WatchEvent::Added`.  `names` is not allowed to be empty.
    pub fn added(names: Vec<String>) -> Self {
        Self::new(fio::WatchMask::ADDED, fio::WatchEvent::Added, names)
    }

    /// Constructs a new [`EventProducer`] that is producing names form the specified list,
    /// building events of type `WatchEvent::Removed`.  `names` is not allowed to be empty.
    pub fn removed(names: Vec<String>) -> Self {
        Self::new(fio::WatchMask::REMOVED, fio::WatchEvent::Removed, names)
    }

    /// Constructs a new [`EventProducer`] that is producing names form the specified list,
    /// building events of type `WatchEvent::Existing`.  `names` is not allowed to be empty.
    pub fn existing(names: Vec<String>) -> Self {
        Self::new(fio::WatchMask::EXISTING, fio::WatchEvent::Existing, names)
    }

    fn new(mask: fio::WatchMask, event: fio::WatchEvent, names: Vec<String>) -> Self {
        debug_assert!(!names.is_empty());
        Self { names, next: 0, mask, event, buffer: Vec::new() }
    }
}

impl EventProducer for StaticVecEventProducer {
    fn mask(&self) -> fio::WatchMask {
        self.mask
    }

    fn event(&self) -> fio::WatchEvent {
        self.event
    }

    fn prepare_for_next_buffer(&mut self) -> bool {
        self.buffer.clear();
        self.next < self.names.len()
    }

    fn buffer(&mut self) -> Vec<u8> {
        if self.buffer.is_empty() {
            while self.next < self.names.len() {
                if !encode_name(&mut self.buffer, self.event, &self.names[self.next]) {
                    break;
                }
                self.next += 1;
            }
        }
        self.buffer.clone()
    }
}

/// An event producer for an event containing only one name.
pub struct SingleNameEventProducer<'a> {
    name: &'a str,
    buffer: Vec<u8>,
    mask: fio::WatchMask,
    event: fio::WatchEvent,
}

impl<'a> SingleNameEventProducer<'a> {
    /// Constructs a new [`SingleNameEventProducer`] that will produce an event for one name of
    /// type `WatchEvent::Deleted`. Deleted refers to the directory the watcher itself is on, and
    /// therefore statically refers to itself as ".".
    pub fn deleted() -> Self {
        Self::new(fio::WatchMask::DELETED, fio::WatchEvent::Deleted, ".")
    }

    /// Constructs a new [`SingleNameEventProducer`] that will produce an event for one name of
    /// type `WatchEvent::Added`.
    pub fn added(name: &'a str) -> Self {
        Self::new(fio::WatchMask::ADDED, fio::WatchEvent::Added, name)
    }

    /// Constructs a new [`SingleNameEventProducer`] that will produce an event for one name of
    /// type `WatchEvent::Removed`.
    pub fn removed(name: &'a str) -> Self {
        Self::new(fio::WatchMask::REMOVED, fio::WatchEvent::Removed, name)
    }

    /// Constructs a new [`SingleNameEventProducer`] that will produce an event for one name of
    /// type `WatchEvent::Existing`.
    pub fn existing(name: &'a str) -> Self {
        Self::new(fio::WatchMask::EXISTING, fio::WatchEvent::Existing, name)
    }

    /// Constructs a new [`SingleNameEventProducer`] that will produce an `WatchEvent::Idle` event.
    pub fn idle() -> Self {
        Self::new(fio::WatchMask::IDLE, fio::WatchEvent::Idle, "")
    }

    fn new(mask: fio::WatchMask, event: fio::WatchEvent, name: &'a str) -> Self {
        Self { name, buffer: Vec::new(), mask, event }
    }
}

impl EventProducer for SingleNameEventProducer<'_> {
    fn mask(&self) -> fio::WatchMask {
        self.mask
    }

    fn event(&self) -> fio::WatchEvent {
        self.event
    }

    fn prepare_for_next_buffer(&mut self) -> bool {
        // The buffer is populated the first time `EventProducer::buffer` is called. If the buffer
        // is empty then we are able to produce another buffer. If the buffer is already populated
        // then this event has already been sent.
        self.buffer.is_empty()
    }

    fn buffer(&mut self) -> Vec<u8> {
        if self.buffer.is_empty() {
            encode_name(&mut self.buffer, self.event, self.name);
        }
        self.buffer.clone()
    }
}

fn encode_name(buffer: &mut Vec<u8>, event: fio::WatchEvent, name: &str) -> bool {
    let event_size = 2 + name.len();
    if buffer.len() + event_size > fio::MAX_BUF as usize {
        return false;
    }

    // We are going to encode the file name length as u8.
    const_assert!(u8::MAX as u64 >= fio::MAX_NAME_LENGTH);

    // TODO(https://fxbug.dev/293948129): Event producers should take a type that enforces this
    // constraint. "." is valid here, so taking [`crate::Name`] directly isn't sufficient.
    assert!(
        name.len() <= fio::MAX_NAME_LENGTH as usize,
        "Entry names are expected to be no longer than MAX_FILENAME ({}) bytes.\n\
         Got entry: '{}'\n\
         Length: {} bytes",
        fio::MAX_NAME_LENGTH,
        name,
        name.len()
    );

    buffer.reserve(event_size);
    buffer.push(event.into_primitive());
    buffer.push(name.len() as u8);
    buffer.extend_from_slice(name.as_bytes());
    true
}
