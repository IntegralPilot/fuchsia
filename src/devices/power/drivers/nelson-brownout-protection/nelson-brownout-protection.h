// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_POWER_DRIVERS_NELSON_BROWNOUT_PROTECTION_NELSON_BROWNOUT_PROTECTION_H_
#define SRC_DEVICES_POWER_DRIVERS_NELSON_BROWNOUT_PROTECTION_NELSON_BROWNOUT_PROTECTION_H_

#include <fidl/fuchsia.hardware.audio/cpp/wire.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.sensor/cpp/wire.h>
#include <lib/simple-codec/simple-codec-client.h>
#include <lib/zx/interrupt.h>
#include <threads.h>

#include <atomic>

#include <ddktl/device.h>
#include <ddktl/protocol/empty-protocol.h>

namespace brownout_protection {

class NelsonBrownoutProtection;
using DeviceType = ddk::Device<NelsonBrownoutProtection>;

class CodecClientAgl {
 public:
  zx_status_t Init(fidl::ClientEnd<fuchsia_hardware_audio::Codec> codec_client_end);
  zx_status_t SetAgl(bool enable);

 private:
  fidl::WireSyncClient<fuchsia_hardware_audio_signalprocessing::SignalProcessing>
      signal_processing_;
  std::optional<uint64_t> agl_id_;
};

class NelsonBrownoutProtection : public DeviceType {
 public:
  static zx_status_t Create(void* ctx, zx_device_t* parent);
  static zx_status_t Create(void* ctx, zx_device_t* parent, zx::duration voltage_poll_interval);

  NelsonBrownoutProtection(zx_device_t* parent,
                           fidl::WireSyncClient<fuchsia_hardware_power_sensor::Device> power_sensor,
                           fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> alert_gpio,
                           zx::interrupt alert_interrupt, zx::duration voltage_poll_interval)
      : DeviceType(parent),
        power_sensor_(std::move(power_sensor)),
        alert_gpio_(std::move(alert_gpio)),
        alert_interrupt_(std::move(alert_interrupt)),
        voltage_poll_interval_(voltage_poll_interval) {}
  ~NelsonBrownoutProtection() {
    alert_interrupt_.destroy();
    run_thread_ = false;
    thrd_join(thread_, nullptr);
  }

  void DdkRelease() { delete this; }

 private:
  zx_status_t Init(fidl::ClientEnd<fuchsia_hardware_audio::Codec> codec_client_end);

  int Thread();

  thrd_t thread_;
  CodecClientAgl codec_;
  fidl::WireSyncClient<fuchsia_hardware_power_sensor::Device> power_sensor_;
  fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> alert_gpio_;
  const zx::interrupt alert_interrupt_;
  std::atomic_bool run_thread_ = true;
  const zx::duration voltage_poll_interval_;
};

}  // namespace brownout_protection

#endif  // SRC_DEVICES_POWER_DRIVERS_NELSON_BROWNOUT_PROTECTION_NELSON_BROWNOUT_PROTECTION_H_
