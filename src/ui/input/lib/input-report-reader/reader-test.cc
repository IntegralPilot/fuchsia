// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/input/lib/input-report-reader/reader.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/assert.h>
#include <zircon/time.h>

#include <algorithm>
#include <cinttypes>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

struct MouseReport {
  int64_t movement_x;
  int64_t movement_y;
  void ToFidlInputReport(
      fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
      fidl::AnyArena& allocator) const {
    auto mouse = fuchsia_input_report::wire::MouseInputReport::Builder(allocator);
    mouse.movement_x(this->movement_x);
    mouse.movement_y(this->movement_y);

    input_report.mouse(mouse.Build());
  }
};

template <size_t kMaxBatchSize = 1, zx_duration_t kMaxBatchDelayNs = 0>
class MouseDevice : public fidl::WireServer<fuchsia_input_report::InputDevice> {
 public:
  zx_status_t Start();

  size_t SendReport(const MouseReport& report);
  // Function for testing that blocks until a new reader is connected.
  zx_status_t WaitForNextReader(zx::duration timeout) {
    zx_status_t status = next_reader_wait_.Wait(timeout);
    if (status == ZX_OK) {
      next_reader_wait_.Reset();
    }
    return ZX_OK;
  }
  void SetInitialReport(MouseReport report) { initial_report_ = report; }

  // The FIDL methods for InputDevice.
  void GetInputReportsReader(GetInputReportsReaderRequestView request,
                             GetInputReportsReaderCompleter::Sync& completer) override;
  void GetInputReportsReaderV2(GetInputReportsReaderV2RequestView request,
                               GetInputReportsReaderV2Completer::Sync& completer) override;
  void GetDescriptor(GetDescriptorCompleter::Sync& completer) override;
  void SendOutputReport(SendOutputReportRequestView request,
                        SendOutputReportCompleter::Sync& completer) override;
  void GetFeatureReport(GetFeatureReportCompleter::Sync& completer) override;
  void SetFeatureReport(SetFeatureReportRequestView request,
                        SetFeatureReportCompleter::Sync& completer) override;
  void GetInputReport(GetInputReportRequestView request,
                      GetInputReportCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_input_report::InputDevice> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fprintf(stderr, "Unexpected fidl method invoked: %" PRIu64 "\n", metadata.method_ordinal);
  }

