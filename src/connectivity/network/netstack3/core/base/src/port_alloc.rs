// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Ephemeral port allocation provider.
//!
//! Defines [`PortAllocImpl`] trait and [`simple_randomized_port_alloc`], used
//! for ephemeral port allocations in transport protocols.

use core::hash::Hash;
use core::marker::PhantomData;
use core::ops::RangeInclusive;

use rand::{Rng, RngExt as _};

/// A port number.
// NB: `PortNumber` could be a trait, but given the expected use of the
// PortAlloc algorithm is to allocate `u16` ports, it's just defined as a type
// alias for simplicity.
type PortNumber = u16;

/// Trait that configures the behavior of port allocation.
///
/// `PortAllocImpl` provides the types, custom behaviors, and port availability
/// checks necessary to operate the port allocation algorithm.
pub trait PortAllocImpl {
    /// The range of ports that can be allocated.
    ///
    /// Local ports used in transport protocols are called [Ephemeral Ports].
    /// Different transport protocols may define different ranges for the issued
    /// ports. Port allocation algorithms should guarantee to return a port in
    /// this range.
    ///
    /// [Ephemeral Ports]: https://tools.ietf.org/html/rfc6056#section-2
    const EPHEMERAL_RANGE: RangeInclusive<PortNumber>;
    /// The "flow" identifier used to allocate port Ids.
    ///
    /// The `Id` is typically the 3 elements other other than the local port in
    /// the 4-tuple (local IP:port, remote IP:port) that is used to uniquely
    /// identify the flow information of a connection.
    type Id: Hash;

    /// An extra argument passed to `is_port_available`.
    type PortAvailableArg;

    /// Returns a random ephemeral port in `EPHEMERAL_RANGE`
    fn rand_ephemeral<R: Rng>(rng: &mut R) -> EphemeralPort<Self> {
        EphemeralPort::new_random(rng)
    }

    /// Checks if `port` is available to be used for the flow `id`.
    ///
    /// Implementers return `true` if the provided `port` is available to be
    /// used for a given flow `id`. An available port is a port that would not
    /// conflict for the given `id` *plus* ideally the port is not in LISTEN or
    /// CLOSED states for a given protocol (see [RFC 6056]).
    ///
    /// Note: Callers must guarantee that the given port being checked is within
    /// the `EPHEMERAL_RANGE`.
    ///
    /// [RFC 6056]: https://tools.ietf.org/html/rfc6056#section-2.2
    fn is_port_available(
        &self,
        id: &Self::Id,
        port: PortNumber,
        arg: &Self::PortAvailableArg,
    ) -> bool;
}

/// A witness type for a port within some ephemeral port range.
///
/// `EphemeralPort` is always guaranteed to contain a port that is within
/// `I::EPHEMERAL_RANGE`.
pub struct EphemeralPort<I: PortAllocImpl + ?Sized> {
    port: PortNumber,
    _marker: PhantomData<I>,
}

impl<I: PortAllocImpl + ?Sized> EphemeralPort<I> {
    /// Creates a new `EphemeralPort` with a port chosen randomly in `range`.
    pub fn new_random<R: Rng>(rng: &mut R) -> Self {
        let port = rng.random_range(I::EPHEMERAL_RANGE);
        Self { port, _marker: PhantomData }
    }

    /// Increments the current [`PortNumber`] to the next value in the contained
    /// range, wrapping around to the start of the range.
    pub fn next(&mut self) {
        if self.port == *I::EPHEMERAL_RANGE.end() {
            self.port = *I::EPHEMERAL_RANGE.start();
        } else {
            self.port += 1;
        }
    }

    /// Gets the `PortNumber` value.
    pub fn get(&self) -> PortNumber {
        self.port
    }
}

