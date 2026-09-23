// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::{Ref, RefCell, RefMut};
use std::cmp::min;
use std::collections::btree_map::Entry as BTreeMapEntry;
use std::collections::hash_map::Entry as HashMapEntry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::{Bound, Deref, DerefMut};
use std::rc::Rc;

use num_traits::FromPrimitive as _;
use p256::elliptic_curve::common::Generate as _;
use p256::elliptic_curve::sec1::ToSec1Point as _;
use rsa::traits::{PrivateKeyParts as _, PublicKeyParts as _};
use rsa::{BigUint, RsaPrivateKey};
use tee_internal::{
    Attribute, AttributeId, BufferOrValue, DATA_MAX_POSITION, EccCurve, Error, HandleFlags, MemRef,
    OBJECT_ID_MAX_LEN, ObjectEnumHandle, ObjectHandle, ObjectInfo, Result as TeeResult,
    Storage as TeeStorage, Type, Usage, ValueFields, Whence,
};
use thiserror::Error;

use crate::ErrorWithSize;
use crate::crypto::Rng;

type P256SecretKey = p256::SecretKey;

pub struct Storage {
    persistent_objects: PersistentObjects,
    transient_objects: TransientObjects,
}

//
// We establish the private convention that all persistent object handles are
// odd in value, while all transient object handles are even.
//

fn is_persistent_handle(object: ObjectHandle) -> bool {
    *object % 2 == 1
}

// We define the inverse of is_persistent_handle() for readability at callsites
// where we want to more directly check for transience.
fn is_transient_handle(object: ObjectHandle) -> bool {
    !is_persistent_handle(object)
}

//
// Key type definitions.
//
// See Table 5-9: TEE_AllocateTransientObject Object Types and Key Sizes 4.
//

// A representation of a retrieved buffer attribute. Ideally, we'd just use
// Option<&[u8]>, but some of the APIs we're gluing here like to return vector
// representations.
pub enum BufferAttribute<'a> {
    Slice(&'a [u8]),
    Vector(Vec<u8>),
}

// A internal trait representing the common key operations.
//
// TODO(https://fxbug.dev/371213067): Right now it's convenient to give default
// implementations to generate "unimplemented" stubs for the various key types
// we don't yet support with minimal boilerplate. When more is
// supported/implemented, we can remove these defaults.
pub trait KeyType {
    fn new(_max_size: u32) -> TeeResult<Self>
    where
        Self: Sized, // Not a subtrait to keep KeyType dyn-compatible
    {
        unimplemented!()
    }

    fn is_valid_size(_size: u32) -> bool
    where
        Self: Sized, // Not a subtrait to keep KeyType dyn-compatible
    {
        false
    }

    fn size(&self) -> u32 {
        0
    }

    fn max_size(&self) -> u32 {
        0
    }

    fn buffer_attribute(&self, _id: AttributeId) -> Option<BufferAttribute<'_>> {
        None
    }

    fn value_attribute(&self, _id: AttributeId) -> Option<ValueFields> {
        None
    }

    fn reset(&mut self) {
        unimplemented!()
    }

    fn populate(&mut self, _attributes: &[Attribute]) -> TeeResult {
        unimplemented!()
    }

    fn generate(&mut self, _size: u32, _params: &[Attribute]) -> TeeResult {
        unimplemented!()
    }
}

// An error type in service of extract_attributes!() below.
#[derive(Error, Debug)]
enum ExtractAttributeError {
    #[error("{0:?} provided twice")]
    ProvidedTwice(AttributeId),

    #[error("Unexpected attribute: {0:?}")]
    Unexpected(AttributeId),
}

// This trait exists only to aid in the definition of extract_attributes!
// just below, injecting a dose of generics to support both the memory and
// value attribute cases, which seems hard to do with macro tricks alone.
trait ExtractAttributeInto<T> {
    fn extract_into(self, value: &mut T) -> Result<(), ExtractAttributeError>;
}

impl<'a> ExtractAttributeInto<&'a [u8]> for &'a Attribute {
    fn extract_into(self, value: &mut &'a [u8]) -> Result<(), ExtractAttributeError> {
        if !value.is_empty() {
            Err(ExtractAttributeError::ProvidedTwice(self.id))
        } else {
            *value = self.as_memory_reference().as_slice();
            Ok(())
        }
    }
}

impl ExtractAttributeInto<Option<ValueFields>> for &Attribute {
    fn extract_into(self, value: &mut Option<ValueFields>) -> Result<(), ExtractAttributeError> {
        if value.is_some() {
            Err(ExtractAttributeError::ProvidedTwice(self.id))
        } else {
            *value = Some(*self.as_value());
            Ok(())
        }
    }
}

// Extracts the expected attributes from a list, returning
// Result<(), ExtractAttributeError>, ensuring that only expected ones are
// present and nothing expected is duplicated.
//
// Example usage:
// ```
// let mut mem_attr: &[u8] = &[];
// let mut val_attr: Option<ValueFields> = None;
// ...
// extract_attributes!(
//     attrs,
//     AttributeId::A => mem_attr, // If present, will set mem_attr as the A payload
//     AttributeId::B => val_attr, // If present, will val_attr as the B payload
//     ...
// ).unwrap();
// ```
macro_rules! extract_attributes {
    ($attributes:expr, $($id:path => $var:ident),*) => {
        || -> Result<(), ExtractAttributeError> {
            for attr in $attributes {
                match attr.id {
                    $( $id => attr.extract_into(&mut $var)?, )*
                    _ => return Err(ExtractAttributeError::Unexpected(attr.id)),
                }
            }
            Ok(())
        }()
    };
}

#[derive(Clone)]
pub struct SimpleSymmetricKey<const SIZE_MIN: u32, const SIZE_MAX: u32, const SIZE_MULTIPLE: u32> {
    pub secret: Vec<u8>, // TEE_ATTR_SECRET_VALUE
}

impl<const SIZE_MIN: u32, const SIZE_MAX: u32, const SIZE_MULTIPLE: u32> KeyType
    for SimpleSymmetricKey<SIZE_MIN, SIZE_MAX, SIZE_MULTIPLE>
{
    fn new(max_size: u32) -> TeeResult<Self> {
        // Would that we could make these static asserts directly in the
        // definition of the type, or out-of-line next to it.
        const { assert!((SIZE_MIN % SIZE_MULTIPLE) == 0) };
        const { assert!((SIZE_MAX % SIZE_MULTIPLE) == 0) };

        if Self::is_valid_size(max_size) {
            Ok(Self { secret: Vec::with_capacity((max_size / u8::BITS) as usize) })
        } else {
            Err(Error::NotSupported)
        }
    }

    fn is_valid_size(size: u32) -> bool {
        size >= SIZE_MIN && size <= SIZE_MAX && (size % SIZE_MULTIPLE) == 0
    }

    fn size(&self) -> u32 {
        (self.secret.len() as u32) * u8::BITS
    }

    fn max_size(&self) -> u32 {
        (self.secret.capacity() as u32) * u8::BITS
    }

    fn buffer_attribute(&self, id: AttributeId) -> Option<BufferAttribute<'_>> {
        if id == AttributeId::SecretValue {
            Some(BufferAttribute::Slice(&self.secret))
        } else {
            None
        }
    }

    fn reset(&mut self) {
        self.secret.clear();
    }

    fn populate(&mut self, attributes: &[Attribute]) -> TeeResult {
        debug_assert!(self.secret.is_empty());

        let mut secret: &[u8] = &[];
        extract_attributes!(
            attributes,
            AttributeId::SecretValue => secret
        )
        .unwrap();
        assert!(!secret.is_empty(), "Missing attribute for secret value");

        assert!(secret.len() <= self.secret.capacity());
        self.secret.extend_from_slice(secret);
        Ok(())
    }

    fn generate(&mut self, size: u32, params: &[Attribute]) -> TeeResult {
        assert!(Self::is_valid_size(size));
        assert!(size <= self.max_size());
        if !params.is_empty() {
            return Err(Error::BadParameters);
        }
        self.secret.resize((size / u8::BITS) as usize, 0);
        zx::cprng_draw(self.secret.as_mut_slice());
        Ok(())
    }
}

