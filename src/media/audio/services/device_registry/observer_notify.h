// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_OBSERVER_NOTIFY_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_OBSERVER_NOTIFY_H_

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>

#include "src/media/audio/services/device_registry/basic_types.h"

namespace media_audio {

// An ObserverServer exposes this interface, to the Device that it is observing. The Device uses it
// for asynchronous notifications. Note that the Device stores this interface as a weak_ptr, since
// the ObserverServer can be destroyed at any time.
class ObserverNotify {
 public:
  virtual void DeviceIsRemoved() = 0;
  virtual void DeviceHasError() = 0;

  virtual void PlugStateIsChanged(const fuchsia_audio_device::PlugState& new_plug_state,
                                  zx::time plug_change_time) = 0;

  virtual void TopologyIsChanged(TopologyId topology_id) = 0;
  virtual void ElementStateIsChanged(
      ElementId element_id,
      fuchsia_hardware_audio_signalprocessing::ElementState element_state) = 0;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_OBSERVER_NOTIFY_H_
