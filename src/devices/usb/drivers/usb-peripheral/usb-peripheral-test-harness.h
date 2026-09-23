// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_PERIPHERAL_TEST_HARNESS_H_
#define SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_PERIPHERAL_TEST_HARNESS_H_

#include <fidl/fuchsia.hardware.usb.dci/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.descriptor/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.function/cpp/wire_test_base.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/clock.h>
#include <lib/zx/interrupt.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>

#include <atomic>
#include <cstdio>
#include <cstring>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <vector>

#include <gtest/gtest.h>
#include <sdk/lib/inspect/testing/cpp/inspect.h>
#include <usb-inspect/usb-inspect-test-helper.h>
#include <usb/peripheral.h>
#include <usb/request-cpp.h>
#include <usb/usb.h>

#include "src/devices/usb/drivers/usb-peripheral/usb-function.h"
#include "src/devices/usb/drivers/usb-peripheral/usb-peripheral.h"
#include "src/lib/testing/predicates/status.h"
#include "usb/descriptors.h"

namespace fdci = fuchsia_hardware_usb_dci;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;
namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace ffunction = fuchsia_hardware_usb_function;
namespace fperipheral = fuchsia_hardware_usb_peripheral;

namespace usb_peripheral {

inline std::ostream& operator<<(std::ostream& os, const UsbPeripheral::DeviceState& state) {
  return os << std::format("{}", state);
}

}  // namespace usb_peripheral

namespace usb_peripheral::test {

using inspect::testing::BoolIs;
using inspect::testing::NameMatches;
using inspect::testing::NodeMatches;
using inspect::testing::PropertyList;
using inspect::testing::StringIs;
using inspect::testing::UintIs;
using ::testing::AllOf;
using ::testing::Contains;

inline fuchsia_hardware_usb_endpoint::wire::EndpointInfo BulkEpInfo(fidl::AnyArena& arena) {
  return fuchsia_hardware_usb_endpoint::wire::EndpointInfo::WithBulk(
      arena, fuchsia_hardware_usb_endpoint::wire::BulkEndpointInfo::Builder(arena).Build());
}
inline fuchsia_hardware_usb_endpoint::wire::EndpointInfo InterruptEpInfo(fidl::AnyArena& arena) {
  return fuchsia_hardware_usb_endpoint::wire::EndpointInfo::WithInterrupt(
      arena, fuchsia_hardware_usb_endpoint::wire::InterruptEndpointInfo::Builder(arena).Build());
}

class FakeDevice : public fidl::WireServer<fdci::UsbDci> {
 public:
  FakeDevice() = default;

  fdci::UsbDciService::InstanceHandler GetHandler() {
    return fdci::UsbDciService::InstanceHandler(
        {.device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                           fidl::kIgnoreBindingClosure)});
  }

  enum class EpType {
    kBulk,
    kInterrupt,
    kIsochronous,
  };
  struct EndpointCaps {
    uint8_t ep_address;
    uint32_t max_packet_size_limit;
    std::vector<EpType> supported_types;
  };

  void SetHardwareInfo(std::vector<EndpointCaps> caps, bool supports_dynamic) {
    std::lock_guard lock(lock_);
    caps_ = std::move(caps);
    supports_dynamic_ = supports_dynamic;
    has_caps_ = true;
  }

  // fdci::UsbDci protocol.
  void ConnectToEndpoint(ConnectToEndpointRequestView req,
                         ConnectToEndpointCompleter::Sync& completer) override {
    if (fail_already_bound_.load()) {
      completer.ReplyError(ZX_ERR_ALREADY_BOUND);
      return;
    }
    {
      std::lock_guard lock(lock_);
      endpoints_[req->ep_addr] = std::move(req->ep);
    }
    completer.ReplySuccess();
  }

  void SetInterface(SetInterfaceRequestView req, SetInterfaceCompleter::Sync& completer) override {
    fidl::Arena arena;
    client_.emplace(std::move(req->interface));
    completer.buffer(arena).ReplySuccess();
    set_interface_called_.Signal();
  }

  void StartController(StartControllerCompleter::Sync& completer) override {
    if (fail_start_.load()) {
      completer.ReplyError(ZX_ERR_IO_NOT_PRESENT);
      return;
    }
    {
      std::lock_guard lock(lock_);
      controller_started_ = true;
    }
    completer.ReplySuccess();
  }

  void StopController(StopControllerCompleter::Sync& completer) override {
    if (fail_stop_.load()) {
      completer.ReplyError(ZX_ERR_IO);
      return;
    }
    libsync::Completion* stop_completion = nullptr;
    {
      std::lock_guard lock(lock_);
      controller_started_ = false;
      endpoints_.clear();
      stop_completion = stop_completion_;
    }
    completer.ReplySuccess();
    if (stop_completion) {
      stop_completion->Signal();
    }
  }

  void ConfigureEndpoint(ConfigureEndpointRequestView req,
                         ConfigureEndpointCompleter::Sync& completer) override {
    if (fail_configure_.load()) {
      completer.ReplyError(ZX_ERR_IO_NOT_PRESENT);
      return;
    }
    {
      std::lock_guard lock(lock_);
      configured_endpoints_.push_back(req->ep_descriptor);
      configured_endpoints_ss_companion_.push_back(req->ss_comp_descriptor);
    }
    completer.ReplySuccess();
  }