pub type AesKey = SimpleSymmetricKey<128, 256, 64>; // 128, 192, or 256
pub type HmacSha1Key = SimpleSymmetricKey<80, 512, 8>;
pub type HmacSha224Key = SimpleSymmetricKey<112, 512, 8>;
pub type HmacSha256Key = SimpleSymmetricKey<192, 1024, 8>;
pub type HmacSha384Key = SimpleSymmetricKey<256, 512, 8>;
pub type HmacSha512Key = SimpleSymmetricKey<256, 512, 8>;

pub struct RsaKeypair {
    private: Option<Rc<RsaPrivateKey>>,
    max_size: u32,
}

impl RsaKeypair {
    pub fn private_key(&self) -> Rc<RsaPrivateKey> {
        self.private.as_ref().unwrap().clone()
    }
}

// A deep clone implementation as we don't want to the lifetimes of copied keys
// to be tied to their originals.
impl Clone for RsaKeypair {
    fn clone(&self) -> Self {
        let max_size = self.max_size;
        if let Some(private) = &self.private {
            Self { private: Some(Rc::new(private.deref().clone())), max_size }
        } else {
            Self { private: None, max_size }
        }
    }
}

impl KeyType for RsaKeypair {
    fn new(max_size: u32) -> TeeResult<Self> {
        if !Self::is_valid_size(max_size) {
            return Err(Error::NotSupported);
        }
        Ok(Self { private: None, max_size })
    }

    fn is_valid_size(size: u32) -> bool {
        (size % u8::BITS) == 0 && 512 <= size && size <= 4096
    }

    fn size(&self) -> u32 {
        if let Some(private) = &self.private { (private.size() as u32) * u8::BITS } else { 0 }
    }

    fn max_size(&self) -> u32 {
        self.max_size
    }

    fn buffer_attribute(&self, id: AttributeId) -> Option<BufferAttribute<'_>> {
        let Some(private) = &self.private else {
            return None;
        };
        match id {
            AttributeId::RsaModulus => Some(BufferAttribute::Vector(private.n().to_bytes_be())),
            AttributeId::RsaPublicExponent => {
                Some(BufferAttribute::Vector(private.e().to_bytes_be()))
            }
            AttributeId::RsaPrivateExponent => {
                Some(BufferAttribute::Vector(private.d().to_bytes_be()))
            }
            AttributeId::RsaPrime1 => {
                Some(BufferAttribute::Vector(private.primes()[0].to_bytes_be()))
            }
            AttributeId::RsaPrime2 => {
                Some(BufferAttribute::Vector(private.primes()[1].to_bytes_be()))
            }
            AttributeId::RsaExponent1 => {
                Some(BufferAttribute::Vector(private.dp().unwrap().to_bytes_be()))
            }
            AttributeId::RsaExponent2 => {
                Some(BufferAttribute::Vector(private.dq().unwrap().to_bytes_be()))
            }
            AttributeId::RsaCoefficient => {
                Some(BufferAttribute::Vector(private.crt_coefficient().unwrap().to_bytes_be()))
            }
            _ => None,
        }
    }

    fn reset(&mut self) {
        self.private = None;
    }

    fn populate(&mut self, attributes: &[Attribute]) -> TeeResult {
        assert!(self.private.is_none());

        let mut modulus: &[u8] = &[];
        let mut public_exponent: &[u8] = &[];
        let mut private_exponent: &[u8] = &[];
        let mut prime1: &[u8] = &[];
        let mut prime2: &[u8] = &[];
        let mut exponent1: &[u8] = &[];
        let mut exponent2: &[u8] = &[];
        let mut coefficient: &[u8] = &[];
        extract_attributes!(
            attributes,
            AttributeId::RsaModulus => modulus,
            AttributeId::RsaPublicExponent => public_exponent,
            AttributeId::RsaPrivateExponent => private_exponent,
            AttributeId::RsaPrime1 => prime1,
            AttributeId::RsaPrime2 => prime2,
            AttributeId::RsaExponent1 => exponent1,
            AttributeId::RsaExponent2 => exponent2,
            AttributeId::RsaCoefficient => coefficient
        )
        .unwrap();
        assert!(!modulus.is_empty(), "Missing attribute for RSA modulus");
        assert!(!public_exponent.is_empty(), "Missing attribute for RSA public exponent");
        assert!(!private_exponent.is_empty(), "Missing attribute for RSA private exponent");

        if !prime1.is_empty()
            || prime2.is_empty()
            || !exponent1.is_empty()
            || !exponent2.is_empty()
            || !coefficient.is_empty()
        {
            assert!(
                !prime1.is_empty(),
                "TEE_ATTR_RSA_PRIME1 is required if another CRT attribute is provided"
            );
            assert!(
                !prime2.is_empty(),
                "TEE_ATTR_RSA_PRIME2 is required if another CRT attribute is provided"
            );
            assert!(
                !exponent1.is_empty(),
                "TEE_ATTR_RSA_EXPONENT1 is required if another CRT attribute is provided"
            );
            assert!(
                !exponent2.is_empty(),
                "TEE_ATTR_RSA_EXPONENT2 is required if another CRT attribute is provided"
            );
            assert!(
                !coefficient.is_empty(),
                "TEE_ATTR_RSA_COEFFICIENT is required if another CRT attribute is provided"
            );
        }

        let len: u32 = modulus.len().try_into().unwrap();
        assert!(u8::BITS * len <= self.max_size());

        let mut private_key = RsaPrivateKey::from_components(
            BigUint::from_bytes_be(modulus),
            BigUint::from_bytes_be(public_exponent),
            BigUint::from_bytes_be(private_exponent),
            vec![BigUint::from_bytes_be(prime1), BigUint::from_bytes_be(prime2)],
        )
        .map_err(|_| Error::BadParameters)?;

        // Computes and populates the CRT coefficients accessed below and
        // possibly in subsequent key use.
        private_key.precompute().unwrap();

        if !exponent1.is_empty() {
            if *private_key.dp().unwrap() != BigUint::from_bytes_be(exponent1) {
                return Err(Error::BadParameters);
            }
            if *private_key.dq().unwrap() != BigUint::from_bytes_be(exponent2) {
                return Err(Error::BadParameters);
            }
            if private_key.crt_coefficient().unwrap() != BigUint::from_bytes_be(coefficient) {
                return Err(Error::BadParameters);
            }
        }

        self.private = Some(Rc::new(private_key));
        Ok(())
    }

    fn generate(&mut self, size: u32, params: &[Attribute]) -> TeeResult {
        assert!(Self::is_valid_size(size));
        assert!(size <= self.max_size());
        assert!(self.private.is_none());

        let mut public_exponent: &[u8] = &[];
        extract_attributes!(params, AttributeId::RsaPublicExponent => public_exponent)
            .map_err(|_| Error::BadParameters)?;

        let mut private_key = if public_exponent.is_empty() {
            RsaPrivateKey::new(&mut Rng {}, size as usize)
        } else {
            RsaPrivateKey::new_with_exp(
                &mut Rng {},
                size as usize,
                &BigUint::from_bytes_be(public_exponent),
            )
        }
        .unwrap();

        // Computes and populates the CRT coefficients accessed below and
        // possibly in subsequent key use.
        private_key.precompute().unwrap();

        self.private = Some(Rc::new(private_key));

        Ok(())
    }
}

