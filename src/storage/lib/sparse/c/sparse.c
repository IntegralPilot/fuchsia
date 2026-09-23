// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sparse.h"

#include <inttypes.h>
#include <sparse_format.h>
#include <stdbool.h>
#include <string.h>

// Can't reliably depend on endian macros, so just define them ourselves
#ifndef __BYTE_ORDER__
#error Compiler does not provide __BYTE_ORDER__
#endif

// The compiler provides it, use what it says
#ifndef LITTLE_ENDIAN
#define LITTLE_ENDIAN __ORDER_LITTLE_ENDIAN__
#endif

#ifndef BIG_ENDIAN
#define BIG_ENDIAN __ORDER_BIG_ENDIAN__
#endif

#ifndef BYTE_ORDER
#define BYTE_ORDER __BYTE_ORDER__
#endif

// We only want macros to convert from little-endian
// because that's what the fields in a sparse header are defined as.
#if BYTE_ORDER == BIG_ENDIAN
static inline uint64_t SWAP_64(uint64_t x) { return __builtin_bswap64(x); }
static inline uint32_t SWAP_32(uint32_t x) { return __builtin_bswap32(x); }
static inline uint16_t SWAP_16(uint16_t x) { return __builtin_bswap16(x); }

#define LE64(val) SWAP_64(val)
#define LE32(val) SWAP_32(val)
#define LE16(val) SWAP_16(val)
#else
#define LE64(val) (val)
#define LE32(val) (val)
#define LE16(val) (val)
#endif

static void DumpStruct(SparseLogFn log, const void* s, size_t size) {
#ifdef ENABLE_VERBOSE_SPARSE_LOG
  for (size_t i = 0; i < size; i++) {
    log("0x%02X ", ((uint8_t*)s)[i]);
  }
#endif
  log("\n");
}

// Check the values of the fields in header to make sure they conform to
// the expected semantics of a valid sparse file header.
//
// @header: The sparse header to be checked.
//
// Return true if it is valid, false otherwise.
static bool IsValidFileHeader(const sparse_header_t* header) {
  return header && LE32(header->magic) == SPARSE_HEADER_MAGIC &&
         LE16(header->major_version) <= 0x1 && sizeof(*header) <= LE16(header->file_hdr_sz) &&
         sizeof(chunk_header_t) <= LE16(header->chunk_hdr_sz) && LE32(header->blk_sz) % 4 == 0;
}

// Check that a sparse image header is valid, i.e. fields are set to reasonable values,
// and if it is valid copy the header to an aligned header struct.
//
// Returns true if the header is valid, false otherwise.
// The header output parameter is only valid if the function returns true.
static bool ValidateHeader(SparseIoBufferOps* handle_ops, SparseIoBufferHandle handle,
                           sparse_header_t* header) {
  return handle_ops->read(handle, 0, (uint8_t*)header, sizeof(*header)) &&
         IsValidFileHeader(header);
}

static bool ValidateChunkSize(const sparse_header_t* header, const chunk_header_t* chunk,
                              size_t payload_size) {
  return LE32(chunk->total_sz) == header->chunk_hdr_sz + payload_size;
}

int sparse_nop_logger(const char* msg, ...) { return 0; }

bool sparse_is_sparse_image(SparseIoBufferOps* handle_ops, SparseIoBufferHandle src) {
  if (handle_ops == NULL) {
    return false;
  }
  sparse_header_t header;
  return ValidateHeader(handle_ops, src, &header);
}

static bool sparse_write_fill(SparseIoInterface* io, SparseLogFn log, SparseIoBufferHandle src,
                              uint32_t fill, uint64_t out_offset, uint64_t write_bytes) {
  static bool fill_cache_valid = false;
  uint32_t tmp;
  if (!io->handle_ops.read(io->fill_handle, 0, (uint8_t*)&tmp, sizeof(tmp))) {
    log("Failed to read fill buffer\n");
    return false;
  }

  // Setup: populate the fill buffer with the fill data.
  if (!fill_cache_valid || tmp != fill) {
    // Minor optimization for repeated fill chunks with the same payload.
    fill_cache_valid = true;
    if (!io->handle_ops.fill(io->fill_handle, fill)) {
      log("Failed to populate fill buffer\n");
      return false;
    }
  }

  // Phase 1: write the entire fill buffer, possibly multiple times.
  // Remember that the fill buffer size must be a multiple of both the block size
  // and the storage buffer alignment.
  size_t fill_size = io->handle_ops.size(io->fill_handle);
  for (uint64_t i = 0; i < write_bytes / fill_size; i++, out_offset += fill_size) {
    if (!io->write(io->ctx, out_offset, io->fill_handle, 0, fill_size)) {
      log("Failed to write sparse fill block of size %zu\n", fill_size);
      return false;
    }
  }

  // Phase 2: write the smaller-than-fill-buffer-sized remainder.
  size_t trailing_bytes = write_bytes % fill_size;
  if (trailing_bytes) {
    if (!io->write(io->ctx, out_offset, io->fill_handle, 0, trailing_bytes)) {
      log("Failed to write sparse fill chunk of size %zu\n", trailing_bytes);
      return false;
    }
  }

  return true;
}

