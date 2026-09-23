// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! This is an implementation of a simple RCU primitive that uses two generation counts to track
//! readers. Benefits of this primitive are:
//!  * The implementation is extremely simple and easy to reason about.
//!  * Does not require any global state.
//!
//! However there are a number of limitations:
//! * Does not use per-cpu counters, limiting scalability of readers.
//! * All state shares the same cache line, further limiting scalability.
//! * Readers must execute with interrupts disabled.
//! * Writers must spin, whilst holding their write lock, until previous readers complete.
//!
//! In the case where read paths are always extremely short and there is not an extremely high
//! concurrent reader load, these limitations are tolerable.
//!
//! This RCU strategy is specifically not suitable for scenarios where:
//!  * A large number of readers can be expected to operate concurrently. Here the single counter
//!    will bounce between CPUs cache lines, limiting scalability.
//!  * Read paths might be long and/or require blocking. Not only should interrupt disable paths be
//!    short as a general rule, but writers must actively spin waiting for readers to complete.
//!  * Writers need to minimize their own latency. Due to needing to actively spin wait for readers,
//!    this limits the ability of writers to operate with low latency (unless the read paths are
//!    truly short).
//!
//! Although this is presented as an rcu primitive, due to the lack of safeties and convenience
//! wrappers this is really a building block for rcu systems. For usage in the mmu code this is
//! completely fine, which is why this library has an mmu_ prefix. As this gets more fleshed out
//! the mmu_ prefix could eventually be dropped.
//!
//! A simple model of this object can be found in models/simple_generational.cc, and is designed to
//! be tested by the CDSchecker tool. Unfortunately due to state space explosion using what I
//! believe to be correct memory orders results in the checker not terminating (at least in the
//! reasonable time frames I attempted). As such some memory orders have been increased beyond what
//! I think should be necessary to match the model. These cases are documented with comments.

use core::marker::PhantomData;
use core::sync::atomic::{AtomicU64, Ordering};

/// A simple RCU primitive that uses two generation counts to track readers.
#[derive(Debug)]
#[repr(C)]
pub struct SimpleGenerational {
    state: AtomicU64,
}

const _: () = assert!(core::mem::size_of::<SimpleGenerational>() == 8);
const _: () = assert!(core::mem::align_of::<SimpleGenerational>() == 8);

impl SimpleGenerational {
    // Use the high bit to select between which is the current generation, and hence which of the
    // two counts a reader should increment.
    const GEN_BIT: usize = 63;
    // Pack in two counters, each 31 bits. After the generation bit this leaves 1 free bit that is
    // unused. 31 bits for the count is far more than necessary (since 2^31 threads seems unlikely),
    // but there's nothing else to store for the moment.
    const COUNT_BITS: usize = 31;
    // Helper constant to extract the count, assuming its already been shifted to the bottom bits.
    const COUNT_MASK: u64 = (1u64 << Self::COUNT_BITS) - 1;

    /// Creates a new `SimpleGenerational` primitive initialized with zero readers and generation 0.
    pub const fn new() -> Self {
        Self { state: AtomicU64::new(0) }
    }

