// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_INTERNAL_REQUEST_FIDL_H_
#define SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_INTERNAL_REQUEST_FIDL_H_

#include <fidl/fuchsia.hardware.usb.request/cpp/fidl.h>

// Figure out which logging runtime and function to use, and bind it to the FDF_LOG() macro where
// necessary. The possible cases are:
//   1. DFv2 runtime using the comapt shim. To link against the compat shim, build with
//      -DDFV2_COMPAT_LOGGING.
//   2. DFv1 runtime using the DDK. To use link against the DDK, build with -DDFV1_LOGGING.
//   3. (default) DFv2 runtime using the native DFv2 logging runtime. With Neither flag in effect,
//      link against the native DFv2 runtime.
//
// If building against the SDK, the binary must be linked against the native DFv2 logging runtime as
// neither the DDK nor compat shim are baked into the SDK.
#ifdef DFV2_COMPAT_LOGGING
#include <lib/driver/compat/cpp/logging.h>  // nogncheck
#elif defined(DFV1_LOGGING)
#include <lib/ddk/debug.h>  // nogncheck
#define FDF_LOG(args, ...) zxlogf(args, __VA_ARGS__)
#else
#include "sdk/lib/driver/logging/cpp/logger.h"  // nogncheck
#endif

#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <zircon/status.h>

#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <queue>
#include <span>
#include <utility>
#include <vector>

#include "src/devices/usb/lib/usb/align.h"

namespace usb::internal {

// EndpointType: One of 4 endpoint types as defined by USB.
enum EndpointType : uint8_t {
  UNKNOWN,

  CONTROL,
  BULK,
  ISOCHRONOUS,
  INTERRUPT,
};

// MappedVmo is a container that keeps track of the virtual address and size of the VMO mapped.
struct MappedVmo {
  // addr: virtual address.
  zx_vaddr_t addr;
  // size: size of VMO that was mapped to this virtual address.
  size_t size;
};

// FidlRequest is a wrapper around a fuchsia_hardware_usb_request::Request implementing common
// functionality. Especially, FidlRequest keeps track of VMOs that were pinned upon PhysMap() and
// unpins them upon destruction.
template <typename PhysIterType>
class FidlRequest {
 public:
  using get_mapped_func_t = std::function<zx::result<std::optional<MappedVmo>>(
      const fuchsia_hardware_usb_request::Buffer& buffer)>;

  FidlRequest() = default;
  explicit FidlRequest(EndpointType ep_type) {
    switch (ep_type) {
      case CONTROL:
        set_control();
        break;
      case BULK:
        set_bulk();
        break;
      case ISOCHRONOUS:
        set_isochronous();
        break;
      case INTERRUPT:
        set_interrupt();
        break;
      default:
        ZX_ASSERT(false);
    };
  }
  explicit FidlRequest(fuchsia_hardware_usb_request::Request request)
      : request_(std::move(request)) {}

  // Disallow copy and assign, allow move.
  FidlRequest(FidlRequest&& request) = default;
  FidlRequest& operator=(FidlRequest&& request) = delete;
  FidlRequest(const FidlRequest&) = delete;
  FidlRequest& operator=(const FidlRequest&) = delete;

  ~FidlRequest() { Unpin(); }

  FidlRequest& set_control(fuchsia_hardware_usb_descriptor::UsbSetup setup = {}) {
    request_.information(fuchsia_hardware_usb_request::RequestInfo::WithControl(
        fuchsia_hardware_usb_request::ControlRequestInfo().setup(std::move(setup))));
    return *this;
  }
  FidlRequest& set_bulk() {
    request_.information(fuchsia_hardware_usb_request::RequestInfo::WithBulk(
        fuchsia_hardware_usb_request::BulkRequestInfo()));
    return *this;
  }
  FidlRequest& set_isochronous(uint64_t frame_id = 0) {
    request_.information(fuchsia_hardware_usb_request::RequestInfo::WithIsochronous(
        fuchsia_hardware_usb_request::IsochronousRequestInfo().frame_id(frame_id)));
    return *this;
  }
  FidlRequest& set_interrupt() {
    request_.information(fuchsia_hardware_usb_request::RequestInfo::WithInterrupt(
        fuchsia_hardware_usb_request::InterruptRequestInfo()));
    return *this;
  }