bool sparse_unpack_image(SparseIoInterface* io, SparseLogFn log, SparseIoBufferHandle src) {
  if (io == NULL || log == NULL) {
    return false;
  }
  const size_t size = io->handle_ops.size(src);

  sparse_header_t file_header;
  if (!io->handle_ops.read(src, 0, (uint8_t*)(&file_header), sizeof(sparse_header_t))) {
    log("Failed to read image header\n");
    return false;
  }
  if (!IsValidFileHeader(&file_header)) {
    log("invalid sparse image header: ");
    DumpStruct(log, &file_header, sizeof(file_header));
    return false;
  }

  uint64_t in_offset = file_header.file_hdr_sz;
  uint64_t out_offset = 0;
  uint64_t total_expected_size = (uint64_t)LE32(file_header.total_blks) * LE32(file_header.blk_sz);
  for (size_t i = 0; i < LE32(file_header.total_chunks); i++) {
    if (in_offset + sizeof(chunk_header_t) > size) {
      log("Chunk header exceeds handle length\n");
      return false;
    }
    chunk_header_t chunk;
    if (!io->handle_ops.read(src, in_offset, (uint8_t*)(&chunk), sizeof(chunk_header_t))) {
      log("Failed to read chunk header\n");
      return false;
    }

    in_offset += LE16(file_header.chunk_hdr_sz);

    switch (LE16(chunk.chunk_type)) {
      case CHUNK_TYPE_RAW:
        if (!ValidateChunkSize(&file_header, &chunk,
                               (size_t)LE32(file_header.blk_sz) * LE32(chunk.chunk_sz))) {
          log("inconsistent chunk size for chunk: ");
          DumpStruct(log, &chunk, sizeof(chunk));
          return false;
        }

        uint64_t data_len = (uint64_t)LE32(chunk.chunk_sz) * LE32(file_header.blk_sz);
        if (in_offset + data_len > size) {
          log("chunk exceeds handle length\n");
          return false;
        }
        if (out_offset + data_len > total_expected_size || out_offset + data_len < out_offset) {
          log("raw chunk write out of bounds: out_offset=%" PRIu64 " + data_len=%" PRIu64
              " > total_expected=%" PRIu64 "\n",
              out_offset, data_len, total_expected_size);
          return false;
        }
        if (!io->write(io->ctx, out_offset, src, in_offset, data_len)) {
          log("Failed to write %" PRIu64 " @ %" PRIu64 " from %" PRIu64 "\n", data_len, out_offset,
              in_offset);
          return false;
        }
        in_offset += data_len;
        out_offset += data_len;
        break;
      case CHUNK_TYPE_DONT_CARE:
        if (!ValidateChunkSize(&file_header, &chunk, 0)) {
          log("inconsistent chunk size for chunk: ");
          DumpStruct(log, &chunk, sizeof(chunk));
          return false;
        }
        uint64_t skip_len = (uint64_t)LE32(chunk.chunk_sz) * LE32(file_header.blk_sz);
        if (out_offset + skip_len > total_expected_size || out_offset + skip_len < out_offset) {
          log("dont_care chunk skips out of bounds: out_offset=%" PRIu64 " + skip_len=%" PRIu64
              " > total_expected=%" PRIu64 "\n",
              out_offset, skip_len, total_expected_size);
          return false;
        }
        out_offset += skip_len;
        break;
      case CHUNK_TYPE_FILL:
        if (!ValidateChunkSize(&file_header, &chunk, sizeof(uint32_t))) {
          log("inconsistent chunk size for chunk: ");
          DumpStruct(log, &chunk, sizeof(chunk));
          return false;
        }

        uint32_t fill;
        if (in_offset + sizeof(fill) > size) {
          log("chunk exceeds handle length\n");
          return false;
        }
        if (!io->handle_ops.read(src, in_offset, (uint8_t*)&fill, sizeof(fill))) {
          log("Failed to read fill word\n");
          return false;
        }
        fill = LE32(fill);
        in_offset += sizeof(fill);

        uint64_t write_size = (uint64_t)LE32(file_header.blk_sz) * LE32(chunk.chunk_sz);
        if (out_offset + write_size > total_expected_size || out_offset + write_size < out_offset) {
          log("fill chunk write out of bounds: out_offset=%" PRIu64 " + write_size=%" PRIu64
              " > total_expected=%" PRIu64 "\n",
              out_offset, write_size, total_expected_size);
          return false;
        }
        if (!sparse_write_fill(io, log, src, fill, out_offset, write_size)) {
          return false;
        }
        out_offset += write_size;
        break;
      case CHUNK_TYPE_CRC32:
        if (!ValidateChunkSize(&file_header, &chunk, sizeof(uint32_t))) {
          log("inconsistent chunk size for chunk: ");
          DumpStruct(log, &chunk, sizeof(chunk));
          return false;
        }
        // Ignore CRC32 chunks for the time being
        in_offset += sizeof(uint32_t);
        break;
      default:
        log("unexpected chunk type: 0x%08X\n", LE16(chunk.chunk_type));
        return false;
    }
  }

  if (out_offset != total_expected_size) {
    log("unpacked size mismatch: unpacked %" PRIu64 " blocks, expected %" PRIu64
        " blocks (total_expected_size=%" PRIu64 ")\n",
        out_offset / LE32(file_header.blk_sz), (uint64_t)LE32(file_header.total_blks),
        total_expected_size);
    return false;
  }

  return true;
}