 private:
  libsync::Completion next_reader_wait_;
  input_report_reader::InputReportReaderManager<MouseReport, 10, kMaxBatchSize, kMaxBatchDelayNs>
      input_report_readers_;
  async::Loop loop_ = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  std::optional<MouseReport> initial_report_ = std::nullopt;
};

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
zx_status_t MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::Start() {
  zx_status_t status = ZX_OK;
  if ((status = loop_.StartThread("MouseDeviceReaderThread")) != ZX_OK) {
    return status;
  }
  return ZX_OK;
}

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
size_t MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::SendReport(const MouseReport& report) {
  return input_report_readers_.SendReportToAllReaders(report);
}

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
void MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::GetInputReportsReader(
    GetInputReportsReaderRequestView request, GetInputReportsReaderCompleter::Sync& completer) {
  ZX_PANIC("GetInputReportsReader is no longer supported; use GetInputReportsReaderV2");
}

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
void MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::GetInputReportsReaderV2(
    GetInputReportsReaderV2RequestView request, GetInputReportsReaderV2Completer::Sync& completer) {
  const uint16_t max_unacknowledged_reports =
      std::max<uint16_t>(1, request->max_unacknowledged_reports_limit);
  libsync::Completion wait;
  zx_status_t status = ZX_OK;
  async::PostTask(loop_.dispatcher(), [&]() {
    status = input_report_readers_.CreateReaderV2(loop_.dispatcher(), std::move(request->reader),
                                                  max_unacknowledged_reports, initial_report_);
    if (status == ZX_OK) {
      next_reader_wait_.Signal();
    }
    wait.Signal();
  });
  wait.Wait();
  if (status != ZX_OK) {
    completer.Close(status);
    return;
  }
  completer.Reply(max_unacknowledged_reports);
}

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
void MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::GetDescriptor(
    GetDescriptorCompleter::Sync& completer) {
  fidl::Arena allocator;

  completer.Reply(fuchsia_input_report::wire::DeviceDescriptor::Builder(allocator).Build());
}

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
void MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::SendOutputReport(
    SendOutputReportRequestView request, SendOutputReportCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
void MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::GetFeatureReport(
    GetFeatureReportCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
void MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::SetFeatureReport(
    SetFeatureReportRequestView request, SetFeatureReportCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

template <size_t kMaxBatchSize, zx_duration_t kMaxBatchDelayNs>
void MouseDevice<kMaxBatchSize, kMaxBatchDelayNs>::GetInputReport(
    GetInputReportRequestView request, GetInputReportCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

class InputReportReaderTests : public testing::Test {
  void SetUp() override {
    ASSERT_EQ(mouse_.Start(), ZX_OK);
    auto [client, server] = fidl::Endpoints<fuchsia_input_report::InputDevice>::Create();
    auto result = fidl::BindServer(loop_.dispatcher(), std::move(server), &mouse_);
    input_device_ = fidl::WireSyncClient<fuchsia_input_report::InputDevice>(std::move(client));
    ASSERT_EQ(loop_.StartThread("MouseDeviceThread"), ZX_OK);
  }

  void TearDown() override {}

 protected:
  MouseDevice<> mouse_;
  async::Loop loop_ = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  fidl::WireSyncClient<fuchsia_input_report::InputDevice> input_device_;
};

class TestReaderV2EventHandler
    : public fidl::WireAsyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
 public:
  // `loop` must outlive the `TestReaderV2EventHandler` instance.
  explicit TestReaderV2EventHandler(async::Loop& loop) : loop_(loop) {}

  void OnInputReports(
      fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) override {
    reports_received_ += event->reports.size();
    last_stamp_ = event->last_report_stamp;
    events_received_++;
    loop_.Quit();
  }
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

  size_t events_received() const { return events_received_; }
  size_t reports_received() const { return reports_received_; }
  uint64_t last_stamp() const { return last_stamp_; }

 private:
  async::Loop& loop_;
  size_t events_received_ = 0;
  size_t reports_received_ = 0;
  uint64_t last_stamp_ = 0;
};

TEST_F(InputReportReaderTests, V2Basic) {
  async::Loop loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> get_reader_result =
      input_device_->GetInputReportsReaderV2(std::move(server_end),
                                             /*max_unacknowledged_reports=*/5);
  ASSERT_OK(get_reader_result.status());
  ASSERT_EQ(5, get_reader_result.value().max_unacknowledged_reports);
  mouse_.WaitForNextReader(zx::duration::infinite());

  TestReaderV2EventHandler event_handler(loop);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> client(
      std::move(client_end), loop.dispatcher(), &event_handler);

  MouseReport report;
  report.movement_x = 100;
  report.movement_y = 200;
  mouse_.SendReport(report);

  loop.Run();
  ASSERT_EQ(1u, event_handler.events_received());
  ASSERT_EQ(1u, event_handler.reports_received());
  ASSERT_EQ(1u, event_handler.last_stamp());

  // Acknowledge and send another.
  ASSERT_EQ(ZX_OK, client->AcknowledgeReports(event_handler.last_stamp()).status());

  loop.ResetQuit();
  mouse_.SendReport(report);
  loop.Run();

  ASSERT_EQ(2u, event_handler.events_received());
  ASSERT_EQ(2u, event_handler.reports_received());
  ASSERT_EQ(2u, event_handler.last_stamp());
}

TEST_F(InputReportReaderTests, V2UnacknowledgedLimit) {
  async::Loop loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> get_reader_result =
      input_device_->GetInputReportsReaderV2(std::move(server_end),
                                             /*max_unacknowledged_reports=*/2);
  ASSERT_OK(get_reader_result.status());
  ASSERT_EQ(2, get_reader_result.value().max_unacknowledged_reports);
  mouse_.WaitForNextReader(zx::duration::infinite());

  TestReaderV2EventHandler event_handler(loop);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> client(
      std::move(client_end), loop.dispatcher(), &event_handler);

  MouseReport report = {.movement_x = 10, .movement_y = 20};
  // Send 3 reports while max_unacknowledged_reports is 2.
  mouse_.SendReport(report);
  mouse_.SendReport(report);
  mouse_.SendReport(report);

  // Run the loop twice to receive the first two events.
  loop.Run();
  loop.ResetQuit();
  loop.Run();

  ASSERT_EQ(2u, event_handler.events_received());
  ASSERT_EQ(2u, event_handler.last_stamp());

  // Running until idle should show no 3rd event yet because limit is reached.
  loop.ResetQuit();
  loop.RunUntilIdle();
  ASSERT_EQ(2u, event_handler.events_received());

  // Now acknowledge up to stamp 2.
  ASSERT_EQ(ZX_OK, client->AcknowledgeReports(2).status());

  // Now the 3rd event should be received.
  loop.Run();
  ASSERT_EQ(3u, event_handler.events_received());
  ASSERT_EQ(3u, event_handler.last_stamp());
}

TEST(InputReportReaderTestsV2, V2BatchSizeGrouping) {
  async::Loop client_loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  async::Loop device_loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  ASSERT_EQ(device_loop.StartThread("DeviceThread"), ZX_OK);

  MouseDevice</*kMaxBatchSize=*/5, /*kMaxBatchDelayNs=*/1'000'000'000> mouse;
  ASSERT_EQ(mouse.Start(), ZX_OK);

  auto [client, server] = fidl::Endpoints<fuchsia_input_report::InputDevice>::Create();
  fidl::BindServer(device_loop.dispatcher(), std::move(server), &mouse);
  fidl::WireSyncClient<fuchsia_input_report::InputDevice> input_device(std::move(client));

  // Get reader
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> get_reader_result =
      input_device->GetInputReportsReaderV2(std::move(server_end), 10);
  ASSERT_OK(get_reader_result.status());
  mouse.WaitForNextReader(zx::duration::infinite());

  TestReaderV2EventHandler event_handler(client_loop);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> client_v2(
      std::move(client_end), client_loop.dispatcher(), &event_handler);

  MouseReport report = {.movement_x = 1, .movement_y = 2};
  // Send 5 reports (equal to max batch size).
  for (size_t i = 0; i < 5; i++) {
    mouse.SendReport(report);
  }

  client_loop.Run();
  // They should be grouped into 1 event.
  ASSERT_EQ(1u, event_handler.events_received());
  ASSERT_EQ(5u, event_handler.reports_received());
}

TEST(InputReportReaderTestsV2, V2BatchSizeLimit) {
  async::Loop client_loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  async::Loop device_loop = async::Loop(&kAsyncLoopConfigNeverAttachToThread);
  ASSERT_EQ(device_loop.StartThread("DeviceThread"), ZX_OK);

  MouseDevice</*kMaxBatchSize=*/3, /*kMaxBatchDelayNs=*/1'000'000'000> mouse;
  ASSERT_EQ(mouse.Start(), ZX_OK);

  auto [client, server] = fidl::Endpoints<fuchsia_input_report::InputDevice>::Create();
  fidl::BindServer(device_loop.dispatcher(), std::move(server), &mouse);
  fidl::WireSyncClient<fuchsia_input_report::InputDevice> input_device(std::move(client));

  // Get reader with max_unacknowledged_events = 2.
  auto [client_end, server_end] =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> get_reader_result =
      input_device->GetInputReportsReaderV2(std::move(server_end), 2);
  ASSERT_OK(get_reader_result.status());
  mouse.WaitForNextReader(zx::duration::infinite());

  TestReaderV2EventHandler event_handler(client_loop);
  fidl::WireClient<fuchsia_input_report::InputReportsReaderV2> client_v2(
      std::move(client_end), client_loop.dispatcher(), &event_handler);

  MouseReport report = {.movement_x = 10, .movement_y = 20};
  // Send 9 reports (3 batches of 3).
  for (size_t i = 0; i < 9; i++) {
    mouse.SendReport(report);
  }

  // Run the loop twice to receive the first two batches.
  client_loop.Run();
  client_loop.ResetQuit();
  client_loop.Run();

  ASSERT_EQ(2u, event_handler.events_received());
  ASSERT_EQ(6u, event_handler.reports_received());
  ASSERT_EQ(2u, event_handler.last_stamp());

  // Running until idle should show no 3rd batch yet because limit is reached.
  client_loop.ResetQuit();
  client_loop.RunUntilIdle();
  ASSERT_EQ(2u, event_handler.events_received());

  // Now acknowledge up to stamp 2.
  ASSERT_EQ(ZX_OK, client_v2->AcknowledgeReports(2).status());

  // Now the 3rd batch should be received.
  client_loop.Run();
  ASSERT_EQ(3u, event_handler.events_received());
  ASSERT_EQ(9u, event_handler.reports_received());
  ASSERT_EQ(3u, event_handler.last_stamp());
}
