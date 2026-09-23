// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_PERCPU_WRITER_INCLUDE_LIB_PERCPU_WRITER_KERNEL_ASPACE_ALLOCATOR_H_
#define ZIRCON_KERNEL_LIB_PERCPU_WRITER_INCLUDE_LIB_PERCPU_WRITER_KERNEL_ASPACE_ALLOCATOR_H_

#include <cstdint>

#include <ktl/byte.h>

namespace percpu_writer {

// SpscBuffer requires us to specify how the underlying memory is managed -- it abstracts over a
// buffer provided by userspace or the kernel. Since we're writing to buffers in the kernel, we
// provide an allocator backed by a VmAspace.
class KernelAspaceAllocator {
 public:
  static ktl::byte* Allocate(uint32_t size, const char* buffer_name);
  static void Free(ktl::byte* ptr);
};
}  // namespace percpu_writer

#endif  // ZIRCON_KERNEL_LIB_PERCPU_WRITER_INCLUDE_LIB_PERCPU_WRITER_KERNEL_ASPACE_ALLOCATOR_H_
