// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MICROFUCHSIA_TEE_TEE_INTERNAL_API_INCLUDE_LIB_TEE_INTERNAL_API_TEE_INTERNAL_API_H_
#define SRC_MICROFUCHSIA_TEE_TEE_INTERNAL_API_INCLUDE_LIB_TEE_INTERNAL_API_TEE_INTERNAL_API_H_

#include <zircon/compiler.h>

#include "tee_internal_api_types.h"

// Fuchsia definition of the TEE Core Internal API v1.3.1
//
// https://globalplatform.org/specs-library/tee-internal-core-api-specification/

// Common Definitions 3.1

// 3.1.1 API Version

#define TEE_CORE_API_MAJOR_VERSION (1)
#define TEE_CORE_API_MINOR_VERSION (3)
#define TEE_CORE_API_MAINTENANCE_VERSION (1)
#define TEE_CORE_API_VERSION                                                 \
  ((TEE_CORE_API_MAJOR_VERSION << 24) + (TEE_CORE_API_MINOR_VERSION << 16) + \
   (TEE_CORE_API_MAINTENANCE_VERSION << 8))
#define TEE_CORE_API_1_3_1

// 3.5.1 Version Compatibility Definitions

#if defined(TEE_CORE_API_REQUIRED_MAINTENANCE_VERSION)
#if !defined(TEE_CORE_API_REQUIRED_MAJOR_VERSION)
#error TEE_CORE_API_REQUIRED_MAJOR_VERSION is required if TEE_CORE_API_REQUIRED_MAINTENANCE_VERSION is defined
#elif !defined(TEE_CORE_API_REQUIRED_MINOR_VERSION)
#error TEE_CORE_API_REQUIRED_MINOR_VERSION is required if TEE_CORE_API_REQUIRED_MAINTENANCE_VERSION is defined
#endif
#endif  // defined(TEE_CORE_API_REQUIRED_MAINTENANCE_VERSION)

__BEGIN_CDECLS

// 4.3 TA Interface

#define TA_EXPORT __EXPORT

// 4.3.1 TA_CreateEntryPoint
TEE_Result TA_EXPORT TA_CreateEntryPoint(void);

// 4.3.2 TA_DestroyEntryPoint
void TA_EXPORT TA_DestroyEntryPoint(void);

// 4.3.3 TA_OpenSessionEntryPoint
TEE_Result TA_EXPORT TA_OpenSessionEntryPoint(uint32_t paramTypes,
                                              /* inout */ TEE_Param params[4],
                                              /* out */ /* ctx */ void** sessionContext);

// 4.3.4 TA_CloseSessionEntryPoint
void TA_EXPORT TA_CloseSessionEntryPoint(
    /* ctx */ void* sessionContext);

// 4.3.5 TA_InvokeCommandEntryPoint
TEE_Result TA_EXPORT TA_InvokeCommandEntryPoint(
    /* ctx */ void* sessionContext, uint32_t commandID, uint32_t paramTypes,
    /* inout */ TEE_Param params[4]);

// 4.4.1 TEE_GetPropertyAsString

TEE_Result TEE_GetPropertyAsString(TEE_PropSetHandle propsetOrEnumerator,
                                   /* instringopt */ char* name,
                                   /* outstring */ char* valueBuffer, size_t* valueBufferLen);

// 4.4.2 TEE_GetPropertyAsBool

TEE_Result TEE_GetPropertyAsBool(TEE_PropSetHandle propsetOrEnumerator,
                                 /* instringopt */ char* name, /* out */ bool* value);

// 4.4.3.1 TEE_GetPropertyAsU32

TEE_Result TEE_GetPropertyAsU32(TEE_PropSetHandle propsetOrEnumerator, /* instringopt */ char* name,
                                /* out */ uint32_t* value);

// 4.4.3.2 TEE_GetPropertyAsU64

TEE_Result TEE_GetPropertyAsU64(TEE_PropSetHandle propsetOrEnumerator, /* instringopt */ char* name,
                                /* out */ uint64_t* value);

// 4.4.4 TEE_GetPropertyAsBinaryBlock

TEE_Result TEE_GetPropertyAsBinaryBlock(TEE_PropSetHandle propsetOrEnumerator,
                                        /* instringopt */ char* name,
                                        /* outbuf */ void* valueBuffer, size_t* valueBufferLen);

