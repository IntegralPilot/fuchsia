// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::boringssl::{self, Bignum, BignumCtx, EcGroup, EcGroupId, EcGroupParams, EcPoint};
use crate::fcg::FiniteCyclicGroup;
use crate::sae::{PweMethod, SaeParameters};
use anyhow::{Error, bail};
use ieee80211::MacAddr;
use log::warn;
use num::ToPrimitive;
use num::integer::Integer;

/// An elliptic curve group to be used as the finite cyclic group for SAE.
pub struct Group {
    id: EcGroupId,
    group: EcGroup,
    bn_ctx: BignumCtx,
}

impl Group {
    /// Construct a new FCG using the curve parameters specified by the given ID
    /// for underlying operations.
    pub fn new(ec_group: EcGroupId) -> Result<Self, Error> {
        Ok(Self { id: ec_group.clone(), group: EcGroup::new(ec_group)?, bn_ctx: BignumCtx::new()? })
    }
}

/// Concatenates two MAC addresses in canonical order (largest one first)
fn concat_mac_addrs(sta_a_mac: &MacAddr, sta_b_mac: &MacAddr) -> Vec<u8> {
    let mut result: Vec<u8> = Vec::with_capacity(sta_a_mac.len() + sta_b_mac.len());
    match sta_a_mac.cmp(sta_b_mac) {
        std::cmp::Ordering::Less => {
            result.extend_from_slice(sta_b_mac.as_slice());
            result.extend_from_slice(sta_a_mac.as_slice());
        }
        _ => {
            result.extend_from_slice(sta_a_mac.as_slice());
            result.extend_from_slice(sta_b_mac.as_slice());
        }
    };

    result
}

// The minimum number of times we must attempt to generate a PWE to avoid timing attacks.
// IEEE Std 802.11-2016 12.4.4.2.2 states that this number should be high enough that the
// probability of not finding a PWE is "sufficiently small". For a given odd prime modulus, there
// are (p + 1) / 2 quadratic residues. This means that for large p, there's a ~50% chance of
// finding a residue on each PWE iteration, so the probability of exceeding our number of iters is
// (1/2)^MIN_PWE_ITER. At 50 iterations, that's about 1 in 80 quadrillion... Seems sufficient.
const MIN_PWE_ITER: u8 = 50;
const KDF_LABEL: &'static str = "SAE Hunting and Pecking";

/// Computes the left side of an elliptic curve equation, y^2 = x^3 + ax + b mod p
fn compute_y_squared(x: &Bignum, curve: &EcGroupParams, ctx: &BignumCtx) -> Result<Bignum, Error> {
    // x^3 mod p
    let y = x.mod_exp(&Bignum::new_from_u64(3)?, &curve.p, ctx)?;
    // x^3 + ax mod p
    let y = y.mod_add(&curve.a.mod_mul(&x, &curve.p, ctx)?, &curve.p, ctx)?;
    // x^3 + ax + b mod p
    y.mod_add(&curve.b, &curve.p, ctx)
}

#[derive(PartialEq, Debug)]
enum LegendreSymbol {
    QuadResidue,
    NonQuadResidue,
    ZeroCongruent,
}

/// Computes (a | p), as defined by https://en.wikipedia.org/wiki/Legendre_symbol.
fn legendre(a: &Bignum, p: &Bignum, ctx: &BignumCtx) -> Result<LegendreSymbol, Error> {
    let exp = p.sub(Bignum::one()?)?.rshift1()?;
    let res = a.mod_exp(&exp, p, ctx)?;
    if res.is_one() {
        Ok(LegendreSymbol::QuadResidue)
    } else if res.is_zero() {
        Ok(LegendreSymbol::ZeroCongruent)
    } else {
        Ok(LegendreSymbol::NonQuadResidue)
    }
}