  void DisableEndpoint(DisableEndpointRequestView req,
                       DisableEndpointCompleter::Sync& completer) override {
    if (zx_status_t status = fail_disable_status_.load(); status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    {
      std::lock_guard lock(lock_);
      disabled_endpoints_.push_back(req->ep_address);
    }
    completer.ReplySuccess();
  }

  void EndpointSetStall(EndpointSetStallRequestView req,
                        EndpointSetStallCompleter::Sync& completer) override {
    if (fail_stall_.load()) {
      completer.ReplyError(ZX_ERR_IO_NOT_PRESENT);
      return;
    }
    {
      std::lock_guard lock(lock_);
      set_stalls_.push_back(req->ep_address);
    }
    completer.ReplySuccess();
  }

  void EndpointClearStall(EndpointClearStallRequestView req,
                          EndpointClearStallCompleter::Sync& completer) override {
    if (fail_stall_.load()) {
      completer.ReplyError(ZX_ERR_IO_NOT_PRESENT);
      return;
    }
    {
      std::lock_guard lock(lock_);
      clear_stalls_.push_back(req->ep_address);
    }
    completer.ReplySuccess();
  }

  void CancelAll(CancelAllRequestView req, CancelAllCompleter::Sync& completer) override {
    {
      std::lock_guard lock(lock_);
      cancelled_endpoints_.push_back(req->ep_address);
    }
    if (zx_status_t status = fail_cancel_all_status_.load(); status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    completer.ReplySuccess();
  }

  void GetHardwareInfo(GetHardwareInfoCompleter::Sync& completer) override {
    std::vector<EndpointCaps> caps;
    bool supports_dynamic = false;
    bool has_caps = false;
    {
      std::lock_guard lock(lock_);
      caps = caps_;
      supports_dynamic = supports_dynamic_;
      has_caps = has_caps_;
    }
    if (!has_caps) {
      completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
      return;
    }

    fidl::Arena arena;
    auto endpoints =
        fidl::VectorView<fuchsia_hardware_usb_dci::wire::EndpointInfo>(arena, caps.size());
    for (size_t i = 0; i < caps.size(); i++) {
      auto types = fidl::VectorView<fuchsia_hardware_usb_dci::wire::SupportedEndpointInfo>(
          arena, caps[i].supported_types.size());
      for (size_t j = 0; j < caps[i].supported_types.size(); j++) {
        fuchsia_hardware_usb_descriptor::wire::EndpointType ep_type =
            fuchsia_hardware_usb_descriptor::wire::EndpointType::kBulk;
        switch (caps[i].supported_types[j]) {
          case EpType::kBulk:
            ep_type = fuchsia_hardware_usb_descriptor::wire::EndpointType::kBulk;
            break;
          case EpType::kInterrupt:
            ep_type = fuchsia_hardware_usb_descriptor::wire::EndpointType::kInterrupt;
            break;
          case EpType::kIsochronous:
            ep_type = fuchsia_hardware_usb_descriptor::wire::EndpointType::kIsochronous;
            break;
        }
        types[j] = fuchsia_hardware_usb_dci::wire::SupportedEndpointInfo::Builder(arena)
                       .endpoint_type(ep_type)
                       .max_packet_size_limit(static_cast<uint16_t>(caps[i].max_packet_size_limit))
                       .Build();
      }

      endpoints[i] = fuchsia_hardware_usb_dci::wire::EndpointInfo::Builder(arena)
                         .ep_address(caps[i].ep_address)
                         .supported_types(types)
                         .Build();
    }

    auto info = fuchsia_hardware_usb_dci::wire::DciHardwareInfo::Builder(arena)
                    .endpoints(endpoints)
                    .supports_dynamic_ep_sizing(supports_dynamic)
                    .Build();

    completer.ReplySuccess(info);
  }

  void AllocEndpoint(AllocEndpointRequestView req,
                     AllocEndpointCompleter::Sync& completer) override {
    zx_status_t status = ZX_OK;
    uint8_t allocated_ep = 0;
    {
      std::lock_guard lock(lock_);
      if (!supports_dynamic_) {
        status = ZX_ERR_NOT_SUPPORTED;
      } else {
        alloc_called_ = true;
        if (max_allocs_.has_value() && alloc_count_ >= *max_allocs_) {
          status = ZX_ERR_NO_RESOURCES;
        } else {
          alloc_count_++;
          if (req->direction() == fuchsia_hardware_usb_descriptor::wire::EndpointDirection::kIn) {
            allocated_ep = next_in_ep_++;
          } else {
            allocated_ep = next_out_ep_++;
          }
        }
      }
    }

    if (status != ZX_OK) {
      completer.ReplyError(status);
    } else {
      completer.ReplySuccess(allocated_ep);
    }
  }

  void FreeEndpoint(FreeEndpointRequestView req, FreeEndpointCompleter::Sync& completer) override {
    {
      std::lock_guard lock(lock_);
      freed_endpoints_.push_back(req->ep_address);
    }
    completer.ReplySuccess();
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fdci::UsbDci> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  fidl::ClientEnd<fdci::UsbDciInterface> TakeClient() {
    auto client = std::move(client_);
    EXPECT_TRUE(client.has_value());
    return std::move(client.value());
  }

  libsync::Completion& set_interface_called() { return set_interface_called_; }

  bool controller_started() const {
    std::lock_guard lock(lock_);
    return controller_started_;
  }
  void set_controller_started(bool started) {
    std::lock_guard lock(lock_);
    controller_started_ = started;
  }

  void set_stop_completion(libsync::Completion* stop_completion) {
    std::lock_guard lock(lock_);
    stop_completion_ = stop_completion;
  }

  fidl::ServerEnd<fendpoint::Endpoint> TakeEndpoint(uint8_t addr) {
    std::lock_guard lock(lock_);
    auto it = endpoints_.find(addr);
    if (it == endpoints_.end()) {
      return {};
    }
    auto ep = std::move(it->second);
    endpoints_.erase(it);
    return ep;
  }

  void set_max_allocs(size_t max) {
    std::lock_guard lock(lock_);
    max_allocs_ = max;
  }
  std::vector<uint8_t> freed_endpoints() const {
    std::lock_guard lock(lock_);
    return freed_endpoints_;
  }
  void clear_freed_endpoints() {
    std::lock_guard lock(lock_);
    freed_endpoints_.clear();
  }

  std::vector<uint8_t> disabled_endpoints() const {
    std::lock_guard lock(lock_);
    return disabled_endpoints_;
  }
  void clear_disabled_endpoints() {
    std::lock_guard lock(lock_);
    disabled_endpoints_.clear();
  }

  std::vector<fdescriptor::wire::UsbEndpointDescriptor> configured_endpoints() const {
    std::lock_guard lock(lock_);
    return configured_endpoints_;
  }
  std::vector<fdescriptor::wire::UsbSsEpCompDescriptor> configured_endpoints_ss_companion() const {
    std::lock_guard lock(lock_);
    return configured_endpoints_ss_companion_;
  }

  std::vector<uint8_t> set_stalls() const {
    std::lock_guard lock(lock_);
    return set_stalls_;
  }
  std::vector<uint8_t> clear_stalls() const {
    std::lock_guard lock(lock_);
    return clear_stalls_;
  }
  std::vector<uint8_t> cancelled_endpoints() const {
    std::lock_guard lock(lock_);
    return cancelled_endpoints_;
  }
  void clear_cancelled_endpoints() {
    std::lock_guard lock(lock_);
    cancelled_endpoints_.clear();
  }

  std::atomic<bool> fail_already_bound_{false};
  std::atomic<bool> fail_stall_{false};
  std::atomic<bool> fail_start_{false};
  std::atomic<bool> fail_stop_{false};
  std::atomic<bool> fail_configure_{false};
  std::atomic<zx_status_t> fail_disable_status_{ZX_OK};
  std::atomic<zx_status_t> fail_cancel_all_status_{ZX_ERR_NOT_SUPPORTED};

  bool alloc_called() const {
    std::lock_guard lock(lock_);
    return alloc_called_;
  }
  void reset_alloc_called() {
    std::lock_guard lock(lock_);
    alloc_called_ = false;
  }

 private:
  mutable std::mutex lock_;
  bool controller_started_ = false;
  libsync::Completion set_interface_called_;
  libsync::Completion* stop_completion_ = nullptr;
  fidl::ServerBindingGroup<fdci::UsbDci> bindings_;
  std::optional<fidl::ClientEnd<fdci::UsbDciInterface>> client_;
  std::map<uint8_t, fidl::ServerEnd<fendpoint::Endpoint>> endpoints_;
  std::vector<uint8_t> set_stalls_;
  std::vector<uint8_t> clear_stalls_;
  std::vector<uint8_t> cancelled_endpoints_;
  std::vector<fdescriptor::wire::UsbEndpointDescriptor> configured_endpoints_;
  std::vector<fdescriptor::wire::UsbSsEpCompDescriptor> configured_endpoints_ss_companion_;
  std::vector<uint8_t> disabled_endpoints_;

  std::vector<EndpointCaps> caps_;
  bool supports_dynamic_ = false;
  bool has_caps_ = false;
  bool alloc_called_ = false;
  uint8_t next_in_ep_ = 0x81;
  uint8_t next_out_ep_ = 0x01;
  std::vector<uint8_t> freed_endpoints_;
  std::optional<size_t> max_allocs_;
  size_t alloc_count_ = 0;
};

class FakeUsbFunction : public fidl::testing::WireTestBase<ffunction::UsbFunctionInterface>,
                        public std::enable_shared_from_this<FakeUsbFunction> {
 public:
  using SetConfiguredCompleterAsync = SetConfiguredCompleter::Async;

  void Control(ControlRequestView req, ControlCompleter::Sync& completer) override {
    zx_status_t status;
    {
      std::lock_guard lock(lock_);
      control_called_ = true;
      control_req_ = req->setup.b_request;
      status = control_status_;
    }
    if (status != ZX_OK) {
      completer.ReplyError(status);
    } else {
      fidl::Arena arena;
      std::vector<uint8_t> read_data = {1, 2, 3};
      completer.buffer(arena).ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(read_data));
    }
    call_completed_.Signal();
  }

  void SetConfigured(SetConfiguredRequestView req,
                     SetConfiguredCompleter::Sync& completer) override {
    std::function<void(bool, SetConfiguredCompleterAsync)> on_async;
    std::function<void()> on_sync;
    zx_status_t status = ZX_OK;
    {
      std::lock_guard lock(lock_);
      set_configured_called_ = true;
      configured_ = req->configured;
      configured_history_.push_back(req->configured);
      on_async = on_set_configured_async_;
      on_sync = on_set_configured_;
      status = set_configured_status_;
    }

    if (on_async) {
      auto async_completer = completer.ToAsync();
      on_async(req->configured, std::move(async_completer));
      call_completed_.Signal();
      return;
    }
    if (on_sync) {
      on_sync();
    }
    if (status != ZX_OK) {
      completer.ReplyError(status);
    } else {
      completer.ReplySuccess();
    }
    call_completed_.Signal();
  }

  void SetInterface(SetInterfaceRequestView req, SetInterfaceCompleter::Sync& completer) override {
    zx_status_t status;
    {
      std::lock_guard lock(lock_);
      set_interface_called_ = true;
      interface_ = req->interface;
      alt_setting_ = req->alt_setting;
      status = set_interface_status_;
    }
    if (status != ZX_OK) {
      completer.ReplyError(status);
    } else {
      completer.ReplySuccess();
    }
    call_completed_.Signal();
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ADD_FAILURE() << "Not implemented: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  zx_status_t WaitUntilCalled(zx::duration timeout = zx::sec(5)) {
    zx_status_t status = call_completed_.Wait(timeout);
    EXPECT_EQ(status, ZX_OK);
    call_completed_.Reset();
    return status;
  }

  void ResetCalled() { call_completed_.Reset(); }

  zx_status_t WaitUntilUnbound(zx::duration timeout = zx::sec(5)) {
    zx_status_t status = unbound_completion_.Wait(timeout);
    EXPECT_EQ(status, ZX_OK);
    unbound_completion_.Reset();
    return status;
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<ffunction::UsbFunctionInterface> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  void Bind(fdf::UnownedSynchronizedDispatcher dispatcher,
            fidl::ServerEnd<ffunction::UsbFunctionInterface> server_end) {
    dispatcher_ = std::move(dispatcher);
    binding_.emplace(fidl::BindServer(
        dispatcher_.value()->async_dispatcher(), std::move(server_end), shared_from_this(),
        [](FakeUsbFunction* impl, fidl::UnbindInfo info,
           fidl::ServerEnd<ffunction::UsbFunctionInterface> server_end) {
          impl->unbound_completion_.Signal();
        }));
  }

  void Unbind() { binding_->Unbind(); }

  bool control_called() const {
    std::lock_guard lock(lock_);
    return control_called_;
  }
  uint8_t control_req() const {
    std::lock_guard lock(lock_);
    return control_req_;
  }
  bool set_configured_called() const {
    std::lock_guard lock(lock_);
    return set_configured_called_;
  }
  void clear_set_configured_called() {
    std::lock_guard lock(lock_);
    set_configured_called_ = false;
  }
  bool configured() const {
    std::lock_guard lock(lock_);
    return configured_;
  }
  std::vector<bool> configured_history() const {
    std::lock_guard lock(lock_);
    return configured_history_;
  }
  bool set_interface_called() const {
    std::lock_guard lock(lock_);
    return set_interface_called_;
  }

  void set_on_set_configured(std::function<void()> cb) {
    std::lock_guard lock(lock_);
    on_set_configured_ = std::move(cb);
  }
  void set_control_status(zx_status_t status) {
    std::lock_guard lock(lock_);
    control_status_ = status;
  }
  void set_set_configured_status(zx_status_t status) {
    std::lock_guard lock(lock_);
    set_configured_status_ = status;
  }
  void set_set_interface_status(zx_status_t status) {
    std::lock_guard lock(lock_);
    set_interface_status_ = status;
  }
  void set_on_set_configured_async(std::function<void(bool, SetConfiguredCompleterAsync)> cb) {
    std::lock_guard lock(lock_);
    on_set_configured_async_ = std::move(cb);
  }
  uint8_t interface() const {
    std::lock_guard lock(lock_);
    return interface_;
  }
  uint8_t alt_setting() const {
    std::lock_guard lock(lock_);
    return alt_setting_;
  }

  fdf::UnownedSynchronizedDispatcher& dispatcher() {
    ZX_ASSERT(dispatcher_.has_value());
    return dispatcher_.value();
  }

 private:
  mutable std::mutex lock_;

  libsync::Completion call_completed_;
  libsync::Completion unbound_completion_;

  bool control_called_ = false;
  uint8_t control_req_ = 0;
  zx_status_t control_status_ = ZX_OK;

  bool set_configured_called_ = false;
  bool configured_ = false;
  zx_status_t set_configured_status_ = ZX_OK;
  zx_status_t set_interface_status_ = ZX_OK;
  std::vector<bool> configured_history_;
  std::function<void()> on_set_configured_;
  std::function<void(bool, SetConfiguredCompleterAsync)> on_set_configured_async_;

  bool set_interface_called_ = false;
  uint8_t interface_ = 0;
  uint8_t alt_setting_ = 0;

  std::optional<fdf::UnownedSynchronizedDispatcher> dispatcher_;
  std::optional<fidl::ServerBindingRef<ffunction::UsbFunctionInterface>> binding_;
};

class FakeEvents : public fidl::WireServer<fperipheral::Events> {
 public:
  FakeEvents() = default;
  ~FakeEvents() { Unbind(); }

  void FunctionRegistered(FunctionRegisteredCompleter::Sync& completer) override {
    completer.Reply();
  }
  void FunctionsCleared(FunctionsClearedCompleter::Sync& completer) override {
    cleared_called_ = true;
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<fperipheral::Events> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}

  void WaitUntilCleared(fdf_testing::DriverRuntime& runtime) {
    runtime.RunUntil([&]() { return cleared_called_; });
    cleared_called_ = false;
  }

  void Bind(fidl::ServerEnd<fperipheral::Events> server_end) {
    binding_.emplace(fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                      std::move(server_end), this));
  }

  void Unbind() {
    if (binding_) {
      binding_->Unbind();
      binding_.reset();
    }
  }

 private:
  bool cleared_called_ = false;
  std::optional<fidl::ServerBindingRef<fperipheral::Events>> binding_;
};

class UsbPeripheralTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(std::string_view serial_number) {
    serial_number_ = fuchsia_boot_metadata::SerialNumberMetadata{{.serial_number{serial_number}}};
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    if (serial_number_.has_value()) {
      if (zx::result result = serial_number_metadata_server_.Serve(
              to_driver_vfs, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
              serial_number_.value());
          result.is_error()) {
        return result.take_error();
      }
    }

    if (zx::result result = to_driver_vfs.AddService<fdci::UsbDciService>(dci_.GetHandler());
        result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  fidl::ClientEnd<fdci::UsbDciInterface> TakeDciClient() { return dci_.TakeClient(); }

  FakeDevice& dci() { return dci_; }

 private:
  FakeDevice dci_;
  fdf_metadata::MetadataServer<fuchsia_boot_metadata::SerialNumberMetadata>
      serial_number_metadata_server_;
  std::optional<fuchsia_boot_metadata::SerialNumberMetadata> serial_number_;
};

class UsbPeripheralTestConfig {
 public:
  using DriverType = UsbPeripheral;
  using EnvironmentType = UsbPeripheralTestEnvironment;
};

template <bool manage_lifetime>
class UsbPeripheralHarness : public ::testing::Test {
 public:
  void SetUp() override {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) { env.Init(kSerialNumber); });

    if constexpr (manage_lifetime) {
      StartDriverWithConfig(GetDriverConfig());
    }
  }

  virtual usb_peripheral_config::Config GetDriverConfig() {
    return usb_peripheral_config::Config{};
  }

  void StartDriverWithConfig(const usb_peripheral_config::Config& config) {
    ASSERT_OK(driver_test_.StartDriverWithCustomStartArgs(
        [&](auto& start_args) { start_args.config().emplace(config.ToVmo()); }));
    if (config.functions().empty()) {
      ExpectState(UsbPeripheral::DeviceState::kNoConfiguration);
    } else {
      ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
    }
    started_driver_ = true;
    driver_test_.runtime().RunUntil([&]() {
      return driver_test_.RunInEnvironmentTypeContext<bool>([](UsbPeripheralTestEnvironment& env) {
        return env.dci().set_interface_called().signaled();
      });
    });

    dci_.Bind(driver_test_.RunInEnvironmentTypeContext<fidl::ClientEnd<fdci::UsbDciInterface>>(
        [](auto& env) { return env.TakeDciClient(); }));
  }

  void WaitForChildNode(const std::string& name) {
    bool found = false;
    dut().runtime().RunUntil([&]() {
      dut().RunInNodeContext([&](fdf_testing::TestNode& root) {
        auto it = root.children().find(std::string(UsbPeripheral::kChildNodeName));
        if (it != root.children().end()) {
          auto& peripheral_node = it->second;
          auto func_it = peripheral_node.children().find(name);
          if (func_it != peripheral_node.children().end()) {
            found = true;
          }
        }
      });
      return found;
    });
  }

  void SimulateFunctionUnbind(const std::vector<std::string>& function_names) {
    for (const auto& name : function_names) {
      WaitForChildNode(name);
    }

    this->dut().RunInNodeContext([&](fdf_testing::TestNode& root) {
      auto it = root.children().find(std::string(UsbPeripheral::kChildNodeName));
      if (it == root.children().end()) {
        ADD_FAILURE() << "Peripheral child node not found: " << UsbPeripheral::kChildNodeName;
        return;
      }
      auto& peripheral_node = it->second;

      for (const auto& name : function_names) {
        auto it_func = peripheral_node.children().find(name);
        if (it_func == peripheral_node.children().end()) {
          ADD_FAILURE() << "Function child node not found: " << name;
          return;
        }
        // Dropping the returned Node channel triggers an unbind.
        (void)it_func->second.CreateNodeChannel();
      }
    });
  }

  void ExpectChildNodeCount(size_t expected_count) {
    this->dut().RunInNodeContext([&](fdf_testing::TestNode& root) {
      auto it = root.children().find(std::string(UsbPeripheral::kChildNodeName));
      if (it == root.children().end()) {
        ADD_FAILURE() << "Peripheral child node not found: " << UsbPeripheral::kChildNodeName;
        return;
      }
      auto& peripheral_node = it->second;
      EXPECT_EQ(peripheral_node.children().size(), expected_count);
    });
  }

  void WaitUntilChildNodeCount(size_t expected_count) {
    dut().runtime().RunUntilIdle();
    dut().runtime().RunUntil([&]() {
      std::optional<size_t> actual_count;
      dut().RunInNodeContext([&](fdf_testing::TestNode& root) {
        auto it = root.children().find(std::string(UsbPeripheral::kChildNodeName));
        if (it != root.children().end()) {
          actual_count = it->second.children().size();
        }
      });
      return actual_count == expected_count;
    });
  }

  void ExpectControllerStarted(bool expected) {
    this->dut().RunInEnvironmentTypeContext([expected](UsbPeripheralTestEnvironment& env) {
      if (expected) {
        EXPECT_TRUE(env.dci().controller_started());
      } else {
        EXPECT_FALSE(env.dci().controller_started());
      }
    });
  }

  void TearDown() override {
    if (started_driver_) {
      // StopDriver will call PrepareStop, which should stop the controller.
      ASSERT_OK(driver_test_.StopDriver());

      ExpectControllerStarted(false);
    }
  }

  void RegisterFakeEvents(std::shared_ptr<FakeEvents> fake_events) {
    auto [client_end, server_end] = fidl::Endpoints<fperipheral::Events>::Create();
    fake_events->Bind(std::move(server_end));
    auto client = Client();
    ASSERT_TRUE(client->SetStateChangeListener(std::move(client_end)).ok());
  }

 protected:
  static constexpr std::string_view kSerialNumber = "Test serial number";

  fidl::WireSyncClient<fdci::UsbDciInterface>& dci() { return dci_; }

  void ExpectState(UsbPeripheral::DeviceState state) {
    this->dut().RunInDriverContext([state](UsbPeripheral& peripheral) {
      EXPECT_EQ(peripheral.SnapshotState(), state);

      auto hierarchy = usb_inspect::ReadHierarchyFromInspector(peripheral.inspector());
      auto* node = hierarchy.GetByPath({"usb-peripheral", "dci_metrics"});
      if (node == nullptr) {
        ADD_FAILURE() << "dci_metrics inspect node not found";
        return;
      }
      EXPECT_THAT(*node,
                  NodeMatches(PropertyList(Contains(StringIs("state", std::format("{}", state))))));
    });
  }

  void ExpectStateEventual(UsbPeripheral::DeviceState state) {
    WaitUntilState(state);
    ExpectState(state);
  }

  void WaitUntilState(UsbPeripheral::DeviceState state) {
    dut().runtime().RunUntil([&]() {
      bool matched = false;
      dut().RunInDriverContext(
          [&](UsbPeripheral& peripheral) { matched = (peripheral.SnapshotState() == state); });
      return matched;
    });
  }

  fdf_testing::BackgroundDriverTest<UsbPeripheralTestConfig>& dut() { return driver_test_; }

  fidl::WireSyncClient<fperipheral::Device> Client() {
    auto client_end = driver_test_.Connect<fperipheral::Service::Device>();
    ZX_ASSERT_MSG(client_end.is_ok(), "Failed to connect to peripheral service: %s",
                  client_end.status_string());
    return fidl::WireSyncClient<fperipheral::Device>{std::move(client_end.value())};
  }

  zx::result<fidl::WireSyncClient<ffunction::UsbFunction>> ConnectFunction(
      std::string name = "function-000") {
    ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
    zx::result result = dut().template Connect<ffunction::UsbFunctionService::Device>(name);
    if (result.is_error()) {
      return result.take_error();
    }
    ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
    return zx::ok(fidl::WireSyncClient<ffunction::UsbFunction>(std::move(result.value())));
  }

  zx::result<fidl::ClientEnd<ffunction::UsbFunction>> ConnectFunctionClientEnd(
      std::string name = "function-000") {
    ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
    zx::result result = dut().template Connect<ffunction::UsbFunctionService::Device>(name);
    if (result.is_error()) {
      return result.take_error();
    }
    ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
    return zx::ok(std::move(result.value()));
  }

  zx::result<fidl::WireSyncClient<fperipheral::Device>> ConnectPeripheral() {
    zx::result result = dut().template Connect<fperipheral::Service::Device>();
    if (result.is_error()) {
      return result.take_error();
    }
    return zx::ok(fidl::WireSyncClient<fperipheral::Device>(std::move(result.value())));
  }

  zx::result<std::tuple<std::shared_ptr<FakeUsbFunction>,
                        fidl::ClientEnd<ffunction::UsbFunctionInterface>>>
  BindFakeFunction() {
    ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
    zx::result endpoints = fidl::CreateEndpoints<ffunction::UsbFunctionInterface>();
    if (endpoints.is_error()) {
      return endpoints.take_error();
    }
    auto fake_function = std::make_shared<FakeUsbFunction>();
    fake_function->Bind(dut().runtime().StartBackgroundDispatcher(), std::move(endpoints->server));
    ExpectState(UsbPeripheral::DeviceState::kWaitForFunctionBind);
    return zx::ok(std::make_tuple(fake_function, std::move(endpoints->client)));
  }

 private:
  fidl::WireSyncClient<fdci::UsbDciInterface> dci_;
  fdf_testing::BackgroundDriverTest<UsbPeripheralTestConfig> driver_test_;
  bool started_driver_ = false;
};

class ManagedUsbPeripheralTest : public UsbPeripheralHarness<true> {
 public:
  usb_peripheral_config::Config GetDriverConfig() override {
    return usb_peripheral_config::Config{};
  }
};
using UnmanagedUsbPeripheralTest = UsbPeripheralHarness<false>;

template <bool manage_lifetime>
class PeripheralReadyTestBase : public UsbPeripheralHarness<manage_lifetime> {
 public:
  struct FunctionClients {
    std::vector<fidl::WireSyncClient<ffunction::UsbFunction>> clients;
    std::vector<std::shared_ptr<FakeUsbFunction>> fakes;
  };

