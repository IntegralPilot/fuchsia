// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Opaque interface identifier (IID) used in SLAAC.

use core::fmt::{self, Debug};

use hmac::{KeyInit as _, Mac as _};
use net_types::ip::{Ipv6Addr, Subnet};
use rand::{Rng, RngExt as _};

/// The length in bytes of the [`IidSecret`] secret key.
const IID_SECRET_KEY_BYTES: usize = 32;

/// A secret for generating [`OpaqueIid`]s.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct IidSecret([u8; IID_SECRET_KEY_BYTES]);

impl IidSecret {
    /// A static secret for use in tests.
    #[cfg(any(test, feature = "testutils"))]
    pub const ALL_ONES: Self = Self([1u8; IID_SECRET_KEY_BYTES]);

    /// A static secret for use in tests.
    #[cfg(any(test, feature = "testutils"))]
    pub const ALL_TWOS: Self = Self([2u8; IID_SECRET_KEY_BYTES]);

    /// Creates a new random secret with the provided `rng`.
    pub fn new_random<R: Rng>(rng: &mut R) -> Self {
        let mut bytes = [0u8; IID_SECRET_KEY_BYTES];
        rng.fill(&mut bytes[..]);
        Self(bytes)
    }

    /// Creates a new secret from the provided bytes.
    pub fn new(bytes: [u8; IID_SECRET_KEY_BYTES]) -> Self {
        Self(bytes)
    }

    /// Returns the inner bytes of the secret.
    pub fn into_inner(self) -> [u8; IID_SECRET_KEY_BYTES] {
        self.0
    }
}

impl Debug for IidSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Avoid putting the secret in logs.
        f.write_str("IidSecret(..)")
    }
}

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// An opaque interface identifier (IID).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct OpaqueIid(u128);