// 4.4.5 TEE_GetPropertyAsUUID

TEE_Result TEE_GetPropertyAsUUID(TEE_PropSetHandle propsetOrEnumerator,
                                 /* instringopt */ char* name, /* out */ TEE_UUID* value);

// 4.4.6 TEE_GetPropertyAsIdentity

TEE_Result TEE_GetPropertyAsIdentity(TEE_PropSetHandle propsetOrEnumerator,
                                     /* instringopt */ char* name, /* out */ TEE_Identity* value);

// 4.4.7 TEE_AllocatePropertyEnumerator

TEE_Result TEE_AllocatePropertyEnumerator(/* out */ TEE_PropSetHandle* enumerator);

// 4.4.8 TEE_FreePropertyEnumerator

void TEE_FreePropertyEnumerator(TEE_PropSetHandle enumerator);

// 4.4.9 TEE_StartPropertyEnumerator

void TEE_StartPropertyEnumerator(TEE_PropSetHandle enumerator,

                                 TEE_PropSetHandle propSet);

// 4.4.10 TEE_ResetPropertyEnumerator

void TEE_ResetPropertyEnumerator(TEE_PropSetHandle enumerator);

// 4.4.11 TEE_GetPropertyName

TEE_Result TEE_GetPropertyName(TEE_PropSetHandle enumerator,
                               /* outstring */ void* nameBuffer, size_t* nameBufferLen);

// 4.4.12 TEE_GetNextProperty

TEE_Result TEE_GetNextProperty(TEE_PropSetHandle enumerator);

// 4.8.1 TEE_Panic

__NO_RETURN void TEE_Panic(TEE_Result panicCode);

// 4.9.1 TEE_OpenTASession
TEE_Result TEE_OpenTASession(
    /* in */ const TEE_UUID* destination, uint32_t cancellationRequestTimeout, uint32_t paramTypes,
    /* inout */ TEE_Param params[4],
    /* out */ TEE_TASessionHandle* session,
    /* out */ uint32_t* returnOrigin);

// 4.9.2 TEE_CloseTASession
void TEE_CloseTASession(TEE_TASessionHandle session);

// 4.9.3 TEE_InvokeTACommand
TEE_Result TEE_InvokeTACommand(TEE_TASessionHandle session, uint32_t cancellationRequestTimeout,
                               uint32_t commandID, uint32_t paramTypes,
                               /* inout */ TEE_Param params[4],
                               /* out */ uint32_t* returnOrigin);

// 4.10.1 TEE_GetCancellationFlag
bool TEE_GetCancellationFlag(void);

// 4.10.2 TEE_UnmaskCancellation
bool TEE_UnmaskCancellation(void);

// 4.10.3 TEE_MaskCancellation
bool TEE_MaskCancellation(void);

// 4.11.1 TEE_CheckMemoryAccessRights
TEE_Result TEE_CheckMemoryAccessRights(uint32_t accessFlags,
                                       /* inbuf */ void* buffer, size_t size);

// 4.11.2 TEE_SetInstanceData
void TEE_SetInstanceData(
    /* ctx */ void* instanceData);

// 4.11.3 TEE_GetInstanceData
/* ctx */ void* TEE_GetInstanceData(void);

// 4.11.4 TEE_Malloc
void* TEE_Malloc(size_t size, uint32_t hint);

#define TEE_MALLOC_FILL_ZERO ((uint32_t)0)
#define TEE_MALLOC_NO_FILL ((uint32_t)1)
#define TEE_MALLOC_NO_SHARE ((uint32_t)2)

// 4.11.5 TEE_Realloc
void* TEE_Realloc(
    /* inout */ void* buffer, size_t newSize);

// 4.11.6 TEE_Free
void TEE_Free(void* buffer);

// 4.11.7 TEE_MemMove
void TEE_MemMove(/* outbuf(size) */ void* dest, /* inbuf(size) */ const void* src, size_t size);

// 4.11.8 TEE_MemCompare
int32_t TEE_MemCompare(/* inbuf(size) */ const void* buffer1, /* inbuf(size) */ const void* buffer2,
                       size_t size);

// 4.11.9 TEE_MemFill
void TEE_MemFill(/* outbuf(size) */ void* buffer, uint8_t x, size_t size);

