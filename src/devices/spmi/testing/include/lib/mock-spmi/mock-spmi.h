// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SPMI_TESTING_INCLUDE_LIB_MOCK_SPMI_MOCK_SPMI_H_
#define SRC_DEVICES_SPMI_TESTING_INCLUDE_LIB_MOCK_SPMI_MOCK_SPMI_H_

#include <fidl/fuchsia.hardware.spmi/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spmi/cpp/test_base.h>

#include <memory>
#include <optional>
#include <queue>

#include <gtest/gtest.h>

namespace mock_spmi {

class MockSpmi : public fidl::testing::TestBase<fuchsia_hardware_spmi::Device> {
 public:
  void ExpectGetProperties(uint16_t sid, std::string name, uint32_t register_width_bytes = 1) {
    expectations_.push({
        .type = CallType::kGetProperties,
        .sid = sid,
        .name = name,
        .register_width_bytes = register_width_bytes,
    });
  }

  void ExpectRegisterRead(uint16_t address, uint32_t size_bytes,
                          std::vector<uint8_t> expected_data) {
    expectations_.push({
        .type = CallType::kRead,
        .address = address,
        .size_bytes = size_bytes,
        .data = std::move(expected_data),
    });
  }

  void ExpectRegisterRead(uint16_t address, uint32_t size_bytes,
                          fuchsia_hardware_spmi::DriverError expected_error) {
    expectations_.push({
        .type = CallType::kRead,
        .error = expected_error,
        .address = address,
        .size_bytes = size_bytes,
    });
  }

  void ExpectRegisterWrite(
      uint16_t address, std::vector<uint8_t> data,
      std::optional<fuchsia_hardware_spmi::DriverError> expected_error = std::nullopt) {
    expectations_.push({
        .type = CallType::kWrite,
        .error = expected_error,
        .address = address,
        .data = std::move(data),
    });
  }

  struct QueuedExpectation {
    fuchsia_hardware_spmi::Register8 data;
    std::shared_ptr<zx::eventpair> out_lease;
  };

  zx::eventpair ExpectWatchControllerWriteCommands(
      uint8_t address, uint16_t data, std::shared_ptr<zx::eventpair> out_lease = nullptr) {
    if (std::holds_alternative<std::deque<QueuedExpectation>>(expected_watches_[address])) {
      std::get<std::deque<QueuedExpectation>>(expected_watches_[address])
          .emplace_back(
              QueuedExpectation{fuchsia_hardware_spmi::Register8{address, data}, out_lease});
      return SyncWatchControllerWriteCommands();
    }

    auto& request = std::get<WatchControllerRequest>(expected_watches_[address]);
    zx::eventpair e0, e1;
    EXPECT_EQ(zx::eventpair::create(0, &e0, out_lease ? out_lease.get() : &e1), ZX_OK);
    request.completer.Reply(
        zx::ok(fuchsia_hardware_spmi::DeviceWatchControllerWriteCommandsResponse{
            std::vector{fuchsia_hardware_spmi::Register8{address, data}},
            request.wake_lease_requested ? std::move(e0) : zx::eventpair{}}));
    expected_watches_[address] = std::deque<QueuedExpectation>{};

    return SyncWatchControllerWriteCommands();
  }

  // Returns true if the SPMI device is currently watching for write commands
  // at `address`.
  bool IsWatchingControllerWriteCommandsAt(uint8_t address) {
    if (!expected_watches_.contains(address)) {
      return false;
    }
    if (!std::holds_alternative<WatchControllerRequest>(expected_watches_.at(address))) {
      return false;
    }
    return true;
  }

  void VerifyAndClear() {
    EXPECT_TRUE(expectations_.empty());
    for (auto const& [addr, watch] : expected_watches_) {
      if (std::holds_alternative<std::deque<QueuedExpectation>>(watch)) {
        EXPECT_TRUE(std::get<std::deque<QueuedExpectation>>(watch).empty());
      }
    }
    expectations_ = {};
    expected_watches_.clear();
  }

  zx::eventpair SyncWatchControllerWriteCommands() {
    zx::eventpair e0, e1;
    EXPECT_EQ(zx::eventpair::create(0, &e0, &e1), ZX_OK);
    sync_watch_.emplace(std::move(e0));
    return std::move(e1);
  }

  fidl::ServerBindingGroup<fuchsia_hardware_spmi::Device> bindings_;

 private:
  enum CallType : uint8_t {
    kRead = 0,
    kWrite = 1,
    kGetProperties = 2,
  };

  struct SpmiExpectation {
    CallType type;

    std::optional<fuchsia_hardware_spmi::DriverError> error = std::nullopt;

    uint16_t address;
    uint32_t size_bytes;
    std::vector<uint8_t> data;

    uint16_t sid;
    std::string name;
    uint32_t register_width_bytes;
  };

