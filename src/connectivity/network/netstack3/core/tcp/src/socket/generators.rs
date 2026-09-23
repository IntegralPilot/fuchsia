// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Generate TCP parameters securely.

use core::hash::{Hash, Hasher};

use netstack3_base::{Instant, Milliseconds, SeqNum, Timestamp};
use rand::{Rng, RngExt as _};
use siphasher::sip128::SipHasher24;

/// A secure generator of TCP parameters.
///
/// This generator is modeled off ISN generation as specified in RFC 6528,
/// however it applies more broadly to other TCP parameters (e.g. the initial
/// timestamp used in the timestamp option.)
#[derive(Default)]
struct Generator<Instant> {
    // Secret used to choose secure values. It will be filled by a
    // CSPRNG upon initialization. RFC suggests an implementation "could"
    // change the secret key on a regular basis, this is not something we are
    // considering as Linux doesn't seem to do that either.
    secret: [u8; 16],
    // The initial timestamp that will be used to calculate the elapsed time
    // since the beginning and that information will then be used to generate
    // secure values being requested.
    timestamp: Instant,
}

impl<I: Instant> Generator<I> {
    pub(crate) fn new(now: I, rng: &mut impl Rng) -> Self {
        let mut secret = [0; 16];
        rng.fill(&mut secret[..]);
        Self { secret, timestamp: now }
    }

    pub(crate) fn generate<A: Hash, P: Hash>(&self, now: I, local: (A, P), remote: (A, P)) -> u32 {
        let Self { secret, timestamp } = self;

        // Per RFC 6528 Section 3 (https://tools.ietf.org/html/rfc6528#section-3):
        //
        // TCP SHOULD generate its Initial Sequence Numbers with the expression:
        //
        //   ISN = M + F(localip, localport, remoteip, remoteport, secretkey)
        //
        // where M is the 4 microsecond timer, and F() is a pseudorandom
        // function (PRF) of the connection-id.
        //
        // Siphash is used here as it is the hash function used by Linux.
        let h = {
            let mut hasher = SipHasher24::new_with_key(secret);
            local.hash(&mut hasher);
            remote.hash(&mut hasher);
            hasher.finish()
        };

        // Reduce the hashed output (h: u64) to 32 bits using XOR, but also
        // preserve entropy.
        let elapsed = now.saturating_duration_since(*timestamp);
        ((elapsed.as_micros() / 4) as u32).wrapping_add(h as u32 ^ (h >> 32) as u32)
    }
}

/// A generator of Initial Sequence Numbers, as specified in RFC 6528.
#[derive(Default)]
pub struct IsnGenerator<Instant> {
    inner: Generator<Instant>,
}

impl<I: Instant> IsnGenerator<I> {
    pub(crate) fn new(now: I, rng: &mut impl Rng) -> Self {
        Self { inner: Generator::new(now, rng) }
    }

    pub(crate) fn generate<A: Hash, P: Hash>(
        &self,
        now: I,
        local: (A, P),
        remote: (A, P),
    ) -> SeqNum {
        SeqNum::new(self.inner.generate(now, local, remote))
    }
}

/// A generator of offsets for the timestamp option, as specified in RFC 7323.
#[derive(Default)]
pub struct TimestampOffsetGenerator<Instant> {
    inner: Generator<Instant>,
}

impl<I: Instant> TimestampOffsetGenerator<I> {
    pub(crate) fn new(now: I, rng: &mut impl Rng) -> Self {
        Self { inner: Generator::new(now, rng) }
    }

    pub(crate) fn generate<A: Hash, P: Hash>(
        &self,
        now: I,
        local: (A, P),
        remote: (A, P),
    ) -> Timestamp<Milliseconds> {
        // Per RFC 7323, section 5.4:
        //   A random offset may be added to the timestamp clock on a per-
        //   connection basis.  See [RFC6528], Section 3, on randomizing the
        //   initial sequence number (ISN).  The same function with a different
        //   secret key can be used to generate the per-connection timestamp
        //   offset.
        Timestamp::new(self.inner.generate(now, local, remote))
    }
}
