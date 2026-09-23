// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CLOCK_DRIVERS_CLOCK_CLOCK_H_
#define SRC_DEVICES_CLOCK_DRIVERS_CLOCK_CLOCK_H_

#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <fidl/fuchsia.hardware.clockimpl/cpp/driver/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/power/state_recorder/cpp/inspect_buffer.h>

using ByteBuffer = power_observability::internal::TimestampedBuffer<uint8_t>;
using BitBuffer = power_observability::internal::TimestampedBuffer<bool>;

class ClockDriver;

class ClockDevice : public fidl::WireServer<fuchsia_hardware_clock::Clock> {
 public:
  explicit ClockDevice(const std::shared_ptr<ByteBuffer>& inspect_rate_buffer,
                       const std::shared_ptr<BitBuffer>& inspect_enable_buffer, ClockDriver* parent,
                       uint32_t id, std::string_view name)
      : inspect_rate_buffer_(inspect_rate_buffer),
        inspect_enable_buffer_(inspect_enable_buffer),
        parent_(parent),
        id_(id),
        name_(name) {}

  zx_status_t Init(const std::shared_ptr<fdf::Namespace>& incoming,
                   const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                   std::optional<int32_t> node_id,
                   const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent,
                   bool report_initial_conditions);

  bool pending_driver() const;
  std::string_view child_name() const;

 private:
  void WaitForDriverCompleted(
      fidl::WireUnownedResult<fuchsia_driver_framework::NodeController::WaitForDriver>& result);

  // fuchsia.hardware.clock/Clock protocol implementation
  void Enable(EnableCompleter::Sync& completer) override;
  void Disable(DisableCompleter::Sync& completer) override;
  void IsEnabled(IsEnabledCompleter::Sync& completer) override;
  void SetRate(SetRateRequestView request, SetRateCompleter::Sync& completer) override;
  void QuerySupportedRate(QuerySupportedRateRequestView request,
                          QuerySupportedRateCompleter::Sync& completer) override;
  void GetRate(GetRateCompleter::Sync& completer) override;
  void SetInput(SetInputRequestView request, SetInputCompleter::Sync& completer) override;
  void GetNumInputs(GetNumInputsCompleter::Sync& completer) override;
  void GetInput(GetInputCompleter::Sync& completer) override;
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_clock::Clock> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  std::shared_ptr<ByteBuffer> inspect_rate_buffer_;
  std::shared_ptr<BitBuffer> inspect_enable_buffer_;
  ClockDriver* parent_;
  fdf::WireClient<fuchsia_hardware_clockimpl::ClockImpl> clock_impl_;
  const uint32_t id_;
  fidl::WireClient<fuchsia_driver_framework::NodeController> child_node_;
  fidl::ServerBindingGroup<fuchsia_hardware_clock::Clock> bindings_;
  std::string name_;

  std::string child_name_;
  bool pending_driver_ = true;
};

class ClockDriver : public fdf::DriverBase2 {
 public:
  static constexpr char kDriverName[] = "clock";

  using fdf::DriverBase2::AddChild;

  explicit ClockDriver() : fdf::DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override;

  void CheckIfReady();

  fpromise::promise<inspect::Inspector> PowerObservabilityInspectCallback();

  uint8_t GetDataForRate(uint64_t rate);

  uint64_t GetRateForData(uint8_t data);

 protected:
 private:
  zx_status_t ConfigureClocks(const fuchsia_hardware_clockimpl::InitMetadata& metadata,
                              fdf::ClientEnd<fuchsia_hardware_clockimpl::ClockImpl> clock_impl,
                              std::unordered_set<uint32_t>& reported_initial_conditions);

  zx_status_t CreateClockDevices(std::unordered_set<uint32_t>& reported_initial_conditions,
                                 const std::shared_ptr<fdf::Namespace>& incoming);

  const std::shared_ptr<ByteBuffer>& GetOrCreateRateBuffer(uint32_t clock_id);

  const std::shared_ptr<BitBuffer>& GetOrCreateEnableBuffer(uint32_t clock_id);

  std::unordered_map<uint32_t, std::shared_ptr<ByteBuffer>> inspect_rate_buffers_;
  std::unordered_map<uint32_t, std::shared_ptr<BitBuffer>> inspect_enable_buffers_;
  std::unordered_map<uint32_t, std::string> id_to_name_;

  std::vector<std::unique_ptr<ClockDevice>> clock_devices_;
  std::optional<inspect::ComponentInspector> component_inspector_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> clock_init_child_node_;
  std::unordered_map<uint64_t, uint8_t> rate_to_index_table_;
};

#endif  // SRC_DEVICES_CLOCK_DRIVERS_CLOCK_CLOCK_H_