  FidlRequest& add_vmo_id(uint64_t vmo_id, size_t size = 0, size_t offset = 0) {
    if (!request_.data().has_value()) {
      request_.data().emplace();
    }

    request_.data()
        ->emplace_back()
        .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(vmo_id))
        .offset(offset)
        .size(size);
    return *this;
  }
  FidlRequest& add_data(std::vector<uint8_t> data = {}, size_t size = 0, size_t offset = 0) {
    if (!request_.data().has_value()) {
      request_.data().emplace();
    }

    request_.data()
        ->emplace_back()
        .buffer(fuchsia_hardware_usb_request::Buffer::WithData(std::move(data)))
        .offset(offset)
        .size(size);
    return *this;
  }
  FidlRequest& clear_buffers() {
    for (auto& d : *request_.data()) {
      d.offset(0);
      d.size(0);
      if (d.buffer()->Which() == fuchsia_hardware_usb_request::Buffer::Tag::kData) {
        d.buffer()->data()->clear();
      }
    }
    return *this;
  }
  FidlRequest& reset_buffers(const get_mapped_func_t& get_mapped) {
    for (auto& d : *request_.data()) {
      d.offset(0);
      switch (d.buffer()->Which()) {
        case fuchsia_hardware_usb_request::Buffer::Tag::kVmoId: {
          auto mapped = get_mapped(*d.buffer());
          ZX_ASSERT(mapped.is_ok());
          d.size(mapped->size);
        } break;
        case fuchsia_hardware_usb_request::Buffer::Tag::kData:
          d.size(fuchsia_hardware_usb_request::kMaxTransferSize);
          break;
        default:
          break;
      }
    }
    return *this;
  }

  // CopyTo: tries to copy `size` bytes from `buffer` to contiguous `request` buffers from
  // `offset`. Returns the number of bytes copied for each buffer.
  //
  // Cache management is the responsibility of the user. See `CachedCopyTo` below for a
  // version of this method that performs cache management on the user's behalf.
  std::vector<size_t> CopyTo(size_t offset, const void* buffer, size_t size,
                             const get_mapped_func_t& get_mapped) {
    std::vector<size_t> cp_sizes(request_.data()->size(), 0);
    if (size == 0) {
      return cp_sizes;
    }
    const uint8_t* start = static_cast<const uint8_t*>(buffer);
    size_t todo = size;
    size_t cur_offset = offset;
    int32_t i = -1;
    for (auto& d : *request_.data()) {
      // Accumulator accounting done at head of loop due to multiple exit logic paths.
      i++;
      auto mapped = get_mapped(*d.buffer());
      if (mapped.is_error()) {
        return cp_sizes;
      }

      uint8_t* addr;
      size_t buffer_size;
      if (!mapped.value()) {
        // Buffer is fuchsia_hardware_usb_request::Buffer::Tag::kData
        if (*d.size() > 0) {
          buffer_size = *d.size();
        } else {
          buffer_size = static_cast<size_t>(fuchsia_hardware_usb_request::kMaxTransferSize);
          if (buffer_size > *d.offset()) {
            buffer_size -= *d.offset();
          } else {
            buffer_size = 0;
          }
        }
        d.buffer()->data()->resize(std::min(cur_offset + todo, buffer_size));
        addr = d.buffer()->data()->data();
      } else {
        if (*d.size() > 0) {
          buffer_size = *d.size();
        } else {
          buffer_size = mapped->size;
          if (buffer_size > *d.offset()) {
            buffer_size -= *d.offset();
          } else {
            buffer_size = 0;
          }
        }
        addr = reinterpret_cast<uint8_t*>(static_cast<uintptr_t>(mapped->addr)) + *d.offset();
      }

      if (cur_offset >= buffer_size) {
        cur_offset -= buffer_size;
        continue;
      }

      size_t cp_size = std::min(todo, buffer_size - cur_offset);
      memcpy(addr + cur_offset, start, cp_size);
      if (*d.size() < cur_offset + cp_size) {
        d.size(cur_offset + cp_size);
      }
      cur_offset = 0;
      start += cp_size;
      todo -= cp_size;
      cp_sizes[i] = cp_size;

      if (todo == 0) {
        // Break out early.
        break;
      }
    }

    return cp_sizes;
  }

