// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/fidl-metadata/i2c.h"

#include <fidl/fuchsia.hardware.i2c.businfo/cpp/wire.h>

// Put this static assert to avoid having all clients to need to take a dep on
// fuchsia.hardware.i2c.businfo_cpp_wire.
static_assert(fuchsia_hardware_i2c::wire::kMaxI2CNameLen == kMaxI2cNameLength);

namespace fidl_metadata::i2c {
zx::result<std::vector<uint8_t>> I2CChannelsToFidl(const uint32_t bus_id,
                                                   const cpp20::span<const Channel> channels) {
  fidl::Arena allocator;
  fidl::VectorView<fuchsia_hardware_i2c_businfo::wire::I2CChannel> i2c_channels(allocator,
                                                                                channels.size());

  for (size_t i = 0; i < channels.size(); i++) {
    auto& chan = i2c_channels[i];
    auto& src_chan = channels[i];
    chan.Allocate(allocator);

    chan.set_address(src_chan.address);
    chan.set_pid(src_chan.pid);
    chan.set_did(src_chan.did);
    chan.set_vid(src_chan.vid);
    chan.set_name(fidl::ObjectView<fidl::StringView>(allocator, allocator, src_chan.name));
  }

  fuchsia_hardware_i2c_businfo::wire::I2CBusMetadata metadata(allocator);
  metadata.set_bus_id(bus_id);
  metadata.set_channels(allocator, i2c_channels);

  return zx::result<std::vector<uint8_t>>{
      fidl::Persist(metadata).map_error(std::mem_fn(&fidl::Error::status))};
}
}  // namespace fidl_metadata::i2c