/// Returns a random tuple of (quadratic residue, non quadratic residue) mod p.
fn generate_qr_and_qnr(p: &Bignum, ctx: &BignumCtx) -> Result<(Bignum, Bignum), Error> {
    // Randomly selected values have a roughly 50% chance of being quadratic residues or
    // non-residues, so both of these loops will always terminate quickly.
    let mut qr = Bignum::rand(p)?;
    while legendre(&qr, p, ctx)? != LegendreSymbol::QuadResidue {
        qr = Bignum::rand(p)?;
    }

    let mut qnr = Bignum::rand(p)?;
    while legendre(&qnr, p, ctx)? != LegendreSymbol::NonQuadResidue {
        qnr = Bignum::rand(p)?;
    }

    Ok((qr, qnr))
}

/// IEEE 802.11-2016 12.4.4.2.2
/// Uses the given quadratic residue and non-quadratic residue to determine whether the given value
/// is also a residue, without leaving potential timing attacks.
fn is_quadratic_residue_blind(
    v: &Bignum,
    p: &Bignum,
    qr: &Bignum,
    qnr: &Bignum,
    ctx: &BignumCtx,
) -> Result<bool, Error> {
    // r = (random() mod (p - 1)) + 1
    let r = Bignum::rand(&p.sub(Bignum::one()?)?)?.add(Bignum::one()?)?;
    let num = v.mod_mul(&r, p, ctx)?.mod_mul(&r, p, ctx)?;
    if num.is_odd() {
        let num = num.mod_mul(qr, p, ctx)?;
        Ok(legendre(&num, p, ctx)? == LegendreSymbol::QuadResidue)
    } else {
        let num = num.mod_mul(qnr, p, ctx)?;
        Ok(legendre(&num, p, ctx)? == LegendreSymbol::NonQuadResidue)
    }
}

impl Group {
    // IEEE Std 802.11-2020 12.4.4.2.2
    fn generate_pwe_loop(&self, params: &SaeParameters) -> Result<EcPoint, Error> {
        if params.password_id.is_some() {
            // IEEE Std 802.11-2020 12.4.4.3.2
            bail!("Password ID cannot be used with looping PWE generation");
        }

        let group_params = self.group.get_params(&self.bn_ctx)?;
        let length = group_params.p.bits();
        let p_vec = group_params.p.to_be_vec(group_params.p.len());
        let (qr, qnr) = generate_qr_and_qnr(&group_params.p, &self.bn_ctx)?;
        // Our loop will set these two values.
        let mut x: Option<Bignum> = None;
        let mut save: Option<Vec<u8>> = None;

        let mut counter = 1;
        while counter <= MIN_PWE_ITER || x.is_none() {
            let pwd_seed = {
                let salt = concat_mac_addrs(&params.sta_a_mac, &params.sta_b_mac);
                let mut ikm = params.password.clone();
                ikm.push(counter as u8);
                params.hmac.hkdf_extract(&salt[..], &ikm[..])
            };
            let pwd_value =
                params.hmac.kdf_hash_length(&pwd_seed[..], KDF_LABEL, &p_vec[..], length);
            // This is a candidate value for our PWE x-coord. We now determine whether or not it
            // has all of our desired properties to form a PWE.
            let pwd_value = Bignum::new_from_slice(&pwd_value[..])?;
            if pwd_value < group_params.p {
                let y_squared = compute_y_squared(&pwd_value, &group_params, &self.bn_ctx)?;
                if is_quadratic_residue_blind(&y_squared, &group_params.p, &qr, &qnr, &self.bn_ctx)?
                {
                    // We have a valid x coord for our PWE! Save it if it's the first we've found.
                    if x.is_none() {
                        x = Some(pwd_value);
                        save = Some(pwd_seed);
                    }
                }
            }
            counter += 1;
        }

        // x and save are now guaranteed to contain values.
        let x = x.unwrap();
        let save = save.unwrap();

        // Finally compute the PWE.
        let y_squared = compute_y_squared(&x, &group_params, &self.bn_ctx)?;
        let mut y = y_squared.mod_sqrt(&group_params.p, &self.bn_ctx)?;
        // Use (p - y) if the LSB of save is not equal to the LSB of y.
        if save[save.len() - 1].is_odd() != y.is_odd() {
            y = group_params.p.copy()?.sub(y)?;
        }
        EcPoint::new_from_affine_coords(x, y, &self.group, &self.bn_ctx)
    }

