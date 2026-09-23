// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use std::hash::Hash;
use std::ops::{Deref, DerefMut};

pub mod binding;

//
// As described in the crate README, here we define and export the richer, more
// conventional/ergonomic, layout-compatible bindings for the TEE internal API.
//
// To conveniently translate between these types and the bindgen-generated
// ones, non-integral types Foo that correspond to input parameters in the API
// should admit a zero-copy
// ```
// fn from_binding<'a>(foo: &'a TEE_Foo) -> &'a Foo { ... }
// ```
// constructor, while those that correspond to output parameters should admit a
// zero-copy
// ```
// fn to_binding<'a>(&'a self) -> &'a TEE_Foo { ... }
// ```
// method. The input_parameter!, output_parameter!, and inout_parameter! macros
// can be used to automatically define these.
//
// Types below are listed in the order they appear in the spec.
//
// Layout-compatibility and zero-copy translation are verified in the tests below.
//

macro_rules! input_parameter {
    ($name:ident, $tee_name:path) => {
        impl $name {
            pub fn from_binding<'a>(input: &'a $tee_name) -> &'a Self {
                // SAFETY: ABI-compatibility checked below.
                unsafe { &*((input as *const $tee_name) as *const Self) }
            }
        }
    };
}

macro_rules! output_parameter {
    ($name:ident, $tee_name:path) => {
        impl $name {
            pub fn to_binding<'a>(&'a self) -> &'a $tee_name {
                // SAFETY:  ABI-compatibility checked below.
                unsafe { &*((self as *const Self) as *const $tee_name) }
            }
        }
    };
}

macro_rules! inout_parameter {
    ($name:ident, $tee_name:path) => {
        input_parameter!($name, $tee_name);
        output_parameter!($name, $tee_name);
    };
}

macro_rules! handle {
    ($name:ident, $tee_name:path) => {
        #[repr(C)]
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub struct $name(u64);

        impl $name {
            pub fn from_value(value: u64) -> Self {
                Self(value)
            }

            pub fn is_null(&self) -> bool {
                self.0 == binding::TEE_HANDLE_NULL.into()
            }
        }

        inout_parameter!($name, $tee_name);

        impl Deref for $name {
            type Target = u64;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };
}

//
// Common data types
//

handle!(TaSessionHandle, binding::TEE_TASessionHandle);
handle!(PropSetHandle, binding::TEE_PropSetHandle);
handle!(ObjectHandle, binding::TEE_ObjectHandle);
handle!(ObjectEnumHandle, binding::TEE_ObjectEnumHandle);
handle!(OperationHandle, binding::TEE_OperationHandle);

// Bindgen didn't carry these values into the generated Rust bindings.
pub const TEE_PROPSET_TEE_IMPLEMENTATION: PropSetHandle = PropSetHandle(0xfffffffd);
pub const TEE_PROPSET_CURRENT_CLIENT: PropSetHandle = PropSetHandle(0xfffffffe);
pub const TEE_PROPSET_CURRENT_TA: PropSetHandle = PropSetHandle(0xffffffff);

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Corrupt object")]
    CorruptObject = binding::TEE_ERROR_CORRUPT_OBJECT,
    #[error("Corrupt object")]
    CorruptObject2 = binding::TEE_ERROR_CORRUPT_OBJECT_2,
    #[error("Not available")]
    NotAvailable = binding::TEE_ERROR_STORAGE_NOT_AVAILABLE,
    #[error("Not available")]
    NotAvailable2 = binding::TEE_ERROR_STORAGE_NOT_AVAILABLE_2,
    #[error("Unsupported version")]
    UnsupportedVersion = binding::TEE_ERROR_UNSUPPORTED_VERSION,
    #[error("Invalid ciphertext")]
    CiphertextInvalid = binding::TEE_ERROR_CIPHERTEXT_INVALID,
    #[error("Generic error")]
    Generic = binding::TEE_ERROR_GENERIC,
    #[error("Access denied")]
    AccessDenied = binding::TEE_ERROR_ACCESS_DENIED,
    #[error("Canceled")]
    Cancel = binding::TEE_ERROR_CANCEL,
    #[error("Access conflict")]
    AccessConflict = binding::TEE_ERROR_ACCESS_CONFLICT,
    #[error("Excess data")]
    ExcessData = binding::TEE_ERROR_EXCESS_DATA,
    #[error("Bad format")]
    BadFormat = binding::TEE_ERROR_BAD_FORMAT,
    #[error("Bad parameters")]
    BadParameters = binding::TEE_ERROR_BAD_PARAMETERS,
    #[error("Bad state")]
    BadState = binding::TEE_ERROR_BAD_STATE,
    #[error("Item not found")]
    ItemNotFound = binding::TEE_ERROR_ITEM_NOT_FOUND,
    #[error("Not implemented")]
    NotImplemented = binding::TEE_ERROR_NOT_IMPLEMENTED,
    #[error("Not supported")]
    NotSupported = binding::TEE_ERROR_NOT_SUPPORTED,
    #[error("No data")]
    NoData = binding::TEE_ERROR_NO_DATA,
    #[error("Out of memory")]
    OutOfMemory = binding::TEE_ERROR_OUT_OF_MEMORY,
    #[error("Busy")]
    Busy = binding::TEE_ERROR_BUSY,
    #[error("Communication error")]
    Communication = binding::TEE_ERROR_COMMUNICATION,
    #[error("Security error")]
    Security = binding::TEE_ERROR_SECURITY,
    #[error("Buffer too small")]
    ShortBuffer = binding::TEE_ERROR_SHORT_BUFFER,
    #[error("Externally canceled")]
    ExternalCancel = binding::TEE_ERROR_EXTERNAL_CANCEL,
    #[error("Timeout")]
    Timeout = binding::TEE_ERROR_TIMEOUT,
    #[error("Overflow")]
    Overflow = binding::TEE_ERROR_OVERFLOW,
    #[error("Target is dead")]
    TargetDead = binding::TEE_ERROR_TARGET_DEAD,
    #[error("Out of storage space")]
    StorageNoSpace = binding::TEE_ERROR_STORAGE_NO_SPACE,
    #[error("Invalid MAC")]
    MacInvalid = binding::TEE_ERROR_MAC_INVALID,
    #[error("Invalid signature")]
    SignatureInvalid = binding::TEE_ERROR_SIGNATURE_INVALID,
    #[error("Time is not set")]
    TimeNotSet = binding::TEE_ERROR_TIME_NOT_SET,
    #[error("Time needs to be reset")]
    TimeNeedsReset = binding::TEE_ERROR_TIME_NEEDS_RESET,
}

impl Error {
    pub fn from_tee_result(result: binding::TEE_Result) -> Option<Self> {
        match result {
            binding::TEE_SUCCESS => None,
            error => Some(Error::from_u32(error).unwrap_or(Error::Generic)),
        }
    }
}

pub type Result<T = ()> = std::result::Result<T, Error>;