/// Implements the [algorithm 1] as described in RFC 6056.
///
/// [algorithm 1]: https://datatracker.ietf.org/doc/html/rfc6056#section-3.3.1
pub fn simple_randomized_port_alloc<I: PortAllocImpl + ?Sized, R: Rng>(
    rng: &mut R,
    id: &I::Id,
    state: &I,
    arg: &I::PortAvailableArg,
) -> Option<PortNumber> {
    let num_ephemeral = u32::from(I::EPHEMERAL_RANGE.end() - I::EPHEMERAL_RANGE.start()) + 1;
    let mut candidate = EphemeralPort::<I>::new_random(rng);
    for _ in 0..num_ephemeral {
        if state.is_port_available(id, candidate.get(), arg) {
            return Some(candidate.get());
        }
        candidate.next();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeCryptoRng;

    /// A fake flow identifier.
    #[derive(Hash)]
    struct FakeId(usize);

    /// Number of different RNG seeds used in tests in this mod.
    const RNG_ROUNDS: u128 = 128;

    /// Hard-coded fake of available port filter.
    enum FakeAvailable {
        /// Only a single port is available.
        AllowSingle(PortNumber),
        /// No ports are available.
        DenyAll,
        /// Only even-numbered ports are available.
        AllowEvens,
    }

    /// Fake implementation of [`PortAllocImpl`].
    ///
    /// The `available` field will dictate the return of
    /// [`PortAllocImpl::is_port_available`] and can be set to get the expected
    /// testing behavior.
    struct FakeImpl {
        available: FakeAvailable,
    }

    impl PortAllocImpl for FakeImpl {
        const EPHEMERAL_RANGE: RangeInclusive<u16> = 100..=200;
        type Id = FakeId;
        type PortAvailableArg = ();

        fn is_port_available(&self, _id: &Self::Id, port: u16, (): &()) -> bool {
            match self.available {
                FakeAvailable::AllowEvens => (port & 1) == 0,
                FakeAvailable::DenyAll => false,
                FakeAvailable::AllowSingle(p) => port == p,
            }
        }
    }

    /// Helper fn to test that if only a single port is available, we will
    /// eventually get that
    fn test_allow_single(single: u16) {
        FakeCryptoRng::with_fake_rngs(RNG_ROUNDS, |mut rng| {
            let fake = FakeImpl { available: FakeAvailable::AllowSingle(single) };
            let port = simple_randomized_port_alloc(&mut rng, &FakeId(0), &fake, &());
            assert_eq!(port.unwrap(), single);
        });
    }

    #[test]
    fn test_single_range_start() {
        // Test boundary condition for first ephemeral port.
        test_allow_single(FakeImpl::EPHEMERAL_RANGE.start().clone())
    }

    #[test]
    fn test_single_range_end() {
        // Test boundary condition for last ephemeral port.
        test_allow_single(FakeImpl::EPHEMERAL_RANGE.end().clone())
    }

    #[test]
    fn test_single_range_mid() {
        // Test some other ephemeral port.
        test_allow_single((FakeImpl::EPHEMERAL_RANGE.end() + FakeImpl::EPHEMERAL_RANGE.start()) / 2)
    }

    #[test]
    fn test_allow_none() {
        // Test that if no ports are available, try_alloc must return none.
        FakeCryptoRng::with_fake_rngs(RNG_ROUNDS, |mut rng| {
            let fake = FakeImpl { available: FakeAvailable::DenyAll };
            let port = simple_randomized_port_alloc(&mut rng, &FakeId(0), &fake, &());
            assert_eq!(port, None);
        });
    }

    #[test]
    fn test_allow_evens() {
        // Test that if we only allow even ports, we will always get ports in
        // the specified range, and they'll always be even.
        FakeCryptoRng::with_fake_rngs(RNG_ROUNDS, |mut rng| {
            let fake = FakeImpl { available: FakeAvailable::AllowEvens };
            let port = simple_randomized_port_alloc(&mut rng, &FakeId(0), &fake, &()).unwrap();
            assert!(FakeImpl::EPHEMERAL_RANGE.contains(&port));
            assert_eq!(port & 1, 0);
        });
    }

    #[test]
    fn test_ephemeral_port_random() {
        // Test that random ephemeral ports are always in range.
        let mut rng = FakeCryptoRng::new_xorshift(0);
        for _ in 0..1000 {
            let rnd_port = EphemeralPort::<FakeImpl>::new_random(&mut rng);
            assert!(FakeImpl::EPHEMERAL_RANGE.contains(&rnd_port.port));
        }
    }

    #[test]
    fn test_ephemeral_port_next() {
        let mut port = EphemeralPort::<FakeImpl> {
            port: *FakeImpl::EPHEMERAL_RANGE.start(),
            _marker: PhantomData,
        };
        // Loop over all the range twice so we see the wrap-around.
        for _ in 0..=1 {
            for x in FakeImpl::EPHEMERAL_RANGE {
                assert_eq!(port.port, x);
                port.next();
            }
        }
    }
}
