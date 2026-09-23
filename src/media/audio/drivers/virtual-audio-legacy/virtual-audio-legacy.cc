// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-legacy.h"

#include <lib/async/cpp/task.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <optional>

#include <ddktl/fidl.h>

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-codec.h"
#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-dai.h"
#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-device.h"
#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-stream.h"

namespace virtual_audio {

zx_status_t VirtualAudioLegacy::Bind(void* ctx, zx_device_t* parent) {
  auto device = std::make_unique<VirtualAudioLegacy>(parent);
  if (zx::result result = device->Init(); result.is_error()) {
    zxlogf(ERROR, "Failed to initialize device: %s", result.status_string());
    return result.status_value();
  }

  // On successful Add, Devmgr takes ownership (relinquished on DdkRelease), so transfer our
  // ownership to a local var, and let it go out of scope.
  [[maybe_unused]] auto _ = device.release();

  return ZX_OK;
}

zx::result<> VirtualAudioLegacy::Init() {
  zx_status_t status =
      DdkAdd(ddk::DeviceAddArgs("virtual-audio-legacy").set_flags(DEVICE_ADD_NON_BINDABLE));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add device: %s", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok();
}

void VirtualAudioLegacy::DdkUnbind(ddk::UnbindTxn txn) {
  if (devices_.empty()) {
    zxlogf(INFO, "Unbinding immediately: No devices to shutdown");
    txn.Reply();
    return;
  }

  unbind_txn_.emplace(std::move(txn));
  ShutdownAllDevices();
}

void VirtualAudioLegacy::DdkRelease() {
  // By now, all our lists should be empty and we release.
  ZX_ASSERT(devices_.empty());
  delete this;
}

void VirtualAudioLegacy::GetDefaultConfiguration(
    GetDefaultConfigurationRequestView request, GetDefaultConfigurationCompleter::Sync& completer) {
  fidl::Arena arena;
  switch (request->type) {
    case fuchsia_virtualaudio::wire::DeviceType::kDai:
      completer.ReplySuccess(
          fidl::ToWire(arena, VirtualAudioDai::GetDefaultConfig(request->direction.is_input())));
      return;
    case fuchsia_virtualaudio::wire::DeviceType::kStreamConfig:
      completer.ReplySuccess(
          fidl::ToWire(arena, VirtualAudioStream::GetDefaultConfig(request->direction.is_input())));
      return;
    case fuchsia_virtualaudio::wire::DeviceType::kCodec:
      completer.ReplySuccess(fidl::ToWire(
          arena, VirtualAudioCodec::GetDefaultConfig(
                     (request->direction.has_is_input()
                          ? static_cast<std::optional<bool>>(request->direction.is_input())
                          : std::nullopt))));
      return;
    default:
      zxlogf(ERROR, "Failed to get default configuration: Device type %u not supported",
             static_cast<uint32_t>(request->type));
      completer.ReplyError(fuchsia_virtualaudio::wire::Error::kNotSupported);
      return;
  }
}

void VirtualAudioLegacy::AddDevice(AddDeviceRequestView request,
                                   AddDeviceCompleter::Sync& completer) {
  auto config = fidl::ToNatural(request->config);
  if (!config.device_specific().has_value()) {
    completer.ReplyError(fuchsia_virtualaudio::wire::Error::kInvalidArgs);
    return;
  }
  auto device_id = next_device_id_++;
  auto result = VirtualAudioDevice::Create(config, std::move(request->server), parent_,
                                           [this, device_id]() { OnDeviceShutdown(device_id); });
  if (!result.is_ok()) {
    zxlogf(ERROR, "Device creation failed with status %d",
           fidl::ToUnderlying(result.error_value()));
    completer.ReplyError(result.error_value());
    return;
  }
  devices_[device_id] = result.value();
  completer.ReplySuccess();
}

void VirtualAudioLegacy::GetNumDevices(GetNumDevicesCompleter::Sync& completer) {
  uint32_t num_inputs = 0;
  uint32_t num_outputs = 0;
  uint32_t num_unspecified_direction = 0;
  for (auto& [_, device] : devices_) {
    if (device->is_input().has_value()) {
      if (device->is_input().value()) {
        num_inputs++;
      } else {
        num_outputs++;
      }
    } else {
      num_unspecified_direction++;
    }
  }
  completer.Reply(num_inputs, num_outputs, num_unspecified_direction);
}

void VirtualAudioLegacy::RemoveAll(RemoveAllCompleter::Sync& completer) {
  if (devices_.empty()) {
    completer.Reply();
    return;
  }

  remove_all_completers_.emplace_back(completer.ToAsync());
  ShutdownAllDevices();
}

void VirtualAudioLegacy::OnDeviceShutdown(DeviceId device_id) {
  zxlogf(INFO, "Device %lu has shutdown", device_id);
  devices_.erase(device_id);
  if (devices_.empty()) {
    zxlogf(INFO, "All devices have shutdown");
    for (auto& completer : remove_all_completers_) {
      completer.Reply();
    }
    remove_all_completers_.clear();
    if (unbind_txn_.has_value()) {
      zxlogf(INFO, "Completing unbind");
      unbind_txn_->Reply();
      unbind_txn_.reset();
    }
  }
}

void VirtualAudioLegacy::ShutdownAllDevices() {
  for (auto& [id, device] : devices_) {
    zxlogf(INFO, "Shutting down device %lu", id);
    device->ShutdownAsync();
  }
}

}  // namespace virtual_audio

static constexpr zx_driver_ops_t virtual_audio_legacy_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = &virtual_audio::VirtualAudioLegacy::Bind;
  return ops;
}();

ZIRCON_DRIVER(virtual_audio_legacy, virtual_audio_legacy_driver_ops, "fuchsia", "0.1");