#[derive(Clone)]
pub struct EccKeypair {
    secret: Option<Box<P256SecretKey>>,
}

// Only NIST P-256 curves are supported at this time.
impl KeyType for EccKeypair {
    fn new(max_size: u32) -> TeeResult<Self> {
        if !Self::is_valid_size(max_size) {
            return Err(Error::NotSupported);
        }
        Ok(Self { secret: None })
    }

    fn is_valid_size(size: u32) -> bool {
        size == 256
    }

    fn size(&self) -> u32 {
        256
    }

    fn max_size(&self) -> u32 {
        256
    }

    fn buffer_attribute(&self, id: AttributeId) -> Option<BufferAttribute<'_>> {
        let Some(secret) = &self.secret else {
            return None;
        };
        match id {
            AttributeId::EccPrivateValue => {
                Some(BufferAttribute::Vector(secret.to_bytes().as_slice().to_vec()))
            }
            AttributeId::EccPublicValueX => Some(BufferAttribute::Vector(
                secret
                    .public_key()
                    .as_affine()
                    .to_sec1_point(/*compress=*/ false)
                    .x()
                    .unwrap()
                    .as_slice()
                    .to_vec(),
            )),
            AttributeId::EccPublicValueY => Some(BufferAttribute::Vector(
                secret
                    .public_key()
                    .as_affine()
                    .to_sec1_point(/*compress=*/ false)
                    .y()
                    .unwrap()
                    .as_slice()
                    .to_vec(),
            )),
            _ => None,
        }
    }

    fn value_attribute(&self, id: AttributeId) -> Option<ValueFields> {
        match id {
            AttributeId::EccCurve => Some(ValueFields { a: EccCurve::NistP256 as u32, b: 0 }),
            _ => None,
        }
    }

    fn reset(&mut self) {
        self.secret = None;
    }

    fn populate(&mut self, attributes: &[Attribute]) -> TeeResult {
        assert!(self.secret.is_none());

        let mut private_value: &[u8] = &[];
        let mut public_value_x: &[u8] = &[];
        let mut public_value_y: &[u8] = &[];
        let mut curve: Option<ValueFields> = None;
        extract_attributes!(
            attributes,
            AttributeId::EccPrivateValue => private_value,
            AttributeId::EccPublicValueX => public_value_x,
            AttributeId::EccPublicValueY => public_value_y,
            AttributeId::EccCurve => curve
        )
        .unwrap();
        assert!(!private_value.is_empty(), "Missing attribute for ECC private value");
        assert!(!public_value_x.is_empty(), "Missing attribute for ECC public value X");
        assert!(!public_value_y.is_empty(), "Missing attribute for ECC public value Y");
        assert!(curve.is_some(), "Missing attribute for ECC curve");

        let curve = EccCurve::from_u32(curve.unwrap().a).ok_or(Error::NotSupported)?;
        if curve != EccCurve::NistP256 {
            return Err(Error::NotSupported);
        }

        let secret = P256SecretKey::from_slice(private_value).unwrap();

        // Check that provided public parameters coincide with those computed
        // by the private key abstraction.
        let point = secret.public_key().as_affine().to_sec1_point(/*compress=*/ false);
        if point.x().unwrap().as_slice() != public_value_x {
            return Err(Error::BadParameters);
        }
        if point.y().unwrap().as_slice() != public_value_y {
            return Err(Error::BadParameters);
        }
        self.secret = Some(Box::new(secret));
        Ok(())
    }

    fn generate(&mut self, size: u32, params: &[Attribute]) -> TeeResult {
        assert!(Self::is_valid_size(size));
        assert!(size <= self.max_size());
        assert!(self.secret.is_none());

        let mut curve: Option<ValueFields> = None;
        extract_attributes!(params, AttributeId::EccCurve => curve)
            .map_err(|_| Error::BadParameters)?;
        assert!(curve.is_some(), "Missing attribute for ECC curve");

        let curve = EccCurve::from_u32(curve.unwrap().a).ok_or(Error::NotSupported)?;
        if curve != EccCurve::NistP256 {
            return Err(Error::NotSupported);
        }

        self.secret = Some(Box::new(P256SecretKey::generate_from_rng(&mut Rng {})));
        Ok(())
    }
}

#[derive(Clone)]
pub struct NoKey {}

impl KeyType for NoKey {
    fn new(max_size: u32) -> TeeResult<Self> {
        if max_size == 0 { Ok(Self {}) } else { Err(Error::NotSupported) }
    }

    fn is_valid_size(size: u32) -> bool {
        size == 0
    }

    fn reset(&mut self) {}

    fn populate(&mut self, attributes: &[Attribute]) -> TeeResult {
        assert!(attributes.is_empty());
        Ok(())
    }

    fn generate(&mut self, size: u32, params: &[Attribute]) -> TeeResult {
        assert_eq!(size, 0);
        assert!(params.is_empty());
        Ok(())
    }
}

/// A cryptographic key (or key pair).
#[derive(Clone)]
pub enum Key {
    Aes(AesKey),
    HmacSha1(HmacSha1Key),
    HmacSha224(HmacSha224Key),
    HmacSha256(HmacSha256Key),
    HmacSha384(HmacSha384Key),
    HmacSha512(HmacSha512Key),
    RsaKeypair(RsaKeypair),
    EcdsaKeypair(EccKeypair),
    EcdhKeypair(EccKeypair),
    Data(NoKey),
}

// Reduces boilerplate a little.
macro_rules! get_key_variant {
    ($key:ident) => {
        match $key {
            Key::Aes(key) => key,
            Key::HmacSha1(key) => key,
            Key::HmacSha224(key) => key,
            Key::HmacSha256(key) => key,
            Key::HmacSha384(key) => key,
            Key::HmacSha512(key) => key,
            Key::RsaKeypair(key) => key,
            Key::EcdsaKeypair(key) => key,
            Key::EcdhKeypair(key) => key,
            Key::Data(key) => key,
        }
    };
}

impl Key {
    pub fn new(type_: Type, max_size: u32) -> TeeResult<Key> {
        match type_ {
            Type::Aes => AesKey::new(max_size).map(Self::Aes),
            Type::HmacSha1 => HmacSha1Key::new(max_size).map(Self::HmacSha1),
            Type::HmacSha224 => HmacSha224Key::new(max_size).map(Self::HmacSha224),
            Type::HmacSha256 => HmacSha256Key::new(max_size).map(Self::HmacSha256),
            Type::HmacSha384 => HmacSha384Key::new(max_size).map(Self::HmacSha384),
            Type::HmacSha512 => HmacSha512Key::new(max_size).map(Self::HmacSha512),
            Type::RsaKeypair => RsaKeypair::new(max_size).map(Self::RsaKeypair),
            Type::EcdsaKeypair => EccKeypair::new(max_size).map(Self::EcdsaKeypair),
            Type::EcdhKeypair => EccKeypair::new(max_size).map(Self::EcdhKeypair),
            Type::Data => NoKey::new(max_size).map(Self::Data),
            _ => Err(Error::NotSupported),
        }
    }

