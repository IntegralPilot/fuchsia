// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "resetctl.h"

#include <fidl/fuchsia.hardware.reset/cpp/fidl.h>
#include <fidl/fuchsia.hardware.reset/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include <gtest/gtest.h>

namespace resetctl {

class FakeResetDevice : public fidl::Server<fuchsia_hardware_reset::Reset> {
 public:
  FakeResetDevice() : loop_(&kAsyncLoopConfigNeverAttachToThread) { loop_.StartThread(); }

  async_dispatcher_t* dispatcher() { return loop_.dispatcher(); }

  void Assert(AssertCompleter::Sync& completer) override {
    asserted_ = true;
    completer.Reply(zx::ok());
  }

  void Deassert(DeassertCompleter::Sync& completer) override {
    asserted_ = false;
    completer.Reply(zx::ok());
  }

  void Toggle(ToggleCompleter::Sync& completer) override {
    toggle_called_ = true;
    completer.Reply(zx::ok());
  }

  void ToggleWithTimeout(ToggleWithTimeoutRequest& request,
                         ToggleWithTimeoutCompleter::Sync& completer) override {
    timeout_called_ = true;
    timeout_value_ = request.timeout();
    completer.Reply(zx::ok());
  }

  void Status(StatusCompleter::Sync& completer) override { completer.Reply(zx::ok(asserted_)); }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_reset::Reset> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  bool asserted_ = false;
  bool toggle_called_ = false;
  bool timeout_called_ = false;
  zx_duration_t timeout_value_ = 0;

 private:
  async::Loop loop_;
};

TEST(ResetCtlTest, Assert) {
  FakeResetDevice fake_device;
  auto endpoints = fidl::Endpoints<fuchsia_hardware_reset::Reset>::Create();

  fidl::BindServer(fake_device.dispatcher(), std::move(endpoints.server), &fake_device);

  const char* argv[] = {"resetctl", "assert"};
  auto result = resetctl::Run(2, argv, std::move(endpoints.client));

  EXPECT_TRUE(result.is_ok());
  EXPECT_TRUE(fake_device.asserted_);
}

TEST(ResetCtlTest, Deassert) {
  FakeResetDevice fake_device;
  fake_device.asserted_ = true;
  auto endpoints = fidl::Endpoints<fuchsia_hardware_reset::Reset>::Create();

  fidl::BindServer(fake_device.dispatcher(), std::move(endpoints.server), &fake_device);

  const char* argv[] = {"resetctl", "deassert"};
  auto result = resetctl::Run(2, argv, std::move(endpoints.client));

  EXPECT_TRUE(result.is_ok());
  EXPECT_FALSE(fake_device.asserted_);
}

TEST(ResetCtlTest, Toggle) {
  FakeResetDevice fake_device;
  auto endpoints = fidl::Endpoints<fuchsia_hardware_reset::Reset>::Create();

  fidl::BindServer(fake_device.dispatcher(), std::move(endpoints.server), &fake_device);

  const char* argv[] = {"resetctl", "toggle"};
  auto result = resetctl::Run(2, argv, std::move(endpoints.client));

  EXPECT_TRUE(result.is_ok());
  EXPECT_TRUE(fake_device.toggle_called_);
}

TEST(ResetCtlTest, ToggleWithTimeout) {
  FakeResetDevice fake_device;
  auto endpoints = fidl::Endpoints<fuchsia_hardware_reset::Reset>::Create();

  fidl::BindServer(fake_device.dispatcher(), std::move(endpoints.server), &fake_device);

  const char* argv[] = {"resetctl", "toggle", "1000"};
  auto result = resetctl::Run(3, argv, std::move(endpoints.client));

  EXPECT_TRUE(result.is_ok());
  EXPECT_TRUE(fake_device.timeout_called_);
  EXPECT_EQ(fake_device.timeout_value_, 1000);
}

TEST(ResetCtlTest, Status) {
  FakeResetDevice fake_device;
  fake_device.asserted_ = true;
  auto endpoints = fidl::Endpoints<fuchsia_hardware_reset::Reset>::Create();

  fidl::BindServer(fake_device.dispatcher(), std::move(endpoints.server), &fake_device);

  const char* argv[] = {"resetctl", "status"};
  auto result = resetctl::Run(2, argv, std::move(endpoints.client));

  EXPECT_TRUE(result.is_ok());
}

}  // namespace resetctl
