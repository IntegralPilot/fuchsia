// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, bail};
use mundane::hash::Sha256;
use num::FromPrimitive;
use wlan_common::ie::rsn::akm::{self, Akm};

use crate::boringssl::{Bignum, EcGroupId, EcPoint};
use crate::ecc;
use crate::fcg::FiniteCyclicGroup;
use crate::hmac_utils::{HmacUtils, HmacUtilsImpl};

/// We store an FcgConstructor rather than a FiniteCyclicGroup so that our handshake
/// can impl `Send`. FCGs are not generally `Send`, so we construct them on the fly.
type FcgConstructor<E> =
    Box<dyn Fn() -> Result<Box<dyn FiniteCyclicGroup<Element = E>>, Error> + Send + 'static>;

#[derive(Debug)]
pub enum OweUpdate {
    TxPublicKey { group_id: u16, key: Vec<u8> },
    Success { key: Vec<u8> },
}

pub type OweUpdateSink = Vec<OweUpdate>;

/// IEEE 8021.11-2024, 12.14: Opportunistic Wireless Encryption
///
/// An OWE handshake occurs during a association between a client and an AP. The client
/// initiates the handshake by generating a public/private key pair and sending its public key
/// to the AP. The AP then also generates a own public/private key pair and responds with its
/// own public key. Both parties then compute a shared secret and derive the Pairwise Master Key
/// (PMK) from it.
pub trait ClientOweHandshake: Send {
    /// Initiate OWE by generating a public/private key pair and putting the public key
    /// in the update sink.
    fn initiate_owe(&mut self, sink: &mut OweUpdateSink) -> Result<(), Error>;

    /// Handle the received public key from the AP and compute the shared secret.
    /// A PMK is derived and put in the update sink.
    fn handle_public_key(
        &mut self,
        sink: &mut OweUpdateSink,
        group: u16,
        peer_public_key: Vec<u8>,
    ) -> Result<(), Error>;
}

/// IEEE 8021.11-2024, 12.14: Opportunistic Wireless Encryption
pub trait ApOweHandshake: Send {
    /// Handle the received public key from the client by generating a public/private key pair,
    /// computing the shared secret, and deriving the PMK. The AP's public key and the PMK
    /// will be put in the update sink.
    fn handle_public_key(
        &mut self,
        sink: &mut OweUpdateSink,
        group: u16,
        peer_public_key: Vec<u8>,
    ) -> Result<(), Error>;
}

pub enum OweHandshakeEntity {
    Client,
    Ap,
}

/// Creates a client OWE handshake for the given group ID and authentication parameters.
pub fn new_client_owe_handshake(
    group_id: u16,
    akm: Akm,
) -> Result<Box<dyn ClientOweHandshake>, Error> {
    new_owe_handshake(group_id, akm, OweHandshakeEntity::Client)
        .map(|h| Box::new(h) as Box<dyn ClientOweHandshake>)
}

/// Creates a AP OWE handshake for the given group ID and authentication parameters.
pub fn new_ap_owe_handshake(group_id: u16, akm: Akm) -> Result<Box<dyn ApOweHandshake>, Error> {
    new_owe_handshake(group_id, akm, OweHandshakeEntity::Ap)
        .map(|h| Box::new(h) as Box<dyn ApOweHandshake>)
}

fn new_owe_handshake(
    group_id: u16,
    akm: Akm,
    entity: OweHandshakeEntity,
) -> Result<OweHandshakeImpl<EcPoint>, Error> {
    match akm.suite_type {
        akm::OWE => (),
        _ => bail!("Cannot construct OWE handshake with AKM {:?}", akm),
    };
    let (hmac, group_constructor) = match EcGroupId::from_u16(group_id) {
        Some(EcGroupId::P256) => {
            // IEEE 802.11-2024, 12.4.2
            // Group 19 has a 256-bit prime length, thus we use SHA256.
            let hmac = Box::new(HmacUtilsImpl::<Sha256>::new());
            let group_constructor = Box::new(|| {
                ecc::Group::new(EcGroupId::P256).map(|group| {
                    Box::new(group)
                        as Box<
                            dyn FiniteCyclicGroup<
                                Element = <ecc::Group as FiniteCyclicGroup>::Element,
                            >,
                        >
                })
            });
            (hmac, group_constructor)
        }
        _ => bail!("Unsupported OWE group id: {}", group_id),
    };

    let params = OweParameters { hmac, group_id, entity };
    Ok(OweHandshakeImpl::new(group_constructor, params))
}

