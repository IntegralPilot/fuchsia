// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zircon_device.h"

#include <dirent.h>
#include <fidl/fuchsia.hardware.intel.hda/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/client.h>
#include <stdio.h>
#include <zircon/device/intel-hda.h>

#include <limits>

#include <fbl/unique_fd.h>

namespace audio {
namespace intel_hda {

namespace {

// Wraps a call to ComponentController.GetChannel or CodecController.GetChannel.
template <typename Protocol>
zx::result<zx::channel> GetChannel(const char* device_path) {
  auto channel = component::Connect<Protocol>(device_path);
  if (!channel.is_ok()) {
    printf("[%s] Failed to connect to device channel (%d)\n", device_path, channel.status_value());
    return channel.take_error();
  }
  auto result = fidl::WireCall(*channel)->GetChannel();
  if (!result.ok()) {
    printf("[%s] Failed to fetch device channel (%d)\n", device_path, result.status());
    return zx::error(result.status());
  }
  return zx::ok(std::move(result->ch));
}

}  // namespace

uint32_t ZirconDevice::transaction_id_ = 0;

zx_status_t ZirconDevice::Connect() {
  if (dev_channel_ != ZX_HANDLE_INVALID) {
    return ZX_OK;
  }
  if (!dev_name_) {
    return ZX_ERR_NO_MEMORY;
  }

  zx::result<zx::channel> dev_channel;

  switch (type_) {
    case Type::Controller:
      dev_channel = GetChannel<fuchsia_hardware_intel_hda::ControllerDevice>(dev_name_);
      break;

    case Type::Codec:
      dev_channel = GetChannel<fuchsia_hardware_intel_hda::ControllerDevice>(dev_name_);
      break;

    default:
      return ZX_ERR_INTERNAL;
  }

  if (!dev_channel.is_ok()) {
    return dev_channel.status_value();
  }

  dev_channel_ = std::move(dev_channel.value());
  return ZX_OK;
}

void ZirconDevice::Disconnect() { dev_channel_.reset(); }

zx_status_t ZirconDevice::CallDevice(const zx_channel_call_args_t& args, zx::duration timeout) {
  uint32_t resp_size;
  uint32_t resp_handles;
  zx::time deadline =
      timeout == zx::duration::infinite() ? zx::time::infinite() : zx::deadline_after(timeout);

  return dev_channel_.call(0, deadline, &args, &resp_size, &resp_handles);
}

zx_status_t ZirconDevice::Enumerate(void* ctx, const char* const dev_path, EnumerateCbk cbk) {
  static constexpr size_t FILENAME_SIZE = 256;

  struct dirent* de;
  DIR* dir = opendir(dev_path);
  zx_status_t res = ZX_OK;
  char buf[FILENAME_SIZE];

  if (!dir)
    return ZX_ERR_NOT_FOUND;

  while ((de = readdir(dir)) != NULL) {
    uint32_t id;
    if (sscanf(de->d_name, "%u", &id) == 1) {
      size_t total = 0;

      total += snprintf(buf + total, sizeof(buf) - total, "%s/", dev_path);
      total += snprintf(buf + total, sizeof(buf) - total, "%03u", id);

      res = cbk(ctx, id, buf);
      if (res != ZX_OK)
        goto done;
    }
  }

done:
  closedir(dir);
  return res;
}

}  // namespace intel_hda
}  // namespace audio
