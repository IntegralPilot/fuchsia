// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements a clock backed by memory mapped into this process' virtual address
//! space.  See [MappedClock] for details.

use zx_status::Status;

/// The size of the memory region used by the memory mapped clock. While in theory this size could
/// change in future Fuchsia releases, in practice it is likely never going to. So we expose it
/// as a constant.
pub const CLOCK_SIZE: usize = 4096;

/// A clock backed by memory mapped into this process' virtual address space.
///
/// A memory mapped clock can be read more efficiently than a regular kernel
/// clock object in contexts where making syscalls are undesirable, for example
/// for efficiency reasons.
///
/// A memory mapped clock will clean up after itself when going out of scope.
///
/// To create one, you will need a [zx::Clock], a [zx::Vmar] and a call to
/// [MappedClock::try_new].
#[derive(Debug)]
pub struct MappedClock<Reference: zx::Timeline, Output: zx::Timeline> {
    // The address range that the clock is mapped into.  We keep a reference
    // so we can unmap the range when this struct goes out of scope.
    parent_vmar: zx::Vmar,
    // The virtual address of the memory mapped clock.
    addr: usize,
    // The size of the memory area used by this mappable clock. It is constant
    // for the lifetime of the clock. This value is set by Zircon.
    clock_size: usize,
    // Unmap the clock when dropping MappedClock.
    unmap_on_drop: bool,
    _mark: std::marker::PhantomData<(Reference, Output)>,
}

impl<Reference: zx::Timeline, Output: zx::Timeline> Drop for MappedClock<Reference, Output> {
    fn drop(&mut self) {
        if self.unmap_on_drop {
            unsafe {
                // SAFETY: try_new ensures `addr` and `clock_size` are correctly initialized and
                // valid while this struct lives. The only way for the unmap to fail is if vmar
                // is somehow invalid, or if someone already called unmap for this clock, both of
                // which require unsafe code, which can not happen by accident.
                self.parent_vmar
                    .unmap(self.raw_addr(), self.clock_size)
                    .expect("address should be unmappable");
            }
        }
    }
}

impl<Reference: zx::Timeline, Output: zx::Timeline> MappedClock<Reference, Output> {
    /// Tries to convert the supplied regular `clock` into a memory mapped clock.
    ///
    /// A memory mapped clock can be read more efficiently than a regular kernel
    /// clock object in contexts where calling into the kernel is undesirable.
    /// At the same time, updates to the memory mapped clock can be
    /// observed consistently with any other observers of the same underlying clock,
    /// a property guaranteed by Zircon.
    ///
    /// As a tradeoff, a memory mapped clock may offer a restricted set of methods,
    /// and has more complex construction and lifecycle as compared to [zx::Clock].
    ///
    /// To ensure that there is no confusion as to how the clock is accessed, this
    /// conversion consumes [zx::Clock], and is not reversible. If you need
    /// to use [Self] both as a regular and mapped clock, it is probably a good idea
    /// to call `duplicate_handle()` on [zx::Clock] before calling this method.
    ///
    /// # Args
    ///
    /// - `clock`: the clock to convert to a mapped clock.
    /// - `parent_vmar`: a handle to the virtual memory address range to map the clock into.
    ///   The clock will be unmapped when [Self] goes out of scope. Must be cloneable.
    /// - `vmar_flags`: flags to apply when mapping the clock. Usually this needs to be at least
    ///   `zx::VmarFlags::PERM_READ`.
    ///
    /// # Errors
    ///
    /// The conversion may fail if `clock` was not created as a mappable clock using
    /// [ClockOpts::MAPPABLE] at its creation time.
    pub fn try_new(
        clock: zx::Clock<Reference, Output>,
        parent_vmar: &zx::Vmar,
        vmar_flags: zx::VmarFlags,
    ) -> Result<MappedClock<Reference, Output>, Status> {
        Self::try_new_internal(
            &clock,
            parent_vmar,
            vmar_flags,
            /*unmap_on_drop=*/ true,
            /*offset=*/ 0,
        )
    }

    /// Same as [try_new], but allows mapping with a specified offset.
    ///
    /// # Args
    ///
    /// Same as [try_new], except:
    /// - `vmar_flags`: must include `SPECIFIC` if `offset` is not zero.
    pub fn try_new_with_offset(
        clock: zx::Clock<Reference, Output>,
        parent_vmar: &zx::Vmar,
        vmar_flags: zx::VmarFlags,
        offset: u64,
    ) -> Result<MappedClock<Reference, Output>, Status> {
        Self::try_new_internal(
            &clock,
            parent_vmar,
            vmar_flags,
            /*unmap_on_drop=*/ true,
            offset,
        )
    }