// 5.5.1 TEE_GetObjectInfo1
TEE_Result TEE_GetObjectInfo1(TEE_ObjectHandle object,
                              /* out */ TEE_ObjectInfo* objectInfo);

// 5.5.2 TEE_RestrictObjectUsage1
TEE_Result TEE_RestrictObjectUsage1(TEE_ObjectHandle object, uint32_t objectUsage);

// 5.5.3 TEE_GetObjectBufferAttribute
TEE_Result TEE_GetObjectBufferAttribute(TEE_ObjectHandle object, uint32_t attributeID,
                                        /* outbuf */ void* buffer, size_t* size);

// 5.5.4 TEE_GetObjectValueAttribute
TEE_Result TEE_GetObjectValueAttribute(TEE_ObjectHandle object, uint32_t attributeID,
                                       /* outopt */ uint32_t* a,
                                       /* outopt */ uint32_t* b);

// 5.5.5 TEE_CloseObject
void TEE_CloseObject(TEE_ObjectHandle object);

// 5.6.1 TEE_AllocateTransientObject
TEE_Result TEE_AllocateTransientObject(uint32_t objectType, uint32_t maxObjectSize,
                                       /* out */ TEE_ObjectHandle* object);

// 5.6.2 TEE_FreeTransientObject
void TEE_FreeTransientObject(TEE_ObjectHandle object);

// 5.6.3 TEE_ResetTransientObject
void TEE_ResetTransientObject(TEE_ObjectHandle object);

// 5.6.4 TEE_PopulateTransientObject
TEE_Result TEE_PopulateTransientObject(TEE_ObjectHandle object,
                                       /* in */ TEE_Attribute* attrs, uint32_t attrCount);

// 5.6.5 TEE_InitRefAttribute, TEE_InitValueAttribute
void TEE_InitRefAttribute(
    /* out */ TEE_Attribute* attr, uint32_t attributeID,
    /* inbuf */ void* buffer, size_t length);

void TEE_InitValueAttribute(
    /* out */ TEE_Attribute* attr, uint32_t attributeID, uint32_t a, uint32_t b);

// 5.6.6 TEE_CopyObjectAttributes1
TEE_Result TEE_CopyObjectAttributes1(TEE_ObjectHandle destObject, TEE_ObjectHandle srcObject);

// 5.6.7 TEE_GenerateKey
TEE_Result TEE_GenerateKey(TEE_ObjectHandle object, uint32_t keySize,
                           /* in */ TEE_Attribute* params, uint32_t paramCount);

// 5.7.1 TEE_OpenPersistentObject
TEE_Result TEE_OpenPersistentObject(uint32_t storageID, /* in(objectIDLength) */ void* objectID,
                                    size_t objectIDLen, uint32_t flags,
                                    /* out */ TEE_ObjectHandle* object);

// 5.7.2 TEE_CreatePersistentObject
TEE_Result TEE_CreatePersistentObject(uint32_t storageID, /* in(objectIDLength) */ void* objectID,
                                      size_t objectIDLen, uint32_t flags,
                                      TEE_ObjectHandle attributes,
                                      /* inbuf */ void* initialData, size_t initialDataLen,
                                      /* outopt */ TEE_ObjectHandle* object);

// 5.7.4 TEE_CloseAndDeletePersistentObject1
TEE_Result TEE_CloseAndDeletePersistentObject1(TEE_ObjectHandle object);

// 5.7.5 TEE_RenamePersistentObject
TEE_Result TEE_RenamePersistentObject(TEE_ObjectHandle object,
                                      /* in(newObjectIDLen) */ void* newObjectID,
                                      size_t newObjectIDLen);

// 5.8.1 TEE_AllocatePersistentObjectEnumerator
TEE_Result TEE_AllocatePersistentObjectEnumerator(
    /* out */ TEE_ObjectEnumHandle* objectEnumerator);

// 5.8.2 TEE_FreePersistentObjectEnumerator
void TEE_FreePersistentObjectEnumerator(TEE_ObjectEnumHandle objectEnumerator);

// 5.8.3 TEE_ResetPersistentObjectEnumerator
void TEE_ResetPersistentObjectEnumerator(TEE_ObjectEnumHandle objectEnumerator);