  std::vector<size_t> CopyTo(size_t offset, std::span<const uint8_t> buffer,
                             const get_mapped_func_t& get_mapped) {
    return CopyTo(offset, buffer.data(), buffer.size(), get_mapped);
  }

  // CopyFrom: tries to copy `size` bytes from `request` (starting from `offset`) to `buffer`.
  // Returns the number of bytes copied for each buffer.
  //
  // Cache management is the responsibility of the user. See `CachedCopyFrom` below for a
  // version of this method that performs cache management on the user's behalf.
  std::vector<size_t> CopyFrom(size_t offset, void* buffer, size_t size,
                               const get_mapped_func_t& get_mapped) {
    std::vector<size_t> cp_sizes(request_.data()->size(), 0);
    if (size == 0) {
      return cp_sizes;
    }
    uint8_t* start = static_cast<uint8_t*>(buffer);
    size_t todo = size;
    size_t cur_offset = offset;
    int32_t i = -1;
    for (const auto& d : *request_.data()) {
      // Accumulator accounting done at head of loop due to multiple exit logic paths.
      i++;
      if (cur_offset >= *d.size()) {
        cur_offset -= *d.size();
        continue;
      }

      auto mapped = get_mapped(*d.buffer());
      if (mapped.is_error()) {
        return cp_sizes;
      }

      zx_vaddr_t addr;
      if (!mapped.value()) {
        // Buffer is fuchsia_hardware_usb_request::Buffer::Tag::kData.
        addr = reinterpret_cast<zx_vaddr_t>(d.buffer()->data()->data());
      } else {
        addr = mapped->addr + *d.offset();
      }

      size_t cp_size = std::min(todo, *d.size() - cur_offset);
      memcpy(start, reinterpret_cast<uint8_t*>(static_cast<uintptr_t>(addr)) + cur_offset, cp_size);
      cur_offset = 0;
      start += cp_size;
      todo -= cp_size;
      cp_sizes[i] = cp_size;

      if (todo == 0) {
        // Break out early.
        break;
      }
    }

    return cp_sizes;
  }

  std::vector<size_t> CopyFrom(size_t offset, std::span<uint8_t> buffer,
                               const get_mapped_func_t& get_mapped) {
    return CopyFrom(offset, buffer.data(), buffer.size(), get_mapped);
  }

  // About CacheFlush and CacheFlushInvalidate: CacheFlush or CacheFlush invalidate MUST always be
  // called after data is written to the VMO and before the data is processed on schedule (see
  // comment in endpoint.fidl on `RequestQueue`).
  // CacheFlush should be called for operations where data is to be written out, but not read in
  // and CacheFlushInvalidate should be called for operations where data needs to be read in.
  // CacheFlush and CacheFlushInvalidate flush and invalidate cache for all buffer regions of a
  // request.
  zx_status_t CacheFlushInvalidate(get_mapped_func_t get_mapped) {
    return CacheRangeHelper(0, length(), ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE,
                            get_mapped);
  }

  zx_status_t CacheFlushInvalidate(get_mapped_func_t get_mapped, size_t offset, size_t size) {
    return CacheRangeHelper(offset, size, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE,
                            get_mapped);
  }

  zx_status_t CacheFlush(get_mapped_func_t get_mapped) {
    return CacheRangeHelper(0, length(), ZX_CACHE_FLUSH_DATA, get_mapped);
  }

  zx_status_t CacheFlush(get_mapped_func_t get_mapped, size_t offset, size_t size) {
    return CacheRangeHelper(offset, size, ZX_CACHE_FLUSH_DATA, get_mapped);
  }