    pub fn get_type(&self) -> Type {
        match self {
            Key::Aes(_) => Type::Aes,
            Key::HmacSha1(_) => Type::HmacSha1,
            Key::HmacSha224(_) => Type::HmacSha224,
            Key::HmacSha256(_) => Type::HmacSha256,
            Key::HmacSha384(_) => Type::HmacSha384,
            Key::HmacSha512(_) => Type::HmacSha512,
            Key::RsaKeypair(_) => Type::RsaKeypair,
            Key::EcdsaKeypair(_) => Type::EcdsaKeypair,
            Key::EcdhKeypair(_) => Type::EcdhKeypair,
            Key::Data(_) => Type::Data,
        }
    }
}

impl Deref for Key {
    type Target = dyn KeyType;

    fn deref(&self) -> &Self::Target {
        get_key_variant!(self)
    }
}

impl DerefMut for Key {
    fn deref_mut(&mut self) -> &mut Self::Target {
        get_key_variant!(self)
    }
}

// The common object abstraction implemented by transient and persistent
// storage objects.
pub trait Object {
    fn key(&self) -> &Key;

    fn usage(&self) -> &Usage;
    fn usage_mut(&mut self) -> &mut Usage;

    fn flags(&self) -> &HandleFlags;

    fn restrict_usage(&mut self, restriction: Usage) {
        let usage = self.usage_mut();
        *usage = usage.intersection(restriction)
    }

    fn get_info(&self, data_size: usize, data_position: usize) -> ObjectInfo {
        let all_info_flags = HandleFlags::PERSISTENT
            | HandleFlags::INITIALIZED
            | HandleFlags::DATA_ACCESS_READ
            | HandleFlags::DATA_ACCESS_WRITE
            | HandleFlags::DATA_ACCESS_WRITE_META
            | HandleFlags::DATA_SHARE_READ
            | HandleFlags::DATA_SHARE_WRITE;
        let flags = self.flags().intersection(all_info_flags);
        let key_size = self.key().size();
        let object_size = if key_size > 0 { key_size } else { data_size.try_into().unwrap() };
        ObjectInfo {
            object_type: self.key().get_type(),
            max_object_size: self.key().max_size(),
            object_size,
            object_usage: *self.usage(),
            data_position: data_position,
            data_size: data_size,
            handle_flags: flags,
        }
    }
}

struct TransientObject {
    key: Key,
    usage: Usage,
    flags: HandleFlags,
}

impl TransientObject {
    fn new(key: Key) -> Self {
        TransientObject { key, usage: Usage::default(), flags: HandleFlags::empty() }
    }
}

impl Object for TransientObject {
    fn key(&self) -> &Key {
        &self.key
    }

    fn usage(&self) -> &Usage {
        &self.usage
    }
    fn usage_mut(&mut self) -> &mut Usage {
        &mut self.usage
    }

    fn flags(&self) -> &HandleFlags {
        &self.flags
    }
}

// A class abstraction implementing the transient storage interface.
struct TransientObjects {
    by_handle: HashMap<ObjectHandle, Rc<RefCell<TransientObject>>>,
    next_handle_value: u64,
}

impl TransientObjects {
    fn new() -> Self {
        Self {
            by_handle: HashMap::new(),
            // Always even, per the described convention above - also non-null.
            next_handle_value: 2,
        }
    }

    fn allocate(&mut self, type_: Type, max_size: u32) -> TeeResult<ObjectHandle> {
        let key = Key::new(type_, max_size)?;
        let handle = self.mint_handle();
        let prev =
            self.by_handle.insert(handle.clone(), Rc::new(RefCell::new(TransientObject::new(key))));
        debug_assert!(prev.is_none());

        Ok(handle)
    }

    fn free(&mut self, handle: ObjectHandle) {
        if handle.is_null() {
            return;
        }
        match self.by_handle.entry(handle) {
            HashMapEntry::Occupied(entry) => {
                let _ = entry.remove();
            }
            HashMapEntry::Vacant(_) => panic!("{handle:?} is not a valid handle"),
        }
    }

    fn reset(&self, handle: ObjectHandle) {
        match self.by_handle.get(&handle) {
            Some(obj) => {
                let mut obj = obj.borrow_mut();
                obj.flags.remove(HandleFlags::INITIALIZED);
                obj.key.reset()
            }
            None => panic!("{handle:?} is not a valid handle"),
        }
    }

    fn populate(&self, handle: ObjectHandle, attributes: &[Attribute]) -> TeeResult {
        match self.by_handle.get(&handle) {
            Some(obj) => {
                let mut obj = obj.borrow_mut();
                assert!(
                    !obj.flags.contains(HandleFlags::INITIALIZED),
                    "{handle:?} is already initialized"
                );
                obj.flags.insert(HandleFlags::INITIALIZED);
                obj.key.populate(attributes)
            }
            None => panic!("{handle:?} is not a valid handle"),
        }
    }

    fn generate_key(&self, handle: ObjectHandle, size: u32, params: &[Attribute]) -> TeeResult {
        match self.by_handle.get(&handle) {
            Some(obj) => {
                let mut obj = obj.borrow_mut();
                assert!(
                    !obj.flags.contains(HandleFlags::INITIALIZED),
                    "{handle:?} is already initialized"
                );
                obj.flags.insert(HandleFlags::INITIALIZED);
                obj.key.generate(size, params)
            }
            None => panic!("{handle:?} is not a valid handle"),
        }
    }

    // Returns a shared reference to the associated object, if `handle` is
    // valid; panics otherwise.
    fn get(&self, handle: ObjectHandle) -> Ref<'_, TransientObject> {
        self.by_handle
            .get(&handle)
            .unwrap_or_else(|| panic!("{handle:?} is not a valid handle"))
            .borrow()
    }

    // Returns an exclusive reference to the associated object view, if `handle` is
    // valid; panics otherwise.
    fn get_mut(&self, handle: ObjectHandle) -> RefMut<'_, TransientObject> {
        self.by_handle
            .get(&handle)
            .unwrap_or_else(|| panic!("{handle:?} is not a valid handle"))
            .borrow_mut()
    }

    fn mint_handle(&mut self) -> ObjectHandle {
        let handle_value = self.next_handle_value;
        self.next_handle_value += 2;
        ObjectHandle::from_value(handle_value)
    }
}

struct PersistentObject {
    key: Key,
    usage: Usage,
    base_flags: HandleFlags,
    data: zx::Vmo,
    data_size: usize,
    id: Vec<u8>,

    // The open handles to this object. Tracking these in this way conveniently
    // enables their invalidation in the case of object overwriting.
    handles: HashSet<ObjectHandle>,
}

impl Object for PersistentObject {
    fn key(&self) -> &Key {
        &self.key
    }

    fn usage(&self) -> &Usage {
        &self.usage
    }
    fn usage_mut(&mut self) -> &mut Usage {
        &mut self.usage
    }

    fn flags(&self) -> &HandleFlags {
        &self.base_flags
    }
}

