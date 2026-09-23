// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/fidl-metadata/adc.h"

#include <fidl/fuchsia.hardware.adcimpl/cpp/fidl.h>

namespace fidl_metadata::adc {

zx::result<std::vector<uint8_t>> AdcChannelsToFidl(const cpp20::span<const Channel> channels) {
  std::vector<fuchsia_hardware_adcimpl::AdcChannel> adc_channels;
  for (const auto& src_chan : channels) {
    adc_channels.emplace_back(
        fuchsia_hardware_adcimpl::AdcChannel().idx(src_chan.idx).name(src_chan.name));
  }

  fuchsia_hardware_adcimpl::Metadata metadata;
  metadata.channels(std::move(adc_channels));

  return zx::result<std::vector<uint8_t>>{
      fidl::Persist(metadata).map_error(std::mem_fn(&fidl::Error::status))};
}

}  // namespace fidl_metadata::adc
