// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_REQUEST_FIDL_H_
#define SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_REQUEST_FIDL_H_

#include <lib/io-buffer/phys-iter.h>

#include "src/devices/usb/lib/usb/include/usb/internal/request-fidl.h"

namespace usb {

// Exposed to header users.
using internal::EndpointType;
using internal::MappedVmo;

using FidlRequest = internal::FidlRequest<io_buffer::PhysIter>;
using FidlRequestPool = internal::FidlRequestPool<FidlRequest>;

// Template specialization for internal::FidlRequest<io_buffer::PhysIter>.
template <>
inline io_buffer::PhysIter FidlRequest::phys_iter(size_t idx, size_t max_length) const {
  ZX_ASSERT(request_.data()->at(idx).size());
  ZX_ASSERT(pinned_vmos_.find(idx) != pinned_vmos_.end());
  auto length = request_.data()->at(idx).size().value();
  // phys_iter is only called for Buffer::Tag::kData via PhysMap pinned_vmos_.
  // PhysMap pins the sub-range and already offsets phys_list[0] by any sub-page offset,
  // so vmo_offset must be 0 to avoid double-offsetting.
  constexpr uint64_t vmo_offset = 0;
  phys_iter_buffer_t buf = {.phys = pinned_vmos_.at(idx).phys_list,
                            .phys_count = pinned_vmos_.at(idx).phys_count,
                            .length = length,
                            .vmo_offset = vmo_offset,
                            .sg_list = nullptr,
                            .sg_count = 0};
  return io_buffer::PhysIter(buf, max_length);
}

}  // namespace usb

#endif  // SRC_DEVICES_USB_LIB_USB_INCLUDE_USB_REQUEST_FIDL_H_
