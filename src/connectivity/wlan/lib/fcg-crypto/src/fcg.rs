// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::boringssl::Bignum;
use crate::sae::SaeParameters;
use anyhow::Error;

/// IEEE 802.11-2016 12.4.4
/// SAE may use many different finite cyclic groups (FCGs) to compute the various values used
/// during the handshake. This trait allows our SAE implementation to seamlessly handle
/// different classes of FCG. IEEE 802.11-2016 defines support for both elliptic curve groups
/// and finite field cryptography groups.
///
/// All functions provided by this trait will only return an Error when something internal has
/// gone wrong.
pub trait FiniteCyclicGroup {
    /// Different classes of FCG have different Element types, but scalars can always be
    /// represented by a Bignum.
    type Element;

    fn group_id(&self) -> u16;

    /// IEEE 802.11-2016 12.4.3
    /// Generates a new password element, a secret value shared by the two peers in SAE.
    fn generate_pwe(&self, params: &SaeParameters) -> Result<Self::Element, Error>;

    /// IEEE 12.4.4.1
    /// These three operators are used to manipulate FCG elements for the purposes of the
    /// Diffie-Hellman key exchange used by protocols like SAE and OWE.
    fn scalar_op(
        &self,
        scalar: &Bignum,
        element: &Self::Element,
    ) -> Result<Self::Element, Error>;
    fn elem_op(
        &self,
        element1: &Self::Element,
        element2: &Self::Element,
    ) -> Result<Self::Element, Error>;
    fn inverse_op(&self, element: Self::Element) -> Result<Self::Element, Error>;

    /// Returns the prime order of the FCG.
    fn order(&self) -> Result<Bignum, Error>;
    /// Return the generator point of the FCG.
    fn generator(&self) -> Result<Self::Element, Error>;

    /// IEEE 802.11-2016 12.4.5.4
    /// Maps the given secret element to the shared secret value. Returns None if this is the
    /// identity element for this FCG, indicating that we have in invalid secret element.
    fn map_to_secret_value(&self, element: &Self::Element) -> Result<Option<Vec<u8>>, Error>;
    /// IEEE 802.11-2016 12.4.2: The FCG Element must convert into an octet string such
    /// that it may be included in the confirmation hash when completing SAE.
    fn element_to_octets(&self, element: &Self::Element) -> Result<Vec<u8>, Error>;
    /// RFC 8110 4.3 + RFC 6090 4.2 and 6.2
    /// For OWE, The compact representation may be used to send the public key. In that case,
    /// the FCG Element is converted into an octet string containing only the x-coordinate.
    fn element_to_octets_compact(&self, element: &Self::Element) -> Result<Vec<u8>, Error>;
    /// Convert octets into an element. Returns None if the given octet string does not
    /// contain a valid element for this group.
    fn element_from_octets(&self, octets: &[u8]) -> Result<Option<Self::Element>, Error>;
    /// Convert octets that use the compact representation into an element. Returns None
    /// if the given octet string does not contain a valid element for this group.
    fn element_from_octets_compact(
        &self,
        octets: &[u8],
    ) -> Result<Option<Self::Element>, Error>;

    /// Return the expected size of scalar and element values when serialized into a frame.
    fn scalar_size(&self) -> Result<usize, Error> {
        self.order().map(|order| order.len())
    }
}
