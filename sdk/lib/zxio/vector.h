// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZXIO_VECTOR_H_
#define LIB_ZXIO_VECTOR_H_

#include <lib/zxio/zxio.h>
#include <zircon/types.h>

#include <algorithm>

// Calls the callback |fn| for every buffer in |vector| or until the callback
// returns |ZX_ERR_STOP|.
//
// **NOTE**: This method will return an error even if *some* progress has been
// made which is typically not what you want and does not match some POSIX APIs
// (generally on stream-like objects like Files, TCP sockets) which will return
// success for short reads and writes. If this method is used by such APIs, you
// most likely want to use |zxio_stream_do_vector| below, which will return
// partial success.
//
// TODO(https://fxbug.dev/377534441): This function is not thread-safe! Multithreaded reads/writes
// can become interleaved with each other.
template <typename F>
zx_status_t zxio_do_vector(const zx_iovec_t* vector, size_t vector_count, size_t* out_actual,
                           F fn) {
  size_t total = 0;
  for (size_t i = 0; i < vector_count; ++i) {
    size_t actual = 0;
    zx_status_t status = fn(vector[i].buffer, vector[i].capacity, total, &actual);
    switch (status) {
      case ZX_OK:
      case ZX_ERR_NEXT:
        total += actual;
        if (actual == vector[i].capacity) {
          continue;
        }
        // Short read.
        break;
      case ZX_ERR_STOP:
        total += actual;
        break;
      default:
        return status;
    }

    break;
  }
  *out_actual = total;
  return ZX_OK;
}

// This method is like |zxio_do_vector| except it implements stream semantics.
// That is, this method will swallow the error returned by |fn|, stop iterating
// the buffers and return |ZX_OK| as long as a previous call to |fn| successfully
// performed some I/O on at least one byte. If |fn| returns an error before any
// I/O was performed (no bytes were read/written), then that error is returned
// to the caller.
//
// TODO(https://fxbug.dev/377534441): This function is not thread-safe! Multithreaded reads/writes
// can become interleaved with each other.
template <typename F>
zx_status_t zxio_stream_do_vector(const zx_iovec_t* vector, size_t vector_count, size_t* out_actual,
                                  F fn) {
  return zxio_do_vector(
      vector, vector_count, out_actual,
      [&fn](void* buffer, size_t capacity, size_t total_so_far, size_t* out_actual) {
        zx_status_t status = fn(buffer, capacity, out_actual);
        if (status != ZX_OK) {
          if (total_so_far == 0) {
            return status;
          }
          return ZX_ERR_STOP;
        }
        return ZX_ERR_NEXT;
      });
}

// Like |zxio_stream_do_vector| except calls |fn| repeatedly with a given chunk length. Intended
// for use where the underlying callback has a maximum read/write size in a single request
// and is not peekable.
//
// TODO(https://fxbug.dev/377534441): This function is not thread-safe! Multithreaded reads/writes
// can become interleaved with each other.
template <size_t kChunkSize, typename F>
zx_status_t zxio_chunked_do_vector(const zx_iovec_t* vector, size_t vector_count,
                                   zxio_flags_t flags, size_t* out_actual, F fn) {
  if (flags) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  auto fn_chunked = [&fn](void* data, size_t capacity, size_t* out_actual) {
    auto buffer = static_cast<uint8_t*>(data);
    size_t total = 0;
    while (capacity > 0) {
      const size_t chunk = std::min(capacity, kChunkSize);
      size_t actual;
      const zx_status_t status = fn(buffer, chunk, &actual);
      if (status != ZX_OK) {
        if (total > 0) {
          break;
        }
        return status;
      }
      total += actual;
      if (actual != chunk) {
        break;
      }
      buffer += actual;
      capacity -= actual;
    }
    *out_actual = total;
    return ZX_OK;
  };
  return zxio_stream_do_vector(vector, vector_count, out_actual, std::move(fn_chunked));
}

#endif  // LIB_ZXIO_VECTOR_H_