// 5.8.4 TEE_StartPersistentObjectEnumerator
TEE_Result TEE_StartPersistentObjectEnumerator(TEE_ObjectEnumHandle objectEnumerator,
                                               uint32_t storageID);

// 5.8.5 TEE_GetNextPersistentObject
TEE_Result TEE_GetNextPersistentObject(TEE_ObjectEnumHandle objectEnumerator,
                                       /* out */ TEE_ObjectInfo* objectInfo,
                                       /* out */ void* objectID,
                                       /* out */ size_t* objectIDLen);

// 5.9.1 TEE_ReadObjectData
TEE_Result TEE_ReadObjectData(TEE_ObjectHandle object,
                              /* out */ void* buffer, size_t size,
                              /* out */ size_t* count);

// 5.9.2 TEE_WriteObjectData
TEE_Result TEE_WriteObjectData(TEE_ObjectHandle object,
                               /* inbuf */ void* buffer, size_t size);

// 5.9.3 TEE_TruncateObjectData
TEE_Result TEE_TruncateObjectData(TEE_ObjectHandle object, size_t size);

// 5.9.4 TEE_SeekObjectData
TEE_Result TEE_SeekObjectData(TEE_ObjectHandle object, intmax_t offset, TEE_Whence whence);

// 6.2.1 TEE_AllocateOperation
TEE_Result TEE_AllocateOperation(TEE_OperationHandle* operation, uint32_t algorithm, uint32_t mode,
                                 uint32_t maxKeySize);

// 6.2.2 TEE_FreeOperation
void TEE_FreeOperation(TEE_OperationHandle operation);

// 6.2.3 TEE_GetOperationInfo
void TEE_GetOperationInfo(TEE_OperationHandle operation,
                          /* out */ TEE_OperationInfo* operationInfo);

// 6.2.4 TEE_GetOperationInfoMultiple
TEE_Result TEE_GetOperationInfoMultiple(
    TEE_OperationHandle operation,
    /* outbuf */ TEE_OperationInfoMultiple* operationInfoMultiple, size_t* operationSize);

// 6.2.5 TEE_ResetOperation
void TEE_ResetOperation(TEE_OperationHandle operation);

// 6.2.6 TEE_SetOperationKey
TEE_Result TEE_SetOperationKey(TEE_OperationHandle operation, TEE_ObjectHandle key);

// 6.2.7 TEE_SetOperationKey2
TEE_Result TEE_SetOperationKey2(TEE_OperationHandle operation, TEE_ObjectHandle key1,
                                TEE_ObjectHandle key2);

// 6.2.8 TEE_CopyOperation
void TEE_CopyOperation(TEE_OperationHandle dstOperation, TEE_OperationHandle srcOperation);

// 6.2.9 TEE_IsAlgorithmSupported
TEE_Result TEE_IsAlgorithmSupported(uint32_t algId, uint32_t element);

// 6.3.1 TEE_DigestUpdate
void TEE_DigestUpdate(TEE_OperationHandle operation,
                      /* inbuf */ void* chunk, size_t chunkSize);

// 6.3.2 TEE_DigestDoFinal
TEE_Result TEE_DigestDoFinal(TEE_OperationHandle operation,
                             /* inbuf */ void* chunk, size_t chunkLen,
                             /* outbuf */ void* hash, size_t* hashLen);

// 6.3.3 TEE_DigestExtract
TEE_Result TEE_DigestExtract(TEE_OperationHandle operation, /* outbuf */ void* hash,
                             size_t* hashLen);

// 6.4.1 TEE_CipherInit
void TEE_CipherInit(TEE_OperationHandle operation,
                    /* inbuf */ void* IV, size_t IVLen);

// 6.4.2 TEE_CipherUpdate
TEE_Result TEE_CipherUpdate(TEE_OperationHandle operation,
                            /* inbuf */ void* srcData, size_t srcLen,
                            /* outbuf */ void* destData, size_t* destLen);

// 6.4.3 TEE_CipherDoFinal
TEE_Result TEE_CipherDoFinal(TEE_OperationHandle operation,
                             /* inbuf */ void* srcData, size_t srcLen,
                             /* outbufopt */ void* destData, size_t* destLen);

// 6.5.1 TEE_MACInit
void TEE_MACInit(TEE_OperationHandle operation, void* IV, size_t IVLen);