pub fn to_tee_result(result: crate::Result) -> binding::TEE_Result {
    match result {
        Ok(()) => binding::TEE_SUCCESS,
        Err(error) => error as binding::TEE_Result,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Uuid {
    pub time_low: u32,
    pub time_mid: u16,
    pub time_hi_and_version: u16,
    pub clock_seq_and_node: [u8; 8],
}

inout_parameter!(Uuid, binding::TEE_UUID);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueFields {
    pub a: u32,
    pub b: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemRef {
    pub buffer: *mut u8,
    pub size: usize,
}

impl MemRef {
    pub fn from_mut_slice(slice: &mut [u8]) -> Self {
        MemRef { buffer: slice.as_mut_ptr(), size: slice.len() }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.assert_as_slice_preconditions();
        // SAFETY: preconditions asserted above.
        unsafe { std::slice::from_raw_parts(self.buffer, self.size) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.assert_as_slice_preconditions();
        // SAFETY: preconditions asserted above.
        unsafe { std::slice::from_raw_parts_mut(self.buffer, self.size) }
    }

    // Asserts that the preconditions to std::slice::from_raw_parts(_mut)? are
    // met.
    fn assert_as_slice_preconditions(&self) {
        assert!(!self.buffer.is_null());
        assert!(self.buffer.is_aligned());
        assert!(self.size * size_of::<u8>() < isize::MAX.try_into().unwrap());
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union BufferOrValue {
    pub memref: MemRef,
    pub value: ValueFields,
}

//
// Trusted Core Framework data types
//

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Identity {
    pub login: Login,
    pub uuid: Uuid,
}

output_parameter!(Identity, binding::TEE_Identity);

pub type Param = BufferOrValue;

inout_parameter!(Param, binding::TEE_Param);

pub fn param_list_to_binding_mut<'a>(
    params: &'a mut [Param; 4],
) -> &'a mut [binding::TEE_Param; 4] {
    // SAFETY: By virtue of Param-TEE_Param ABI compatibility.
    unsafe {
        &mut *(((params as *mut Param) as *mut binding::TEE_Param) as *mut [binding::TEE_Param; 4])
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum ParamType {
    None = binding::TEE_PARAM_TYPE_NONE as u8,
    ValueInput = binding::TEE_PARAM_TYPE_VALUE_INPUT as u8,
    ValueOutput = binding::TEE_PARAM_TYPE_VALUE_OUTPUT as u8,
    ValueInout = binding::TEE_PARAM_TYPE_VALUE_INOUT as u8,
    MemrefInput = binding::TEE_PARAM_TYPE_MEMREF_INPUT as u8,
    MemrefOutput = binding::TEE_PARAM_TYPE_MEMREF_OUTPUT as u8,
    MemrefInout = binding::TEE_PARAM_TYPE_MEMREF_INOUT as u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParamTypes(u32);

impl ParamTypes {
    pub fn as_u32(&self) -> u32 {
        self.0
    }

    pub fn get(&self, idx: usize) -> ParamType {
        if idx >= 4 {
            panic!("ParamTypes::get({}) too big; must be < 4", idx);
        }
        // We only expose a constructor that deals in valid ParamType values,
        // so this cannot panic.
        ParamType::from_u32((self.0 >> 4 * idx) & 0xf).unwrap()
    }

    pub fn from_types(types: [ParamType; 4]) -> Self {
        let t0 = types[0] as u32;
        let t1 = types[1] as u32;
        let t2 = types[2] as u32;
        let t3 = types[3] as u32;
        Self(t0 | (t1 << 4) | (t2 << 8) | (t3 << 12))
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum Login {
    Public = binding::TEE_LOGIN_PUBLIC,
    User = binding::TEE_LOGIN_USER,
    Group = binding::TEE_LOGIN_GROUP,
    Application = binding::TEE_LOGIN_APPLICATION,
    ApplicationUser = binding::TEE_LOGIN_APPLICATION_USER,
    ApplicationGroup = binding::TEE_LOGIN_APPLICATION_GROUP,
    TrustedApp = binding::TEE_LOGIN_TRUSTED_APP,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    Api = binding::TEE_ORIGIN_API,
    Comms = binding::TEE_ORIGIN_COMMS,
    Tee = binding::TEE_ORIGIN_TEE,
    TrustedApp = binding::TEE_ORIGIN_TRUSTED_APP,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryAccess(u32);

bitflags! {
    impl MemoryAccess : u32 {
        const READ = binding::TEE_MEMORY_ACCESS_READ;
        const WRITE = binding::TEE_MEMORY_ACCESS_WRITE;
        const ANY_OWNER = binding::TEE_MEMORY_ACCESS_ANY_OWNER;
  }
}

// Handle-like in practice if not one technically.
handle!(SessionContext, usize);

//
// Trusted Storage data types
//

pub const OBJECT_ID_MAX_LEN: usize = binding::TEE_OBJECT_ID_MAX_LEN as usize;
pub const DATA_MAX_POSITION: usize = binding::TEE_DATA_MAX_POSITION as usize;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Attribute {
    pub id: AttributeId,
    pub content: BufferOrValue,
}

output_parameter!(Attribute, binding::TEE_Attribute);

impl Attribute {
    // A fallible version that checks for a valid attribute ID.
    pub fn from_binding<'a>(attr: &'a binding::TEE_Attribute) -> Option<&'a Self> {
        // Check for valid attribute ID.
        let _ = AttributeId::from_u32(attr.attributeID)?;

        // SAFETY:  ABI-compatibility checked below.
        unsafe { Some(&*((attr as *const binding::TEE_Attribute) as *const Self)) }
    }

    // Whether the attribute is public (i.e., can be extracted from an object
    // regardless of its defined usage).
    pub fn is_public(&self) -> bool {
        self.id.public()
    }

    // Whether the attribute is given by value fields.
    pub fn is_value(&self) -> bool {
        self.id.value()
    }

    // Whether the attribute is given by a memory reference.
    pub fn is_memory_reference(&self) -> bool {
        self.id.memory_reference()
    }

    pub fn as_value(&self) -> &ValueFields {
        assert!(self.is_value());
        // SAFETY: The above assertion ensures that this variant is what is
        // 'live'.
        unsafe { &self.content.value }
    }

    pub fn as_memory_reference(&self) -> &MemRef {
        assert!(self.is_memory_reference());
        // SAFETY: The above assertion ensures that this variant is what is
        // 'live'.
        unsafe { &self.content.memref }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectInfo {
    pub object_type: Type,
    pub object_size: u32,
    pub max_object_size: u32,
    pub object_usage: Usage,
    pub data_size: usize,
    pub data_position: usize,
    pub handle_flags: HandleFlags,
}

output_parameter!(ObjectInfo, binding::TEE_ObjectInfo);

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum Whence {
    DataSeekSet = binding::TEE_DATA_SEEK_SET,
    DataSeekCur = binding::TEE_DATA_SEEK_CUR,
    DataSeekEnd = binding::TEE_DATA_SEEK_END,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum Storage {
    Private = binding::TEE_STORAGE_PRIVATE,
    Perso = binding::TEE_STORAGE_PERSO,
    Protected = binding::TEE_STORAGE_PROTECTED,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage(u32);

bitflags! {
    impl Usage : u32 {
        const EXTRACTABLE = binding::TEE_USAGE_EXTRACTABLE;
        const ENCRYPT = binding::TEE_USAGE_ENCRYPT;
        const DECRYPT = binding::TEE_USAGE_DECRYPT;
        const MAC = binding::TEE_USAGE_MAC;
        const SIGN = binding::TEE_USAGE_SIGN;
        const VERIFY = binding::TEE_USAGE_VERIFY;
        const DERIVE = binding::TEE_USAGE_DERIVE;
  }
}

impl Usage {
    pub fn default() -> Usage {
        Usage::from_bits_retain(0xffffffff)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandleFlags(u32);

bitflags! {
    impl HandleFlags : u32 {
        const PERSISTENT = binding::TEE_HANDLE_FLAG_PERSISTENT;
        const INITIALIZED = binding::TEE_HANDLE_FLAG_INITIALIZED;
        const KEY_SET = binding::TEE_HANDLE_FLAG_KEY_SET;
        const EXPECT_TWO_KEYS = binding::TEE_HANDLE_FLAG_EXPECT_TWO_KEYS;
        const EXTRACTING = binding::TEE_HANDLE_FLAG_EXTRACTING;

        // Only valid for persistent objects.
        const DATA_ACCESS_READ = binding::TEE_DATA_FLAG_ACCESS_READ;
        const DATA_ACCESS_WRITE = binding::TEE_DATA_FLAG_ACCESS_WRITE;
        const DATA_ACCESS_WRITE_META = binding::TEE_DATA_FLAG_ACCESS_WRITE_META;
        const DATA_SHARE_READ = binding::TEE_DATA_FLAG_SHARE_READ;
        const DATA_SHARE_WRITE = binding::TEE_DATA_FLAG_SHARE_WRITE;
        const DATA_FLAG_OVERWRITE = binding::TEE_DATA_FLAG_OVERWRITE;
  }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Cipher = binding::TEE_OPERATION_CIPHER,
    Mac = binding::TEE_OPERATION_MAC,
    Ae = binding::TEE_OPERATION_AE,
    Digest = binding::TEE_OPERATION_DIGEST,
    AsymmetricCipher = binding::TEE_OPERATION_ASYMMETRIC_CIPHER,
    AsymmetricSignature = binding::TEE_OPERATION_ASYMMETRIC_SIGNATURE,
    KeyDerivation = binding::TEE_OPERATION_KEY_DERIVATION,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationState {
    Initial = binding::TEE_OPERATION_STATE_INITIAL,
    Active = binding::TEE_OPERATION_STATE_ACTIVE,
    Extracting = binding::TEE_OPERATION_STATE_EXTRACTING,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum Type {
    Aes = binding::TEE_TYPE_AES,
    Des = binding::TEE_TYPE_DES,
    Des3 = binding::TEE_TYPE_DES3,
    Md5 = binding::TEE_TYPE_HMAC_MD5,
    HmacSha1 = binding::TEE_TYPE_HMAC_SHA1,
    HmacSha224 = binding::TEE_TYPE_HMAC_SHA224,
    HmacSha256 = binding::TEE_TYPE_HMAC_SHA256,
    HmacSha384 = binding::TEE_TYPE_HMAC_SHA384,
    HmacSha512 = binding::TEE_TYPE_HMAC_SHA512,
    HmacSm3 = binding::TEE_TYPE_HMAC_SM3,
    HmacSha3_224 = binding::TEE_TYPE_HMAC_SHA3_224,
    HmacSha3_256 = binding::TEE_TYPE_HMAC_SHA3_256,
    HmacSha3_384 = binding::TEE_TYPE_HMAC_SHA3_384,
    HmacSha3_512 = binding::TEE_TYPE_HMAC_SHA3_512,
    RsaPublicKey = binding::TEE_TYPE_RSA_PUBLIC_KEY,
    RsaKeypair = binding::TEE_TYPE_RSA_KEYPAIR,
    DsaPublicKey = binding::TEE_TYPE_DSA_PUBLIC_KEY,
    DsaKeypair = binding::TEE_TYPE_DSA_KEYPAIR,
    DhKeypair = binding::TEE_TYPE_DH_KEYPAIR,
    EcdsaPublicKey = binding::TEE_TYPE_ECDSA_PUBLIC_KEY,
    EcdsaKeypair = binding::TEE_TYPE_ECDSA_KEYPAIR,
    EcdhPublicKey = binding::TEE_TYPE_ECDH_PUBLIC_KEY,
    EcdhKeypair = binding::TEE_TYPE_ECDH_KEYPAIR,
    Ed25519PublicKey = binding::TEE_TYPE_ED25519_PUBLIC_KEY,
    Ed25519Keypair = binding::TEE_TYPE_ED25519_KEYPAIR,
    X25519PublicKey = binding::TEE_TYPE_X25519_PUBLIC_KEY,
    X25519Keypair = binding::TEE_TYPE_X25519_KEYPAIR,
    Sm2DsaPublicKey = binding::TEE_TYPE_SM2_DSA_PUBLIC_KEY,
    Sm2DsaKeypair = binding::TEE_TYPE_SM2_DSA_KEYPAIR,
    Sm2KepPublicKey = binding::TEE_TYPE_SM2_KEP_PUBLIC_KEY,
    Sm2KepKeypair = binding::TEE_TYPE_SM2_KEP_KEYPAIR,
    Sm2PkePublicKey = binding::TEE_TYPE_SM2_PKE_PUBLIC_KEY,
    Sm2PkeKeypair = binding::TEE_TYPE_SM2_PKE_KEYPAIR,
    Sm4 = binding::TEE_TYPE_SM4,
    Hkdf = binding::TEE_TYPE_HKDF,
    GenericSecret = binding::TEE_TYPE_GENERIC_SECRET,
    CorruptedObject = binding::TEE_TYPE_CORRUPTED_OBJECT,
    Data = binding::TEE_TYPE_DATA,
}

//
// Cryptographic data types
//

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum Mode {
    Encrypt = binding::TEE_MODE_ENCRYPT,
    Decrypt = binding::TEE_MODE_DECRYPT,
    Sign = binding::TEE_MODE_SIGN,
    Verify = binding::TEE_MODE_VERIFY,
    Mac = binding::TEE_MODE_MAC,
    Digest = binding::TEE_MODE_DIGEST,
    Derive = binding::TEE_MODE_DERIVE,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OperationInfo {
    pub algorithm: Algorithm,
    pub operation_class: Operation,
    pub mode: Mode,
    pub digest_length: u32,
    pub max_key_size: u32,
    pub key_size: u32,
    pub required_key_usage: Usage,
    pub handle_state: HandleFlags,
}

output_parameter!(OperationInfo, binding::TEE_OperationInfo);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OperationInfoKey {
    pub key_size: u32,
    pub required_key_usage: Usage,
}

output_parameter!(OperationInfoKey, binding::TEE_OperationInfoKey);

#[repr(C)]
#[derive(Debug)]
pub struct OperationInfoMultiple {
    pub algorithm: Algorithm,
    pub operation_class: Operation,
    pub mode: Mode,
    pub digest_length: u32,
    pub max_key_size: u32,
    pub handle_state: HandleFlags,
    pub operation_state: OperationState,
    pub number_of_keys: u32,
    pub key_information: binding::__IncompleteArrayField<OperationInfoKey>,
}

output_parameter!(OperationInfoMultiple, binding::TEE_OperationInfoMultiple);

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum Algorithm {
    AesEcbNopad = binding::TEE_ALG_AES_ECB_NOPAD,
    AesCbcNopad = binding::TEE_ALG_AES_CBC_NOPAD,
    AesCtr = binding::TEE_ALG_AES_CTR,
    AesCts = binding::TEE_ALG_AES_CTS,
    AesXts = binding::TEE_ALG_AES_XTS,
    AesCbcMacNopad = binding::TEE_ALG_AES_CBC_MAC_NOPAD,
    AesCbcMacPkcs5 = binding::TEE_ALG_AES_CBC_MAC_PKCS5,
    AesCmac = binding::TEE_ALG_AES_CMAC,
    AesCcm = binding::TEE_ALG_AES_CCM,
    AesGcm = binding::TEE_ALG_AES_GCM,
    DesEcbNopad = binding::TEE_ALG_DES_ECB_NOPAD,
    DesCbcNopad = binding::TEE_ALG_DES_CBC_NOPAD,
    DesCbcMacNopad = binding::TEE_ALG_DES_CBC_MAC_NOPAD,
    DesCbcMacPkcs5 = binding::TEE_ALG_DES_CBC_MAC_PKCS5,
    Des3EcbNopad = binding::TEE_ALG_DES3_ECB_NOPAD,
    Des3CbcNopad = binding::TEE_ALG_DES3_CBC_NOPAD,
    Des3CbcMacNopad = binding::TEE_ALG_DES3_CBC_MAC_NOPAD,
    Des3CbcMacPkcs5 = binding::TEE_ALG_DES3_CBC_MAC_PKCS5,
    RsassaPkcs1V1_5Md5 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_MD5,
    RsassaPkcs1V1_5Sha1 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA1,
    RsassaPkcs1V1_5Sha224 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA224,
    RsassaPkcs1V1_5Sha256 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA256,
    RsassaPkcs1V1_5Sha384 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA384,
    RsassaPkcs1V1_5Sha512 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA512,
    RsassaPkcs1V1_5Sha3_224 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA3_224,
    RsassaPkcs1V1_5Sha3_256 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA3_256,
    RsassaPkcs1V1_5Sha3_384 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA3_384,
    RsassaPkcs1V1_5Sha3_512 = binding::TEE_ALG_RSASSA_PKCS1_V1_5_SHA3_512,
    RsassaPkcs1PssMgf1Sha1 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA1,
    RsassaPkcs1PssMgf1Sha224 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA224,
    RsassaPkcs1PssMgf1Sha256 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA256,
    RsassaPkcs1PssMgf1Sha384 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA384,
    RsassaPkcs1PssMgf1Sha512 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA512,
    RsassaPkcs1PssMgf1Sha3_224 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA3_224,
    RsassaPkcs1PssMgf1Sha3_256 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA3_256,
    RsassaPkcs1PssMgf1Sha3_384 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA3_384,
    RsassaPkcs1PssMgf1Sha3_512 = binding::TEE_ALG_RSASSA_PKCS1_PSS_MGF1_SHA3_512,
    RsaesPkcs1V1_5 = binding::TEE_ALG_RSAES_PKCS1_V1_5,
    RsaesPkcs1OaepMgf1Sha1 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA1,
    RsaesPkcs1OaepMgf1Sha224 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA224,
    RsaesPkcs1OaepMgf1Sha256 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA256,
    RsaesPkcs1OaepMgf1Sha384 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA384,
    RsaesPkcs1OaepMgf1Sha512 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA512,
    RsaesPkcs1OaepMgf1Sha3_224 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA3_224,
    RsaesPkcs1OaepMgf1Sha3_256 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA3_256,
    RsaesPkcs1OaepMgf1Sha3_384 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA3_384,
    RsaesPkcs1OaepMgf1Sha3_512 = binding::TEE_ALG_RSAES_PKCS1_OAEP_MGF1_SHA3_512,
    RsaNopad = binding::TEE_ALG_RSA_NOPAD,
    DsaSha1 = binding::TEE_ALG_DSA_SHA1,
    DsaSha224 = binding::TEE_ALG_DSA_SHA224,
    DsaSha256 = binding::TEE_ALG_DSA_SHA256,
    DsaSha3_224 = binding::TEE_ALG_DSA_SHA3_224,
    DsaSha3_256 = binding::TEE_ALG_DSA_SHA3_256,
    DsaSha3_384 = binding::TEE_ALG_DSA_SHA3_384,
    DsaSha3_512 = binding::TEE_ALG_DSA_SHA3_512,
    DhDeriveSharedSecret = binding::TEE_ALG_DH_DERIVE_SHARED_SECRET,
    Md5 = binding::TEE_ALG_MD5,
    Sha1 = binding::TEE_ALG_SHA1,
    Sha224 = binding::TEE_ALG_SHA224,
    Sha256 = binding::TEE_ALG_SHA256,
    Sha384 = binding::TEE_ALG_SHA384,
    Sha512 = binding::TEE_ALG_SHA512,
    Sha3_224 = binding::TEE_ALG_SHA3_224,
    Sha3_256 = binding::TEE_ALG_SHA3_256,
    Sha3_384 = binding::TEE_ALG_SHA3_384,
    Sha3_512 = binding::TEE_ALG_SHA3_512,
    HmacMd5 = binding::TEE_ALG_HMAC_MD5,
    HmacSha1 = binding::TEE_ALG_HMAC_SHA1,
    HmacSha224 = binding::TEE_ALG_HMAC_SHA224,
    HmacSha256 = binding::TEE_ALG_HMAC_SHA256,
    HmacSha384 = binding::TEE_ALG_HMAC_SHA384,
    HmacSha512 = binding::TEE_ALG_HMAC_SHA512,
    HmacSha3_224 = binding::TEE_ALG_HMAC_SHA3_224,
    HmacSha3_256 = binding::TEE_ALG_HMAC_SHA3_256,
    HmacSha3_384 = binding::TEE_ALG_HMAC_SHA3_384,
    HmacSha3_512 = binding::TEE_ALG_HMAC_SHA3_512,
    Hkdf = binding::TEE_ALG_HKDF,
    Shake128 = binding::TEE_ALG_SHAKE128,
    Shake256 = binding::TEE_ALG_SHAKE256,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum EccCurve {
    None = binding::TEE_CRYPTO_ELEMENT_NONE,
    NistP192 = binding::TEE_ECC_CURVE_NIST_P192,
    NistP224 = binding::TEE_ECC_CURVE_NIST_P224,
    NistP256 = binding::TEE_ECC_CURVE_NIST_P256,
    NistP384 = binding::TEE_ECC_CURVE_NIST_P384,
    NistP521 = binding::TEE_ECC_CURVE_NIST_P521,
    BsiP160r1 = binding::TEE_ECC_CURVE_BSI_P160r1,
    BsiP192r1 = binding::TEE_ECC_CURVE_BSI_P192r1,
    BsiP224r1 = binding::TEE_ECC_CURVE_BSI_P224r1,
    BsiP256r1 = binding::TEE_ECC_CURVE_BSI_P256r1,
    BsiP320r1 = binding::TEE_ECC_CURVE_BSI_P320r1,
    BsiP384r1 = binding::TEE_ECC_CURVE_BSI_P384r1,
    BsiP512r1 = binding::TEE_ECC_CURVE_BSI_P512r1,
    BsiP160t1 = binding::TEE_ECC_CURVE_BSI_P160t1,
    BsiP192t1 = binding::TEE_ECC_CURVE_BSI_P192t1,
    BsiP224t1 = binding::TEE_ECC_CURVE_BSI_P224t1,
    BsiP256t1 = binding::TEE_ECC_CURVE_BSI_P256t1,
    BsiP320t1 = binding::TEE_ECC_CURVE_BSI_P320t1,
    BsiP384t1 = binding::TEE_ECC_CURVE_BSI_P384t1,
    BsiP512t1 = binding::TEE_ECC_CURVE_BSI_P512t1,
    _448 = binding::TEE_ECC_CURVE_448,
    Sm2 = binding::TEE_ECC_CURVE_SM2,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, FromPrimitive, PartialEq)]
pub enum AttributeId {
    SecretValue = binding::TEE_ATTR_SECRET_VALUE,
    RsaModulus = binding::TEE_ATTR_RSA_MODULUS,
    RsaPublicExponent = binding::TEE_ATTR_RSA_PUBLIC_EXPONENT,
    RsaPrivateExponent = binding::TEE_ATTR_RSA_PRIVATE_EXPONENT,
    RsaPrime1 = binding::TEE_ATTR_RSA_PRIME1,
    RsaPrime2 = binding::TEE_ATTR_RSA_PRIME2,
    RsaExponent1 = binding::TEE_ATTR_RSA_EXPONENT1,
    RsaExponent2 = binding::TEE_ATTR_RSA_EXPONENT2,
    RsaCoefficient = binding::TEE_ATTR_RSA_COEFFICIENT,
    DsaPrime = binding::TEE_ATTR_DSA_PRIME,
    DsaSubprimme = binding::TEE_ATTR_DSA_SUBPRIME,
    DsaBase = binding::TEE_ATTR_DSA_BASE,
    DsaPublicValue = binding::TEE_ATTR_DSA_PUBLIC_VALUE,
    DsaPrivateValue = binding::TEE_ATTR_DSA_PRIVATE_VALUE,
    DhPrime = binding::TEE_ATTR_DH_PRIME,
    DhSubprime = binding::TEE_ATTR_DH_SUBPRIME,
    DhBase = binding::TEE_ATTR_DH_BASE,
    DhXBits = binding::TEE_ATTR_DH_X_BITS,
    DhPublicValue = binding::TEE_ATTR_DH_PUBLIC_VALUE,
    DhPrivateValue = binding::TEE_ATTR_DH_PRIVATE_VALUE,
    RsaOaepLabel = binding::TEE_ATTR_RSA_OAEP_LABEL,
    RsaOaepMgfHash = binding::TEE_ATTR_RSA_OAEP_MGF_HASH,
    RsaPssSaltLength = binding::TEE_ATTR_RSA_PSS_SALT_LENGTH,
    EccPublicValueX = binding::TEE_ATTR_ECC_PUBLIC_VALUE_X,
    EccPublicValueY = binding::TEE_ATTR_ECC_PUBLIC_VALUE_Y,
    EccPrivateValue = binding::TEE_ATTR_ECC_PRIVATE_VALUE,
    EccEphemeralPublicValueX = binding::TEE_ATTR_ECC_EPHEMERAL_PUBLIC_VALUE_X,
    EccEphemeralPublicValueY = binding::TEE_ATTR_ECC_EPHEMERAL_PUBLIC_VALUE_Y,
    EccCurve = binding::TEE_ATTR_ECC_CURVE,
    EddsaCtx = binding::TEE_ATTR_EDDSA_CTX,
    Ed25519PublicValue = binding::TEE_ATTR_ED25519_PUBLIC_VALUE,
    Ed25519PrivateValue = binding::TEE_ATTR_ED25519_PRIVATE_VALUE,
    X25519PublicValue = binding::TEE_ATTR_X25519_PUBLIC_VALUE,
    X25519PrivateValue = binding::TEE_ATTR_X25519_PRIVATE_VALUE,
    Ed448PublicValue = binding::TEE_ATTR_ED448_PUBLIC_VALUE,
    Ed448PrivateValue = binding::TEE_ATTR_ED448_PRIVATE_VALUE,
    EddsaPrehash = binding::TEE_ATTR_EDDSA_PREHASH,
    X448PublicValue = binding::TEE_ATTR_X448_PUBLIC_VALUE,
    X448PrivateValue = binding::TEE_ATTR_X448_PRIVATE_VALUE,
    Sm2IdInitiator = binding::TEE_ATTR_SM2_ID_INITIATOR,
    Sm2IdResponder = binding::TEE_ATTR_SM2_ID_RESPONDER,
    Sm2KepUser = binding::TEE_ATTR_SM2_KEP_USER,
    Sm2KepConfirmationIn = binding::TEE_ATTR_SM2_KEP_CONFIRMATION_IN,
    Sm2KepConfirmationOut = binding::TEE_ATTR_SM2_KEP_CONFIRMATION_OUT,
    HkdfSalt = binding::TEE_ATTR_HKDF_SALT,
    HkdfInfo = binding::TEE_ATTR_HKDF_INFO,
    HkdfHashAlgorithm = binding::TEE_ATTR_HKDF_HASH_ALGORITHM,
    KdfKeySize = binding::TEE_ATTR_KDF_KEY_SIZE,
}

impl AttributeId {
    // Whether the ID represents an attribute that is public (i.e., can be
    // extracted from an object regardless of its defined usage).
    pub fn public(self) -> bool {
        (self as u32) & binding::TEE_ATTR_FLAG_PUBLIC != 0
    }

    // Whether the ID represents a value attribute.
    pub fn value(self) -> bool {
        (self as u32) & binding::TEE_ATTR_FLAG_VALUE != 0
    }

    // Whether the ID represents a memory reference attribute.
    pub fn memory_reference(self) -> bool {
        !self.value()
    }
}

//
// Time data types
//

pub type Time = binding::TEE_Time;

#[cfg(test)]
pub mod tests {
    use super::*;

    use binding::{
        TEE_Attribute, TEE_Attribute__bindgen_ty_1, TEE_Attribute__bindgen_ty_1__bindgen_ty_1,
        TEE_Attribute__bindgen_ty_1__bindgen_ty_2, TEE_Identity, TEE_ObjectEnumHandle,
        TEE_ObjectHandle, TEE_ObjectInfo, TEE_OperationHandle, TEE_OperationInfo,
        TEE_OperationInfoKey, TEE_OperationInfoMultiple, TEE_Param, TEE_Param__bindgen_ty_1,
        TEE_Param__bindgen_ty_2, TEE_PropSetHandle, TEE_Result, TEE_TASessionHandle, TEE_UUID,
    };
    use std::mem::{align_of, align_of_val, offset_of, size_of, size_of_val};
    use std::ptr::addr_of;

    const EMPTY_TEE_UUID: TEE_UUID =
        TEE_UUID { timeLow: 0, timeMid: 0, timeHiAndVersion: 0, clockSeqAndNode: [0; 8] };

    #[test]
    pub fn test_param_types() {
        let types = ParamTypes::from_types([
            ParamType::ValueInout,
            ParamType::MemrefInput,
            ParamType::None,
            ParamType::ValueOutput,
        ]);
        assert_eq!(types.get(0), ParamType::ValueInout);
        assert_eq!(types.get(1), ParamType::MemrefInput);
        assert_eq!(types.get(2), ParamType::None);
        assert_eq!(types.get(3), ParamType::ValueOutput);

        assert_eq!(3u32 | 5u32 << 4 | 2u32 << 12, types.as_u32());
    }

    #[test]
    pub fn test_abi_compat_ta_session_handle() {
        assert_eq!(size_of::<TaSessionHandle>(), size_of::<TEE_TASessionHandle>());
        assert_eq!(align_of::<TaSessionHandle>(), align_of::<TEE_TASessionHandle>());

        let new = TaSessionHandle::from_value(0);
        let old: TEE_TASessionHandle = std::ptr::null_mut();

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*TaSessionHandle::from_binding(&old)) as usize, addr_of!(old) as usize);
        assert_eq!(addr_of!(*TaSessionHandle::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_prop_set_handle() {
        assert_eq!(size_of::<PropSetHandle>(), size_of::<TEE_PropSetHandle>());
        assert_eq!(align_of::<PropSetHandle>(), align_of::<TEE_PropSetHandle>());

        let new = PropSetHandle::from_value(0);
        let old: TEE_PropSetHandle = std::ptr::null_mut();

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*PropSetHandle::from_binding(&old)) as usize, addr_of!(old) as usize);
        assert_eq!(addr_of!(*PropSetHandle::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_object_handle() {
        assert_eq!(size_of::<ObjectHandle>(), size_of::<TEE_ObjectHandle>());
        assert_eq!(align_of::<ObjectHandle>(), align_of::<TEE_ObjectHandle>());

        let new = ObjectHandle::from_value(0);
        let old: TEE_ObjectHandle = std::ptr::null_mut();

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*ObjectHandle::from_binding(&old)) as usize, addr_of!(old) as usize);
        assert_eq!(addr_of!(*ObjectHandle::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_object_enum_handle() {
        assert_eq!(size_of::<ObjectEnumHandle>(), size_of::<TEE_ObjectEnumHandle>());
        assert_eq!(align_of::<ObjectEnumHandle>(), align_of::<TEE_ObjectEnumHandle>());

        let new = ObjectEnumHandle::from_value(0);
        let old: TEE_ObjectEnumHandle = std::ptr::null_mut();

        // Verify zero-copy translation.
        assert_eq!(
            addr_of!(*ObjectEnumHandle::from_binding(&old)) as usize,
            addr_of!(old) as usize
        );
        assert_eq!(addr_of!(*ObjectEnumHandle::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_operation_handle() {
        assert_eq!(size_of::<OperationHandle>(), size_of::<TEE_OperationHandle>());
        assert_eq!(align_of::<OperationHandle>(), align_of::<TEE_OperationHandle>());

        let new = OperationHandle::from_value(0);
        let old: TEE_OperationHandle = std::ptr::null_mut();

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*OperationHandle::from_binding(&old)) as usize, addr_of!(old) as usize);
        assert_eq!(addr_of!(*OperationHandle::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_result() {
        assert_eq!(size_of::<Error>(), size_of::<TEE_Result>());
        assert_eq!(align_of::<Error>(), align_of::<TEE_Result>());
    }

    #[test]
    pub fn test_abi_compat_uuid() {
        assert_eq!(size_of::<Uuid>(), size_of::<TEE_UUID>());
        assert_eq!(align_of::<Uuid>(), align_of::<TEE_UUID>());

        let new = Uuid::default();
        let old = EMPTY_TEE_UUID;

        assert_eq!(offset_of!(Uuid, time_low), offset_of!(TEE_UUID, timeLow));
        assert_eq!(size_of_val(&new.time_low), size_of_val(&old.timeLow));
        assert_eq!(align_of_val(&new.time_low), align_of_val(&old.timeLow));

        assert_eq!(offset_of!(Uuid, time_mid), offset_of!(TEE_UUID, timeMid));
        assert_eq!(size_of_val(&new.time_mid), size_of_val(&old.timeMid));
        assert_eq!(align_of_val(&new.time_mid), align_of_val(&old.timeMid));

        assert_eq!(offset_of!(Uuid, time_hi_and_version), offset_of!(TEE_UUID, timeHiAndVersion));
        assert_eq!(size_of_val(&new.time_hi_and_version), size_of_val(&old.timeHiAndVersion));
        assert_eq!(align_of_val(&new.time_hi_and_version), align_of_val(&old.timeHiAndVersion));

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*Uuid::from_binding(&old)) as usize, addr_of!(old) as usize);
        assert_eq!(addr_of!(*Uuid::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_identity() {
        assert_eq!(size_of::<Identity>(), size_of::<TEE_Identity>());
        assert_eq!(align_of::<Identity>(), align_of::<TEE_Identity>());

        let new = Identity { login: Login::TrustedApp, uuid: Uuid::default() };
        let old = TEE_Identity { login: 0, uuid: EMPTY_TEE_UUID };

        assert_eq!(offset_of!(Identity, login), offset_of!(TEE_Identity, login));
        assert_eq!(size_of_val(&new.login), size_of_val(&old.login));
        assert_eq!(align_of_val(&new.login), align_of_val(&old.login));

        assert_eq!(offset_of!(Identity, uuid), offset_of!(TEE_Identity, uuid));
        assert_eq!(size_of_val(&new.uuid), size_of_val(&old.uuid));
        assert_eq!(align_of_val(&new.uuid), align_of_val(&old.uuid));

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*Identity::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_param() {
        assert_eq!(size_of::<Param>(), size_of::<TEE_Param>());
        assert_eq!(align_of::<Param>(), align_of::<TEE_Param>());

        assert_eq!(size_of::<MemRef>(), size_of::<TEE_Param__bindgen_ty_1>());
        assert_eq!(align_of::<MemRef>(), align_of::<TEE_Param__bindgen_ty_1>());

        assert_eq!(size_of::<ValueFields>(), size_of::<TEE_Param__bindgen_ty_2>());
        assert_eq!(align_of::<ValueFields>(), align_of::<TEE_Param__bindgen_ty_2>());

        let new = Param { value: ValueFields { a: 0, b: 0 } };
        let old = TEE_Param { value: TEE_Param__bindgen_ty_2 { a: 0, b: 0 } };

        // SAFETY:  Accessing the fields of a repr(C) union is unsafe.
        let new_memref = unsafe { new.memref };
        let old_memref = unsafe { old.memref };

        assert_eq!(offset_of!(MemRef, buffer), offset_of!(TEE_Param__bindgen_ty_1, buffer));
        assert_eq!(size_of_val(&new_memref.buffer), size_of_val(&old_memref.buffer));
        assert_eq!(align_of_val(&new_memref.buffer), align_of_val(&old_memref.buffer));

        assert_eq!(offset_of!(MemRef, size), offset_of!(TEE_Param__bindgen_ty_1, size));
        assert_eq!(size_of_val(&new_memref.size), size_of_val(&old_memref.size));
        assert_eq!(align_of_val(&new_memref.size), align_of_val(&old_memref.size));

        // SAFETY:  Accessing the fields of a repr(C) union is unsafe.
        let new_value = unsafe { new.value };
        let old_value = unsafe { old.value };

        assert_eq!(offset_of!(ValueFields, a), offset_of!(TEE_Param__bindgen_ty_2, a));
        assert_eq!(size_of_val(&new_value.a), size_of_val(&old_value.a));
        assert_eq!(align_of_val(&new_value.a), align_of_val(&old_value.a));

        assert_eq!(offset_of!(ValueFields, b), offset_of!(TEE_Param__bindgen_ty_2, b));
        assert_eq!(size_of_val(&new_value.b), size_of_val(&old_value.b));
        assert_eq!(align_of_val(&new_value.b), align_of_val(&old_value.b));

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*Param::from_binding(&old)) as usize, addr_of!(old) as usize);
        assert_eq!(addr_of!(*Param::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_param_types() {
        assert_eq!(size_of::<ParamTypes>(), size_of::<u32>());
        assert_eq!(align_of::<ParamTypes>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_login() {
        assert_eq!(size_of::<Login>(), size_of::<u32>());
        assert_eq!(align_of::<Login>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_origin() {
        assert_eq!(size_of::<Origin>(), size_of::<u32>());
        assert_eq!(align_of::<Origin>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_memory_access() {
        assert_eq!(size_of::<MemoryAccess>(), size_of::<u32>());
        assert_eq!(align_of::<MemoryAccess>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_session_context() {
        assert_eq!(size_of::<SessionContext>(), size_of::<usize>());
        assert_eq!(align_of::<SessionContext>(), align_of::<usize>());

        let new = SessionContext::from_value(0);
        let old: usize = 0;

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*SessionContext::from_binding(&old)) as usize, addr_of!(old) as usize);
        assert_eq!(addr_of!(*SessionContext::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_attribute() {
        assert_eq!(size_of::<Attribute>(), size_of::<TEE_Attribute>());
        assert_eq!(align_of::<Attribute>(), align_of::<TEE_Attribute>());

        let new = Attribute {
            id: AttributeId::SecretValue,
            content: BufferOrValue { value: ValueFields { a: 0, b: 0 } },
        };
        let old = TEE_Attribute {
            attributeID: AttributeId::SecretValue as u32,
            __bindgen_padding_0: [0; 4],
            content: TEE_Attribute__bindgen_ty_1 {
                value: TEE_Attribute__bindgen_ty_1__bindgen_ty_2 { a: 0, b: 0 },
            },
        };

        assert_eq!(offset_of!(Attribute, id), offset_of!(TEE_Attribute, attributeID));
        assert_eq!(size_of_val(&new.id), size_of_val(&old.attributeID));
        assert_eq!(align_of_val(&new.id), align_of_val(&old.attributeID));

        assert_eq!(offset_of!(Attribute, content), offset_of!(TEE_Attribute, content));
        assert_eq!(size_of_val(&new.content), size_of_val(&old.content));
        assert_eq!(align_of_val(&new.content), align_of_val(&old.content));

        // SAFETY:  Accessing the fields of a repr(C) union is unsafe.
        let new_memref = unsafe { new.content.memref };
        let old_content_memref = unsafe { old.content.ref_ };

        assert_eq!(
            offset_of!(MemRef, buffer),
            offset_of!(TEE_Attribute__bindgen_ty_1__bindgen_ty_1, buffer)
        );
        assert_eq!(size_of_val(&new_memref.buffer), size_of_val(&old_content_memref.buffer));
        assert_eq!(align_of_val(&new_memref.buffer), align_of_val(&old_content_memref.buffer));

        assert_eq!(
            offset_of!(MemRef, size),
            offset_of!(TEE_Attribute__bindgen_ty_1__bindgen_ty_1, length)
        );
        assert_eq!(size_of_val(&new_memref.size), size_of_val(&old_content_memref.length));
        assert_eq!(align_of_val(&new_memref.size), align_of_val(&old_content_memref.length));

        // SAFETY:  Accessing the fields of a repr(C) union is unsafe.
        let new_value = unsafe { new.content.value };
        let old_value = unsafe { old.content.value };

        assert_eq!(
            offset_of!(ValueFields, a),
            offset_of!(TEE_Attribute__bindgen_ty_1__bindgen_ty_2, a)
        );
        assert_eq!(size_of_val(&new_value.a), size_of_val(&old_value.a));
        assert_eq!(align_of_val(&new_value.a), align_of_val(&old_value.a));

        assert_eq!(
            offset_of!(ValueFields, b),
            offset_of!(TEE_Attribute__bindgen_ty_1__bindgen_ty_2, b)
        );
        assert_eq!(size_of_val(&new_value.b), size_of_val(&old_value.b));
        assert_eq!(align_of_val(&new_value.b), align_of_val(&old_value.b));

        // Verify zero-copy translation.
        assert_eq!(
            addr_of!(*Attribute::from_binding(&old).unwrap()) as usize,
            addr_of!(old) as usize
        );
        assert_eq!(addr_of!(*Attribute::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_object_info() {
        assert_eq!(size_of::<ObjectInfo>(), size_of::<TEE_ObjectInfo>());
        assert_eq!(align_of::<ObjectInfo>(), align_of::<TEE_ObjectInfo>());

        let new = ObjectInfo {
            object_type: Type::Aes,
            object_size: 0,
            max_object_size: 0,
            object_usage: Usage::empty(),
            data_size: 0,
            data_position: 0,
            handle_flags: HandleFlags::empty(),
        };
        let old = TEE_ObjectInfo {
            objectType: 0,
            objectSize: 0,
            maxObjectSize: 0,
            objectUsage: 0,
            dataSize: 0,
            dataPosition: 0,
            handleFlags: 0,
            __bindgen_padding_0: [0; 4],
        };

        assert_eq!(offset_of!(ObjectInfo, object_type), offset_of!(TEE_ObjectInfo, objectType));
        assert_eq!(size_of_val(&new.object_type), size_of_val(&old.objectType));
        assert_eq!(align_of_val(&new.object_type), align_of_val(&old.objectType));

        assert_eq!(offset_of!(ObjectInfo, object_size), offset_of!(TEE_ObjectInfo, objectSize));
        assert_eq!(size_of_val(&new.object_size), size_of_val(&old.objectSize));
        assert_eq!(align_of_val(&new.object_size), align_of_val(&old.objectSize));

        assert_eq!(
            offset_of!(ObjectInfo, max_object_size),
            offset_of!(TEE_ObjectInfo, maxObjectSize)
        );
        assert_eq!(size_of_val(&new.max_object_size), size_of_val(&old.maxObjectSize));
        assert_eq!(align_of_val(&new.max_object_size), align_of_val(&old.maxObjectSize));

        assert_eq!(offset_of!(ObjectInfo, object_usage), offset_of!(TEE_ObjectInfo, objectUsage));
        assert_eq!(size_of_val(&new.object_usage), size_of_val(&old.objectUsage));
        assert_eq!(align_of_val(&new.object_usage), align_of_val(&old.objectUsage));

        assert_eq!(offset_of!(ObjectInfo, data_size), offset_of!(TEE_ObjectInfo, dataSize));
        assert_eq!(size_of_val(&new.data_size), size_of_val(&old.dataSize));
        assert_eq!(align_of_val(&new.data_size), align_of_val(&old.dataSize));

        assert_eq!(offset_of!(ObjectInfo, data_position), offset_of!(TEE_ObjectInfo, dataPosition));
        assert_eq!(size_of_val(&new.data_position), size_of_val(&old.dataPosition));
        assert_eq!(align_of_val(&new.data_position), align_of_val(&old.dataPosition));

        assert_eq!(offset_of!(ObjectInfo, handle_flags), offset_of!(TEE_ObjectInfo, handleFlags));
        assert_eq!(size_of_val(&new.handle_flags), size_of_val(&old.handleFlags));
        assert_eq!(align_of_val(&new.handle_flags), align_of_val(&old.handleFlags));

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*ObjectInfo::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_whence() {
        assert_eq!(size_of::<Whence>(), size_of::<u32>());
        assert_eq!(align_of::<Whence>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_storage() {
        assert_eq!(size_of::<Storage>(), size_of::<u32>());
        assert_eq!(align_of::<Storage>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_usage() {
        assert_eq!(size_of::<Usage>(), size_of::<u32>());
        assert_eq!(align_of::<Usage>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_handle_flags() {
        assert_eq!(size_of::<HandleFlags>(), size_of::<u32>());
        assert_eq!(align_of::<HandleFlags>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_operation() {
        assert_eq!(size_of::<Operation>(), size_of::<u32>());
        assert_eq!(align_of::<Operation>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_operation_state() {
        assert_eq!(size_of::<OperationState>(), size_of::<u32>());
        assert_eq!(align_of::<OperationState>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_type() {
        assert_eq!(size_of::<Type>(), size_of::<u32>());
        assert_eq!(align_of::<Type>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_mode() {
        assert_eq!(size_of::<Mode>(), size_of::<u32>());
        assert_eq!(align_of::<Mode>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_operation_info() {
        assert_eq!(size_of::<OperationInfo>(), size_of::<TEE_OperationInfo>());
        assert_eq!(align_of::<OperationInfo>(), align_of::<TEE_OperationInfo>());

        let new = OperationInfo {
            algorithm: Algorithm::AesEcbNopad,
            operation_class: Operation::Ae,
            mode: Mode::Decrypt,
            digest_length: 0,
            max_key_size: 0,
            key_size: 0,
            required_key_usage: Usage::empty(),
            handle_state: HandleFlags::empty(),
        };
        let old = TEE_OperationInfo {
            algorithm: 0,
            operationClass: 0,
            mode: 0,
            digestLength: 0,
            maxKeySize: 0,
            keySize: 0,
            requiredKeyUsage: 0,
            handleState: 0,
        };

        assert_eq!(offset_of!(OperationInfo, algorithm), offset_of!(TEE_OperationInfo, algorithm));
        assert_eq!(size_of_val(&new.algorithm), size_of_val(&old.algorithm));
        assert_eq!(align_of_val(&new.algorithm), align_of_val(&old.algorithm));

        assert_eq!(
            offset_of!(OperationInfo, operation_class),
            offset_of!(TEE_OperationInfo, operationClass)
        );
        assert_eq!(size_of_val(&new.operation_class), size_of_val(&old.operationClass));
        assert_eq!(align_of_val(&new.operation_class), align_of_val(&old.operationClass));

        assert_eq!(offset_of!(OperationInfo, mode), offset_of!(TEE_OperationInfo, mode));
        assert_eq!(size_of_val(&new.mode), size_of_val(&old.mode));
        assert_eq!(align_of_val(&new.mode), align_of_val(&old.mode));

        assert_eq!(
            offset_of!(OperationInfo, digest_length),
            offset_of!(TEE_OperationInfo, digestLength)
        );
        assert_eq!(size_of_val(&new.digest_length), size_of_val(&old.digestLength));
        assert_eq!(align_of_val(&new.digest_length), align_of_val(&old.digestLength));

        assert_eq!(
            offset_of!(OperationInfo, max_key_size),
            offset_of!(TEE_OperationInfo, maxKeySize)
        );
        assert_eq!(size_of_val(&new.max_key_size), size_of_val(&old.maxKeySize));
        assert_eq!(align_of_val(&new.max_key_size), align_of_val(&old.maxKeySize));

        assert_eq!(offset_of!(OperationInfo, key_size), offset_of!(TEE_OperationInfo, keySize));
        assert_eq!(size_of_val(&new.key_size), size_of_val(&old.keySize));
        assert_eq!(align_of_val(&new.key_size), align_of_val(&old.keySize));

        assert_eq!(
            offset_of!(OperationInfo, required_key_usage),
            offset_of!(TEE_OperationInfo, requiredKeyUsage)
        );
        assert_eq!(size_of_val(&new.required_key_usage), size_of_val(&old.requiredKeyUsage));
        assert_eq!(align_of_val(&new.required_key_usage), align_of_val(&old.requiredKeyUsage));

        assert_eq!(
            offset_of!(OperationInfo, handle_state),
            offset_of!(TEE_OperationInfo, handleState)
        );
        assert_eq!(size_of_val(&new.handle_state), size_of_val(&old.handleState));
        assert_eq!(align_of_val(&new.handle_state), align_of_val(&old.handleState));

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*OperationInfo::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_operation_info_key() {
        assert_eq!(size_of::<OperationInfoKey>(), size_of::<TEE_OperationInfoKey>());
        assert_eq!(align_of::<OperationInfoKey>(), align_of::<TEE_OperationInfoKey>());

        let new = OperationInfoKey { key_size: 0, required_key_usage: Usage::empty() };
        let old = TEE_OperationInfoKey { keySize: 0, requiredKeyUsage: 0 };

        assert_eq!(
            offset_of!(OperationInfoKey, key_size),
            offset_of!(TEE_OperationInfoKey, keySize)
        );
        assert_eq!(size_of_val(&new.key_size), size_of_val(&old.keySize));
        assert_eq!(align_of_val(&new.key_size), align_of_val(&old.keySize));

        assert_eq!(
            offset_of!(OperationInfoKey, required_key_usage),
            offset_of!(TEE_OperationInfoKey, requiredKeyUsage)
        );
        assert_eq!(size_of_val(&new.required_key_usage), size_of_val(&old.requiredKeyUsage));
        assert_eq!(align_of_val(&new.required_key_usage), align_of_val(&old.requiredKeyUsage));

        // Verify zero-copy translation.
        assert_eq!(addr_of!(*OperationInfoKey::to_binding(&new)) as usize, addr_of!(new) as usize);
    }

    #[test]
    pub fn test_abi_compat_operation_info_multiple() {
        assert_eq!(size_of::<OperationInfoMultiple>(), size_of::<TEE_OperationInfoMultiple>());
        assert_eq!(align_of::<OperationInfoMultiple>(), align_of::<TEE_OperationInfoMultiple>());

        let new = OperationInfoMultiple {
            algorithm: Algorithm::AesEcbNopad,
            operation_class: Operation::Ae,
            mode: Mode::Decrypt,
            digest_length: 0,
            max_key_size: 0,
            handle_state: HandleFlags::empty(),
            operation_state: OperationState::Active,
            number_of_keys: 0,
            key_information: binding::__IncompleteArrayField::<OperationInfoKey>::new(),
        };
        let old = TEE_OperationInfoMultiple {
            algorithm: 0,
            operationClass: 0,
            mode: 0,
            digestLength: 0,
            maxKeySize: 0,
            handleState: 0,
            operationState: 0,
            numberOfKeys: 0,
            keyInformation: binding::__IncompleteArrayField::<TEE_OperationInfoKey>::new(),
        };

        assert_eq!(
            offset_of!(OperationInfoMultiple, algorithm),
            offset_of!(TEE_OperationInfoMultiple, algorithm)
        );
        assert_eq!(size_of_val(&new.algorithm), size_of_val(&old.algorithm));
        assert_eq!(align_of_val(&new.algorithm), align_of_val(&old.algorithm));

        assert_eq!(
            offset_of!(OperationInfoMultiple, operation_class),
            offset_of!(TEE_OperationInfoMultiple, operationClass)
        );
        assert_eq!(size_of_val(&new.operation_class), size_of_val(&old.operationClass));
        assert_eq!(align_of_val(&new.operation_class), align_of_val(&old.operationClass));

        assert_eq!(
            offset_of!(OperationInfoMultiple, mode),
            offset_of!(TEE_OperationInfoMultiple, mode)
        );
        assert_eq!(size_of_val(&new.mode), size_of_val(&old.mode));
        assert_eq!(align_of_val(&new.mode), align_of_val(&old.mode));

        assert_eq!(
            offset_of!(OperationInfoMultiple, digest_length),
            offset_of!(TEE_OperationInfoMultiple, digestLength)
        );
        assert_eq!(size_of_val(&new.digest_length), size_of_val(&old.digestLength));
        assert_eq!(align_of_val(&new.digest_length), align_of_val(&old.digestLength));

        assert_eq!(
            offset_of!(OperationInfoMultiple, max_key_size),
            offset_of!(TEE_OperationInfoMultiple, maxKeySize)
        );
        assert_eq!(size_of_val(&new.max_key_size), size_of_val(&old.maxKeySize));
        assert_eq!(align_of_val(&new.max_key_size), align_of_val(&old.maxKeySize));

        assert_eq!(
            offset_of!(OperationInfoMultiple, handle_state),
            offset_of!(TEE_OperationInfoMultiple, handleState)
        );
        assert_eq!(size_of_val(&new.handle_state), size_of_val(&old.handleState));
        assert_eq!(align_of_val(&new.handle_state), align_of_val(&old.handleState));

        assert_eq!(
            offset_of!(OperationInfoMultiple, operation_state),
            offset_of!(TEE_OperationInfoMultiple, operationState)
        );
        assert_eq!(size_of_val(&new.operation_state), size_of_val(&old.operationState));
        assert_eq!(align_of_val(&new.operation_state), align_of_val(&old.operationState));

        assert_eq!(
            offset_of!(OperationInfoMultiple, number_of_keys),
            offset_of!(TEE_OperationInfoMultiple, numberOfKeys)
        );
        assert_eq!(size_of_val(&new.number_of_keys), size_of_val(&old.numberOfKeys));
        assert_eq!(align_of_val(&new.number_of_keys), align_of_val(&old.numberOfKeys));

        assert_eq!(
            offset_of!(OperationInfoMultiple, key_information),
            offset_of!(TEE_OperationInfoMultiple, keyInformation)
        );
        assert_eq!(size_of_val(&new.key_information), size_of_val(&old.keyInformation));
        assert_eq!(align_of_val(&new.key_information), align_of_val(&old.keyInformation));

        // Verify zero-copy translation.
        assert_eq!(
            addr_of!(*OperationInfoMultiple::to_binding(&new)) as usize,
            addr_of!(new) as usize
        );
    }

    #[test]
    pub fn test_abi_compat_algorithm() {
        assert_eq!(size_of::<Algorithm>(), size_of::<u32>());
        assert_eq!(align_of::<Algorithm>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_ecc_curve() {
        assert_eq!(size_of::<EccCurve>(), size_of::<u32>());
        assert_eq!(align_of::<EccCurve>(), align_of::<u32>());
    }

    #[test]
    pub fn test_abi_compat_attribute_id() {
        assert_eq!(size_of::<AttributeId>(), size_of::<u32>());
        assert_eq!(align_of::<AttributeId>(), align_of::<u32>());
    }
}