  static fperipheral::wire::DeviceDescriptor CreateTestDeviceDescriptor() {
    fperipheral::wire::DeviceDescriptor device_desc = {};
    device_desc.bcd_usb = 0x0200;
    device_desc.b_device_class = 0;
    device_desc.b_device_sub_class = 0;
    device_desc.b_device_protocol = 0;
    device_desc.b_max_packet_size0 = 64;
    device_desc.id_vendor = 0x18D1;
    device_desc.id_product = 0xA4A2;
    device_desc.bcd_device = 0x0100;
    device_desc.manufacturer = "Google";
    device_desc.product = "Fuchsia";
    device_desc.serial = "123456";
    device_desc.b_num_configurations = 1;
    return device_desc;
  }

  static fidl::VectorView<fidl::VectorView<fperipheral::wire::FunctionDescriptor>>
  CreateTestFunctionDescriptors(fidl::AnyArena& arena) {
    fperipheral::wire::FunctionDescriptor func_desc = {
        .interface_class = 0xFF,
        .interface_subclass = 0,
        .interface_protocol = 0,
    };
    fidl::VectorView<fperipheral::wire::FunctionDescriptor> functions(arena, 1);
    functions[0] = func_desc;
    fidl::VectorView<fidl::VectorView<fperipheral::wire::FunctionDescriptor>> configs(arena, 1);
    configs[0] = functions;
    return configs;
  }