// 6.5.2 TEE_MACUpdate
void TEE_MACUpdate(TEE_OperationHandle operation,
                   /* inbuf */ void* chunk, size_t chunkSize);

// 6.5.3 TEE_MACComputeFinal
TEE_Result TEE_MACComputeFinal(TEE_OperationHandle operation,
                               /* inbuf */ void* message, size_t messageLen,
                               /* outbuf */ void* mac, size_t* macLen);

// 6.5.4 TEE_MACCompareFinal
TEE_Result TEE_MACCompareFinal(TEE_OperationHandle operation,
                               /* inbuf */ void* message, size_t messageLen,
                               /* inbuf */ void* mac, size_t macLen);

// 6.6.1 TEE_AEInit
TEE_Result TEE_AEInit(TEE_OperationHandle operation,
                      /* inbuf */ void* nonce, size_t nonceLen, uint32_t tagLen, size_t AADLen,
                      size_t payloadLen);

// 6.6.2 TEE_AEUpdateAAD
void TEE_AEUpdateAAD(TEE_OperationHandle operation,
                     /* inbuf */ void* AADdata, size_t AADdataLen);

// 6.6.3 TEE_AEUpdate
TEE_Result TEE_AEUpdate(TEE_OperationHandle operation,
                        /* inbuf */ void* srcData, size_t srcLen,
                        /* outbuf */ void* destData, size_t* destLen);

// 6.6.4 TEE_AEEncryptFinal
TEE_Result TEE_AEEncryptFinal(TEE_OperationHandle operation,
                              /* inbuf */ void* srcData, size_t srcLen,
                              /* outbufopt */ void* destData, size_t* destLen,
                              /* outbuf */ void* tag, size_t* tagLen);

// 6.6.5 TEE_AEDecryptFinal
TEE_Result TEE_AEDecryptFinal(TEE_OperationHandle operation,
                              /* inbuf */ void* srcData, size_t srcLen,
                              /* outbuf */ void* destData, size_t* destLen,
                              /* in */ void* tag, size_t tagLen);

// 6.7.1 TEE_AsymmetricEncrypt, TEE_AsymmetricDecrypt
TEE_Result TEE_AsymmetricEncrypt(TEE_OperationHandle operation,
                                 /* in */ TEE_Attribute* params, uint32_t paramCount,
                                 /* inbuf */ void* srcData, size_t srcLen,
                                 /* outbuf */ void* destData, size_t* destLen);

TEE_Result TEE_AsymmetricDecrypt(TEE_OperationHandle operation,
                                 /* in */ TEE_Attribute* params, uint32_t paramCount,
                                 /* inbuf */ void* srcData, size_t srcLen,
                                 /* outbuf */ void* destData, size_t* destLen);

// 6.7.2 TEE_AsymmetricSignDigest
TEE_Result TEE_AsymmetricSignDigest(TEE_OperationHandle operation,
                                    /* in */ TEE_Attribute* params, uint32_t paramCount,
                                    /* inbuf */ void* digest, size_t digestLen,
                                    /* outbuf */ void* signature, size_t* signatureLen);

// 6.7.3 TEE_AsymmetricVerifyDigest
TEE_Result TEE_AsymmetricVerifyDigest(TEE_OperationHandle operation,
                                      /* in */ TEE_Attribute* params, uint32_t paramCount,
                                      /* inbuf */ void* digest, size_t digestLen,
                                      /* inbuf */ void* signature, size_t signatureLen);

// 6.8.1 TEE_DeriveKey
void TEE_DeriveKey(TEE_OperationHandle operation,
                   /* inout */ TEE_Attribute* params, uint32_t paramCount,
                   TEE_ObjectHandle derivedKey);

// 6.9.1 TEE_GenerateRandom
void TEE_GenerateRandom(
    /* out */ void* randomBuffer, size_t randomBufferLen);

// 7.2.1 TEE_GetSystemTime
void TEE_GetSystemTime(
    /* out */ TEE_Time* time);

// 7.2.2 TEE_Wait
TEE_Result TEE_Wait(uint32_t timeout);

// 7.2.3 TEE_GetTAPersistentTime
TEE_Result TEE_GetTAPersistentTime(
    /* out */ TEE_Time* time);

// 7.2.4 TEE_SetTAPersistentTime
TEE_Result TEE_SetTAPersistentTime(
    /* in */ const TEE_Time* time);