impl OpaqueIid {
    /// Computes an opaque interface identifier (IID) using the algorithm in [RFC
    /// 7217 Section 5].
    ///
    /// Each argument to this function corresponds to an argument from Section 5 of
    /// the RFC:
    /// - `prefix` corresponds to the "Prefix" argument
    /// - `net_iface` corresponds to the "Net_Iface" argument
    /// - `net_id` corresponds to the "Network_ID" argument
    /// - `nonce` corresponds to the "DAD_Counter" argument if nonce =
    /// [`OpaqueIidNonce::DadCounter`]
    /// - `secret_key` corresponds to the "secret_key" argument
    ///
    /// Callers can set `nonce` = [`OpaqueIidNonce::Random(x)`] to pass in a
    /// randomly-generated value. This guarantees the caller similar privacy
    /// properties as the original algorithm specified in the RFC without requiring
    /// that they keep state in the form of a DAD count.
    ///
    /// For fixed inputs, the output of this function is guaranteed to be stable
    /// across versions of this codebase.
    ///
    /// [RFC 7217 Section 5]: https://tools.ietf.org/html/rfc7217#section-5
    pub fn new<IF, ID>(
        prefix: Subnet<Ipv6Addr>,
        net_iface: IF,
        net_id: Option<ID>,
        dad_counter: OpaqueIidNonce,
        secret_key: &IidSecret,
    ) -> Self
    where
        IF: AsRef<[u8]>,
        ID: AsRef<[u8]>,
    {
        // OVERVIEW
        //
        // This algorithm is simple - use a cryptographically-secure hash-based
        // message authentication code (HMAC). Use the `secret_key` as the HMAC's
        // key, and the other arguments as the HMAC's input.
        //
        // HMACs and PRFs
        //
        // Per RFC 7217 Section 5, the function, "F()", must satisfy the following
        // requirements:
        //
        //  A pseudorandom function (PRF) that MUST NOT be computable from the
        //  outside (without knowledge of the secret key).  F() MUST also be
        //  difficult to reverse, such that it resists attempts to obtain the
        //  secret_key, even when given samples of the output of F() and knowledge
        //  or control of the other input parameters. F() SHOULD produce an output
        //  of at least 64 bits.  F() could be implemented as a cryptographic hash
        //  of the concatenation of each of the function parameters.
        //
        // For some HMACs, given an HMAC and a key, k, which is unknown to an
        // attacker, F(p) = HMAC(k, p) is a PRF. HMAC-SHA256 is *almost certainly*
        // one such HMAC. [1] Thus, the construction here satisfies the PRF
        // requirement.
        //
        // ALGORITHM
        //
        // Our primary goal is to ensure that `OpaqueIid` implements a PRF. Our HMAC
        // implements a PRF, and we just truncate its output to 128 bits and return it.
        // [5] Thus, all we need to do is not somehow negate the HMAC's PRF property in
        // constructing its input.
        //
        // A trivial way to do this is to ensure that any two distinct inputs to
        // `OpaqueIid::new` will result in a distinct byte sequence being fed to the
        // HMAC. We do this by feeding each input to the HMAC one at a time and, for the
        // variable-length inputs, prefixing them with a fixed-length binary
        // representation of their length. [6]
        //
        // [1] See [2]. There is some subtlety [3], however HMAC-SHA256 is used as a
        //     PRF in existing standards (e.g., [4]), and thus it is almost
        //     certainly good enough for the present purpose.
        // [2] https://en.wikipedia.org/wiki/HMAC#Security
        // [3] https://crypto.stackexchange.com/questions/88165/is-hmac-sha256-a-prf
        // [4] https://tools.ietf.org/html/rfc4868
        // [5] A PRF whose output is truncated is still a PRF. A quick proof sketch:
        //
        //     A function is a PRF if, having been drawn uniformly at random from
        //     a larger set of functions (known as a "PRF family"), there does not
        //     exist a polynomial-time adversary who is able to distinguish the
        //     function from a random oracle [7] (a "distinguisher").
        //
        //     Let f be a PRF. Let g(x) = truncate(f(x), N) be a truncated version
        //     of f which returns only the first N bits of f's output. Assume (by
        //     of contradiction) that g is not a PRF. Thus, there exists a
        //     distinguisher, D, for g. Given D, we can construct a new
        //     distinguisher, E, as follows: E(f) = D(g(x) = truncate(f(x), N)).
        //     Since truncate(f(x), N) is equivalent to g(x), then by definition,
        //     A(g(x) = truncate(f(x), N)) is able to distinguish its input from a
        //     random oracle. Thus, E is able to distinguish its input from a random
        //     oracle. This means that E is a distinguisher for f, which implies
        //     that f is not a PRF, which is a contradiction. Thus, g, the truncated
        //     version of f, is a PRF.
        // [6] This representation ensures that it is always possible to reverse the
        //     encoding and decompose the encoding into the separate arguments to
        //     `OpaqueIid::new`. This implies that no two sets of inputs to
        //     `OpaqueIid::new` will ever produce the same encoding.
        // [7] https://en.wikipedia.org/wiki/Random_oracle

        fn write_u64(hmac: &mut HmacSha256, u: u64) {
            hmac.update(&u.to_be_bytes());
        }

        fn write_usize(hmac: &mut HmacSha256, u: usize) {
            // Write `usize` values as `u64` so that we always write the same number
            // of bytes regardless of the platform.
            //
            // This `unwrap` is guaranteed not to panic unless we a) run on a 128-bit
            // platform and, b) call `OpaqueIid::new` on a byte slice which is larger than
            // 2^64 bytes.
            write_u64(hmac, u.try_into().unwrap())
        }

        let IidSecret(secret_key) = secret_key;
        let mut hmac = HmacSha256::new_from_slice(&secret_key[..]).expect("create new HmacSha256");

        // Write prefix address; no need to prefix with length because this is
        // always the same length.
        hmac.update(&prefix.network().ipv6_bytes());
        // Write prefix length, which is a single byte.
        hmac.update(&[prefix.prefix()][..]);

        // `net_iface` is variable length, so write its length first. We make sure
        // to call `net_iface.as_ref()` once and then use its return value in case
        // the `AsRef::as_ref` implementation doesn't always return the same number
        // of bytes, which would break the security of this algorithm.
        let net_iface = net_iface.as_ref();
        write_usize(&mut hmac, net_iface.len());
        hmac.update(net_iface);

        // `net_id` is variable length, so write its length first. We make sure to
        // call `net_iface.as_ref()` once and then use its return value in case the
        // `AsRef::as_ref` implementation doesn't always return the same number of
        // bytes, which would break the security of this algorithm.
        if let Some(net_id) = net_id {
            let net_id = net_id.as_ref();
            write_usize(&mut hmac, net_id.len());
            hmac.update(net_id);
        } else {
            // In a previous version of this function, `net_id` was not optional, and to
            // signify its absence a caller would pass an empty byte slice or array. The
            // zero length would then have been written to `hmac`. To maintain compatibility
            // with `OpaqueIid`s generated before this change, write zero for an unspecified
            // `net_id`.
            write_usize(&mut hmac, 0);
        }

        write_u64(&mut hmac, dad_counter.into());

        let hmac_bytes: [u8; 32] = hmac.finalize().into_bytes().into();
        Self(u128::from_be_bytes((&hmac_bytes[..16]).try_into().unwrap()))
    }

    /// Copies the bytes from this identifier in Big Endian representation.
    pub fn to_be_bytes(&self) -> [u8; 16] {
        let Self(i) = self;
        i.to_be_bytes()
    }
}

/// Describes the value being used as the nonce for [`OpaqueIid`].
///
/// See the documentation on [`OpaqueIid::new`] for more info.
#[derive(Copy, Clone, Debug)]
#[allow(missing_docs)]
pub enum OpaqueIidNonce {
    DadCounter(u8),
    Random(u64),
}

impl From<OpaqueIidNonce> for u64 {
    fn from(nonce: OpaqueIidNonce) -> Self {
        match nonce {
            OpaqueIidNonce::DadCounter(count) => count.into(),
            OpaqueIidNonce::Random(random) => random,
        }
    }
}

#[cfg(test)]
mod tests {
    use net_types::ip::Ipv6;

