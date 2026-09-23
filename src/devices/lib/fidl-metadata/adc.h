// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_FIDL_METADATA_ADC_H_
#define SRC_DEVICES_LIB_FIDL_METADATA_ADC_H_

#include <lib/stdcompat/span.h>
#include <lib/zx/result.h>
#include <stdint.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <string>
#include <vector>

#define DECL_ADC_CHANNEL(channel) \
  { channel, #channel }
#define DECL_ADC_CHANNEL_WITH_NAME(channel, name) \
  { channel, name }

namespace fidl_metadata::adc {

struct Channel {
  uint32_t idx;
  std::string name;
};

zx::result<std::vector<uint8_t>> AdcChannelsToFidl(cpp20::span<const Channel> channels);

}  // namespace fidl_metadata::adc

#endif  // SRC_DEVICES_LIB_FIDL_METADATA_ADC_H_