  // Similar to `CopyTo`, copies `size` bytes from `buffer` to contiguous request buffers starting
  // at `offset`. To ensure cache coherency, this method flushes the cache after copying.
  //
  // Only the range `[offset, offset + size)` is flushed.
  //
  // For multi-part writes into a request buffer (such as copying a header followed by payload
  // data), use `CopyTo` for intermediate writes and only call `CachedCopyTo` on the last
  // write (or use an explicit `CacheFlush` at the end) to avoid flushing cache lines multiple
  // times.
  //
  // Parameters:
  //  * offset: Byte offset into the request buffers to start writing.
  //  * buffer: Source buffer to read data from.
  //  * size: Number of bytes to copy.
  //  * get_mapped: Callback to resolve virtual mapping information for buffer tags.
  // Returns a vector of byte counts copied to each request buffer on success, or an error status
  // if `CacheFlush` fails, mapping resolution fails, or `offset` is out of bounds
  // (`ZX_ERR_OUT_OF_RANGE`).
  zx::result<std::vector<size_t>> CachedCopyTo(size_t offset, const void* buffer, size_t size,
                                               const get_mapped_func_t& get_mapped) {
    if (size == 0) {
      return zx::ok(std::vector<size_t>(request_.data()->size(), 0));
    }
    if (offset >= capacity(get_mapped)) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    auto cp_sizes = CopyTo(offset, buffer, size, get_mapped);
    if (zx_status_t status = CacheFlush(get_mapped, offset, size); status != ZX_OK) {
      FDF_LOG(ERROR, "CacheFlush(): %s", zx_status_get_string(status));
      return zx::error(status);
    }
    return zx::ok(std::move(cp_sizes));
  }

  // Similar to `CopyFrom`, copies `size` bytes from contiguous request buffers starting at
  // `offset` into `buffer`. To ensure cache coherency, this method invalidates the cache before
  // copying.
  //
  // Only the range `[offset, offset + size)` is invalidated.
  //
  // For multi-part reads from a request buffer (such as reading a header followed by payload
  // data), call `CachedCopyFrom` on the first read (or use an explicit `CacheFlushInvalidate`
  // before reading) and use `CopyFrom` for subsequent reads to avoid invalidating cache lines
  // multiple times.
  //
  // Parameters:
  //  * offset: Byte offset into the request buffers to start reading.
  //  * buffer: Destination buffer to write data into.
  //  * size: Number of bytes to copy.
  //  * get_mapped: Callback to resolve virtual mapping information for buffer tags.
  //
  // Returns a vector of byte counts copied from each request buffer on success, or an error status
  // if `CacheFlushInvalidate` fails, mapping resolution fails, or `offset` is out of bounds
  // (`ZX_ERR_OUT_OF_RANGE`).
  zx::result<std::vector<size_t>> CachedCopyFrom(size_t offset, void* buffer, size_t size,
                                                 const get_mapped_func_t& get_mapped) {
    if (size == 0) {
      return zx::ok(std::vector<size_t>(request_.data()->size(), 0));
    }
    if (offset >= length()) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    if (zx_status_t status = CacheFlushInvalidate(get_mapped, offset, size); status != ZX_OK) {
      FDF_LOG(ERROR, "CacheFlushInvalidate(): %s", zx_status_get_string(status));
      return zx::error(status);
    }
    return zx::ok(CopyFrom(offset, buffer, size, get_mapped));
  }

  zx::result<std::vector<size_t>> CachedCopyTo(size_t offset, std::span<const uint8_t> buffer,
                                               const get_mapped_func_t& get_mapped) {
    return CachedCopyTo(offset, buffer.data(), buffer.size(), get_mapped);
  }

  zx::result<std::vector<size_t>> CachedCopyFrom(size_t offset, std::span<uint8_t> buffer,
                                                 const get_mapped_func_t& get_mapped) {
    return CachedCopyFrom(offset, buffer.data(), buffer.size(), get_mapped);
  }

