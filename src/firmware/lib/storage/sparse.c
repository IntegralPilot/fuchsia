// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/sparse/c/sparse.h>
#include <lib/storage/gpt_utils.h>
#include <lib/storage/sparse.h>
#include <sparse_format.h>
#include <string.h>

#include "lib/storage/storage.h"

#ifdef ENABLE_FIRMWARE_STORAGE_LOG
#define sparse_logger firmware_storage_log
#else
#define sparse_logger sparse_nop_logger
#endif

#define kScratchSize 4096

typedef struct IoBuffer {
  uint8_t* data;
  size_t size;
} IoBuffer;

static size_t IoBufferSize(SparseIoBufferHandle handle) {
  IoBuffer* buffer = (IoBuffer*)(handle);
  return buffer->size;
}

static bool IoBufferRead(SparseIoBufferHandle handle, uint64_t offset, uint8_t* dst, size_t size) {
  IoBuffer* buffer = (IoBuffer*)(handle);
  if (offset + size > buffer->size) {
    return false;
  }
  memcpy(dst, buffer->data + offset, size);
  return true;
}

static bool IoBufferFill(SparseIoBufferHandle handle, uint32_t payload) {
  IoBuffer* buffer = (IoBuffer*)(handle);

  // Make sure the buffer fits an exact multiple of payload items
  // and is properly aligned to be assigned via cast.
  if (!buffer || !buffer->data || buffer->size % sizeof(payload) != 0 ||
      (uintptr_t)buffer->data % sizeof(payload) != 0) {
    return false;
  }

  for (size_t i = 0; i < buffer->size / sizeof(payload); i++) {
    ((uint32_t*)buffer->data)[i] = payload;
  }

  return true;
}

typedef struct IoContext {
  FuchsiaFirmwareStorage* ops;
  const GptData* gpt_data;
  const char* name;
} IoContext;

static bool IoWrite(void* ctx, uint64_t device_offset, SparseIoBufferHandle src,
                    uint64_t src_offset, size_t size) {
  IoContext* io = (IoContext*)(ctx);
  IoBuffer* src_buffer = (IoBuffer*)(src);
  if (src_offset + size > src_buffer->size) {
    return false;
  }
  return FuchsiaFirmwareStorageGptWrite(io->ops, io->gpt_data, io->name, device_offset, size,
                                        src_buffer->data + src_offset);
}

bool FuchsiaIsSparseImage(const uint8_t* src, size_t size) {
  static SparseIoBufferOps handle_interface = {
      .size = IoBufferSize,
      .read = IoBufferRead,
      .fill = IoBufferFill,
  };
  IoBuffer src_buffer = {
      .size = size,
      .data = (uint8_t*)src,
  };
  return sparse_is_sparse_image(&handle_interface, &src_buffer);
}

bool FuchsiaWriteSparseImage(FuchsiaFirmwareStorage* ops, const GptData* gpt_data, const char* name,
                             uint8_t* src, size_t size) {
  IoContext context = {ops, gpt_data, name};
  IoBuffer fill_buffer = {
      .size = ops->fill_buffer_size_bytes,
      .data = ops->fill_buffer,
  };
  SparseIoBufferOps handle_ops = {
      .size = IoBufferSize,
      .read = IoBufferRead,
      .fill = IoBufferFill,
  };
  SparseIoInterface io = {
      .ctx = &context,
      .fill_handle = &fill_buffer,
      .handle_ops = handle_ops,
      .write = IoWrite,
  };
  IoBuffer src_buffer = {
      .size = size,
      .data = src,
  };
  return sparse_unpack_image(&io, sparse_logger, &src_buffer);
}
