// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-peripheral/config-parser.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/trace/event.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstdint>
#include <map>
#include <string>

namespace usb_peripheral {

zx_status_t PeripheralConfigParser::SetCompositeProductDescription(uint8_t tag_mask,
                                                                   const std::string_view& desc) {
  TRACE_DURATION("usb-peripheral", __func__);
  if (tag_ & tag_mask) {
    fdf::error("Duplicate function for {}", desc);
    return ZX_ERR_WRONG_TYPE;
  }

  tag_ |= tag_mask;

  if (!kPidLookup.contains(tag_)) {
    fdf::error("No matching pid for this combination of functions: {:#x}", tag_);
    return ZX_ERR_WRONG_TYPE;
  }

  pid_ = kPidLookup.at(tag_);

  if (product_desc_.empty()) {
    product_desc_ += desc;
  } else {
    product_desc_ += kCompositeDeviceConnector;
    product_desc_ += desc;
  }

  return ZX_OK;
}

}  // namespace usb_peripheral