  // CacheHelper flushes and invalidates cache for a slice of a specified buffer region.
  zx_status_t CacheHelper(const fuchsia_hardware_usb_request::BufferRegion& buffer,
                          size_t offset_in_region, size_t flush_size, uint32_t options,
                          const get_mapped_func_t& get_mapped) {
    if (flush_size == 0 || *buffer.size() == 0) {
      return ZX_OK;
    }
    auto mapped = get_mapped(*buffer.buffer());
    if (mapped.is_error()) {
      return mapped.error_value();
    }
    if (!mapped.value()) {
      return ZX_OK;
    }

    zx_status_t status =
        zx_cache_flush(reinterpret_cast<void*>(static_cast<uintptr_t>(mapped->addr) +
                                               *buffer.offset() + offset_in_region),
                       flush_size, options);
    if (status != ZX_OK) {
      return status;
    }
    return ZX_OK;
  }

  // CacheRangeHelper iterates across scatter-gather `BufferRegion` segments in the request and
  // performs cache maintenance (flushing or invalidation) strictly on the byte slice
  // `[offset, offset + size)`.
  //
  // The method tracks `cur_offset` across multiple contiguous buffer regions:
  //  - If `cur_offset >= *d.size()`, the target slice starts in a subsequent region, so we subtract
  //    the region's size from `cur_offset` and continue.
  //  - Once inside the target range, we compute `flush_size` as the minimum of remaining `todo`
  //    bytes
  //    and remaining bytes in the current region (`*d.size() - cur_offset`).
  //  - After flushing the slice in the current region via `CacheHelper`, `cur_offset` is reset to 0
  //    for any subsequent regions until all `todo` bytes are processed.
  zx_status_t CacheRangeHelper(size_t offset, size_t size, uint32_t options,
                               const get_mapped_func_t& get_mapped) {
    if (!request_.data().has_value() || size == 0) {
      return ZX_OK;
    }
    if (offset >= capacity(get_mapped)) {
      return ZX_ERR_OUT_OF_RANGE;
    }
    zx_status_t ret_status = ZX_OK;
    size_t todo = size;
    size_t cur_offset = offset;
    for (const auto& d : *request_.data()) {
      if (*d.size() == 0) {
        continue;
      }
      if (cur_offset >= *d.size()) {
        cur_offset -= *d.size();
        continue;
      }
      size_t flush_size = std::min(todo, *d.size() - cur_offset);
      zx_status_t status = CacheHelper(d, cur_offset, flush_size, options, get_mapped);
      if (status != ZX_OK) {
        ret_status = status;
      }
      cur_offset = 0;
      todo -= flush_size;
      if (todo == 0) {
        break;
      }
    }
    return ret_status;
  }