// A handle's view into a persistent object.
struct PersistentObjectView {
    object: Rc<RefCell<PersistentObject>>,
    flags: HandleFlags,
    data_position: usize,
}

impl PersistentObjectView {
    fn get_info(&self) -> ObjectInfo {
        let obj = self.object.borrow();
        obj.get_info(obj.data_size, self.data_position)
    }

    // See read_object_data().
    fn read_data<'a>(&mut self, buffer: &'a mut [u8]) -> TeeResult<&'a [u8]> {
        let obj = self.object.borrow();
        let read_size = min(obj.data_size - self.data_position, buffer.len());
        let written = &mut buffer[..read_size];
        if read_size > 0 {
            obj.data.read(written, self.data_position as u64).unwrap();
        }
        self.data_position += read_size;
        Ok(written)
    }

    // See write_object_data().
    fn write_data(&mut self, data: &[u8]) -> TeeResult {
        if data.is_empty() {
            return Ok(());
        }
        let mut obj = self.object.borrow_mut();
        let write_end = self.data_position + data.len();

        if write_end > DATA_MAX_POSITION {
            return Err(Error::Overflow);
        }
        if write_end > obj.data_size {
            obj.data.set_size(write_end as u64).unwrap();
            obj.data_size = write_end;
        }
        obj.data.write(data, self.data_position as u64).unwrap();
        self.data_position = write_end;
        Ok(())
    }

    // See truncate_object_data().
    fn truncate_data(&self, size: usize) -> TeeResult {
        let mut obj = self.object.borrow_mut();

        // It's okay to set the size past the position in either direction.
        // However, the spec does not actually cover the case where the
        // provided size is is larger than DATA_MAX_POSITION. Since any
        // part of the data stream past that would be inaccessible; it
        // should be sensible and harmless to not exceed that in resizing.
        let size = min(size, DATA_MAX_POSITION);
        obj.data.set_size(size as u64).unwrap();
        obj.data_size = size;
        Ok(())
    }

    // See seek_object_data().
    fn seek_data(&mut self, offset: isize, whence: Whence) -> TeeResult {
        let start = match whence {
            Whence::DataSeekCur => self.data_position,
            Whence::DataSeekEnd => self.object.borrow().data_size,
            Whence::DataSeekSet => 0,
        };
        let new_position = start.saturating_add_signed(offset);
        if new_position > DATA_MAX_POSITION {
            Err(Error::Overflow)
        } else {
            self.data_position = new_position;
            Ok(())
        }
    }
}

// The state of an object enum handle.
struct EnumState {
    // None if in the allocated/unstarted state.
    id: Option<Vec<u8>>,
}

// A B-tree since enumeration needs to deal in key (i.e., ID) ordering.
//
// Further, the key represents a separately owned copy of the ID; we do this
// instead of representing the key as an Rc<Vec<u8>> as then we would no
// longer be able to perform look-up with slices - since Borrow is not
// implemented for Rc - and would instead have to dynamically allocate a new
// key for the look-up. Better to not touch the heap when bad inputs are
// provided.
type PersistentIdMap = BTreeMap<Vec<u8>, Rc<RefCell<PersistentObject>>>;

type PersistentHandleMap = HashMap<ObjectHandle, RefCell<PersistentObjectView>>;
type PersistentEnumHandleMap = HashMap<ObjectEnumHandle, RefCell<EnumState>>;

// A class abstraction implementing the persistent storage interface.
struct PersistentObjects {
    by_id: PersistentIdMap,
    by_handle: PersistentHandleMap,
    enum_handles: PersistentEnumHandleMap,
    next_handle_value: u64,
    next_enum_handle_value: u64,
}

impl PersistentObjects {
    fn new() -> Self {
        Self {
            by_id: PersistentIdMap::new(),
            by_handle: PersistentHandleMap::new(),
            enum_handles: HashMap::new(),
            next_handle_value: 1, // Always odd, per the described convention above
            next_enum_handle_value: 1,
        }
    }

    fn create(
        &mut self,
        key: Key,
        usage: Usage,
        flags: HandleFlags,
        id: &[u8],
        initial_data: &[u8],
    ) -> TeeResult<ObjectHandle> {
        assert!(id.len() <= OBJECT_ID_MAX_LEN);

        let data = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, initial_data.len() as u64)
            .unwrap();
        if !initial_data.is_empty() {
            data.write(initial_data, 0).unwrap();
        }

        let flags = flags.union(HandleFlags::PERSISTENT | HandleFlags::INITIALIZED);

        let obj = PersistentObject {
            key,
            usage,
            base_flags: flags,
            data,
            data_size: initial_data.len(),
            id: Vec::from(id),
            handles: HashSet::new(),
        };