#[derive(Debug, Clone)]
struct AsymmetricKeyPair {
    private_key: Vec<u8>,
    public_key: Vec<u8>,
}

struct OweHandshakeImpl<E> {
    fcg: FcgConstructor<E>,
    key_pair: Option<AsymmetricKeyPair>,
    params: OweParameters,
}

struct OweParameters {
    pub hmac: Box<dyn HmacUtils + Send>,
    pub group_id: u16,
    entity: OweHandshakeEntity,
}

impl<E> OweHandshakeImpl<E> {
    pub fn new(fcg_constructor: FcgConstructor<E>, params: OweParameters) -> Self {
        Self { fcg: fcg_constructor, key_pair: None, params }
    }

    fn generate_keys(&mut self, sink: &mut OweUpdateSink) -> Result<(), Error> {
        // IEEE 802.11-2024, 12.14.2
        // 1 < private_key < order
        self.internal_generate_keys(sink, |order| Bignum::rand_range_ex(2, &order))
    }

    fn internal_generate_keys(
        &mut self,
        sink: &mut OweUpdateSink,
        generate_private_key: impl FnOnce(Bignum) -> Result<Bignum, Error>,
    ) -> Result<(), Error> {
        let fcg = (self.fcg)()?;
        let order = fcg.order()?;
        let private_key = generate_private_key(order)?;
        // IEEE 802.11-2024, 12.14.2
        // M = scalar-op(m, G) -- where M and m are the public/private key and G is the generator
        let generator: E = fcg.generator()?;
        let public_key_element = fcg.scalar_op(&private_key, &generator)?;
        // RFC 8110, Section 4.3 - If the public key is from a curve defined in [RFC6090],
        // compact representation SHALL be used.
        let public_key = fcg.element_to_octets_compact(&public_key_element)?;
        self.key_pair = Some(AsymmetricKeyPair {
            private_key: private_key.to_be_vec(0),
            public_key: public_key.clone(),
        });
        sink.push(OweUpdate::TxPublicKey { group_id: self.params.group_id, key: public_key });
        Ok(())
    }

    // Internal method to handle the received public key from the peer and compute the shared
    // secret.
    //
    // The behavior is slightly different depending on whether the entity is a client or an AP.
    fn internal_handle_public_key(
        &mut self,
        sink: &mut OweUpdateSink,
        group: u16,
        peer_public_key: Vec<u8>,
    ) -> Result<(), Error> {
        if group != EcGroupId::P256.id() {
            bail!("Received unsupported OWE group {}", group);
        }
        if let OweHandshakeEntity::Ap = self.params.entity {
            if self.key_pair.is_none() {
                self.generate_keys(sink)?;
            }
        }
        let (private_key, self_public_key) = match &self.key_pair {
            Some(key_pair) => {
                (Bignum::new_from_slice(&key_pair.private_key)?, key_pair.public_key.clone())
            }
            _ => bail!("Received public key from peer but private key has not been generated"),
        };
        let fcg = (self.fcg)()?;
        // RFC 8110, Section 4.3 - If the public key is from a curve defined in [RFC6090],
        // compact representation SHALL be used.
        let peer_public_key_element = match fcg.element_from_octets_compact(&peer_public_key)? {
            Some(element) => element,
            None => bail!("Failed to convert peer public key octets to FCG element"),
        };
        // IEEE 802.11-2024, 12.14.2
        // S = scalar-op(m, N) -- where m is the private key and N is the peer's public key
        let element_s = fcg.scalar_op(&private_key, &peer_public_key_element)?;
        // s = F(S) -- where F() is the element-to-scalar mapping function
        let s = match fcg.map_to_secret_value(&element_s)? {
            Some(s) => s,
            None => bail!("Failed to map shared secret element to scalar"),
        };
        // prk = HKDF-Extract(C || A || group, s) -- where C and A are the client and AP public keys
        let (client_public_key, ap_public_key) = match self.params.entity {
            OweHandshakeEntity::Client => (&self_public_key, &peer_public_key),
            OweHandshakeEntity::Ap => (&peer_public_key, &self_public_key),
        };
        let salt =
            concat_public_keys_and_group(client_public_key, ap_public_key, self.params.group_id);
        let prk = self.params.hmac.hkdf_extract(&salt, &s);
        // pmk = HKDF-Expand(prk, "OWE Key Generation", key_length)
        let pmk =
            self.params.hmac.hkdf_expand(&prk, "OWE Key Generation", self.params.hmac.bits() / 8);
        sink.push(OweUpdate::Success { key: pmk });
        Ok(())
    }
}

