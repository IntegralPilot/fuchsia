// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_STORAGE_GPT_UTILS_H_
#define SRC_FIRMWARE_LIB_STORAGE_GPT_UTILS_H_

#ifdef FIRMWARE_STORAGE_CUSTOM_SYSDEPS_HEADER
#include <firmware_storage_sysdeps.h>
#else
#include <stddef.h>
#include <string.h>
#endif

#include "storage.h"
// TODO(b/262617680): Add document for include path setting when porting.
#include <gpt_misc.h>

#ifdef __cplusplus
extern "C" {
#endif

// Checks GPT and repair primary/backup copy if necessary.
//
// @ops: Pointer to `FuchsiaFirmwareStorage`.
// @out_gpt_data: Pointer to output `GptData`.
//
// Returns true if success, false otherwise.
bool FuchsiaFirmwareStorageSyncGpt(FuchsiaFirmwareStorage* ops, GptData* out_gpt_data);

// Frees a GptData initialized by FuchsiaFirmwareStorageSyncGpt
//
// @ops: Pointer to `FuchsiaFirmwareStorage`.
// @data: Pointer to GptData initialized by FuchsiaFirmwareStorageSyncGpt.
//
// Returns true if success, false otherwise.
bool FuchsiaFirmwareStorageFreeGptData(FuchsiaFirmwareStorage* ops, GptData* data);

// Gets the size of a partition.
//
// @ops: Pointer to `FuchsiaFirmwareStorage`.
// @data: Pointer to GptData initialized by FuchsiaFirmwareStorageSyncGpt.
// @name: Name of the partition.
// @out: Output pointer for the size.
//
// Returns true if success, false otherwise.
bool FuchsiaFirmwareStorageGetPartitionSize(FuchsiaFirmwareStorage* ops, const GptData* data,
                                            const char* name, size_t* out);

// Reads from a GPT partition.
//
// @ops: Pointer to `FuchsiaFirmwareStorage`.
// @data: Pointer to GptData initialized by FuchsiaFirmwareStorageSyncGpt.
// @name: Name of the partition. Only support ASCII name.
// @offset: Offset in number of bytes to read from.
// @size: Size in number of bytes to read.
// @dst: Output buffer
//
// Returns true if success, false otherwise.
bool FuchsiaFirmwareStorageGptRead(FuchsiaFirmwareStorage* ops, const GptData* data,
                                   const char* name, size_t offset, size_t size, void* dst);

// Writes to a GPT partition.
//
// @ops: Pointer to `FuchsiaFirmwareStorage`.
// @data: Pointer to GptData initialized by FuchsiaFirmwareStorageSyncGpt.
// @name: Name of the partition. Only support ASCII name.
// @offset: Offset in number of bytes to write.
// @size: Size in number of bytes to write.
// @src: Data to write.
//
// Returns true if success, false otherwise.
bool FuchsiaFirmwareStorageGptWriteConst(FuchsiaFirmwareStorage* ops, const GptData* data,
                                         const char* name, size_t offset, size_t size,
                                         const void* src);

// An optimized version of FuchsiaFirmwareStorageGptWrite. The input buffer is required to be
// modifiable.
bool FuchsiaFirmwareStorageGptWrite(FuchsiaFirmwareStorage* ops, const GptData* data,
                                    const char* name, size_t offset, size_t size, void* src);

#ifdef __cplusplus
}
#endif

#endif  // SRC_FIRMWARE_LIB_STORAGE_GPT_UTILS_H_