        let obj_ref = match self.by_id.get(id) {
            // If there's already an object with that ID, then
            // DATA_FLAG_OVERWRITE permits overwriting. This results in
            // existing handles being invalidated.
            Some(obj_ref) => {
                if !flags.contains(HandleFlags::DATA_FLAG_OVERWRITE) {
                    return Err(Error::AccessConflict);
                }
                {
                    let mut obj_old = obj_ref.borrow_mut();
                    for handle in obj_old.handles.iter() {
                        let removed = self.by_handle.remove(&handle).is_some();
                        debug_assert!(removed);
                    }
                    *obj_old = obj;
                }
                obj_ref.clone()
            }
            None => {
                let id = obj.id.clone();
                let obj_ref = Rc::new(RefCell::new(obj));
                let inserted = self.by_id.insert(id, obj_ref.clone());
                debug_assert!(inserted.is_none());
                obj_ref
            }
        };
        Ok(self.open_internal(obj_ref, flags))
    }

    // See open_persistent_object().
    fn open(&mut self, id: &[u8], flags: HandleFlags) -> TeeResult<ObjectHandle> {
        assert!(id.len() <= OBJECT_ID_MAX_LEN);

        let obj_ref = match self.by_id.get(id) {
            Some(obj_ref) => Ok(obj_ref),
            None => Err(Error::ItemNotFound),
        }?;

        {
            let mut obj = obj_ref.borrow_mut();

            // At any given time, the number of object references should be
            // greater than or equal to the number of handle map values + the
            // number of object ID map values, which should be equal to the #
            // of open handles to that object + 1.
            debug_assert!(Rc::strong_count(obj_ref) >= obj.handles.len() + 1);

            // If we previously closed the last handle to the object and are
            // now reopening its first active handle, overwrite the base flags
            // with the handle's. The spec doesn't dictate this, but it's hard
            // to imagine what else an implementation could or should do in
            // this case.
            if obj.handles.is_empty() {
                obj.base_flags = flags.union(HandleFlags::PERSISTENT | HandleFlags::INITIALIZED);
            } else {
                let combined = flags.union(obj.base_flags);
                let intersection = flags.intersection(obj.base_flags);

                // Check for shared read permissions.
                if flags.contains(HandleFlags::DATA_ACCESS_READ)
                    && !(intersection.contains(HandleFlags::DATA_SHARE_READ))
                {
                    return Err(Error::AccessConflict);
                }

                // Check for shared read permission consistency.
                if combined.contains(HandleFlags::DATA_SHARE_READ)
                    == intersection.contains(HandleFlags::DATA_SHARE_READ)
                {
                    return Err(Error::AccessConflict);
                }

                // Check for shared write permissions.
                if flags.contains(HandleFlags::DATA_ACCESS_WRITE)
                    && !(intersection.contains(HandleFlags::DATA_SHARE_WRITE))
                {
                    return Err(Error::AccessConflict);
                }

                // Check for shared write permission consistency.
                if combined.contains(HandleFlags::DATA_SHARE_WRITE)
                    == intersection.contains(HandleFlags::DATA_SHARE_WRITE)
                {
                    return Err(Error::AccessConflict);
                }
            }
        }

        Ok(self.open_internal(obj_ref.clone(), flags))
    }

    // The common handle opening subroutine of create() and open(), which
    // expects that the operation has been validated.
    fn open_internal(
        &mut self,
        object: Rc<RefCell<PersistentObject>>,
        flags: HandleFlags,
    ) -> ObjectHandle {
        let handle = self.mint_handle();
        let inserted = object.borrow_mut().handles.insert(handle);
        debug_assert!(inserted);
        let view = PersistentObjectView { object, flags, data_position: 0 };
        let inserted = self.by_handle.insert(handle, RefCell::new(view)).is_none();
        debug_assert!(inserted);
        handle
    }

    fn close(&mut self, handle: ObjectHandle) {
        // Note that even if all handle map entries associated with the object
        // are removed, the reference to the object in the ID map remains,
        // keeping it alive for future open() calls.
        match self.by_handle.entry(handle) {
            HashMapEntry::Occupied(entry) => {
                {
                    let view = entry.get().borrow_mut();
                    let mut obj = view.object.borrow_mut();
                    let removed = obj.handles.remove(&handle);
                    debug_assert!(removed);
                }
                let _ = entry.remove();
            }
            HashMapEntry::Vacant(_) => panic!("{handle:?} is not a valid handle"),
        }
    }

    // See close_and_delete_persistent_object(). Although unlike that function,
    // this one returns Error::AccessDenied if `handle` was not opened with
    // DATA_ACCESS_WRITE_META.
    fn close_and_delete(&mut self, handle: ObjectHandle) -> TeeResult {
        // With both maps locked, removal of all entries with the associated
        // object handle should amount to dropping that object.
        match self.by_handle.entry(handle) {
            HashMapEntry::Occupied(entry) => {
                {
                    let state = entry.get().borrow();
                    if !state.flags.contains(HandleFlags::DATA_ACCESS_WRITE_META) {
                        return Err(Error::AccessDenied);
                    }
                    let obj = state.object.borrow();
                    debug_assert_eq!(obj.handles.len(), 1);
                    let removed = self.by_id.remove(&obj.id).is_some();
                    debug_assert!(removed);
                }
                let _ = entry.remove();
                Ok(())
            }
            HashMapEntry::Vacant(_) => panic!("{handle:?} is not a valid handle"),
        }
    }

    // See rename_persistent_object(). Although unlike that function, this one
    // returns Error::AccessDenied if `handle` was not opened with
    // DATA_ACCESS_WRITE_META.
    fn rename(&mut self, handle: ObjectHandle, new_id: &[u8]) -> TeeResult {
        match self.by_handle.entry(handle) {
            HashMapEntry::Occupied(handle_entry) => {
                let state = handle_entry.get().borrow();
                if !state.flags.contains(HandleFlags::DATA_ACCESS_WRITE_META) {
                    return Err(Error::AccessDenied);
                }
                let new_id = Vec::from(new_id);
                match self.by_id.entry(new_id.clone()) {
                    BTreeMapEntry::Occupied(_) => return Err(Error::AccessConflict),
                    BTreeMapEntry::Vacant(id_entry) => {
                        let _ = id_entry.insert(state.object.clone());
                    }
                };
                let mut obj = state.object.borrow_mut();
                let removed = self.by_id.remove(&obj.id);
                debug_assert!(removed.is_some());
                obj.id = new_id;
                Ok(())
            }
            HashMapEntry::Vacant(_) => panic!("{handle:?} is not a valid handle"),
        }
    }

    // Returns a shared reference to the associated object view, if `handle` is
    // valid; panics otherwise.
    fn get(&self, handle: ObjectHandle) -> Ref<'_, PersistentObjectView> {
        self.by_handle
            .get(&handle)
            .unwrap_or_else(|| panic!("{handle:?} is not a valid handle"))
            .borrow()
    }

    // Returns an exclusive reference to the associated object view, if `handle` is
    // valid; panics otherwise.
    fn get_mut(&self, handle: ObjectHandle) -> RefMut<'_, PersistentObjectView> {
        self.by_handle
            .get(&handle)
            .unwrap_or_else(|| panic!("{handle:?} is not a valid handle"))
            .borrow_mut()
    }

    // See allocate_persistent_object_enumerator().
    fn allocate_enumerator(&mut self) -> ObjectEnumHandle {
        let enumerator = self.mint_enumerator_handle();

        let previous =
            self.enum_handles.insert(enumerator.clone(), RefCell::new(EnumState { id: None }));
        debug_assert!(previous.is_none());
        enumerator
    }

    // See free_persistent_object_enumerator().
    fn free_enumerator(&mut self, enumerator: ObjectEnumHandle) -> () {
        match self.enum_handles.entry(enumerator) {
            HashMapEntry::Occupied(entry) => {
                let _ = entry.remove();
            }
            HashMapEntry::Vacant(_) => panic!("{enumerator:?} is not a valid enumerator handle"),
        }
    }

    // See reset_persistent_object_enumerator().
    fn reset_enumerator(&mut self, enumerator: ObjectEnumHandle) -> () {
        match self.enum_handles.get(&enumerator) {
            Some(state) => {
                state.borrow_mut().id = None;
            }
            None => panic!("{enumerator:?} is not a valid enumerator handle"),
        }
    }

    // See get_next_persistent_object().
    fn get_next_object<'a>(
        &self,
        enumerator: ObjectEnumHandle,
        id_buffer: &'a mut [u8],
    ) -> TeeResult<(ObjectInfo, &'a [u8])> {
        match self.enum_handles.get(&enumerator) {
            Some(state) => {
                let mut state = state.borrow_mut();
                let next = if state.id.is_none() {
                    self.by_id.first_key_value()
                } else {
                    // Since we're dealing with an ID-keyed B-tree, we can
                    // straightforwardly get the first entry with an ID larger
                    // than the current.
                    let curr_id = state.id.as_ref().unwrap();
                    self.by_id.range((Bound::Excluded(curr_id.clone()), Bound::Unbounded)).next()
                };
                if let Some((id, obj)) = next {
                    assert!(id_buffer.len() >= id.len());
                    let written = &mut id_buffer[..id.len()];
                    written.copy_from_slice(id);
                    state.id = Some(id.clone());
                    Ok((obj.borrow().get_info(/*data_size=*/ 0, /*data_position=*/ 0), written))
                } else {
                    Err(Error::ItemNotFound)
                }
            }
            None => panic!("{enumerator:?} is not a valid enumerator handle"),
        }
    }

    fn mint_handle(&mut self) -> ObjectHandle {
        // Per the described convention above, always odd. (Initial value is 1.)
        let handle_value = self.next_handle_value;
        self.next_handle_value += 2;
        ObjectHandle::from_value(handle_value)
    }

    fn mint_enumerator_handle(&mut self) -> ObjectEnumHandle {
        let handle_value = self.next_enum_handle_value;
        self.next_enum_handle_value += 1;
        ObjectEnumHandle::from_value(handle_value)
    }
}

//
// Implementation
//

