// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_CONTROL_NOTIFY_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_CONTROL_NOTIFY_H_

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <fidl/fuchsia.hardware.audio/cpp/natural_types.h>
#include <lib/zx/time.h>
#include <zircon/types.h>

#include "src/media/audio/services/device_registry/observer_notify.h"

namespace media_audio {

// A ControlServer exposes this interface, to the Device that it controls. The Device uses it for
// asynchronous notification. Note that ControlNotify includes the entirety of the ObserverNotify
// interface, including methods such as DeviceIsRemoved, DeviceHasError, TopologyIsChanged, etc.
// Also note that the Device stores this interface as a weak_ptr, since the ControlServer can be
// destroyed at any time.
class ControlNotify : public ObserverNotify {
 public:
  virtual void DeviceDroppedRingBuffer(ElementId element_id) = 0;
  virtual void DeviceDroppedPacketStream(ElementId element_id) = 0;
  virtual void DelayInfoIsChanged(ElementId element_id, const fuchsia_audio_device::DelayInfo&) = 0;

  virtual void DaiFormatIsChanged(
      ElementId element_id, const std::optional<fuchsia_hardware_audio::DaiFormat>& dai_format,
      const std::optional<fuchsia_hardware_audio::CodecFormatInfo>& codec_format_info) = 0;
  virtual void DaiFormatIsNotChanged(ElementId element_id,
                                     const fuchsia_hardware_audio::DaiFormat& dai_format,
                                     fuchsia_audio_device::ControlSetDaiFormatError error) = 0;

  virtual void CodecIsStarted(const zx::time& start_time) = 0;
  virtual void CodecIsNotStarted() = 0;
  virtual void CodecIsStopped(const zx::time& stop_time) = 0;
  virtual void CodecIsNotStopped() = 0;
  virtual void DeviceIsReset() = 0;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_CONTROL_NOTIFY_H_
