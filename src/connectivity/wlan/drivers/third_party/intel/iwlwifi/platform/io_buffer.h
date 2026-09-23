// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_IO_BUFFER_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_IO_BUFFER_H_

#include <stdint.h>
#include <zircon/types.h>

#define IO_BUFFER_INVALID_PHYS 0

#define DDK_ROUNDUP(a, b)       \
  ({                            \
    const __typeof(a) _a = (a); \
    const __typeof(b) _b = (b); \
    ((_a + _b - 1) / _b * _b);  \
  })

enum {
  IO_BUFFER_RO = (0 << 0),        // map buffer read-only
  IO_BUFFER_RW = (1 << 0),        // map buffer read/write
  IO_BUFFER_CONTIG = (1 << 1),    // allocate physically contiguous buffer
  IO_BUFFER_UNCACHED = (1 << 2),  // map buffer with ZX_CACHE_POLICY_UNCACHED
  IO_BUFFER_FLAGS_MASK = IO_BUFFER_RW | IO_BUFFER_CONTIG | IO_BUFFER_UNCACHED,
};

struct io_buffer_t {
  zx_handle_t bti_handle;  // borrowed by library
  zx_handle_t vmo_handle;  // owned by library
  zx_handle_t pmt_handle;  // owned by library
  size_t size;
  zx_off_t offset;
  void* virt;
  // Points to the physical page backing the start of the VMO, if this
  // io buffer was created with the IO_BUFFER_CONTIG flag.
  zx_paddr_t phys;

  // This is used for storing the addresses of the physical pages backing non
  // contiguous buffers and is set by io_buffer_physmap().
  // Each entry in the list represents a whole page and the first entry
  // points to the page containing 'offset'.
  zx_paddr_t* phys_list;
  uint64_t phys_count;
};

zx_status_t io_buffer_init(struct io_buffer_t* buffer, zx_handle_t bti, size_t size,
                           uint32_t flags);
size_t io_buffer_size(const struct io_buffer_t* buffer, size_t offset);
void* io_buffer_virt(const struct io_buffer_t* buffer);
zx_paddr_t io_buffer_phys(const struct io_buffer_t* buffer);
zx_status_t io_buffer_cache_flush(struct io_buffer_t* buffer, zx_off_t offset, size_t length);
void io_buffer_release(struct io_buffer_t* buffer);

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_IO_BUFFER_H_