  zx::result<std::vector<uint8_t>> CreateTestRawFunctionDescriptors(
      fidl::WireSyncClient<ffunction::UsbFunction>& function_client) {
    fidl::Arena arena;
    uint8_t interface_num = 0xFF;
    uint8_t ep_out = 0xFF;
    uint8_t ep_in = 0xFF;

    ffunction::wire::EndpointResource endpoints[2];
    zx::result ep_ends1 = fidl::CreateEndpoints<fendpoint::Endpoint>();
    if (ep_ends1.is_error()) {
      return ep_ends1.take_error();
    }
    endpoints[0].direction = fdescriptor::wire::EndpointDirection::kOut;
    endpoints[0].ep_info = BulkEpInfo(arena);
    endpoints[0].max_packet_size = 512;
    endpoints[0].endpoint = std::move(ep_ends1->server);

    zx::result ep_ends2 = fidl::CreateEndpoints<fendpoint::Endpoint>();
    if (ep_ends2.is_error()) {
      return ep_ends2.take_error();
    }
    endpoints[1].direction = fdescriptor::wire::EndpointDirection::kIn;
    endpoints[1].ep_info = BulkEpInfo(arena);
    endpoints[1].max_packet_size = 512;
    endpoints[1].endpoint = std::move(ep_ends2->server);

    fidl::WireResult res = function_client->AllocResources(
        1, fidl::VectorView<ffunction::wire::EndpointResource>::FromExternal(endpoints, 2), {});
    if (!res.ok()) {
      return zx::error(res.status());
    }
    if (res->is_error()) {
      return zx::error(res->error_value());
    }
    interface_num = res->value()->interface_nums[0];
    ep_out = res->value()->endpoint_addrs[0];
    ep_in = res->value()->endpoint_addrs[1];

    std::vector<uint8_t> descriptors = {
        0x09, 0x04, interface_num, 0x00, 0x02, 0xFF, 0x00, 0x00, 0x00,  // Interface
    };
    descriptors.insert(descriptors.end(),
                       {
                           0x07, 0x05, ep_out, 0x02, 0x40, 0x00, 0x00  // Bulk Out
                       });
    descriptors.insert(descriptors.end(), {
                                              0x07, 0x05, ep_in, 0x02, 0x40, 0x00, 0x00  // Bulk In
                                          });

    return zx::ok(descriptors);
  }