    /// IEEE Std 802.11-2020 12.4.4.2.3
    /// Returns the secret PT used to generate the PWE.
    fn generate_pt(&self, params: &SaeParameters) -> Result<EcPoint, Error> {
        let mut password_with_id = params.password.clone();
        if let Some(password_id) = &params.password_id {
            password_with_id.extend_from_slice(password_id);
        }
        EcPoint::new_wpa3_sae_hash_to_curve_p256(&self.group, &params.ssid, &password_with_id)
    }

    // IEEE Std 802.11-2020 12.4.5.2
    fn generate_pwe_direct(&self, params: &SaeParameters) -> Result<EcPoint, Error> {
        // The secret element pt is used for each connection on this SSID and password, and could
        // potentially be cached and reused.  However, in actual use we generate a new Group
        // instance for each call to generate_pwe(), so we do not do any caching here.
        let pt = self.generate_pt(params)?;

        // Now generate the PWE from the PT and MAC addresses.
        let salt = vec![0u8; params.hmac.bits() / 8];
        let ikm = concat_mac_addrs(&params.sta_a_mac, &params.sta_b_mac);
        let val = Bignum::new_from_slice(&params.hmac.hkdf_extract(&salt, &ikm))?;
        let val = val
            .mod_nonnegative(
                &self.group.get_order(&self.bn_ctx)?.sub(Bignum::one()?)?,
                &self.bn_ctx,
            )?
            .add(Bignum::one()?)?;

        self.scalar_op(&val, &pt)
    }
}

impl FiniteCyclicGroup for Group {
    type Element = boringssl::EcPoint;

    fn group_id(&self) -> u16 {
        self.id.to_u16().unwrap()
    }

    fn generate_pwe(&self, params: &SaeParameters) -> Result<Self::Element, Error> {
        match params.pwe_method {
            PweMethod::Loop => self.generate_pwe_loop(params),
            PweMethod::Direct => self.generate_pwe_direct(params),
        }
    }

    fn scalar_op(&self, scalar: &Bignum, element: &Self::Element) -> Result<Self::Element, Error> {
        element.mul(&self.group, &scalar, &self.bn_ctx)
    }

    fn elem_op(
        &self,
        element1: &Self::Element,
        element2: &Self::Element,
    ) -> Result<Self::Element, Error> {
        element1.add(&self.group, &element2, &self.bn_ctx)
    }

    fn inverse_op(&self, element: Self::Element) -> Result<Self::Element, Error> {
        element.invert(&self.group, &self.bn_ctx)
    }

    fn order(&self) -> Result<Bignum, Error> {
        self.group.get_order(&self.bn_ctx)
    }

    fn generator(&self) -> Result<EcPoint, Error> {
        self.group.get_generator()
    }

    fn map_to_secret_value(&self, element: &Self::Element) -> Result<Option<Vec<u8>>, Error> {
        // IEEE Std 802.11-2016 12.4.4.2.1 (end of section)
        if element.is_point_at_infinity(&self.group) {
            Ok(None)
        } else {
            let group_params = self.group.get_params(&self.bn_ctx)?;
            let (x, _y) = element.to_affine_coords(&self.group, &self.bn_ctx)?;
            Ok(Some(x.to_be_vec(group_params.p.len())))
        }
    }

