// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_TEMPERATURE_DRIVERS_AML_TRIP_AML_TRIP_DEVICE_H_
#define SRC_DEVICES_TEMPERATURE_DRIVERS_AML_TRIP_AML_TRIP_DEVICE_H_

#include <fidl/fuchsia.hardware.temperature/cpp/wire.h>
#include <fidl/fuchsia.hardware.trippoint/cpp/driver/wire.h>
#include <lib/async/cpp/irq.h>
#include <lib/async/dispatcher.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/interrupt.h>

#include <deque>
#include <optional>
#include <string>

#include "util.h"

namespace temperature {

enum class TripPointDirection {
  Rise = 1,
  Fall = 2,
};

typedef struct configured_trip_point {
  TripPointDirection direction;
  TemperatureCelsius temperature;
} configured_trip_point_t;

class AmlTripDevice : public fidl::WireServer<fuchsia_hardware_trippoint::TripPoint>,
                      public fidl::WireServer<fuchsia_hardware_temperature::Device> {
 public:
  AmlTripDevice(async_dispatcher_t* dispatcher, uint32_t trim, std::string name,
                fdf::MmioBuffer sensor_mmio, zx::interrupt irq)
      : dispatcher_(dispatcher),
        trim_info_(trim),
        name_(std::move(name)),
        sensor_mmio_(std::move(sensor_mmio)),
        irq_(std::move(irq)) {}

  void Init();
  void Shutdown();

  // Implements fuchsia.hardware.temperature
  void GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) override;
  void GetSensorName(GetSensorNameCompleter::Sync& completer) override;

  // Implements fuchsia.hardware.trippoint
  void GetTripPointDescriptors(GetTripPointDescriptorsCompleter::Sync& completer) override;
  void SetTripPoints(SetTripPointsRequestView request,
                     SetTripPointsCompleter::Sync& completer) override;
  void WaitForAnyTripPoint(WaitForAnyTripPointCompleter::Sync& completer) override;

  void HandleIrq(async_dispatcher_t* dispatcher, async::IrqBase* irq, zx_status_t status,
                 const zx_packet_interrupt_t* interrupt);
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_trippoint::TripPoint> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  void SetRebootTemperatureCelsius(TemperatureCelsius critical_temp_celsius);

 private:
  // Internal Helper Methods
  void InitSensor();
  void ProgramTripPoint(uint32_t temp_code, uint32_t index);
  void EnableIrq(uint32_t index);
  TemperatureCelsius ReadTemperatureCelsius();
  void CompletePendingRead();
  void AckAndDisableIrq(uint32_t index);
  void QueueTripResult(TemperatureCelsius measured, uint32_t index);
  bool AtLeastOneTripConfigured() const;

  static fuchsia_hardware_trippoint::TripPointType GetTripPointType(uint32_t index);

  // The number of hardware trip points supported by this hardware.
  static constexpr uint32_t kNumTripPoints = 8;
  static constexpr std::pair<uint32_t, uint32_t> kRiseTripRange = {0, 3};
  static constexpr std::pair<uint32_t, uint32_t> kFallTripRange = {4, 7};

  async_dispatcher_t* dispatcher_;
  uint32_t trim_info_;
  std::string name_;
  fdf::MmioBuffer sensor_mmio_;

  std::deque<fuchsia_hardware_trippoint::wire::TripPointResult> pending_trips_;
  std::optional<WaitForAnyTripPointCompleter::Async> pending_read_ = std::nullopt;

  std::array<std::optional<configured_trip_point_t>, kNumTripPoints> configured_trip_points_;
  zx::interrupt irq_;
  async::IrqMethod<AmlTripDevice, &AmlTripDevice::HandleIrq> irq_handler_{this};
};

}  // namespace temperature

#endif  // SRC_DEVICES_TEMPERATURE_DRIVERS_AML_TRIP_AML_TRIP_DEVICE_H_