  void GetProperties(GetPropertiesCompleter::Sync& completer) override {
    ASSERT_FALSE(expectations_.empty());
    auto expectation = std::move(expectations_.front());
    expectations_.pop();

    ASSERT_EQ(expectation.type, CallType::kGetProperties);
    completer.Reply({{
        .sid = expectation.sid,
        .name = expectation.name,
        .register_width_bytes = expectation.register_width_bytes,
    }});
  }

  void RegisterRead(RegisterReadRequest& request, RegisterReadCompleter::Sync& completer) override {
    ASSERT_FALSE(expectations_.empty());
    auto expectation = std::move(expectations_.front());
    expectations_.pop();

    ASSERT_EQ(expectation.type, CallType::kRead);
    EXPECT_EQ(expectation.address, request.address());
    EXPECT_EQ(expectation.size_bytes, request.size_bytes());
    EXPECT_EQ(expectation.size_bytes, expectation.data.size());
    if (expectation.error.has_value()) {
      completer.Reply(zx::error(expectation.error.value()));
    } else {
      completer.Reply(zx::ok(std::move(expectation.data)));
    }
  }

  void RegisterWrite(RegisterWriteRequest& request,
                     RegisterWriteCompleter::Sync& completer) override {
    ASSERT_FALSE(expectations_.empty());
    auto expectation = std::move(expectations_.front());
    expectations_.pop();

    ASSERT_EQ(expectation.type, CallType::kWrite);
    EXPECT_EQ(expectation.address, request.address());
    ASSERT_EQ(expectation.data.size(), request.data().size());
    EXPECT_EQ(expectation.data, std::vector<uint8_t>(request.data().begin(), request.data().end()));
    if (expectation.error.has_value()) {
      completer.Reply(zx::error(expectation.error.value()));
    } else {
      completer.Reply(zx::ok());
    }
  }

  void WatchControllerWriteCommands(
      WatchControllerWriteCommandsRequest& request,
      WatchControllerWriteCommandsCompleter::Sync& completer) override {
    EXPECT_EQ(request.size(), 1);

    if (!sync_watch_.empty()) {
      sync_watch_.front().signal_peer(0, ZX_USER_SIGNAL_0);
      sync_watch_.pop();
    }

    if (std::holds_alternative<WatchControllerRequest>(expected_watches_[request.address()])) {
      ZX_ASSERT(false);
      return;
    }

    if (std::get<std::deque<QueuedExpectation>>(expected_watches_[request.address()]).empty()) {
      expected_watches_[request.address()] =
          WatchControllerRequest{.completer = completer.ToAsync(),
                                 .wake_lease_requested = request.setup_wake_lease().is_valid()};
      return;
    }

    auto& expectation =
        std::get<std::deque<QueuedExpectation>>(expected_watches_[request.address()]).front();
    EXPECT_EQ(expectation.data.address(), request.address());

    zx::eventpair e0, e1;
    EXPECT_EQ(
        zx::eventpair::create(0, &e0, expectation.out_lease ? expectation.out_lease.get() : &e1),
        ZX_OK);
    completer.Reply(zx::ok(fuchsia_hardware_spmi::DeviceWatchControllerWriteCommandsResponse{
        std::vector{expectation.data},
        request.setup_wake_lease().is_valid() ? std::move(e0) : zx::eventpair{}}));
    std::get<std::deque<QueuedExpectation>>(expected_watches_[request.address()]).pop_front();
  }

  void CancelWatchControllerWriteCommands(
      CancelWatchControllerWriteCommandsRequest& request,
      CancelWatchControllerWriteCommandsCompleter::Sync& completer) override {
    EXPECT_EQ(request.size(), 1);

    if (std::holds_alternative<std::deque<QueuedExpectation>>(
            expected_watches_[request.address()])) {
      ZX_ASSERT(
          std::get<std::deque<QueuedExpectation>>(expected_watches_[request.address()]).empty());
      completer.Reply(zx::ok());
      return;
    }

    std::get<WatchControllerRequest>(expected_watches_[request.address()])
        .completer.Reply(zx::error(ZX_ERR_CANCELED));
    expected_watches_[request.address()] = std::deque<QueuedExpectation>{};

    completer.Reply(zx::ok());
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_spmi::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    ASSERT_TRUE(false);
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    FAIL();
  }

  std::queue<SpmiExpectation> expectations_;
  struct WatchControllerRequest {
    WatchControllerWriteCommandsCompleter::Async completer;
    bool wake_lease_requested;
  };
  std::map<uint8_t, std::variant<std::deque<QueuedExpectation>, WatchControllerRequest>>
      expected_watches_;
  std::queue<zx::eventpair> sync_watch_;
};

}  // namespace mock_spmi

#endif  // SRC_DEVICES_SPMI_TESTING_INCLUDE_LIB_MOCK_SPMI_MOCK_SPMI_H_
