// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_STORAGE_SPARSE_H_
#define SRC_FIRMWARE_LIB_STORAGE_SPARSE_H_

#include <gpt_misc.h>

#include "storage.h"

#ifdef __cplusplus
extern "C" {
#endif

// Returns true if the passed in buffer contains a well formed sparse image header.
//
// @src: Buffer containing sparse image.
// @size: Size of the sparse image buffer in bytes.
//
bool FuchsiaIsSparseImage(const uint8_t* src, size_t size);

// Writes a sparse image to the selected partition.
//
// @ops: Pointer to FuchsiaFirmwareStorage.
// @data: Pointer to GptData initialized by FuchsiaFirmwareStorageSyncGpt.
// @name: Name of the partition. Only support ASCII names.
// @src: Buffer containing sparse image.
// @size: Size in number of bytes of the sparse image.
//
// Returns true on success, false otherwise.
//
// Failure can occur if any of the following occur:
// *) Invalid sparse image header
// *) Inconsistent sparse image data, e.g. total_blks disagrees with number of chunks and
//    corresponding chunk_sz
// *) Newer sparse image format than expected
// *) A larger expanded image than the destination partition can support
// *) Checksum mismatch
// *) Write failure to the underlying storage
bool FuchsiaWriteSparseImage(FuchsiaFirmwareStorage* ops, const GptData* data, const char* name,
                             uint8_t* src, size_t size);

#ifdef __cplusplus
}  // __cplusplus
#endif

#endif  // SRC_FIRMWARE_LIB_STORAGE_SPARSE_H_