// 7.2.5 TEE_GetREETime
void TEE_GetREETime(
    /* out */ TEE_Time* time);

// 8.4.2 TEE_BigIntFMMContextSizeInU32
size_t TEE_BigIntFMMContextSizeInU32(size_t modulusSizeInBits);

// 8.4.3 TEE_BigIntFMMSizeInU32
size_t TEE_BigIntFMMSizeInU32(size_t modulusSizeInBits);

// 8.5.1 TEE_BigIntInit
void TEE_BigIntInit(
    /* out */ TEE_BigInt* bigInt, size_t len);

// 8.5.2 TEE_BigIntInitFMMContext1
TEE_Result TEE_BigIntInitFMMContext1(
    /* out */ TEE_BigIntFMMContext* context, size_t len,
    /* in */ TEE_BigInt* modulus);

// 8.5.3 TEE_BigIntInitFMM
void TEE_BigIntInitFMM(
    /* in */ TEE_BigIntFMM* bigIntFMM, size_t len);

// 8.6.1 TEE_BigIntConvertFromOctetString
TEE_Result TEE_BigIntConvertFromOctetString(
    /* out */ TEE_BigInt* dest,
    /* inbuf */ uint8_t* buffer, size_t bufferLen, int32_t sign);

// 8.6.2 TEE_BigIntConvertToOctetString
TEE_Result TEE_BigIntConvertToOctetString(
    /* outbuf */ void* buffer, size_t* bufferLen,
    /* in */ TEE_BigInt* bigInt);

// 8.6.3 TEE_BigIntConvertFromS32
void TEE_BigIntConvertFromS32(
    /* out */ TEE_BigInt* dest, int32_t shortVal);

// 8.6.4 TEE_BigIntConvertToS32
TEE_Result TEE_BigIntConvertToS32(
    /* out */ int32_t* dest,
    /* in */ TEE_BigInt* src);

// 8.7.1 TEE_BigIntCmp
int32_t TEE_BigIntCmp(
    /* in */ TEE_BigInt* op1,
    /* in */ TEE_BigInt* op2);

// 8.7.2 TEE_BigIntCmpS32
int32_t TEE_BigIntCmpS32(
    /* in */ TEE_BigInt* op, int32_t shortVal);

// 8.7.3 TEE_BigIntShiftRight
void TEE_BigIntShiftRight(
    /* out */ TEE_BigInt* dest,
    /* in */ TEE_BigInt* op, size_t bits);

// 8.7.4 TEE_BigIntGetBit
bool TEE_BigIntGetBit(
    /* in */ TEE_BigInt* src, uint32_t bitIndex);

// 8.7.5 TEE_BigIntGetBitCount
uint32_t TEE_BigIntGetBitCount(
    /* in */ TEE_BigInt* src);

// 8.7.6 TEE_BigIntSetBit
TEE_Result TEE_BigIntSetBit(
    /* inout */ TEE_BigInt* op, uint32_t bitIndex, bool value);

// 8.7.7 TEE_BigIntAssign
TEE_Result TEE_BigIntAssign(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* src);

// 8.7.8 TEE_BigIntAbs
TEE_Result TEE_BigIntAbs(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* src);

// 8.8.1 TEE_BigIntAdd
void TEE_BigIntAdd(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op1,
    /* in */ const TEE_BigInt* op2);

// 8.8.2 TEE_BigIntSub
void TEE_BigIntSub(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op1,
    /* in */ const TEE_BigInt* op2);

// 8.8.3 TEE_BigIntNeg
void TEE_BigIntNeg(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op);

// 8.8.4 TEE_BigIntMul
void TEE_BigIntMul(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op1,
    /* in */ const TEE_BigInt* op2);

// 8.8.5 TEE_BigIntSquare
void TEE_BigIntSquare(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op);

// 8.8.6 TEE_BigIntDiv
void TEE_BigIntDiv(
    /* out */ TEE_BigInt* dest_q,
    /* out */ TEE_BigInt* dest_r,
    /* in */ const TEE_BigInt* op1,
    /* in */ const TEE_BigInt* op2);

// 8.9.1 TEE_BigIntMod
void TEE_BigIntMod(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op,
    /* in */ const TEE_BigInt* n);

