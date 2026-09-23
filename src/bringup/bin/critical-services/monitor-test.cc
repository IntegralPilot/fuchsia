// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/critical-services/monitor.h"

#include <fidl/fuchsia.power.button/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>

#include <zxtest/zxtest.h>

using PwrAction = fuchsia_power_button::wire::Action;
using PwrButtonEvent = fuchsia_power_button::wire::PowerButtonEvent;

class EventHandler : public fidl::WireAsyncEventHandler<fuchsia_power_button::Monitor> {
 public:
  EventHandler() = default;

  void OnButtonEvent(
      fidl::WireEvent<fuchsia_power_button::Monitor::OnButtonEvent>* event) override {
    e = event->event;
  }

  PwrButtonEvent e = PwrButtonEvent::Unknown();
};

class MonitorTest : public zxtest::Test {
 public:
  MonitorTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}
  void SetUp() override {
    auto endpoints = fidl::Endpoints<fuchsia_power_button::Monitor>::Create();

    client_.Bind(std::move(endpoints.client), loop_.dispatcher(), &event_handler_);
    monitor_.Publish()(std::move(endpoints.server));
  }

 protected:
  async::Loop loop_;
  pwrbtn::PowerButtonMonitor monitor_{loop_.dispatcher()};
  fidl::WireClient<fuchsia_power_button::Monitor> client_;
  EventHandler event_handler_;
};

TEST_F(MonitorTest, TestSetAction) {
  client_->SetAction(PwrAction::kIgnore).ThenExactlyOnce([&](auto& resp) {});
  ASSERT_OK(loop_.RunUntilIdle());
  client_->GetAction().ThenExactlyOnce([&](auto& resp) {
    ASSERT_OK(resp.status());
    ASSERT_EQ(resp->action, PwrAction::kIgnore);
  });
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_OK(monitor_.DoAction());
}

TEST_F(MonitorTest, TestGetActionDefault) {
  client_->GetAction().ThenExactlyOnce([&](auto& resp) {
    ASSERT_OK(resp.status());
    ASSERT_EQ(resp->action, PwrAction::kShutdown);
  });
  ASSERT_OK(loop_.RunUntilIdle());
}

TEST_F(MonitorTest, TestSendButtonEvent) {
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_TRUE(event_handler_.e.IsUnknown());
  ASSERT_NE(event_handler_.e, PwrButtonEvent::kPress);

  ASSERT_OK(monitor_.SendButtonEvent(PwrButtonEvent::kPress));
  ASSERT_OK(loop_.RunUntilIdle());
  ASSERT_EQ(event_handler_.e, PwrButtonEvent::kPress);
}

TEST_F(MonitorTest, TestShutdownFailsWithNoService) { ASSERT_NOT_OK(monitor_.DoAction()); }