impl Storage {
    pub fn new() -> Self {
        Self {
            persistent_objects: PersistentObjects::new(),
            transient_objects: TransientObjects::new(),
        }
    }

    pub fn get(&self, object: ObjectHandle) -> Rc<RefCell<dyn Object>> {
        if is_transient_handle(object) {
            self.transient_objects.by_handle.get(&object).unwrap().clone()
        } else {
            self.persistent_objects.by_handle.get(&object).unwrap().borrow().object.clone()
        }
    }

    /// Returns info about an open object as well of the state of its handle.
    ///
    /// Panics if `object` is not a valid handle.
    pub fn get_object_info(&self, object: ObjectHandle) -> ObjectInfo {
        if is_transient_handle(object) {
            self.transient_objects
                .get(object)
                .get_info(/*data_size=*/ 0, /*data_position=*/ 0)
        } else {
            self.persistent_objects.get(object).get_info()
        }
    }

    /// Restricts the usage of an open object handle.
    ///
    /// Panics if `object` is not a valid handle.
    pub fn restrict_object_usage(&self, object: ObjectHandle, usage: Usage) {
        if is_transient_handle(object) {
            self.transient_objects.get_mut(object).restrict_usage(usage)
        } else {
            self.persistent_objects.get(object).object.borrow_mut().restrict_usage(usage)
        }
    }
}

impl Storage {
    /// Returns the requested buffer-type attribute associated with the given
    /// object, if any. It is written to the provided buffer and the size of
    /// what is written is returned.
    ///
    /// Returns a wrapped value of Error::ItemNotFound if the object does not have
    /// such an attribute.
    ///
    /// Returns a wrapped value of Error::ShortBuffer if the buffer was too small
    /// to read the attribute value into, along with the length of the attribute.
    ///
    /// Panics if `object` is not a valid handle or if `attribute_id` is not of
    /// buffer type.
    pub fn get_object_buffer_attribute(
        &self,
        object: ObjectHandle,
        attribute_id: AttributeId,
        buffer: &mut [u8],
    ) -> Result<usize, ErrorWithSize> {
        assert!(!attribute_id.value());

        let copy_from_key = |obj: &dyn Object, buffer: &mut [u8]| -> Result<usize, ErrorWithSize> {
            if !attribute_id.public() {
                assert!(obj.usage().contains(Usage::EXTRACTABLE));
            }

            let attr = obj.key().buffer_attribute(attribute_id);
            let bytes = match &attr {
                None => return Err(ErrorWithSize::new(Error::ItemNotFound)),
                Some(BufferAttribute::Slice(bytes)) => bytes,
                Some(BufferAttribute::Vector(bytes)) => bytes.as_slice(),
            };
            if buffer.len() < bytes.len() {
                Err(ErrorWithSize::short_buffer(bytes.len()))
            } else {
                let written = &mut buffer[..bytes.len()];
                written.copy_from_slice(bytes);
                Ok(written.len())
            }
        };

        if is_transient_handle(object) {
            copy_from_key(self.transient_objects.get(object).deref(), buffer)
        } else {
            copy_from_key(
                self.persistent_objects.get(object).object.as_ref().borrow().deref(),
                buffer,
            )
        }
    }

    /// Returns the requested value-type attribute associated with the given
    /// object, if any.
    ///
    /// Returns Error::ItemNotFound if the object does not have such an attribute.
    ///
    /// Panics if `object` is not a valid handle or if `attribute_id` is not of
    /// value type.
    pub fn get_object_value_attribute(
        &self,
        object: ObjectHandle,
        attribute_id: AttributeId,
    ) -> TeeResult<ValueFields> {
        assert!(!attribute_id.value());

        let copy_from_key = |obj: &dyn Object| {
            if !attribute_id.public() {
                assert!(obj.usage().contains(Usage::EXTRACTABLE));
            }
            if let Some(value) = obj.key().value_attribute(attribute_id) {
                Ok(value)
            } else {
                Err(Error::ItemNotFound)
            }
        };

        if is_transient_handle(object) {
            copy_from_key(self.transient_objects.get(object).deref())
        } else {
            copy_from_key(self.persistent_objects.get(object).object.borrow().deref())
        }
    }

    /// Closes the given object handle.
    ///
    /// Panics if `object` is neither null or a valid handle.
    pub fn close_object(&mut self, object: ObjectHandle) {
        if object.is_null() {
            return;
        }

        if is_transient_handle(object) {
            self.transient_objects.free(object)
        } else {
            self.persistent_objects.close(object)
        }
    }

    /// Creates a new transient object of the given type and maximum key size.
    ///
    /// Returns Error::NotSupported if the type is unsupported or if the
    /// maximum key size is itself not a valid key size.
    pub fn allocate_transient_object(
        &mut self,
        object_type: Type,
        max_size: u32,
    ) -> TeeResult<ObjectHandle> {
        self.transient_objects.allocate(object_type, max_size)
    }

    /// Destroys a transient object.
    ///
    /// Panics if `object` is not a valid transient object handle.
    pub fn free_transient_object(&mut self, object: ObjectHandle) {
        assert!(is_transient_handle(object));
        self.transient_objects.free(object)
    }

    /// Resets a transient object back to its uninitialized state.
    ///
    /// Panics if `object` is not a valid transient object handle.
    pub fn reset_transient_object(&mut self, object: ObjectHandle) {
        assert!(is_transient_handle(object));
        self.transient_objects.reset(object)
    }

    /// Populates the key information of a transient object from a given list
    /// of attributes.
    ///
    /// Panics if `object` is not a valid transient object handle, or if
    /// `attrs` omits required attributes or includes unrelated ones.
    pub fn populate_transient_object(
        &self,
        object: ObjectHandle,
        attrs: &[Attribute],
    ) -> TeeResult {
        assert!(is_transient_handle(object));
        self.transient_objects.populate(object, attrs)
    }
}

pub fn init_ref_attribute(id: AttributeId, buffer: &mut [u8]) -> Attribute {
    assert!(id.memory_reference(), "Attribute ID {id:?} does not represent a memory reference");
    Attribute { id, content: BufferOrValue { memref: MemRef::from_mut_slice(buffer) } }
}

pub fn init_value_attribute(id: AttributeId, value: ValueFields) -> Attribute {
    assert!(id.value(), "Attribute ID {id:?} does not represent value fields");
    Attribute { id, content: BufferOrValue { value } }
}

impl Storage {
    pub fn copy_object_attributes(&mut self, _src: ObjectHandle, dest: ObjectHandle) -> TeeResult {
        assert!(is_transient_handle(dest));
        unimplemented!()
    }

    /// Generates key information on an uninitialized, transient object, given
    /// a key size and the attributes that serve as inputs to the generation
    /// process.
    ///
    /// Panics if `object` is not a valid handle to an uninitialized, transient
    /// object, if key size is invalid or larger than the prescribed maximum,
    /// or if a mandatory attribute is absent.
    pub fn generate_key(
        &mut self,
        object: ObjectHandle,
        key_size: u32,
        params: &[Attribute],
    ) -> TeeResult {
        self.transient_objects.generate_key(object, key_size, params)
    }