  zx::result<FunctionClients> TransitionToPeripheralReady(uint8_t num_functions = 1) {
    FunctionClients result;
    for (uint8_t i = 0; i < num_functions; i++) {
      char name[16];
      snprintf(name, sizeof(name), "function-%03d", i);
      this->dut().runtime().RunUntilIdle();
      this->WaitForChildNode(name);
      auto function_client = this->ConnectFunction(name);
      if (function_client.is_error()) {
        return function_client.take_error();
      }
      auto bind_res = this->BindFakeFunction();
      if (bind_res.is_error()) {
        return bind_res.take_error();
      }

      auto desc_res = CreateTestRawFunctionDescriptors(function_client.value());
      if (desc_res.is_error()) {
        return desc_res.take_error();
      }
      auto configure_res = function_client.value()->Configure(
          fidl::VectorView<uint8_t>::FromExternal(desc_res.value()),
          std::move(std::get<1>(bind_res.value())));
      if (!configure_res.ok()) {
        return zx::error(configure_res.status());
      }
      if (configure_res->is_error()) {
        return zx::error(configure_res->error_value());
      }
      result.clients.push_back(std::move(function_client.value()));
      result.fakes.push_back(std::get<0>(bind_res.value()));
      this->dut().runtime().RunUntilIdle();
    }
    this->ExpectState(UsbPeripheral::DeviceState::kPeripheralReady);
    this->ExpectControllerStarted(true);
    for (auto& fake : result.fakes) {
      fake->ResetCalled();
    }
    return zx::ok(std::move(result));
  }
};

using UnmanagedUsbPeripheralReadyTest = PeripheralReadyTestBase<false>;

class UsbPeripheralReadyTest : public PeripheralReadyTestBase<true> {
 public:
  void SetUp() override {
    PeripheralReadyTestBase<true>::SetUp();
    auto res = TransitionToPeripheralReady();
    ASSERT_OK(res);
    function_clients_ = std::move(res.value());
  }