    // IEEE Std 802.11-2016 12.4.7.2.4
    fn element_to_octets(&self, element: &Self::Element) -> Result<Vec<u8>, Error> {
        let group_params = self.group.get_params(&self.bn_ctx)?;
        let length = group_params.p.len();
        let (x, y) = element.to_affine_coords(&self.group, &self.bn_ctx)?;
        let mut res = x.to_be_vec(length);
        res.append(&mut y.to_be_vec(length));
        Ok(res)
    }

    fn element_to_octets_compact(&self, element: &Self::Element) -> Result<Vec<u8>, Error> {
        let group_params = self.group.get_params(&self.bn_ctx)?;
        let length = group_params.p.len();
        let x = element.to_affine_coords_x(&self.group, &self.bn_ctx)?;
        let res = x.to_be_vec(length);
        Ok(res)
    }

    // IEEE Std 802.11-2016 12.4.7.2.5
    fn element_from_octets(&self, octets: &[u8]) -> Result<Option<Self::Element>, Error> {
        let group_params = self.group.get_params(&self.bn_ctx)?;
        let length = group_params.p.len();
        if octets.len() != length * 2 {
            warn!("element_from_octets called with wrong number of octets");
            return Ok(None);
        }
        let x = Bignum::new_from_slice(&octets[0..length])?;
        let y = Bignum::new_from_slice(&octets[length..])?;
        Ok(EcPoint::new_from_affine_coords(x, y, &self.group, &self.bn_ctx).ok())
    }

