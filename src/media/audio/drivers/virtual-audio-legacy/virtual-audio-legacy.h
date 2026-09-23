// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_LEGACY_H_
#define SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_LEGACY_H_

#include <fidl/fuchsia.virtualaudio/cpp/fidl.h>
#include <lib/ddk/device.h>

#include <memory>
#include <unordered_map>

#include <ddktl/device.h>

namespace virtual_audio {

class VirtualAudioDevice;

class VirtualAudioLegacy;
using VirtualAudioLegacyDeviceType =
    ddk::Device<VirtualAudioLegacy, ddk::Unbindable,
                ddk::Messageable<fuchsia_virtualaudio::Control>::Mixin>;

class VirtualAudioLegacy : public VirtualAudioLegacyDeviceType {
 public:
  static zx_status_t Bind(void* ctx, zx_device_t* parent);

  explicit VirtualAudioLegacy(zx_device_t* parent) : VirtualAudioLegacyDeviceType(parent) {}

  void DdkRelease();
  void DdkUnbind(ddk::UnbindTxn txn);

 private:
  using DeviceId = uint64_t;

  zx::result<> Init();

  // Implements virtualaudio.Control.
  void GetDefaultConfiguration(GetDefaultConfigurationRequestView request,
                               GetDefaultConfigurationCompleter::Sync& completer) override;
  void AddDevice(AddDeviceRequestView request, AddDeviceCompleter::Sync& completer) override;
  void GetNumDevices(GetNumDevicesCompleter::Sync& completer) override;
  void RemoveAll(RemoveAllCompleter::Sync& completer) override;

  void OnDeviceShutdown(DeviceId device_id);

  void ShutdownAllDevices();

  // `DeviceId` is needed in order to locate a device when the device has shutdown and is to be
  // removed.
  std::unordered_map<DeviceId, std::shared_ptr<VirtualAudioDevice>> devices_;
  DeviceId next_device_id_ = 0;

  // Invoked once all the devices have shutdown.
  std::optional<ddk::UnbindTxn> unbind_txn_;

  // Invoked once all the devices have shutdown.
  std::vector<RemoveAllCompleter::Async> remove_all_completers_;
};

}  // namespace virtual_audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_LEGACY_VIRTUAL_AUDIO_LEGACY_H_
