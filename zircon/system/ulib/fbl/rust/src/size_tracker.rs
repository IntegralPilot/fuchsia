// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Trait for tracking the size of the list.
pub trait SizeTracker: Clone {
    /// The initial value for the tracker.
    const INIT: Self;
    /// True if this tracker actually tracks the size.
    const IS_TRACKING: bool;
    /// Increments the size count.
    fn increment(&mut self);
    /// Decrements the size count.
    fn decrement(&mut self);
    /// Returns the current size.
    fn get(&self) -> usize;
    /// Sets the size count.
    fn set(&mut self, size: usize);
    /// Swaps the size count with another tracker.
    fn swap(&mut self, other: &mut Self);
}

/// A size tracker that does not actually track the size (zero overhead).
#[derive(Clone, Copy)]
pub struct NonTrackingSize;
impl SizeTracker for NonTrackingSize {
    const INIT: Self = NonTrackingSize;
    const IS_TRACKING: bool = false;
    fn increment(&mut self) {}
    fn decrement(&mut self) {}
    fn get(&self) -> usize {
        panic!("Cannot get the size if we are not tracking the size.")
    }
    fn set(&mut self, _size: usize) {}
    fn swap(&mut self, _other: &mut Self) {}
}

/// A size tracker that maintains the count of elements in the list.
#[derive(Clone, Copy)]
pub struct TrackingSize(usize);
impl SizeTracker for TrackingSize {
    const INIT: Self = TrackingSize(0);
    const IS_TRACKING: bool = true;
    fn increment(&mut self) {
        self.0 += 1;
    }
    fn decrement(&mut self) {
        self.0 -= 1;
    }
    fn get(&self) -> usize {
        self.0
    }
    fn set(&mut self, size: usize) {
        self.0 = size;
    }
    fn swap(&mut self, other: &mut Self) {
        core::mem::swap(&mut self.0, &mut other.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "Cannot get the size if we are not tracking the size.")]
    fn test_non_tracking_size_get_panics() {
        let tracker = NonTrackingSize;
        let _ = tracker.get();
    }

    #[test]
    fn test_non_tracking_size_set_swap() {
        let mut tracker1 = NonTrackingSize;
        let mut tracker2 = NonTrackingSize;
        tracker1.increment();
        tracker1.decrement();
        tracker1.set(10);
        tracker1.swap(&mut tracker2);
        assert!(!NonTrackingSize::IS_TRACKING);
    }

    #[test]
    fn test_tracking_size_increment_decrement() {
        let mut tracker = TrackingSize::INIT;
        assert!(TrackingSize::IS_TRACKING);
        assert_eq!(tracker.get(), 0);
        tracker.increment();
        assert_eq!(tracker.get(), 1);
        tracker.increment();
        assert_eq!(tracker.get(), 2);
        tracker.decrement();
        assert_eq!(tracker.get(), 1);
    }

    #[test]
    fn test_tracking_size_set() {
        let mut tracker = TrackingSize::INIT;
        tracker.set(100);
        assert_eq!(tracker.get(), 100);
    }

    #[test]
    fn test_tracking_size_swap() {
        let mut tracker1 = TrackingSize::INIT;
        tracker1.set(100);
        let mut tracker2 = TrackingSize::INIT;
        tracker2.set(50);

        tracker1.swap(&mut tracker2);
        assert_eq!(tracker1.get(), 50);
        assert_eq!(tracker2.get(), 100);
    }
}
