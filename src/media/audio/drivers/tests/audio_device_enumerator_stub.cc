// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/tests/audio_device_enumerator_stub.h"

#include <fuchsia/media/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <gtest/gtest.h>

namespace media::audio::drivers::test {

// fuchsia::media::AudioDeviceEnumerator impl
void AudioDeviceEnumeratorStub::OnStart() {
  ASSERT_EQ(outgoing()->AddPublicService(audio_device_enumerator_bindings_.GetHandler(this)),
            ZX_OK);
}
void AudioDeviceEnumeratorStub::GetDevices(GetDevicesCallback get_devices_callback) {}
void AudioDeviceEnumeratorStub::GetDeviceGain(uint64_t dev_id,
                                              GetDeviceGainCallback get_device_gain_callback) {}
void AudioDeviceEnumeratorStub::SetDeviceGain(uint64_t dev_id,
                                              fuchsia::media::AudioGainInfo gain_info,
                                              fuchsia::media::AudioGainValidFlags flags) {}

void AudioDeviceEnumeratorStub::AddDeviceByChannel(
    std::string device_name, bool is_input,
    fidl::InterfaceHandle<fuchsia::hardware::audio::StreamConfig> channel) {
  channel_ = std::move(channel);
}

// Pass a received StreamConfig channel off, to the responsible test binary
fidl::InterfaceHandle<fuchsia::hardware::audio::StreamConfig>
AudioDeviceEnumeratorStub::TakeChannel() {
  FX_CHECK(channel_);
  return std::move(channel_);
}

}  // namespace media::audio::drivers::test