    /// Begins a read critical section, returning the generation of this reader. The generation can
    /// be considered opaque data that must be returned to [`read_unlock`](Self::read_unlock).
    ///
    /// Must be called with interrupts disabled, and interrupts must stay disabled until calling
    /// [`read_unlock`](Self::read_unlock).
    ///
    /// Although this provides a similar memory barrier as a typical lock acquire, i.e. this has
    /// acquire semantics, because writers are still executing in parallel loads to rcu protected
    /// data must still be done with care to ensure that single loads are done (to avoid TOCTOU
    /// errors) and any additional memory barriers are done if needed to coordinate with writers
    /// (whether this is needed depends entirely on the nature of the data structure and the
    /// actions performed by writers).
    ///
    /// This method is thread-safe.
    ///
    /// Note: You probably do not want to call this directly, and should be using the
    /// [`AutoSimpleGenerationalReader`] RAII wrapper.
    pub fn read_lock(&self) -> u32 {
        debug_assert!(crate::arch_rs::ints_disabled());
        // First find out what the current generation is.
        // Note: Although it is believed this should be relaxed, the model upgrades this to a
        // seq_cst order for state explosion reasons.
        let initial_gen = (self.state.load(Ordering::SeqCst) >> Self::GEN_BIT) as u32;
        // Now both increment the counter for that generation, and simultaneously read back what the
        // actual generation is. This is memory_order_acquire as this is logically acquiring a lock,
        // and we need to not have any loads be reordered before.
        // Note: Although it is believed this should be acq_rel, the model upgrades this to a
        // seq_cst order for state explosion reasons.
        let mut current_gen = (self
            .state
            .fetch_add(1u64 << (initial_gen as usize * Self::COUNT_BITS), Ordering::SeqCst)
            >> Self::GEN_BIT) as u32;
        // Ideally we actually incremented the correct counter. If so, we are done.
        if initial_gen == current_gen {
            return current_gen;
        }
        // First increment the other (actually current) counter. Like before, this is logically
        // acquiring a lock and so needs memory_order_acquire. The previously  'wrong' counter
        // increment is fine. We could in fact always increment both counters initially
        // before checking what the generation is. The purpose behind attempting to only
        // increment the 'correct' counter is just to minimize the possibility of rapid
        // readers preventing Synchronize from making progress. Note: Although it is
        // believed this should be acquire, the model upgrades this to a seq_cst
        // order for state explosion reasons.
        current_gen = (self
            .state
            .fetch_add(1u64 << (current_gen as usize * Self::COUNT_BITS), Ordering::SeqCst)
            >> Self::GEN_BIT) as u32;
        // At this point we have incremented both counters and have effectively constrained the
        // possible races with Synchronize. At this point there are only a couple of
        // scenarios to consider.
        //  * Synchronize has not yet run and/or not yet stored a new generation. In this case
        //    'current_gen' is the counter it would wait on, and so we can safely decrement the
        //    other counter.
        //  * Synchronize stores a new generation after we have loaded it. Here the racing call to
        //    synchronize is going to wait on the 'other_gen' counter, which we are about to
        //    decrement. This is fine since we know, by virtue of synchronize being at this point in
        //    its execution, that all of the stores the writer may have done are globally visible,
        //    and we have not yet performed any loads yet. Therefore we do not need to block this
        //    particular call to synchronize. The fact we *already* incremented the counter for
        //    'current_gen', does mean that even synchronize completes now, and *another* call to
        //    synchronize immediately happens, it will get blocked waiting for us.
        //  * In any other scenario such as synchronize had changed generation before we loaded it,
        //    but had not yet completed its waits, or had just completed its waits, these are all
        //    uninteresting since these are cases where we have no need to cause synchronize to wait
        //    as it has completed all its modifications, and we have not yet performed any reads.
        let other_gen = 1 - current_gen;
        // We have no correctness requirements on when this happens, all our correctness is on the
        // 'current_gen' counter, so this can be relaxed.
        // Note: Although it is believed this should be relaxed, the model upgrades this to a
        // seq_cst order for state explosion reasons.
        self.state.fetch_sub(1u64 << (other_gen as usize * Self::COUNT_BITS), Ordering::SeqCst);
        current_gen
    }

    /// Release the previously acquired read lock.
    ///
    /// This has release semantics and no rcu protected objects should continue to be referenced
    /// after this, as the writer will assume they now have exclusive access to them.
    ///
    /// This method is thread-safe.
    pub fn read_unlock(&self, generation: u32) {
        // Cannot ensure that interrupts were held disabled the whole time, but check what we can.
        debug_assert!(crate::arch_rs::ints_disabled());
        // A release memory order here ensures that all of our loads are completed prior to
        // decrementing the counter.
        // Note: Although it is believed this should be release, the model upgrades this to a
        // seq_cst order for state explosion reasons.
        self.state.fetch_sub(1u64 << (generation as usize * Self::COUNT_BITS), Ordering::AcqRel);
    }