    use super::*;

    #[test]
    fn test_generate_opaque_interface_identifier() {
        // Default values for arguments. When testing a particular argument,
        // these can be used for the values of the other arguments.
        let default_prefix = Ipv6::SITE_LOCAL_UNICAST_SUBNET;
        let default_net_iface = &[0, 1, 2];
        let default_net_id = &[3, 4, 5];
        let default_dad_counter = OpaqueIidNonce::DadCounter(0);
        let default_secret_key = &IidSecret::ALL_ONES;

        // Test that the same arguments produce the same output.
        let iid0 = OpaqueIid::new(
            default_prefix,
            default_net_iface,
            Some(default_net_id),
            default_dad_counter,
            default_secret_key,
        );
        let iid1 = OpaqueIid::new(
            default_prefix,
            default_net_iface,
            Some(default_net_id),
            default_dad_counter,
            default_secret_key,
        );
        assert_eq!(iid0, iid1);

        // Test that modifications to any byte of `net_iface` cause a change
        // to the output.
        let net_iface = &mut default_net_iface.clone();
        let iid0 = OpaqueIid::new(
            default_prefix,
            &net_iface[..],
            Some(default_net_id),
            default_dad_counter,
            default_secret_key,
        );
        for i in 0..net_iface.len() {
            net_iface[i] += 1;
            let iid1 = OpaqueIid::new(
                default_prefix,
                &net_iface[..],
                Some(default_net_id),
                default_dad_counter,
                default_secret_key,
            );
            net_iface[i] -= 1;
            assert_ne!(iid0, iid1);
        }

        // Test that modifications to any byte of `net_id` cause a change to the
        // output.
        let net_id = &mut default_net_id.clone();
        let iid0 = OpaqueIid::new(
            default_prefix,
            default_net_iface,
            Some(&net_id[..]),
            default_dad_counter,
            default_secret_key,
        );
        for i in 0..net_id.len() {
            net_id[i] += 1;
            let iid1 = OpaqueIid::new(
                default_prefix,
                default_net_iface,
                Some(&net_id[..]),
                default_dad_counter,
                default_secret_key,
            );
            net_id[i] -= 1;
            assert_ne!(iid0, iid1);
        }

        // Test that moving a byte between `net_iface` and `net_id` causes a
        // change in the output.
        let iid0 = OpaqueIid::new(
            default_prefix,
            &[0, 1, 2],
            Some(&[3, 4, 5]),
            default_dad_counter,
            default_secret_key,
        );
        let iid1 = OpaqueIid::new(
            default_prefix,
            &[0, 1, 2, 3],
            Some(&[4, 5]),
            default_dad_counter,
            default_secret_key,
        );
        let iid2 = OpaqueIid::new(
            default_prefix,
            &[0, 1],
            Some(&[2, 3, 4, 5]),
            default_dad_counter,
            default_secret_key,
        );
        assert_ne!(iid0, iid1);
        assert_ne!(iid0, iid2);
        assert_ne!(iid1, iid2);

        // Test that a change to `dad_counter` causes a change in the output.
        let iid0 = OpaqueIid::new(
            default_prefix,
            default_net_iface,
            Some(default_net_id),
            default_dad_counter,
            default_secret_key,
        );
        let iid1 = OpaqueIid::new(
            default_prefix,
            default_net_iface,
            Some(default_net_id),
            OpaqueIidNonce::DadCounter(1),
            default_secret_key,
        );
        assert_ne!(iid0, iid1);

        // Test that a change to `secret_key` causes a change in the output.
        let iid0 = OpaqueIid::new(
            default_prefix,
            default_net_iface,
            Some(default_net_id),
            default_dad_counter,
            default_secret_key,
        );
        let mut secret_key = default_secret_key.clone();
        secret_key.0[0] += 1;
        let iid1 = OpaqueIid::new(
            default_prefix,
            default_net_iface,
            Some(default_net_id),
            default_dad_counter,
            &secret_key,
        );
        assert_ne!(iid0, iid1);
    }

    #[test]
    fn test_stable_outputs() {
        // `OpaqueIid` guarantees that it provides a stable output across codebase
        // versions. This test case asserts that, and should not be changed!
        const PREFIX: Subnet<Ipv6Addr> = Ipv6::SITE_LOCAL_UNICAST_SUBNET;
        const NET_IFACE: &[u8] = &[0, 1, 2];
        const NET_ID: Option<&[u8]> = Some(&[3, 4, 5]);
        const DAD_COUNTER: OpaqueIidNonce = OpaqueIidNonce::DadCounter(0);

        assert_eq!(
            OpaqueIid::new(PREFIX, NET_IFACE, NET_ID, DAD_COUNTER, &IidSecret::ALL_ONES),
            OpaqueIid(255541303695013087662815070945404751656),
        );

        assert_eq!(
            OpaqueIid::new(PREFIX, NET_IFACE, None::<[_; 0]>, DAD_COUNTER, &IidSecret::ALL_ONES),
            OpaqueIid(195123822206027417085233478881498908845),
        );
    }
}