  // Pins VMOs if needed. For
  //  - fuchsia_hardware_usb_request::Buffer::Tag::kVmoId -- Uses preregistered VMO. Does nothing.
  //  - fuchsia_hardware_usb_request::Buffer::Tag::kVmo   -- Pins VMO.
  //  - fuchsia_hardware_usb_request::Buffer::Tag::kData  -- Creates, maps, pins, VMO. Copies data
  //                                                         to VMO. (Unmapped and unpinned on
  //                                                         Unpin()).
  zx_status_t PhysMap(const zx::bti& bti) {
    int64_t idx = -1;
    for (auto& d : *request_.data()) {
      idx++;
      zx_handle_t vmo_handle = ZX_HANDLE_INVALID;
      zx_vaddr_t mapped_addr = 0;
      switch (d.buffer()->Which()) {
        case fuchsia_hardware_usb_request::Buffer::Tag::kVmoId:
          // Pre-registered and already pinned. Does not need to be pinned again.
          continue;
        case fuchsia_hardware_usb_request::Buffer::Tag::kData: {
          // The price to pay for using fuchsia_hardware_usb_request::Buffer::Tag::kData instead
          // of VMOs is that a VMO needs to be created, mapped, pinned, data needs to be copied
          // to/from data buffer in both directions regardless of endpoint direction, cache needs
          // to be flushed, vmo then unpinned, and then unmapped.
          zx::vmo vmo;
          zx_status_t status = zx::vmo::create(*d.size(), 0, &vmo);
          if (status != ZX_OK) {
            return status;
          }

          uintptr_t mapped = 0;
          status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo,
                                              *d.offset(), *d.size(), &mapped);
          if (status != ZX_OK) {
            return status;
          }

          const uint8_t* src_buffer = d.buffer()->data()->data();
          memcpy(reinterpret_cast<void*>(mapped), src_buffer, *d.size());
          zx_cache_flush(reinterpret_cast<void*>(mapped), *d.size(),
                         ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
          mapped_addr = mapped;
          vmo_handle = vmo.release();
        } break;
        default:
          return ZX_ERR_NOT_SUPPORTED;
      }

      // zx_bti_pin returns whole pages, so take into account unaligned vmo
      // offset and length when calculating the amount of pages returned
      uint64_t page_offset = USB_ROUNDDOWN(*d.offset(), kPageSize);
      // The buffer size is the vmo size from offset 0.
      uint64_t page_length = *d.size() - page_offset;
      uint64_t pages = USB_ROUNDUP(page_length, kPageSize) / kPageSize;

      std::unique_ptr<zx_paddr_t[]> paddrs{new zx_paddr_t[pages]};
      const size_t sub_offset = page_offset & (kPageSize - 1);
      const size_t pin_offset = page_offset - sub_offset;
      const size_t pin_length = USB_ROUNDUP(page_length + sub_offset, kPageSize);

      if (pin_length / kPageSize != pages) {
        return ZX_ERR_INVALID_ARGS;
      }
      zx_handle_t pmt;
      uint32_t options = ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE;
      zx_status_t status = zx_bti_pin(bti.get(), options, vmo_handle, pin_offset, pin_length,
                                      paddrs.get(), pages, &pmt);
      if (status != ZX_OK) {
        FDF_LOG(ERROR, "zx_bti_pin(): %s", zx_status_get_string(status));
        return status;
      }

      // Account for the initial misalignment if any
      paddrs.get()[0] += sub_offset;

      pinned_vmos_[idx] = {
          pmt,
          paddrs.release(),
          pages,
          {mapped_addr, *d.size()},
      };
    }

    return ZX_OK;
  }

  // Unpins VMOs pinned by PhysMap.
  zx_status_t Unpin() {
    auto pinned_vmos = std::move(pinned_vmos_);
    for (const auto& [idx, pinned] : pinned_vmos) {
      if (request_.data()->at(idx).buffer()->Which() ==
          fuchsia_hardware_usb_request::Buffer::Tag::kData) {
        memcpy((*request_.data())[idx].buffer()->data()->data(),
               reinterpret_cast<void*>(static_cast<uintptr_t>(pinned.mapped.addr)),
               std::min((*request_.data())[idx].size().value(), pinned.mapped.size));

        zx_status_t status = zx::vmar::root_self()->unmap(
            reinterpret_cast<uintptr_t>(pinned.mapped.addr), pinned.mapped.size);
        ZX_DEBUG_ASSERT(status == ZX_OK);
      }

      zx_status_t status = zx_pmt_unpin(pinned.pmt);
      ZX_DEBUG_ASSERT(status == ZX_OK);
      delete[] pinned.phys_list;
    }
    return ZX_OK;
  }

  // Gets a PhysIterType for the buffer at request.data()->at(idx). PhysIterTypes return a
  // std::pair<zx_paddr_t, size_t> on iteration. For template specialization, see the public
  // headers outside the internal namespace.
  PhysIterType phys_iter(size_t idx, size_t max_length) const;

  fuchsia_hardware_usb_request::Request* operator->() { return &request_; }
  const fuchsia_hardware_usb_request::Request* operator->() const { return &request_; }
  const fuchsia_hardware_usb_request::Request& request() const { return request_; }
  // Ensures any removal of `request` is intentional and `Unpin` is called.
  fuchsia_hardware_usb_request::Request take_request() {
    zx_status_t status = Unpin();
    ZX_DEBUG_ASSERT(status == ZX_OK);
    return std::move(request_);
  }
  // Returns the total length of all data in the request. Saves to a variable for future use.
  size_t length() {
    if (!_length) {
      size_t len = 0;
      for (const auto& d : *request_.data()) {
        ZX_ASSERT(d.size());
        len += *d.size();
      }
      *_length = len;
    }
    return *_length;
  }

