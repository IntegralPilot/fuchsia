// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#define DEBUG_ASSERT(x) assert(x)
#define ASSERT(x) assert(x)
#define DEBUG_ASSERT_MSG(x, msg, ...) assert(x)

#include <lib/spsc_buffer/spsc_buffer.h>
#include <zircon/types.h>

#include <new>

namespace {

// A C++ allocator that uses standard new/delete. This is for a user-space test
// environment where the Rust test runner links against the C++ standard library.
struct CppHeapAllocator {
  static ktl::byte* Allocate(uint32_t size) { return new ktl::byte[size]; }

  static void Free(ktl::byte* ptr) { delete[] ptr; }
};

using CppSpscBuffer = SpscBuffer<CppHeapAllocator>;

}  // namespace

extern "C" {

// Allocates and initializes an SpscBuffer using new.
CppSpscBuffer* cpp_spsc_allocate(uint32_t size) {
  CppSpscBuffer* spsc = new CppSpscBuffer();
  zx_status_t status = spsc->Init(size);
  if (status != ZX_OK) {
    delete spsc;
    return nullptr;
  }
  return spsc;
}

// Frees the SpscBuffer and its backing store.
void cpp_spsc_free(CppSpscBuffer* spsc) { delete spsc; }

// C++ writes data to the buffer.
// Returns ZX_OK on success.
zx_status_t cpp_spsc_write(CppSpscBuffer* spsc, const uint8_t* data, uint32_t len) {
  auto result = spsc->Reserve(len);
  if (result.is_error()) {
    return result.status_value();
  }
  auto& reservation = result.value();
  reservation.Write(ktl::span<const ktl::byte>(reinterpret_cast<const ktl::byte*>(data), len));
  reservation.Commit();
  return ZX_OK;
}

// C++ reads data from the buffer.
// Returns the number of bytes read, or a negative error code (zx_status_t).
int32_t cpp_spsc_read(CppSpscBuffer* spsc, uint8_t* dst, uint32_t len) {
  auto copy_fn = [dst](uint32_t offset, ktl::span<ktl::byte> src) -> zx_status_t {
    ktl::copy(src.begin(), src.end(), reinterpret_cast<ktl::byte*>(dst + offset));
    return ZX_OK;
  };
  auto result = spsc->Read(copy_fn, len);
  if (result.is_error()) {
    return result.status_value();
  }
  return static_cast<int32_t>(result.value());
}

}  // extern "C"