    /// Same as `try_new`, but does not unmap the clock at end of this struct's lifetime.
    pub fn try_new_without_unmap(
        clock: &zx::Clock<Reference, Output>,
        parent_vmar: &zx::Vmar,
        vmar_flags: zx::VmarFlags,
        offset: u64,
    ) -> Result<MappedClock<Reference, Output>, Status> {
        Self::try_new_internal(
            clock,
            parent_vmar,
            vmar_flags,
            /*unmap_on_drop=*/ false,
            offset,
        )
    }

    fn try_new_internal(
        clock: &zx::Clock<Reference, Output>,
        parent_vmar: &zx::Vmar,
        vmar_flags: zx::VmarFlags,
        unmap_on_drop: bool,
        offset: u64,
    ) -> Result<MappedClock<Reference, Output>, Status> {
        let offset: usize = offset.try_into().map_err(|err| {
            log::error!(
                "[{}:{}] could not convert into usize: {offset:?}: {err:?}",
                file!(),
                line!()
            );
            zx::Status::INTERNAL
        })?;
        // The easiest way to ensure we can drop() without affecting the caller
        // API. Consider changing if it becomes a problem.
        let parent_vmar = parent_vmar
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .inspect_err(|err| log::error!("MappedClock: try_new: {err:?}"))?;
        // Follows the C++ example from:
        // https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0266_memory_mappable_kernel_clocks
        let clock_size = zx::Clock::get_mapped_size(&clock)
            .inspect_err(|err| log::error!("in get_mapped_size: {err:?}"))?;
        let addr =
            parent_vmar.map_clock(vmar_flags, offset, &clock, clock_size).inspect_err(|err| {
                log::error!("MappedClock: map_clock: {err:?}, {parent_vmar:?}, {offset:0x}")
            })?;

        Ok(Self {
            parent_vmar,
            addr,
            clock_size: CLOCK_SIZE,
            unmap_on_drop,
            _mark: std::marker::PhantomData,
        })
    }

    /// Returns the raw value of the address this clock is mapped to.
    pub fn raw_addr(&self) -> usize {
        self.addr
    }

    /// The size of the memory region occupied by this memory mapped clock.
    pub fn size(&self) -> usize {
        self.clock_size
    }

    /// Read the clock indication.
    ///
    /// This is the same method as on [Clock].
    pub fn read(&self) -> Result<zx::Instant<Output>, Status> {
        unsafe {
            // SAFETY: try_new ensures self.addr is correctly initialized.
            zx::Clock::<Reference, Output>::read_mapped(self.addr)
        }
    }

    /// Get the clock details, such as backtop time and similar.
    ///
    /// This is the same method as on [Clock].
    pub fn get_details(&self) -> Result<zx::ClockDetails<Reference, Output>, Status> {
        unsafe {
            // SAFETY: try_new ensures self.addr is correctly initialized.
            zx::Clock::<Reference, Output>::get_details_mapped(self.addr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_runtime as frt;

    #[test]
    fn try_mapping() {
        let clock = zx::SyntheticClock::create(
            zx::ClockOpts::MAPPABLE | zx::ClockOpts::MONOTONIC,
            Some(zx::SyntheticInstant::from_nanos(42)),
        )
        .unwrap();
        let vmar_root = frt::vmar_root_self();
        {
            let clock_clone = clock.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
            let mapped_clock =
                MappedClock::try_new(clock_clone, &vmar_root, zx::VmarFlags::PERM_READ).unwrap();

            // Check that the clock details are appropriate.
            let details = mapped_clock.get_details().unwrap();
            assert_eq!(zx::SyntheticInstant::from_nanos(42), details.backstop);

            // An unstarted clock is stuck at backstop.
            let now = mapped_clock.read().unwrap();
            assert_eq!(zx::SyntheticInstant::from_nanos(42), now);
            {
                let _clock_ref = &mapped_clock;
                // Doesn't explode.
                let now = _clock_ref.read().unwrap();
                assert_eq!(zx::SyntheticInstant::from_nanos(42), now);
            }
            assert!(mapped_clock.raw_addr() != 0);

            // Unmapped here.
        }
        assert_eq!(zx::SyntheticInstant::from_nanos(42), clock.read().unwrap());
    }
}