  // Returns the total capacity of all buffers in the request.
  size_t capacity(const get_mapped_func_t& get_mapped) const {
    size_t cap = 0;
    if (!request_.data().has_value()) {
      return 0;
    }
    for (const auto& d : *request_.data()) {
      if (*d.size() > 0) {
        cap += *d.size();
        continue;
      }
      auto mapped = get_mapped(*d.buffer());
      if (mapped.is_ok()) {
        if (!mapped.value()) {
          if (*d.offset() < static_cast<size_t>(fuchsia_hardware_usb_request::kMaxTransferSize)) {
            cap +=
                static_cast<size_t>(fuchsia_hardware_usb_request::kMaxTransferSize) - *d.offset();
          }
        } else {
          if (*d.offset() < mapped->size) {
            cap += mapped->size - *d.offset();
          }
        }
      }
    }
    return cap;
  }

 private:
  const size_t kPageSize = zx_system_get_page_size();

  // request_: FIDL request object.
  fuchsia_hardware_usb_request::Request request_;

  struct pinned_vmo_t {
    zx_handle_t pmt;
    uint64_t* phys_list;
    size_t phys_count;
    // mapped: only used for fuchsia_hardware_usb_request::Buffer::Tag::kData.
    MappedVmo mapped;
  };
  // pinned_vmos_: VMOs that were pinned by this request when PhysMap() was called. Will be
  // unpinned by when this request is destructed or when Unpin() is called. Indexed in the same
  // order as request_.data(), where only buffers that are
  // fuchsia_hardware_usb_request::Buffer::Tag::kVmoId are left empty.
  std::map<size_t, pinned_vmo_t> pinned_vmos_ = {};

  // _length: Total length saved so calculation doesn't have to be done multiple times.
  std::optional<size_t> _length = std::nullopt;
};

// FidlRequestPool: pool of FidlRequests.
template <typename RequestType>
class FidlRequestPool {
 public:
  // Add: called when adding a new request to the pool.
  void Add(RequestType&& request) __TA_EXCLUDES(mutex_) {
    std::scoped_lock _(mutex_);
    size_++;
    PutLocked(std::move(request));
  }

  // Remove: called when removing a request from the pool.
  std::optional<RequestType> Remove() __TA_EXCLUDES(mutex_) {
    std::scoped_lock _(mutex_);
    auto req = GetLocked();
    if (req.has_value()) {
      size_--;
    }
    return req;
  }

  std::optional<RequestType> Get() __TA_EXCLUDES(mutex_) {
    std::scoped_lock _(mutex_);
    return GetLocked();
  }

  // Put: called when a request (originally obtained from `get`) is returned to the pool.
  void Put(RequestType&& request) __TA_EXCLUDES(mutex_) {
    std::scoped_lock _(mutex_);
    PutLocked(std::move(request));
  }

  bool Full() __TA_EXCLUDES(mutex_) {
    std::scoped_lock _(mutex_);
    return free_reqs_.size() == size_;
  }

  bool Empty() __TA_EXCLUDES(mutex_) {
    std::scoped_lock _(mutex_);
    return free_reqs_.empty();
  }

  size_t GetInFlightCount() __TA_EXCLUDES(mutex_) {
    std::scoped_lock _(mutex_);
    return size_ - free_reqs_.size();
  }

  size_t GetTotalCount() __TA_EXCLUDES(mutex_) {
    std::scoped_lock _(mutex_);
    return size_;
  }

 private:
  std::optional<RequestType> GetLocked() __TA_REQUIRES(mutex_) {
    if (free_reqs_.empty()) {
      return std::nullopt;
    }

    auto req = std::move(free_reqs_.front());
    free_reqs_.pop();
    return std::move(req);
  }

  void PutLocked(RequestType&& request) __TA_REQUIRES(mutex_) {
    free_reqs_.emplace(std::move(request));
    ZX_DEBUG_ASSERT(free_reqs_.size() <= size_);
  }

  std::mutex mutex_;
  std::queue<RequestType> free_reqs_ __TA_GUARDED(mutex_);
  uint32_t size_ __TA_GUARDED(mutex_) = 0;
};

}  // namespace usb::internal

#endif  // SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_INTERNAL_REQUEST_FIDL_H_
