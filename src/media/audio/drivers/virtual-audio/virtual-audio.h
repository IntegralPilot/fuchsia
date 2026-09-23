// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_H_
#define SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_H_

#include <fidl/fuchsia.virtualaudio/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>

#include "src/media/audio/drivers/virtual-audio/virtual-audio-composite.h"

namespace virtual_audio {

class VirtualAudio : public fdf::DriverBase2,
                     public fidl::WireServer<fuchsia_virtualaudio::Control> {
 public:
  static constexpr std::string_view kDriverName = "virtual-audio";
  static constexpr std::string_view kChildNodeName = "virtual-audio";
  static constexpr std::string_view kClassName = "virtual_audio";

  VirtualAudio() : DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  // Implements virtualaudio.Control.
  void GetDefaultConfiguration(GetDefaultConfigurationRequestView request,
                               GetDefaultConfigurationCompleter::Sync& completer) override;
  void AddDevice(AddDeviceRequestView request, AddDeviceCompleter::Sync& completer) override;
  void GetNumDevices(GetNumDevicesCompleter::Sync& completer) override;
  void RemoveAll(RemoveAllCompleter::Sync& completer) override;

  void Serve(fidl::ServerEnd<fuchsia_virtualaudio::Control> server);

  fdf::OwnedChildNode child_;
  fidl::ServerBindingGroup<fuchsia_virtualaudio::Control> bindings_;
  driver_devfs::Connector<fuchsia_virtualaudio::Control> devfs_connector_{
      fit::bind_member<&VirtualAudio::Serve>(this)};
  std::unordered_map<VirtualAudioComposite::InstanceId, std::unique_ptr<VirtualAudioComposite>>
      devices_;
  VirtualAudioComposite::InstanceId next_device_instance_id_ = 0;
};

}  // namespace virtual_audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_H_
