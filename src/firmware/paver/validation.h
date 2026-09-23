// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_VALIDATION_H_
#define SRC_FIRMWARE_PAVER_VALIDATION_H_

#include <lib/zbi-format/zbi.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include "src/firmware/paver/device-partitioner.h"

namespace paver {

// Extract the payload out of the given ZBI image.
//
// Return "true" on success, or "false" if the input data is invalid.
//
// On success, sets "header" to the header of the ZBI image, and
// "payload" to the payload of the ZBI. Both are guaranteed to be
// completed contained in "data".
bool ExtractZbiPayload(std::span<const uint8_t> data, const zbi_header_t** header,
                       std::span<const uint8_t>* payload);

// Perform some basic safety checks to ensure the given payload is a valid ZBI
// for the given architecture.
bool IsValidKernelZbi(Arch arch, std::span<const uint8_t> data);

// Perform some basic safety checks to ensure the given payload is a valid Android image.
bool IsValidAndroidKernel(std::span<const uint8_t> data);
bool IsValidAndroidVendorKernel(std::span<const uint8_t> data);

// Perform some basic safety checks to ensure the given payload is a valid ChromeOS
// kernel image.
bool IsValidChromeOsKernel(std::span<const uint8_t> data);

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_VALIDATION_H_