impl<E> ClientOweHandshake for OweHandshakeImpl<E> {
    fn initiate_owe(&mut self, sink: &mut OweUpdateSink) -> Result<(), Error> {
        self.generate_keys(sink)
    }

    fn handle_public_key(
        &mut self,
        sink: &mut OweUpdateSink,
        group: u16,
        ap_public_key: Vec<u8>,
    ) -> Result<(), Error> {
        self.internal_handle_public_key(sink, group, ap_public_key)
    }
}

impl<E> ApOweHandshake for OweHandshakeImpl<E> {
    fn handle_public_key(
        &mut self,
        sink: &mut OweUpdateSink,
        group: u16,
        client_public_key: Vec<u8>,
    ) -> Result<(), Error> {
        self.generate_keys(sink)?;
        self.internal_handle_public_key(sink, group, client_public_key)
    }
}

fn concat_public_keys_and_group(
    client_public_key: &[u8],
    ap_public_key: &[u8],
    group_id: u16,
) -> Vec<u8> {
    let mut result: Vec<u8> = Vec::with_capacity(client_public_key.len() + ap_public_key.len() + 2);
    result.extend_from_slice(&client_public_key);
    result.extend_from_slice(&ap_public_key);
    result.extend_from_slice(&group_id.to_le_bytes());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use wlan_common::ie::rsn::akm::AKM_OWE;

    struct TestHandshake {
        client: Box<dyn ClientOweHandshake>,
        ap: Box<dyn ApOweHandshake>,
    }

    struct TxPublicKey {
        group_id: u16,
        key: Vec<u8>,
    }

    impl TestHandshake {
        fn new(group_id: u16) -> Self {
            let client = new_client_owe_handshake(group_id, AKM_OWE).unwrap();
            let ap = new_ap_owe_handshake(group_id, AKM_OWE).unwrap();
            Self { client, ap }
        }

        fn client_initiate_owe(&mut self) -> TxPublicKey {
            let mut sink = OweUpdateSink::new();
            self.client.initiate_owe(&mut sink).unwrap();
            assert_eq!(sink.len(), 1);
            expect_tx_public_key(&mut sink)
        }

        fn ap_handle_public_key(
            &mut self,
            client_public_key: TxPublicKey,
        ) -> (TxPublicKey, Vec<u8>) {
            let mut sink = OweUpdateSink::new();
            self.ap
                .handle_public_key(&mut sink, client_public_key.group_id, client_public_key.key)
                .unwrap();
            assert_eq!(sink.len(), 2);
            let ap_public_key = expect_tx_public_key(&mut sink);
            let pmk = assert_matches!(sink.remove(0), OweUpdate::Success { key } => key);
            (ap_public_key, pmk)
        }

        fn client_handle_public_key(&mut self, ap_public_key: TxPublicKey) -> Vec<u8> {
            let mut sink = OweUpdateSink::new();
            self.client
                .handle_public_key(&mut sink, ap_public_key.group_id, ap_public_key.key)
                .unwrap();
            assert_eq!(sink.len(), 1);
            assert_matches!(sink.remove(0), OweUpdate::Success { key } => key)
        }
    }

    fn expect_tx_public_key(sink: &mut OweUpdateSink) -> TxPublicKey {
        let (group_id, key) = assert_matches!(sink.remove(0), OweUpdate::TxPublicKey { group_id, key } => (group_id, key));
        TxPublicKey { group_id, key }
    }

    #[test]
    fn test_owe_handshake() {
        for i in 0..10 {
            let mut handshake = TestHandshake::new(EcGroupId::P256.id());
            let client_public_key = handshake.client_initiate_owe();
            let (ap_public_key, ap_pmk) = handshake.ap_handle_public_key(client_public_key);
            let client_pmk = handshake.client_handle_public_key(ap_public_key);
            if client_pmk != ap_pmk {
                panic!(
                    "Iteration {}: PMKs do not match:\n client: {:x?}\n ap: {:x?}",
                    i, client_pmk, ap_pmk
                );
            }
        }
    }

    #[test]
    fn test_new_handshake_unsupported_group() {
        for group_id in [20, 21] {
            let result = new_owe_handshake(group_id, AKM_OWE, OweHandshakeEntity::Client);
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_handshake_handle_public_key_unsupported_group() {
        let mut handshake = TestHandshake::new(EcGroupId::P256.id());
        let client_public_key = handshake.client_initiate_owe();
        for group in [20, 21] {
            let result = handshake.ap.handle_public_key(
                &mut Vec::new(),
                group,
                client_public_key.key.clone(),
            );
            assert!(result.is_err());
        }
    }

    // There isn't an official OWE test vector that we know of.
    // The below test vector is generated from running our own implementation and is intended
    // to catch future regressions.
    const SELF_PRIVATE_KEY: &str =
        "5d6edb96f46993e0fb7621f93e5d3450813eb8c63ece17ecbf45376c574debb2";
    const EXPECTED_SELF_PUBLIC_KEY: &str =
        "4abf7c206407893a1c8b27a7b9b2c1dc6e3d7038fed9ad7ae2250d03dc18e080";
    const PEER_PUBLIC_KEY: &str =
        "40ac0ba500964f5872fb969e673d0c83869dd8976c688cec46fe4ba4b140e7ef";
    const EXPECTED_PMK: &str = "afac9db29ab652d7f76054c9c51204a3bb59339e302523d9c114b269a46f5734";

    #[test]
    fn test_handshake_test_vector() {
        let mut handshake =
            new_owe_handshake(EcGroupId::P256.id(), AKM_OWE, OweHandshakeEntity::Client).unwrap();
        let mut sink = OweUpdateSink::new();
        handshake
            .internal_generate_keys(&mut sink, |_| {
                let private_key = hex::decode(SELF_PRIVATE_KEY).unwrap();
                Ok(Bignum::new_from_slice(&private_key).unwrap())
            })
            .unwrap();
        let tx_public_key = expect_tx_public_key(&mut sink);
        let expected_self_public_key = hex::decode(EXPECTED_SELF_PUBLIC_KEY).unwrap();
        assert_eq!(tx_public_key.key, expected_self_public_key);

        let peer_public_key = hex::decode(PEER_PUBLIC_KEY).unwrap();
        handshake
            .internal_handle_public_key(&mut sink, EcGroupId::P256.id(), peer_public_key)
            .unwrap();
        let pmk = assert_matches!(sink.remove(0), OweUpdate::Success { key } => key);
        let expected_pmk = hex::decode(EXPECTED_PMK).unwrap();
        assert_eq!(pmk, expected_pmk);
    }
}