  usb_peripheral_config::Config GetDriverConfig() override {
    usb_peripheral_config::Config config;
    config.functions() = {"test"};
    return config;
  }

  typename PeripheralReadyTestBase<true>::FunctionClients function_clients_;
};

class UsbPeripheralFunctionTest : public ManagedUsbPeripheralTest {
 public:
  usb_peripheral_config::Config GetDriverConfig() override {
    usb_peripheral_config::Config config;
    config.functions() = {"test"};
    return config;
  }

  zx::result<uint8_t> ConfigureDefaultFunction(
      fidl::WireSyncClient<ffunction::UsbFunction>& function_client, fidl::AnyArena& arena) {
    auto endpoints = fidl::VectorView<ffunction::wire::EndpointResource>(arena, 1);
    endpoints[0].direction = fdescriptor::wire::EndpointDirection::kIn;
    endpoints[0].ep_info = BulkEpInfo(arena);
    endpoints[0].max_packet_size = 512;
    auto ep_endpoints = fidl::Endpoints<fendpoint::Endpoint>::Create();
    endpoints[0].endpoint = std::move(ep_endpoints.server);

    fidl::WireResult alloc_res =
        function_client->AllocResources(1, endpoints, fidl::VectorView<fidl::StringView>());
    if (!alloc_res.ok()) {
      return zx::error(alloc_res.status());
    }
    if (alloc_res->is_error()) {
      return zx::error(alloc_res->error_value());
    }

    uint8_t ep_addr = alloc_res->value()->endpoint_addrs[0];
    uint8_t interface_num = alloc_res->value()->interface_nums[0];

    zx::result fake_function_result = BindFakeFunction();
    if (fake_function_result.is_error()) {
      return fake_function_result.take_error();
    }
    auto [fake_function, fake_function_endpoint] = std::move(fake_function_result.value());

    usb_interface_descriptor_t intf_desc = {
        .b_length = sizeof(usb_interface_descriptor_t),
        .b_descriptor_type = USB_DT_INTERFACE,
        .b_interface_number = interface_num,
        .b_alternate_setting = 0,
        .b_num_endpoints = 1,
        .b_interface_class = 8,
        .b_interface_sub_class = 6,
        .b_interface_protocol = 80,
    };
    usb_endpoint_descriptor_t ep_desc = {
        .b_length = sizeof(usb_endpoint_descriptor_t),
        .b_descriptor_type = USB_DT_ENDPOINT,
        .b_endpoint_address = ep_addr,
        .bm_attributes = static_cast<uint8_t>(fdescriptor::EndpointType::kBulk),
        .w_max_packet_size = 512,
    };
    std::vector<uint8_t> descriptors(sizeof(intf_desc) + sizeof(ep_desc));
    memcpy(descriptors.data(), &intf_desc, sizeof(intf_desc));
    memcpy(descriptors.data() + sizeof(intf_desc), &ep_desc, sizeof(ep_desc));

    fidl::WireResult configure_res = function_client->Configure(
        fidl::VectorView<uint8_t>::FromExternal(descriptors.data(), descriptors.size()),
        std::move(fake_function_endpoint));
    if (!configure_res.ok()) {
      return zx::error(configure_res.status());
    }
    if (configure_res->is_error()) {
      return zx::error(configure_res->error_value());
    }

    return zx::ok(ep_addr);
  }
};

class UsbPeripheralFunctionConfigureEndpointTest : public UsbPeripheralFunctionTest,
                                                   public testing::WithParamInterface<bool> {};

}  // namespace usb_peripheral::test

#endif  // SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_PERIPHERAL_TEST_HARNESS_H_