    /// Performs a synchronization with any outstanding readers. On return the caller can be certain
    /// that any read critical sections that began prior to Synchronize are guaranteed to have
    /// completed. (A read critical section is the code that occurs between a ReadLock/ReadUnlock
    /// block).
    /// Any calls to ReadLock that occurred after this function started may or may not complete.
    ///
    /// Prior to calling this the writer must assume that any modifications to pointers, or objects,
    /// are visible to readers. As a consequence any updates to pointers requires separate release
    /// barriers to ensure data is up to date before a reader can observe the pointer change.
    ///
    /// After returning the writer can assume that any objects it had detached are no longer
    /// referenced by any readers and it has complete exclusive access to them.
    ///
    /// This method is thread-compatible and unlike ReadLock and ReadUnlock must be externally
    /// synchronized.
    pub fn synchronize(&self) {
        // Switch the generation. The synchronize operation as a whole can be thought of as both a
        // lock release and a lock acquire, which is split across two parts. Any writes we
        // have done (that resulted in objects no longer being visible to readers) must
        // complete before changing the generation, hence the release order. Similarly any
        // future stores to these now disconnected objects must not be reordered before we
        // know the count is 0 and readers have completed, hence the acquire order.
        let old_state = self.state.fetch_xor(1u64 << Self::GEN_BIT, Ordering::AcqRel);
        let old_gen = (old_state >> Self::GEN_BIT) as u32;
        if ((old_state >> (old_gen as usize * Self::COUNT_BITS)) & Self::COUNT_MASK) == 0 {
            return;
        }
        // There was a reader. Spin until that reader is complete. Again, since we need to ensure
        // loads to our disconnected objects are not performed until all readers complete,
        // this must be done with an acquire barrier.
        while ((self.state.load(Ordering::Acquire) >> (old_gen as usize * Self::COUNT_BITS))
            & Self::COUNT_MASK)
            > 0
        {
            core::hint::spin_loop();
        }
    }
}

impl Default for SimpleGenerational {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SimpleGenerational {
    fn drop(&mut self) {
        // Ensure no outstanding read locks.
        debug_assert!((self.state.load(Ordering::Relaxed) & !(1u64 << Self::GEN_BIT)) == 0);
    }
}

/// RAII wrapper around acquiring and release the read lock of SimpleGenerational. This also
/// performs the interrupt disable and restore.
#[derive(Debug)]
#[must_use = "AutoSimpleGenerationalReader must be held for the duration of the read critical \
              section"]
pub struct AutoSimpleGenerationalReader<'a> {
    parent: &'a SimpleGenerational,
    state: crate::arch_rs::InterruptSavedState,
    generation: u32,
    _marker: PhantomData<*mut ()>,
}

impl<'a> AutoSimpleGenerationalReader<'a> {
    /// Creates a new `AutoSimpleGenerationalReader`, disabling interrupts and acquiring the read
    /// lock.
    #[inline]
    pub fn new(rcu: &'a SimpleGenerational) -> Self {
        let state = crate::arch_rs::arch_interrupt_save();
        let generation = rcu.read_lock();
        Self { parent: rcu, state, generation, _marker: PhantomData }
    }
}

impl Drop for AutoSimpleGenerationalReader<'_> {
    #[inline]
    fn drop(&mut self) {
        self.parent.read_unlock(self.generation);
        crate::arch_rs::arch_interrupt_restore(self.state);
    }
}

/// Tests for the simple generational RCU primitive.
#[cfg(ktest)]
#[unittest::suite(name = "simple_generational_rcu_rust")]
mod tests {
    /// Tests basic read lock and unlock using AutoSimpleGenerationalReader.
    #[test]
    fn basic_read() {
        let rcu = SimpleGenerational::new();
        {
            let _reader = AutoSimpleGenerationalReader::new(&rcu);
        }
    }

    /// Tests synchronization when no readers are present.
    #[test]
    fn synchronize_no_readers() {
        let rcu = SimpleGenerational::new();
        // Should return immediately without readers.
        rcu.synchronize();
    }

    /// Tests read lock completion followed by synchronization.
    #[test]
    fn read_then_synchronize() {
        let rcu = SimpleGenerational::new();
        {
            let _reader = AutoSimpleGenerationalReader::new(&rcu);
        }
        rcu.synchronize();
    }

    /// Tests multiple nested readers followed by synchronization.
    #[test]
    fn multiple_readers() {
        let rcu = SimpleGenerational::new();
        {
            let _reader1 = AutoSimpleGenerationalReader::new(&rcu);
            {
                let _reader2 = AutoSimpleGenerationalReader::new(&rcu);
                {
                    let _reader3 = AutoSimpleGenerationalReader::new(&rcu);
                }
            }
        }
        rcu.synchronize();
    }
}