// 8.9.2 TEE_BigIntAddMod
void TEE_BigIntAddMod(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op1,
    /* in */ const TEE_BigInt* op2,
    /* in */ const TEE_BigInt* n);

// 8.9.3 TEE_BigIntSubMod
void TEE_BigIntSubMod(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op1,
    /* in */ const TEE_BigInt* op2,
    /* in */ const TEE_BigInt* n);

// 8.9.4 TEE_BigIntMulMod
void TEE_BigIntMulMod(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op1,
    /* in */ const TEE_BigInt* op2,
    /* in */ const TEE_BigInt* n);

// 8.9.5 TEE_BigIntSquareMod
void TEE_BigIntSquareMod(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op,
    /* in */ const TEE_BigInt* n);

// 8.9.6 TEE_BigIntInvMod
void TEE_BigIntInvMod(
    /* out */ TEE_BigInt* dest,
    /* in */ const TEE_BigInt* op,
    /* in */ const TEE_BigInt* n);

// 8.9.7 TEE_BigIntExpMod
TEE_Result TEE_BigIntExpMod(
    /* out */ TEE_BigInt* dest,
    /* in */ TEE_BigInt* op1,
    /* in */ TEE_BigInt* op2,
    /* in */ TEE_BigInt* n,
    /* in */ TEE_BigIntFMMContext* context);

// 8.10.1 TEE_BigIntRelativePrime
bool TEE_BigIntRelativePrime(
    /* in */ TEE_BigInt* op1,
    /* in */ TEE_BigInt* op2);

// 8.10.2 TEE_BigIntComputeExtendedGcd
void TEE_BigIntComputeExtendedGcd(
    /* out */ TEE_BigInt* gcd,
    /* out */ TEE_BigInt* u,
    /* out */ TEE_BigInt* v,
    /* in */ TEE_BigInt* op1,
    /* in */ TEE_BigInt* op2);

// 8.10.3 TEE_BigIntIsProbablePrime
int32_t TEE_BigIntIsProbablePrime(
    /* in */ TEE_BigInt* op, uint32_t confidenceLevel);

// 8.11.1 TEE_BigIntConvertToFMM
void TEE_BigIntConvertToFMM(
    /* out */ TEE_BigIntFMM* dest,
    /* in */ TEE_BigInt* src,
    /* in */ TEE_BigInt* n,
    /* in */ TEE_BigIntFMMContext* context);

// 8.11.2 TEE_BigIntConvertFromFMM
void TEE_BigIntConvertFromFMM(
    /* out */ TEE_BigInt* dest,
    /* in */ TEE_BigIntFMM* src,
    /* in */ TEE_BigInt* n,
    /* in */ TEE_BigIntFMMContext* context);

// 8.11.3 TEE_BigIntComputeFMM
void TEE_BigIntComputeFMM(
    /* out */ TEE_BigIntFMM* dest,
    /* in */ TEE_BigIntFMM* op1,
    /* in */ TEE_BigIntFMM* op2,
    /* in */ TEE_BigInt* n,
    /* in */ TEE_BigIntFMMContext* context);

// B.1 Deprecated Functions

// B.1.1 TEE_GetObjectInfo - Deprecated
void TEE_GetObjectInfo(TEE_ObjectHandle object,
                       /* out */ TEE_ObjectInfo* objectInfo);

// B.1.2 TEE_RestrictObjectUsage - Deprecated
void TEE_RestrictObjectUsage(TEE_ObjectHandle object, uint32_t objectUsage);

// B.1.3 TEE_CopyObjectAttributes – Deprecated
void TEE_CopyObjectAttributes(TEE_ObjectHandle destObject, TEE_ObjectHandle srcObject);

// B.1.4 TEE_CloseAndDeletePersistentObject – Deprecated
void TEE_CloseAndDeletePersistentObject(TEE_ObjectHandle object);

// B.1.5 TEE_BigIntInitFMMContext - Deprecated
void TEE_BigIntInitFMMContext(
    /* out */ TEE_BigIntFMMContext* context, size_t len,
    /* in */ const TEE_BigInt* modulus);

__END_CDECLS

#endif  // SRC_MICROFUCHSIA_TEE_TEE_INTERNAL_API_INCLUDE_LIB_TEE_INTERNAL_API_TEE_INTERNAL_API_H_