    /// Opens a new handle to an existing persistent object.
    ///
    /// Returns Error::ItemNotFound: if `storage` does not correspond to a valid
    /// storage space, or if no object with `id` is found.
    ///
    /// Returns Error::AccessConflict if any of the following hold:
    ///   - The object is currently open with DATA_ACCESS_WRITE_META;
    ///   - The object is currently open and `flags` contains
    ///     DATA_ACCESS_WRITE_META
    ///   - The object is currently open without DATA_ACCESS_READ_SHARE
    ///     and `flags` contains DATA_ACCESS_READ or DATA_ACCESS_READ_SHARE;
    ///   - The object is currently open with DATA_ACCESS_READ_SHARE, but `flags`
    ///     does not;
    ///   - The object is currently open without DATA_ACCESS_WRITE_SHARE and
    ///     `flags` contains DATA_ACCESS_WRITE or DATA_ACCESS_WRITE_SHARE;
    ///   - The object is currently open with DATA_ACCESS_WRITE_SHARE, but `flags`
    ///     does not.
    pub fn open_persistent_object(
        &mut self,
        storage: TeeStorage,
        id: &[u8],
        flags: HandleFlags,
    ) -> TeeResult<ObjectHandle> {
        if storage == TeeStorage::Private {
            self.persistent_objects.open(id, flags)
        } else {
            Err(Error::ItemNotFound)
        }
    }

    /// Creates a persistent object and returns a handle to it. The conferred type,
    /// usage, and attributes are given indirectly by `attribute_src`; if
    /// `attribute_src` is null then the conferred type is Data.
    ///
    /// Returns Error::ItemNotFound: if `storage` does not correspond to a valid
    /// storage spac
    ///
    /// Returns Error::AccessConflict if the provided ID already exists but
    /// `flags` does not contain DATA_FLAG_OVERWRITE.
    pub fn create_persistent_object(
        &mut self,
        storage: TeeStorage,
        id: &[u8],
        flags: HandleFlags,
        attribute_src: ObjectHandle,
        initial_data: &[u8],
    ) -> TeeResult<ObjectHandle> {
        if storage != TeeStorage::Private {
            return Err(Error::ItemNotFound);
        }

        let (key, usage, base_flags) = if attribute_src.is_null() {
            (Key::Data(NoKey {}), Usage::default(), HandleFlags::empty())
        } else if is_persistent_handle(attribute_src) {
            let view = self.persistent_objects.get(attribute_src);
            let obj = view.object.borrow();
            (obj.key.clone(), obj.usage, obj.base_flags)
        } else {
            unimplemented!();
        };
        let flags = base_flags.union(flags);
        self.persistent_objects.create(key, usage, flags, id, initial_data)
    }

    /// Closes the given handle to a persistent object and deletes the object.
    ///
    /// Panics if `object` is invalid or was not opened with
    /// DATA_ACCESS_WRITE_META.
    pub fn close_and_delete_persistent_object(&mut self, object: ObjectHandle) -> TeeResult {
        assert!(is_persistent_handle(object));
        self.persistent_objects.close_and_delete(object)
    }

    /// Renames the object's, associating it with a new identifier.
    ///
    /// Returns Error::AccessConflict if `new_id` is the ID of an existing
    /// object.
    ///
    /// Panics if `object` is invalid or was not opened with
    /// DATA_ACCESS_WRITE_META.
    pub fn rename_persistent_object(&mut self, object: ObjectHandle, new_id: &[u8]) -> TeeResult {
        assert!(is_persistent_handle(object));
        self.persistent_objects.rename(object, new_id)
    }

    /// Allocates a new object enumerator and returns a handle to it.
    pub fn allocate_persistent_object_enumerator(&mut self) -> ObjectEnumHandle {
        self.persistent_objects.allocate_enumerator()
    }

    /// Deallocates an object enumerator.
    ///
    /// Panics if `enumerator` is not a valid handle.
    pub fn free_persistent_object_enumerator(&mut self, enumerator: ObjectEnumHandle) {
        self.persistent_objects.free_enumerator(enumerator)
    }

    /// Resets an object enumerator.
    ///
    /// Panics if `enumerator` is not a valid handle.
    pub fn reset_persistent_object_enumerator(&mut self, enumerator: ObjectEnumHandle) {
        self.persistent_objects.reset_enumerator(enumerator)
    }

    /// Starts an object enumerator's enumeration, or resets it if already started.
    ///
    /// Returns Error::ItemNotFound if `storage` is unsupported or it there are no
    /// objects yet created in that storage space.
    ///
    /// Panics if `enumerator` is not a valid handle.
    pub fn start_persistent_object_enumerator(
        &mut self,
        enumerator: ObjectEnumHandle,
        storage: TeeStorage,
    ) -> TeeResult {
        if storage == TeeStorage::Private {
            self.reset_persistent_object_enumerator(enumerator);
            Ok(())
        } else {
            Err(Error::ItemNotFound)
        }
    }

    /// Returns the info and ID associated with the next object in the enumeration,
    /// advancing it in the process. The returns object ID is backed by the
    /// provided buffer.
    ///
    /// Returns Error::ItemNotFound if there are no more objects left to enumerate.
    ///
    /// Panics if `enumerator` is not a valid handle.
    pub fn get_next_persistent_object<'a>(
        &self,
        enumerator: ObjectEnumHandle,
        id_buffer: &'a mut [u8],
    ) -> TeeResult<(ObjectInfo, &'a [u8])> {
        self.persistent_objects.get_next_object(enumerator, id_buffer)
    }

    /// Tries to read as much of the object's data stream from the handle's current
    /// data position as can fill the provided buffer.
    ///
    /// Panics if `object` is invalid or does not have read access.
    pub fn read_object_data<'a>(
        &self,
        object: ObjectHandle,
        buffer: &'a mut [u8],
    ) -> TeeResult<&'a [u8]> {
        assert!(is_persistent_handle(object));
        self.persistent_objects.get_mut(object).read_data(buffer)
    }

    /// Writes the provided data to the object's data stream at the handle's
    /// data position, advancing that position to the end of the written data.
    ///
    /// Returns Error::AccessConflict if the object does not have write
    /// access.
    ///
    /// Returns Error::Overflow if writing the data would advance the data
    /// position past DATA_MAX_POSITION.
    ///
    /// Panics if `object` is invalid or does not have write access.
    pub fn write_object_data(&self, object: ObjectHandle, buffer: &[u8]) -> TeeResult {
        assert!(is_persistent_handle(object));
        self.persistent_objects.get_mut(object).write_data(buffer)
    }

    /// Truncates or zero-extends the object's data stream to provided size.
    /// This does not affect any handle's data position.
    ///
    /// Returns Error::Overflow if `size` is larger than DATA_MAX_POSITION.
    ///
    /// Panics if `object` is invalid or does not have write access.
    pub fn truncate_object_data(&self, object: ObjectHandle, size: usize) -> TeeResult {
        assert!(is_persistent_handle(object));
        self.persistent_objects.get(object).truncate_data(size)
    }

    /// Updates the handle's data positition, seeking at an offset from a
    /// position given by a whence value. The new position saturates at 0.
    ///
    /// Returns Error::Overflow if the would-be position exceeds
    /// DATA_MAX_POSITION.
    ///
    /// Panics if `object` is invalid.
    pub fn seek_data_object(
        &self,
        object: ObjectHandle,
        offset: isize,
        whence: Whence,
    ) -> TeeResult {
        assert!(is_persistent_handle(object));
        self.persistent_objects.get_mut(object).seek_data(offset, whence)
    }
}

// TODO(https://fxbug.dev/376093162): Add TransientObjects testing.
// TODO(https://fxbug.dev/376093162): Add PersistentObjects testing.