    fn element_from_octets_compact(&self, octets: &[u8]) -> Result<Option<Self::Element>, Error> {
        let group_params = self.group.get_params(&self.bn_ctx)?;
        let length = group_params.p.len();
        if octets.len() != length {
            warn!("element_from_octets_compact called with wrong number of octets");
            return Ok(None);
        }
        let x = Bignum::new_from_slice(&octets)?;
        Ok(EcPoint::new_from_compressed_coords(x, &self.group, &self.bn_ctx).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hmac_utils::HmacUtilsImpl;
    use ieee80211::{MacAddr, Ssid};
    use mundane::hash::Sha256;
    use std::convert::TryFrom;
    use std::sync::LazyLock;

    // IEEE Std 802.11-2020 J.10
    // SAE test vectors (common)
    const TEST_GROUP: EcGroupId = EcGroupId::P256;
    const TEST_SSID: &'static str = "byteme";
    const TEST_PWD: &'static str = "mekmitasdigoat";
    const TEST_PWD_ID: &'static str = "psk4internet";

    // IEEE Std 802.11-2020 J.10
    // Test vectors for looping PWE generation
    static TEST_LOOP_STA_A: LazyLock<MacAddr> =
        LazyLock::new(|| MacAddr::from([0x4d, 0x3f, 0x2f, 0xff, 0xe3, 0x87]));
    static TEST_LOOP_STA_B: LazyLock<MacAddr> =
        LazyLock::new(|| MacAddr::from([0xa5, 0xd8, 0xaa, 0x95, 0x8e, 0x3c]));
    const TEST_LOOP_PWE_X: &'static str =
        "da6eb7b06a1ac5624974f90afdd6a8e9d5722634cf987c34defc91a9874e5658";
    const TEST_LOOP_PWE_Y: &'static str =
        "f4fefd130bd5be08fe68af3e4a290272ec065fd3671f3c25bf8ec419ddc9b822";

    // IEEE Std 802.11-2020 J.10
    // Test vectors for direct PWE generation
    static TEST_DIRECT_STA_A: LazyLock<MacAddr> =
        LazyLock::new(|| MacAddr::from([0x00, 0x09, 0x5b, 0x66, 0xec, 0x1e]));
    static TEST_DIRECT_STA_B: LazyLock<MacAddr> =
        LazyLock::new(|| MacAddr::from([0x00, 0x0b, 0x6b, 0xd9, 0x02, 0x46]));

    const TEST_DIRECT_PT_X: &'static str =
        "b6e38c98750c684b5d17c3d8c9a4100b39931279187ca6cced5f37ef46ddfa97";
    const TEST_DIRECT_PT_Y: &'static str =
        "5687e972e50f73e3898861e7edad21bea7d5f622df88243bb804920ae8e647fa";
    const TEST_DIRECT_PWE_X: &'static str =
        "c93049b9e64000f848201649e999f2b5c22dea69b5632c9df4d633b8aa1f6c1e";
    const TEST_DIRECT_PWE_Y: &'static str =
        "73634e94b53d82e7383a8d258199d9dc1a5ee8269d060382ccbf33e614ff59a0";

    fn make_group() -> Group {
        let group = boringssl::EcGroup::new(TEST_GROUP).unwrap();
        let bn_ctx = boringssl::BignumCtx::new().unwrap();
        Group { id: TEST_GROUP, group, bn_ctx }
    }

    fn bn(value: u64) -> Bignum {
        Bignum::new_from_u64(value).unwrap()
    }

    #[test]
    fn get_group_id() {
        let group = make_group();
        assert_eq!(group.group_id(), TEST_GROUP.to_u16().unwrap());
    }

    #[test]
    fn generate_pwe_loop() {
        let group = make_group();
        let group_params = group.group.get_params(&group.bn_ctx).unwrap();
        let params = SaeParameters {
            hmac: Box::new(HmacUtilsImpl::<Sha256>::new()),
            pwe_method: PweMethod::Loop,
            ssid: Ssid::try_from(TEST_SSID).unwrap(),
            password: Vec::from(TEST_PWD),
            password_id: None,
            sta_a_mac: *TEST_LOOP_STA_A,
            sta_b_mac: *TEST_LOOP_STA_B,
        };
        let pwe = group.generate_pwe(&params).unwrap();
        let (x, y) = pwe.to_affine_coords(&group.group, &group.bn_ctx).unwrap();
        assert_eq!(x.to_be_vec(group_params.p.len()), hex::decode(TEST_LOOP_PWE_X).unwrap());
        assert_eq!(y.to_be_vec(group_params.p.len()), hex::decode(TEST_LOOP_PWE_Y).unwrap());

        // The PWE should not change depending on the order of mac addresses.
        let params =
            SaeParameters { sta_a_mac: *TEST_LOOP_STA_B, sta_b_mac: *TEST_LOOP_STA_A, ..params };
        let pwe = group.generate_pwe(&params).unwrap();
        let (x, y) = pwe.to_affine_coords(&group.group, &group.bn_ctx).unwrap();
        assert_eq!(x.to_be_vec(group_params.p.len()), hex::decode(TEST_LOOP_PWE_X).unwrap());
        assert_eq!(y.to_be_vec(group_params.p.len()), hex::decode(TEST_LOOP_PWE_Y).unwrap());
    }

    #[test]
    fn generate_pwe_direct() {
        let group = make_group();
        let group_params = group.group.get_params(&group.bn_ctx).unwrap();
        let params = SaeParameters {
            hmac: Box::new(HmacUtilsImpl::<Sha256>::new()),
            pwe_method: PweMethod::Direct,
            ssid: Ssid::try_from(TEST_SSID).unwrap(),
            password: Vec::from(TEST_PWD),
            password_id: Some(Vec::from(TEST_PWD_ID)),
            sta_a_mac: *TEST_DIRECT_STA_A,
            sta_b_mac: *TEST_DIRECT_STA_B,
        };
        let pwe = group.generate_pwe(&params).unwrap();
        let (x, y) = pwe.to_affine_coords(&group.group, &group.bn_ctx).unwrap();
        assert_eq!(x.to_be_vec(group_params.p.len()), hex::decode(TEST_DIRECT_PWE_X).unwrap());
        assert_eq!(y.to_be_vec(group_params.p.len()), hex::decode(TEST_DIRECT_PWE_Y).unwrap());

        // The PWE should not change depending on the order of mac addresses.
        let params = SaeParameters {
            sta_a_mac: *TEST_DIRECT_STA_B,
            sta_b_mac: *TEST_DIRECT_STA_A,
            ..params
        };
        let pwe = group.generate_pwe(&params).unwrap();
        let (x, y) = pwe.to_affine_coords(&group.group, &group.bn_ctx).unwrap();
        assert_eq!(x.to_be_vec(group_params.p.len()), hex::decode(TEST_DIRECT_PWE_X).unwrap());
        assert_eq!(y.to_be_vec(group_params.p.len()), hex::decode(TEST_DIRECT_PWE_Y).unwrap());
    }

    #[test]
    fn generate_pwe_loop_no_pwd_id() {
        let group = make_group();
        let params = SaeParameters {
            hmac: Box::new(HmacUtilsImpl::<Sha256>::new()),
            pwe_method: PweMethod::Loop,
            ssid: Ssid::try_from(TEST_SSID).unwrap(),
            password: Vec::from(TEST_PWD),
            password_id: Some(Vec::from(TEST_PWD_ID)),
            sta_a_mac: *TEST_LOOP_STA_A,
            sta_b_mac: *TEST_LOOP_STA_B,
        };
        let pwe = group.generate_pwe(&params);
        // IEEE Std 802.11-2020: password ID cannot be used with PWE generation by looping
        assert!(pwe.is_err());
    }

    #[test]
    fn test_legendre() {
        // Test cases from the table in https://en.wikipedia.org/wiki/Legendre_symbol
        let ctx = BignumCtx::new().unwrap();
        assert_eq!(legendre(&bn(13), &bn(23), &ctx).unwrap(), LegendreSymbol::QuadResidue);
        assert_eq!(legendre(&bn(19), &bn(23), &ctx).unwrap(), LegendreSymbol::NonQuadResidue);
        assert_eq!(legendre(&bn(26), &bn(13), &ctx).unwrap(), LegendreSymbol::ZeroCongruent);
    }

    #[test]
    fn generate_qr_qnr() {
        // With prime 3, the only possible qr is 1 and qnr is 2.
        let ctx = BignumCtx::new().unwrap();
        let (qr, qnr) = generate_qr_and_qnr(&bn(3), &ctx).unwrap();
        assert_eq!(qr, bn(1));
        assert_eq!(qnr, bn(2));
    }

    #[test]
    fn quadratic_residue_blind() {
        // Test cases from the table in https://en.wikipedia.org/wiki/Legendre_symbol
        let qr_table = [
            false, true, false, false, true, false, true, false, false, true, true, false, false,
            false, true, true, true, true, false, true, false, true, true, true, true, true, true,
            false, false, true, false,
        ];
        let prime = bn(67);
        let ctx = BignumCtx::new().unwrap();
        let (qr, qnr) = generate_qr_and_qnr(&prime, &ctx).unwrap();
        qr_table.iter().enumerate().for_each(|(i, _is_residue)| {
            assert_eq!(
                qr_table[i],
                is_quadratic_residue_blind(&bn(i as u64), &prime, &qr, &qnr, &ctx).unwrap()
            )
        });
    }

    #[test]
    fn generate_pt() {
        let group = make_group();
        let params = SaeParameters {
            hmac: Box::new(HmacUtilsImpl::<Sha256>::new()),
            pwe_method: PweMethod::Direct,
            ssid: Ssid::try_from(TEST_SSID).unwrap(),
            password: Vec::from(TEST_PWD),
            password_id: Some(Vec::from(TEST_PWD_ID)),
            sta_a_mac: *TEST_DIRECT_STA_A,
            sta_b_mac: *TEST_DIRECT_STA_B,
        };

        let pt = group.generate_pt(&params).unwrap();
        let (pt_x, pt_y) = pt.to_affine_coords(&group.group, &group.bn_ctx).unwrap();
        assert_eq!(hex::encode(pt_x.to_be_vec(0)), TEST_DIRECT_PT_X);
        assert_eq!(hex::encode(pt_y.to_be_vec(0)), TEST_DIRECT_PT_Y);
    }

    #[test]
    fn test_element_to_octets() {
        let x = Bignum::new_from_slice(&hex::decode(TEST_LOOP_PWE_X).unwrap()).unwrap();
        let y = Bignum::new_from_slice(&hex::decode(TEST_LOOP_PWE_Y).unwrap()).unwrap();
        let group = make_group();
        let element = EcPoint::new_from_affine_coords(x, y, &group.group, &group.bn_ctx).unwrap();

        let octets = group.element_to_octets(&element).unwrap();
        let mut expected = hex::decode(TEST_LOOP_PWE_X).unwrap();
        expected.extend_from_slice(&hex::decode(TEST_LOOP_PWE_Y).unwrap());
        assert_eq!(octets, expected);
    }

    #[test]
    fn test_element_to_octets_padding() {
        let group = make_group();
        let params = group.group.get_params(&group.bn_ctx).unwrap();
        // We compute a point on the curve with a short x coordinate -- the
        // generated octets should still be 64 in length, zero padded.
        let x = bn(0xffffffff);
        let y = compute_y_squared(&x, &params, &group.bn_ctx)
            .unwrap()
            .mod_sqrt(&params.p, &group.bn_ctx)
            .unwrap();
        let element = EcPoint::new_from_affine_coords(x, y, &group.group, &group.bn_ctx).unwrap();

        let octets = group.element_to_octets(&element).unwrap();
        let mut expected_x = vec![0x00; 28];
        expected_x.extend_from_slice(&[0xff; 4]);
        assert_eq!(octets.len(), 64);
        assert_eq!(&octets[0..32], &expected_x[0..32]);
    }

    #[test]
    fn test_element_from_octets() {
        let mut octets = hex::decode(TEST_LOOP_PWE_X).unwrap();
        octets.extend_from_slice(&hex::decode(TEST_LOOP_PWE_Y).unwrap());
        let group = make_group();
        let element = group.element_from_octets(&octets).unwrap();
        assert!(element.is_some());
        let element = element.unwrap();

        let expected_x = Bignum::new_from_slice(&hex::decode(TEST_LOOP_PWE_X).unwrap()).unwrap();
        let expected_y = Bignum::new_from_slice(&hex::decode(TEST_LOOP_PWE_Y).unwrap()).unwrap();
        let (x, y) = element.to_affine_coords(&group.group, &group.bn_ctx).unwrap();

        assert_eq!(x, expected_x);
        assert_eq!(y, expected_y);
    }

    #[test]
    fn test_element_from_octets_padded() {
        let mut octets = hex::decode(TEST_LOOP_PWE_X).unwrap();
        octets.extend_from_slice(&hex::decode(TEST_LOOP_PWE_Y).unwrap());
        octets.extend_from_slice(&[0xff; 10]);
        let group = make_group();
        let element = group.element_from_octets(&octets).unwrap();
        assert!(element.is_none());
    }

    #[test]
    fn test_element_from_octets_truncated() {
        let mut octets = hex::decode(TEST_LOOP_PWE_X).unwrap();
        octets.extend_from_slice(&hex::decode(TEST_LOOP_PWE_Y).unwrap());
        octets.truncate(octets.len() - 10);
        let group = make_group();
        let element = group.element_from_octets(&octets).unwrap();
        assert!(element.is_none());
    }

    #[test]
    fn test_element_from_octets_bad_point() {
        let mut octets = hex::decode(TEST_LOOP_PWE_X).unwrap();
        octets.extend_from_slice(&hex::decode(TEST_LOOP_PWE_Y).unwrap());
        let idx = octets.len() - 1;
        octets[idx] += 1; // This is no longer the right Y value for this X.
        let group = make_group();
        let element = group.element_from_octets(&octets).unwrap();
        assert!(element.is_none());
    }
}
