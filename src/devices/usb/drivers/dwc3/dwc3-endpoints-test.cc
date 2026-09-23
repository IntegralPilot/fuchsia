// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/sync/cpp/completion.h>

#include <gtest/gtest.h>

#include "src/devices/usb/drivers/dwc3/dwc3-test-fixture.h"
#include "src/devices/usb/drivers/dwc3/dwc3.h"
#include "src/lib/testing/predicates/status.h"

namespace dwc3 {

namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace fdescriptor = fuchsia_hardware_usb_descriptor;
namespace fdci = fuchsia_hardware_usb_dci;
namespace frequest = fuchsia_hardware_usb_request;

namespace {
void AssertZlpUnchainedControlBits(const std::vector<dwc3_trb_t>& trbs, bool enqueue_many) {
  ASSERT_GE(trbs.size(), 2u);
  // First TRB must exactly match normal and hardware-owned (unchained). Due to deadlock prevention,
  // it must now also have IOC.
  uint32_t expected = TRB_TRBCTL_NORMAL | TRB_IOC | TRB_HWO;
  EXPECT_EQ(trbs[0].control, expected);

  // Ongoing transfers do not use TRB_LST
  uint32_t expected_control = TRB_TRBCTL_NORMAL | TRB_IOC | TRB_HWO;
  if (!enqueue_many) {
    expected_control |= TRB_LST;
  }
  EXPECT_EQ(trbs[1].control, expected_control);
  EXPECT_EQ(TRB_BUFSIZ(trbs[1].status), 0u);
}
}  // namespace

// Non-parameterized base fixture for DWC3 endpoint tests.
class Dwc3EndpointsTestBase : public TestFixture<true> {
 public:
  static constexpr uint32_t kResourceId = 12;

  void SetUp() override {
    TestFixture::SetUp();
    dut_.RunInEnvironmentTypeContext([&](Environment& env) {
      // Mock GHWPARAMS0 to return MDWIDTH = 2 (128-bit = 16 bytes).
      auto& ghwparams0 = env.reg_region()[GHWPARAMS0::Get().addr()];
      ghwparams0.SetReadCallback([]() -> uint32_t {
        return GHWPARAMS0::Get().FromValue(0).set_DWC_USB31_MDWIDTH(2).reg_value();
      });
      // Mock GRXFIFOSIZ for FIFO 0 to have depth 64 (1024 bytes).
      auto& grxfifosiz0 = env.reg_region()[GRXFIFOSIZ::Get(0).addr()];
      grxfifosiz0.SetReadCallback(
          []() -> uint32_t { return GRXFIFOSIZ::Get(0).FromValue(0).set_RXFDEP(64).reg_value(); });
      // Mock GTXFIFOSIZ for FIFOs to have depth 64 (1024 bytes).
      for (unsigned i = 0; i < 16; i++) {
        auto& gtxfifosiz = env.reg_region()[GTXFIFOSIZ::Get(i).addr()];
        gtxfifosiz.SetReadCallback([i]() -> uint32_t {
          return GTXFIFOSIZ::Get(i).FromValue(0).set_TXFDEP(64).reg_value();
        });
      }
    });

    // Start the client loop thread to process async callbacks.
    ASSERT_OK(client_loop_.StartThread("client-loop"));
  }

  void TearDown() override {
    ep_client_ = {};
    dci_ = {};
    client_loop_.Shutdown();
    TestFixture::TearDown();
  }

 protected:
  void TriggerConnection(bool start_controller = true,
                         fdescriptor::UsbSpeed speed = fdescriptor::UsbSpeed::kSuper) {
    TriggerConnectionPlugIn(speed);

    auto dci_service = dut_.Connect<fdci::UsbDciService::Device>();
    ASSERT_TRUE(dci_service.is_ok())
        << "Failed to connect to UsbDciService: " << dci_service.status_string();
    dci_.Bind(std::move(*dci_service));

    if (start_controller) {
      fidl::WireResult res = dci_->StartController();
      ASSERT_OK(res.status());
    }
  }

  void SetupEndpoint(uint8_t ep_address, fdescriptor::EndpointType ep_type,
                     uint16_t max_packet_size) {
    fdescriptor::wire::UsbEndpointDescriptor ep_desc{
        .b_length = sizeof(fdescriptor::wire::UsbEndpointDescriptor),
        .b_descriptor_type = USB_DT_ENDPOINT,
        .b_endpoint_address = ep_address,
        .bm_attributes = static_cast<uint8_t>(ep_type),
        .w_max_packet_size = max_packet_size,
        .b_interval = 0,
    };
    fdescriptor::wire::UsbSsEpCompDescriptor ss_comp_desc{
        .b_length = sizeof(fdescriptor::wire::UsbSsEpCompDescriptor),
        .b_descriptor_type = USB_DT_SS_EP_COMPANION,
        .b_max_burst = 0,
        .bm_attributes = 0,
        .w_bytes_per_interval = 0,
    };

    fidl::WireResult config_res = dci_->ConfigureEndpoint(ep_desc, ss_comp_desc);
    ASSERT_OK(config_res.status());
    ASSERT_TRUE(config_res.value().is_ok())
        << "ConfigureEndpoint protocol failed: "
        << zx_status_get_string(config_res.value().error_value());

    zx::result endpoints = fidl::CreateEndpoints<fendpoint::Endpoint>();
    ASSERT_OK(endpoints);
    auto [client_end, server_end] = std::move(*endpoints);

    fidl::WireResult conn_res = dci_->ConnectToEndpoint(ep_address, std::move(server_end));
    ASSERT_OK(conn_res.status());
    ASSERT_TRUE(conn_res.value().is_ok()) << "ConnectToEndpoint protocol failed: "
                                          << zx_status_get_string(conn_res.value().error_value());

    ep_client_.Bind(std::move(client_end), client_loop_.dispatcher(), &event_handler_);
  }

  void RegisterVmo(uint8_t vmo_id, uint64_t size) {
    fidl::Arena arena;
    fendpoint::wire::VmoInfo vmo_info =
        fendpoint::wire::VmoInfo::Builder(arena).id(vmo_id).size(size).Build();

    fidl::WireResult result = ep_client_.wire_sync()->RegisterVmos(
        fidl::VectorView<fendpoint::wire::VmoInfo>::FromExternal(&vmo_info, 1));
    ASSERT_OK(result.status());
    EXPECT_EQ(result->vmos.size(), 1UL);
    EXPECT_EQ(result->vmos[0].id(), vmo_id);
  }

  void QueueRequest(uint8_t vmo_id, uint64_t offset, uint64_t size,
                    fdescriptor::EndpointType ep_type, bool short_bit = false) {
    QueueRequests(1, vmo_id, offset, size, ep_type, short_bit);
  }

  void QueueRequests(size_t count, uint8_t vmo_id, uint64_t offset, uint64_t size,
                     fdescriptor::EndpointType ep_type, bool short_bit = false) {
    std::vector<frequest::Request> reqs;
    reqs.reserve(count);
    for (size_t i = 0; i < count; i++) {
      frequest::BufferRegion region;
      region.buffer(frequest::Buffer::WithVmoId(vmo_id));
      region.offset(offset + (size * i));
      region.size(size);

      std::vector<frequest::BufferRegion> regions;
      regions.reserve(1);
      regions.push_back(std::move(region));

      frequest::RequestInfo req_info =
          (ep_type == fdescriptor::EndpointType::kBulk)
              ? frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo{})
              : frequest::RequestInfo::WithInterrupt(frequest::InterruptRequestInfo{});

      frequest::Request req;
      req.data(std::move(regions));
      req.defer_completion(false);
      req.information(std::move(req_info));
      req.short_(short_bit);
      reqs.push_back(std::move(req));
    }

    // QueueRequests is a one-way (fire-and-forget) FIDL method; calling it on fidl::SharedClient
    // synchronously writes to the channel and returns fit::result<fidl::OneWayError>.
    fit::result result = ep_client_->QueueRequests({std::move(reqs)});
    ASSERT_TRUE(result.is_ok()) << "QueueRequests failed: "
                                << result.error_value().FormatDescription();
  }

  void WaitForState(uint8_t ep_num, TransferState expected_state) {
    dut_.runtime().RunUntil([&]() {
      return dut_.RunInDriverContext<bool>([&](Dwc3& drv) {
        return GetUserEndpoint(drv, ep_num).ep.transfer_state == expected_state;
      });
    });
  }

  void WaitForQueuedCount(uint8_t ep_num, size_t count) {
    dut_.runtime().RunUntil([&]() {
      return dut_.RunInDriverContext<bool>([&](Dwc3& drv) {
        return GetUserEndpoint(drv, ep_num).server->queued_reqs.size() == count;
      });
    });
  }

  void WaitForActiveCount(uint8_t ep_num, size_t count) {
    dut_.runtime().RunUntil([&]() {
      return dut_.RunInDriverContext<bool>([&](Dwc3& drv) {
        return GetUserEndpoint(drv, ep_num).server->active_reqs.size() == count;
      });
    });
  }

  async::Loop client_loop_{&kAsyncLoopConfigNeverAttachToThread};
  fidl::WireSyncClient<fdci::UsbDci> dci_;

  struct CompletionResult {
    zx_status_t status;
    uint64_t transfer_size;
  };

  class EventHandler : public fidl::AsyncEventHandler<fendpoint::Endpoint> {
   public:
    void OnCompletion(fidl::Event<fendpoint::Endpoint::OnCompletion>& event) override {
      std::lock_guard<std::mutex> lock(mutex_);
      for (const auto& completion : event.completion()) {
        completions_.push_back(CompletionResult{
            .status = completion.status().value_or(ZX_ERR_INTERNAL),
            .transfer_size = completion.transfer_size().value_or(0),
        });
      }
      completion_cond_.notify_all();
    }

    std::vector<CompletionResult> WaitForCompletions(size_t count) {
      std::unique_lock<std::mutex> lock(mutex_);
      // Safety watchdog timeout: Under normal test execution, completions arrive
      // near-instantaneously via dispatcher events. 5 seconds provides protection against hanging
      // tests.
      bool success = completion_cond_.wait_for(lock, std::chrono::seconds(5),
                                               [&]() { return completions_.size() >= count; });
      if (!success) {
        ADD_FAILURE() << "WaitForCompletions timed out waiting for " << count << " completions";
        return {};
      }

      size_t take = std::min(count, completions_.size());
      std::vector<CompletionResult> res(std::make_move_iterator(completions_.begin()),
                                        std::make_move_iterator(completions_.begin() + take));
      completions_.erase(completions_.begin(), completions_.begin() + take);
      return res;
    }

    size_t completion_count() {
      std::lock_guard<std::mutex> lock(mutex_);
      return completions_.size();
    }

   private:
    std::mutex mutex_;
    std::condition_variable completion_cond_;
    std::vector<CompletionResult> completions_;
  };

  void SetEnableEnqueueManyTrbs(bool enable) {
    dut_.RunInDriverContext([&](Dwc3& drv) { drv.SetEnableEnqueueManyTrbs(enable); });
  }

  EventHandler event_handler_;
  fidl::SharedClient<fendpoint::Endpoint> ep_client_;
};

// Parameterized fixture over whether enqueueing multiple TRBs is enabled.
class Dwc3EndpointsTest : public Dwc3EndpointsTestBase, public ::testing::WithParamInterface<bool> {
 public:
  void SetUp() override {
    Dwc3EndpointsTestBase::SetUp();
    SetEnableEnqueueManyTrbs(GetParam());
  }
};

TEST_P(Dwc3EndpointsTest, InterruptEndpointQueueAndComplete) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  // Interrupt endpoints are always using a single-transfer setup.
  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Initially transfer state is kIdle.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kIdle);
    EXPECT_FALSE(uep.ep.got_not_ready);
  });

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_TRUE(uep.ep.got_not_ready);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kIdle);
  });

  // Client queues a request.
  QueueRequest(1, 0, 64, fdescriptor::EndpointType::kInterrupt);
  WaitForState(ep_num, TransferState::kStartingSingle);

  // Trigger started event to initialize rsrc_id and transition to active.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);

  // Check state transitions to kActiveSingle.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
  });

  // Host sends Transfer Complete event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
  WaitForState(ep_num, TransferState::kIdle);

  // State should be back to kIdle.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 0u);
  });

  // Verify completion is received.
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 64UL);
}

TEST_P(Dwc3EndpointsTest, BulkEndpointQueueAndComplete) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });

  // Queue first request.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  auto expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
  });

  // Queue second request.
  QueueRequest(1, 512, 512, fdescriptor::EndpointType::kBulk);
  WaitForQueuedCount(ep_num, 1u);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, expected_starting_state);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
    EXPECT_EQ(uep.server->queued_reqs.size(), 1u);
    EXPECT_EQ(uep.server->active_reqs.size(), 1u);
  });

  // Trigger started event for first request to initialize rsrc_id.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  auto expected_first_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_first_state);

  if (enqueue_many) {
    // If enqueue_many is enabled, starting the transfer automatically queues the next queued
    // requests.
    WaitForActiveCount(ep_num, 2u);

    dut_.RunInDriverContext([&](Dwc3& drv) {
      auto& uep = GetUserEndpoint(drv, ep_num);
      EXPECT_EQ(uep.ep.rsrc_id, kResourceId);
      EXPECT_EQ(uep.fifo.GetActiveCount(), 2u);
      EXPECT_EQ(uep.server->queued_reqs.size(), 0u);
    });

    // Complete request 1.
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
    WaitForActiveCount(ep_num, 1u);

    dut_.RunInDriverContext([&](Dwc3& drv) {
      auto& uep = GetUserEndpoint(drv, ep_num);
      EXPECT_EQ(uep.ep.transfer_state, TransferState::kActiveOngoing);
      EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
    });

    // Complete request 2.
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
    WaitForActiveCount(ep_num, 0u);
  } else {
    // Complete request 1. This transitions state to kIdle, and UserEpQueueNext kicks in to start
    // request 2.
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
    WaitForState(ep_num, TransferState::kStartingSingle);

    dut_.RunInDriverContext([&](Dwc3& drv) {
      auto& uep = GetUserEndpoint(drv, ep_num);
      EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
      EXPECT_EQ(uep.server->queued_reqs.size(), 0u);
      EXPECT_EQ(uep.server->active_reqs.size(), 1u);
    });

    // Trigger started event for second request to initialize rsrc_id.
    dut_.RunInDriverContext(
        [&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId + 1); });
    WaitForState(ep_num, TransferState::kActiveSingle);

    // Complete request 2.
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
    WaitForState(ep_num, TransferState::kIdle);
  }

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    auto expected_final_state = enqueue_many ? TransferState::kActiveOngoing : TransferState::kIdle;
    EXPECT_EQ(uep.ep.transfer_state, expected_final_state);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 0u);
    EXPECT_EQ(uep.server->active_reqs.size(), 0u);
  });

  // Verify completions.
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(2);
  ASSERT_EQ(completions.size(), 2UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 512UL);
  EXPECT_OK(completions[1].status);
  EXPECT_EQ(completions[1].transfer_size, 512UL);
}

TEST_P(Dwc3EndpointsTest, CancelAllRequests) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue two requests.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  QueueRequest(1, 512, 512, fdescriptor::EndpointType::kBulk);
  WaitForQueuedCount(ep_num, 1u);

  auto expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  // Trigger started event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  auto expected_state = enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  if (enqueue_many) {
    WaitForActiveCount(ep_num, 2u);
  }

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, expected_state);
    if (enqueue_many) {
      EXPECT_EQ(uep.server->active_reqs.size(), 2u);
      EXPECT_EQ(uep.server->queued_reqs.size(), 0u);
    } else {
      EXPECT_EQ(uep.server->active_reqs.size(), 1u);
      EXPECT_EQ(uep.server->queued_reqs.size(), 1u);
    }
  });

  // Cancel all requests via client asynchronously.
  auto cancel_result =
      std::make_shared<std::optional<fidl::Result<fendpoint::Endpoint::CancelAll>>>();
  auto cancel_completed = std::make_shared<libsync::Completion>();
  ep_client_->CancelAll().Then(
      [cancel_result, cancel_completed](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
        *cancel_result = std::move(res);
        cancel_completed->Signal();
      });

  WaitForState(ep_num, TransferState::kCanceling);
  EXPECT_FALSE(cancel_completed->signaled());

  // The state should be kCanceling, and active_reqs should not be empty yet.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.server->active_reqs.size(), enqueue_many ? 2u : 1u);
  });

  // Hardware emits Command Complete (End Transfer) to acknowledge End Transfer.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferEnded(drv, ep_num); });
  WaitForState(ep_num, TransferState::kIdle);

  dut_.runtime().RunUntil([&]() { return cancel_completed->signaled(); });
  ASSERT_TRUE(cancel_completed->signaled());
  ASSERT_TRUE(cancel_result->has_value());
  ASSERT_TRUE((*cancel_result)->is_ok());

  // Now, active_reqs should be empty.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.server->active_reqs.size(), 0u);
  });

  // Verify completions returned with cancellation error.
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(2);
  ASSERT_EQ(completions.size(), 2UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_CANCELED);
  EXPECT_EQ(completions[1].status, ZX_ERR_CANCELED);
}

TEST_P(Dwc3EndpointsTest, CancelAllRequestsOnControllerStop) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue two requests.
  QueueRequests(2, 1, 0, 512, fdescriptor::EndpointType::kBulk);
  WaitForActiveCount(ep_num, enqueue_many ? 2 : 1);

  auto expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  // Trigger started event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  auto expected_state = enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  if (enqueue_many) {
    WaitForActiveCount(ep_num, 2u);
  }

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, expected_state);
    if (enqueue_many) {
      EXPECT_EQ(uep.server->active_reqs.size(), 2u);
      EXPECT_EQ(uep.server->queued_reqs.size(), 0u);
    } else {
      EXPECT_EQ(uep.server->active_reqs.size(), 1u);
      EXPECT_EQ(uep.server->queued_reqs.size(), 1u);
    }
  });

  // Stop controller.
  fidl::WireResult res = dci_->StopController();
  ASSERT_OK(res.status());
  WaitForState(ep_num, TransferState::kIdle);

  // Now, active_reqs and queued_reqs should be empty.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.server->active_reqs.size(), 0u);
    EXPECT_EQ(uep.server->queued_reqs.size(), 0u);
  });

  // Verify completions returned with cancellation error.
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(2);
  ASSERT_EQ(completions.size(), 2UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_IO_NOT_PRESENT);
  EXPECT_EQ(completions[1].status, ZX_ERR_IO_NOT_PRESENT);
}

TEST_P(Dwc3EndpointsTest, CancelAllRequestsWhenControllerStopped) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  // Stop controller.
  fidl::WireResult res = dci_->StopController();
  ASSERT_OK(res.status());
  WaitForState(ep_num, TransferState::kIdle);

  // Cancel all requests via client. It should reply immediately because controller is stopped.
  auto cancel_result =
      std::make_shared<std::optional<fidl::Result<fendpoint::Endpoint::CancelAll>>>();
  auto cancel_completed = std::make_shared<libsync::Completion>();
  ep_client_->CancelAll().Then(
      [cancel_result, cancel_completed](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
        *cancel_result = std::move(res);
        cancel_completed->Signal();
      });

  dut_.runtime().RunUntil([&]() { return cancel_completed->signaled(); });
  ASSERT_TRUE(cancel_completed->signaled());
  ASSERT_TRUE(cancel_result->has_value());
  ASSERT_TRUE((*cancel_result)->is_ok());
}

TEST_P(Dwc3EndpointsTest, CancelAllRequestsWhenIdle) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  // Endpoint is idle (no requests queued).
  // Cancel all requests via client. It should reply immediately because endpoint is idle.
  auto cancel_result =
      std::make_shared<std::optional<fidl::Result<fendpoint::Endpoint::CancelAll>>>();
  auto cancel_completed = std::make_shared<libsync::Completion>();
  ep_client_->CancelAll().Then(
      [cancel_result, cancel_completed](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
        *cancel_result = std::move(res);
        cancel_completed->Signal();
      });

  dut_.runtime().RunUntil([&]() { return cancel_completed->signaled(); });
  ASSERT_TRUE(cancel_completed->signaled());
  ASSERT_TRUE(cancel_result->has_value());
  ASSERT_TRUE((*cancel_result)->is_ok());
}

TEST_P(Dwc3EndpointsTest, InputEndpointZlpComplete) {
  TriggerConnection();

  // 0x82 is an INPUT (IN) endpoint.
  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  // Configure endpoint as Bulk IN with max packet size 512.
  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request with short_bit = true, size = 512 (multiple of max packet
  // size).
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk, /*short_bit=*/true);

  // Wait for the endpoint state to become starting.
  bool enqueue_many = GetParam();
  TransferState expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  // Verify that two TRBs were written in the FIFO.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 2u);
    const auto& trbs = uep.fifo.Read(2);

    AssertZlpUnchainedControlBits(trbs, enqueue_many);
  });

  // Trigger started event to initialize rsrc_id.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  TransferState expected_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  // Complete first TRB (data TRB).
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  dut_.runtime().RunUntilIdle();

  // The request should NOT be completed yet, because the ZLP TRB is still
  // pending.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
    EXPECT_EQ(uep.server->active_reqs.size(), 1u);
  });

  // Verify that no completions are received.
  EXPECT_EQ(event_handler_.completion_count(), 0u);

  // Complete the second TRB (ZLP TRB).
  dut_.RunInDriverContext([&](Dwc3& drv) {
    if (enqueue_many) {
      TriggerEpTransferInProgress(drv, ep_num);
    } else {
      TriggerEpTransferComplete(drv, ep_num);
    }
  });

  // Wait for the endpoint transfer to complete.
  if (enqueue_many) {
    WaitForActiveCount(ep_num, 0u);
  } else {
    WaitForState(ep_num, TransferState::kIdle);
  }

  // Verify that the completion is received.
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 512UL);
}

TEST_F(Dwc3EndpointsTestBase, RingBufferWraparoundZlp) {
  SetEnableEnqueueManyTrbs(true);
  TriggerConnection();

  const uint8_t ep_address = 0x82;  // IN endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Enqueue initial test request to trigger start_transfer (so it resets FIFO)
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk, false);
  WaitForState(ep_num, TransferState::kStartingOngoing);

  // Trigger started event to initialize rsrc_id and enter kActiveOngoing
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveOngoing);

  // Complete the initial test request
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  dut_.runtime().RunUntilIdle();

  // Now manually advance write and read pointers to `last_ - 1`
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    // Advance the write pointer so it sits on the very last usable entry before the Link TRB
    // (index last_ - 1). This forces 2-TRB queries to wrap across the boundary.
    // TotalSlots() returns capacity minus Link TRB.
    // Thus TotalSlots() evaluates to 255. The Link TRB operates at index 255.
    // Therefore index 254 (TotalSlots() - 1) is strictly the last usable entry before the wrap.
    ASSERT_LT(uep.fifo.WriteOffset(), uep.fifo.TotalSlots());
    size_t advance_count = uep.fifo.TotalSlots() - 1 - uep.fifo.WriteOffset();
    Dwc3TestHelper::AdvanceFifo(uep.fifo, advance_count);
    ASSERT_EQ(uep.fifo.GetActiveCount(), 0u);
    EXPECT_EQ(uep.fifo.WriteOffset(), uep.fifo.TotalSlots() - 1)
        << "Failed to advance write pointer to the wraparound boundary";
    EXPECT_EQ(uep.fifo.ReadOffset(), uep.fifo.TotalSlots() - 1)
        << "Failed to advance read pointer to the wraparound boundary";
  });

  // Enqueue the primary request that requires two TRBs and will trigger a ring buffer wraparound.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  QueueRequest(1, 512, 512, fdescriptor::EndpointType::kBulk, true);
  WaitForActiveCount(ep_num, 1);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 2u);
    const auto& trbs = uep.fifo.Read(2);

    // Note: RingBufferWraparoundZlp is only ever executed with enqueue_many == true
    AssertZlpUnchainedControlBits(trbs, true);

    // Explicitly verify pointer addresses to prove wrap
    auto* initial_read = uep.fifo.current_read();
    uep.fifo.AdvanceRead();
    auto* wrapped_read = uep.fifo.current_read();

    // Distance should not be linear, proving it wrapped.
    EXPECT_LT(wrapped_read, initial_read);
  });
}

TEST_P(Dwc3EndpointsTest, InputEndpointMultiPacketZlpComplete) {
  TriggerConnection();

  const uint8_t ep_address = 0x82;  // IN endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  // Configure endpoint as Bulk IN with max packet size 512.
  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 8192);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a multi-packet request (4096 bytes = 8 x 512-byte max packets) with short_bit = true.
  QueueRequest(1, 0, 4096, fdescriptor::EndpointType::kBulk, /*short_bit=*/true);

  bool enqueue_many = GetParam();
  TransferState expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  // Verify that two TRBs were written: 1 multi-packet Data TRB (4096 bytes) + 1 ZLP TRB.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 2u);
    const auto& trbs = uep.fifo.Read(2);

    EXPECT_EQ(TRB_BUFSIZ(trbs[0].status), 4096UL);
    EXPECT_EQ(TRB_BUFSIZ(trbs[1].status), 0UL);
    AssertZlpUnchainedControlBits(trbs, enqueue_many);
  });

  // Trigger started event to initialize rsrc_id.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  TransferState expected_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  // Complete data TRB.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  dut_.runtime().RunUntilIdle();

  // Request pending completion of ZLP TRB.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
  });

  // Complete ZLP TRB.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    if (enqueue_many) {
      TriggerEpTransferInProgress(drv, ep_num);
    } else {
      TriggerEpTransferComplete(drv, ep_num);
    }
  });

  if (enqueue_many) {
    WaitForActiveCount(ep_num, 0u);
  } else {
    WaitForState(ep_num, TransferState::kIdle);
  }

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 4096UL);
}

TEST_F(Dwc3EndpointsTestBase, OutEndpointRingBufferWraparound) {
  SetEnableEnqueueManyTrbs(true);
  TriggerConnection();

  const uint8_t ep_address = 0x02;  // OUT endpoint 2
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Enqueue initial test request to trigger start_transfer (resets FIFO)
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk, false);
  WaitForState(ep_num, TransferState::kStartingOngoing);

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveOngoing);

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  dut_.runtime().RunUntilIdle();

  // Manually advance write and read pointers to `last_ - 1` (index 254)
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    ASSERT_LT(uep.fifo.WriteOffset(), uep.fifo.TotalSlots());
    size_t advance_count = uep.fifo.TotalSlots() - 1 - uep.fifo.WriteOffset();
    Dwc3TestHelper::AdvanceFifo(uep.fifo, advance_count);
    ASSERT_EQ(uep.fifo.GetActiveCount(), 0u);
    EXPECT_EQ(uep.fifo.WriteOffset(), uep.fifo.TotalSlots() - 1);
    EXPECT_EQ(uep.fifo.ReadOffset(), uep.fifo.TotalSlots() - 1);
  });

  // Enqueue OUT request sitting on wraparound boundary.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  QueueRequest(1, 512, 1024, fdescriptor::EndpointType::kBulk, false);
  WaitForActiveCount(ep_num, 1);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);

    auto* initial_read = uep.fifo.current_read();
    uep.fifo.AdvanceRead();
    auto* wrapped_read = uep.fifo.current_read();

    // Verify wraparound across slot 255 Link TRB back to index 0
    EXPECT_LT(wrapped_read, initial_read);
  });
}

TEST_F(Dwc3EndpointsTestBase, EndpointStallAndClear) {
  auto last_depcmd = std::make_shared<std::atomic<uint32_t>>(0);
  auto write_called = std::make_shared<std::atomic<bool>>(false);

  auto cleanup_callbacks = DeferClearDepcmdCallbacks(2);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = fuchsia_hardware_usb_descriptor::EndpointType::kBulk;
    uep->ep.max_packet_size = 512;

    // Enable the endpoint
    Dwc3TestHelper::EpSetConfig(drv, uep->ep, true);
  });

  dut_.RunInEnvironmentTypeContext([last_depcmd, write_called](Environment& env) {
    auto& depcmd = env.reg_region()[DEPCMD::Get(2).addr()];
    depcmd.SetWriteCallback([last_depcmd, write_called](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      last_depcmd->store(val);
      write_called->store(true);
    });
  });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    EXPECT_TRUE(uep->ep.enabled);
    Dwc3TestHelper::EpSetStall(drv, uep->ep, true);
  });

  EXPECT_TRUE(write_called->load());
  EXPECT_EQ(DEPCMD::Get(2).FromValue(last_depcmd->load()).CMDTYP(), DEPCMD::DEPSSTALL);

  write_called->store(false);
  last_depcmd->store(0);
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    EXPECT_TRUE(uep->ep.enabled);
    Dwc3TestHelper::EpSetStall(drv, uep->ep, false);
  });

  EXPECT_TRUE(write_called->load());
  EXPECT_EQ(DEPCMD::Get(2).FromValue(last_depcmd->load()).CMDTYP(), DEPCMD::DEPCSTALL);
}

TEST_F(Dwc3EndpointsTestBase, EndpointConfiguration) {
  auto depcfg_called = std::make_shared<std::atomic<bool>>(false);
  auto depxfercfg_called = std::make_shared<std::atomic<bool>>(false);
  auto dalepena_called = std::make_shared<std::atomic<bool>>(false);

  auto cleanup_depcmd = DeferClearDepcmdCallbacks(2);
  auto cleanup_dalepena = fit::defer([this]() {
    dut_.RunInEnvironmentTypeContext([](Environment& env) {
      env.reg_region()[DALEPENA::Get().addr()].SetWriteCallback([](uint64_t) {});
    });
  });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = fuchsia_hardware_usb_descriptor::EndpointType::kBulk;
    uep->ep.max_packet_size = 512;
  });

  dut_.RunInEnvironmentTypeContext(
      [depcfg_called, depxfercfg_called, dalepena_called](Environment& env) {
        auto& depcmd = env.reg_region()[DEPCMD::Get(2).addr()];
        depcmd.SetWriteCallback([depcfg_called, depxfercfg_called](uint64_t val_raw) {
          uint32_t val = static_cast<uint32_t>(val_raw);
          auto cmd = DEPCMD::Get(2).FromValue(val);
          if (cmd.CMDTYP() == DEPCMD::DEPCFG) {
            depcfg_called->store(true);
          } else if (cmd.CMDTYP() == DEPCMD::DEPXFERCFG) {
            depxfercfg_called->store(true);
          }
        });

        auto& dalepena = env.reg_region()[DALEPENA::Get().addr()];
        dalepena.SetWriteCallback(
            [dalepena_called](uint64_t val_raw) { dalepena_called->store(true); });
      });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    Dwc3TestHelper::EpSetConfig(drv, uep->ep, true);
  });

  EXPECT_TRUE(depcfg_called->load());
  EXPECT_TRUE(depxfercfg_called->load());
  EXPECT_TRUE(dalepena_called->load());
}

TEST_F(Dwc3EndpointsTestBase, EndpointReset) {
  auto dalepena_called = std::make_shared<std::atomic<bool>>(false);
  auto dalepena_val = std::make_shared<std::atomic<uint32_t>>(0);

  SetUpAndPowerOnEndpoints();

  auto cleanup_callbacks = fit::defer([&]() {
    dut_.RunInEnvironmentTypeContext([](Environment& env) {
      env.reg_region()[DALEPENA::Get().addr()].SetWriteCallback([](uint64_t) {});
    });
  });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = fuchsia_hardware_usb_descriptor::EndpointType::kBulk;
    uep->ep.max_packet_size = 512;

    // Enable it first
    Dwc3TestHelper::EpSetConfig(drv, uep->ep, true);

    // Set some flags
    Dwc3TestHelper::SetGotNotReady(drv, 2, true);
    Dwc3TestHelper::SetEpRsrcId(drv, 2, 5);
    Dwc3TestHelper::SetEpTransferState(drv, 2, Dwc3TestHelper::TransferState::kActiveSingle);
  });

  dut_.RunInEnvironmentTypeContext([dalepena_called, dalepena_val](Environment& env) {
    auto& dalepena = env.reg_region()[DALEPENA::Get().addr()];
    dalepena.SetWriteCallback([dalepena_called, dalepena_val](uint64_t val_raw) {
      dalepena_called->store(true);
      dalepena_val->store(static_cast<uint32_t>(val_raw));
    });
  });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    Dwc3TestHelper::EpReset(drv, uep->ep);

    // Verify flags are reset
    EXPECT_FALSE(Dwc3TestHelper::GetGotNotReady(drv, 2));
  });

  EXPECT_TRUE(dalepena_called->load());
  EXPECT_EQ(dalepena_val->load() & (1 << 2), 0u);
}

struct EndpointTransferSweepParams {
  uint8_t ep_addr;
  fdescriptor::EndpointType ep_type;
  uint32_t max_packet_size;
};

class Dwc3EndpointTransferSweepTest
    : public TestFixture<true>,
      public testing::WithParamInterface<EndpointTransferSweepParams> {};

TEST_P(Dwc3EndpointTransferSweepTest, DisconnectDuringActiveTransfer) {
  auto params = GetParam();
  uint8_t ep_addr = params.ep_addr;
  fdescriptor::EndpointType ep_type = params.ep_type;
  uint32_t max_packet_size = params.max_packet_size;

  SetUpAndPowerOnEndpoints();

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  ASSERT_TRUE(endpoints.is_ok());

  bool completed = false;
  zx_status_t completion_status = ZX_OK;
  TestEndpointEventHandler event_handler(completed, completion_status);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, ep_addr);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = ep_type;
    uep->ep.max_packet_size = static_cast<uint16_t>(max_packet_size);
    uep->ep.enabled = true;
    uep->ep.got_not_ready = true;
    ASSERT_TRUE(Dwc3TestHelper::InitFifo(drv, ep_addr).is_ok());

    auto* dispatcher = Dwc3TestHelper::GetDispatcher(drv);
    uep->server->Connect(dispatcher, std::move(endpoints->server));
  });

  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> sync_client{
      std::move(endpoints->client)};

  auto requests = CreateVmoBuffer(sync_client, max_packet_size, max_packet_size);
  QueueRequestsAndWaitForStartTransfer(ep_addr, sync_client, std::move(requests));

  // Safe Barrier Fulfillment: Simulate missing hardware interrupt to release deferred command!
  dut_.RunInDriverContext([&](Dwc3& drv) {
    constexpr uint8_t kMockRsrcId = 5;
    Dwc3TestHelper::HandleEpTransferStartedEvent(drv, ep_addr, kMockRsrcId);
  });

  EXPECT_OK(WaitForPhy());

  // Trigger disconnect
  this->dsts_val_.store(DSTS::Get()
                            .FromValue(this->dsts_val_.load())
                            .set_USBLNKST(DSTS::USBLNKST_DISCONNECTED)
                            .reg_value());
  dut_.RunInEnvironmentTypeContext([&](Environment& env) { env.usb_phy().TriggerDisconnect(); });

  // Simulate the hardware core posting the aborted interrupt event!
  dut_.RunInDriverContext(
      [&](Dwc3& drv) { Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, ep_addr); });

  // Wait for request completion
  ASSERT_TRUE(sync_client.HandleOneEvent(event_handler).ok());
  EXPECT_TRUE(completed);
  EXPECT_TRUE(completion_status == ZX_ERR_IO_NOT_PRESENT || completion_status == ZX_ERR_CANCELED)
      << "Unexpected completion status: " << zx_status_get_string(completion_status);

  sync_client = {};
}

TEST_P(Dwc3EndpointTransferSweepTest, StallDuringActiveTransfer) {
  auto params = GetParam();
  uint8_t ep_addr = params.ep_addr;
  fdescriptor::EndpointType ep_type = params.ep_type;
  uint32_t max_packet_size = params.max_packet_size;

  SetUpAndPowerOnEndpoints();

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  ASSERT_TRUE(endpoints.is_ok());

  bool completed = false;
  zx_status_t completion_status = ZX_OK;
  TestEndpointEventHandler event_handler(completed, completion_status);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, ep_addr);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = ep_type;
    uep->ep.max_packet_size = static_cast<uint16_t>(max_packet_size);
    uep->ep.enabled = true;
    uep->ep.got_not_ready = true;
    ASSERT_TRUE(Dwc3TestHelper::InitFifo(drv, ep_addr).is_ok());

    auto* dispatcher = Dwc3TestHelper::GetDispatcher(drv);
    uep->server->Connect(dispatcher, std::move(endpoints->server));
  });

  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> sync_client{
      std::move(endpoints->client)};

  auto requests = CreateVmoBuffer(sync_client, max_packet_size, max_packet_size);
  QueueRequestsAndWaitForStartTransfer(ep_addr, sync_client, std::move(requests));

  // Stall endpoint
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, ep_addr);
    Dwc3TestHelper::EpSetStall(drv, uep->ep, true);
  });

  // Verify request is not completed
  EXPECT_FALSE(completed);

  // Clear stall
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, ep_addr);
    Dwc3TestHelper::EpSetStall(drv, uep->ep, false);
  });

  // Verify request is still not completed
  EXPECT_FALSE(completed);

  // Simulate completion event
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, ep_addr);
    dwc3_trb_t* trb = uep->fifo.current_read();
    trb->control &= ~TRB_HWO;
    Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, ep_addr);
  });

  // Wait for request completion
  ASSERT_TRUE(sync_client.HandleOneEvent(event_handler).ok());
  EXPECT_TRUE(completed);
  EXPECT_OK(completion_status);

  sync_client = {};
}

// TODO(b/509735595): Re-enable once the deferred cancel and reset logic production fixes land.
TEST_P(Dwc3EndpointTransferSweepTest, DISABLED_CancelAllDuringActiveTransfer) {
  auto params = GetParam();
  uint8_t ep_addr = params.ep_addr;
  fdescriptor::EndpointType ep_type = params.ep_type;
  uint32_t max_packet_size = params.max_packet_size;

  SetUpAndPowerOnEndpoints();

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  ASSERT_TRUE(endpoints.is_ok());

  bool completed = false;
  zx_status_t completion_status = ZX_OK;
  TestEndpointEventHandler event_handler(completed, completion_status);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, ep_addr);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = ep_type;
    uep->ep.max_packet_size = static_cast<uint16_t>(max_packet_size);
    uep->ep.enabled = true;
    uep->ep.got_not_ready = true;
    ASSERT_TRUE(Dwc3TestHelper::InitFifo(drv, ep_addr).is_ok());

    auto* dispatcher = Dwc3TestHelper::GetDispatcher(drv);
    uep->server->Connect(dispatcher, std::move(endpoints->server));
  });

  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> sync_client{
      std::move(endpoints->client)};

  auto requests = CreateVmoBuffer(sync_client, max_packet_size, max_packet_size);
  QueueRequestsAndWaitForStartTransfer(ep_addr, sync_client, std::move(requests));

  // Deliver the started event to make the endpoint active with resource ID.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    constexpr uint8_t kMockRsrcId = 5;
    Dwc3TestHelper::HandleEpTransferStartedEvent(drv, ep_addr, kMockRsrcId);
  });

  // Call CancelAll via FIDL to cancel requests during an active transfer.
  auto cancel_result = sync_client->CancelAll();
  ASSERT_TRUE(cancel_result.is_ok())
      << "CancelAll failed: " << cancel_result.error_value().FormatDescription();

  // Wait for request completion (EpServer::CancelAll completes cancelled requests with
  // ZX_ERR_CANCELED).
  ASSERT_TRUE(sync_client.HandleOneEvent(event_handler).ok());
  EXPECT_TRUE(completed);
  EXPECT_EQ(completion_status, ZX_ERR_CANCELED);

  sync_client = {};
}

// TODO(b/509735595): Re-enable once the deferred cancel and reset logic production fixes land.
TEST_P(Dwc3EndpointTransferSweepTest, DISABLED_DisableDuringActiveTransfer) {
  auto params = GetParam();
  uint8_t ep_addr = params.ep_addr;
  fdescriptor::EndpointType ep_type = params.ep_type;
  uint32_t max_packet_size = params.max_packet_size;

  SetUpAndPowerOnEndpoints();

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  ASSERT_TRUE(endpoints.is_ok());

  bool completed = false;
  zx_status_t completion_status = ZX_OK;
  TestEndpointEventHandler event_handler(completed, completion_status);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, ep_addr);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = ep_type;
    uep->ep.max_packet_size = static_cast<uint16_t>(max_packet_size);
    uep->ep.enabled = true;
    uep->ep.got_not_ready = true;
    ASSERT_TRUE(Dwc3TestHelper::InitFifo(drv, ep_addr).is_ok());

    auto* dispatcher = Dwc3TestHelper::GetDispatcher(drv);
    uep->server->Connect(dispatcher, std::move(endpoints->server));
  });

  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> sync_client{
      std::move(endpoints->client)};

  auto requests = CreateVmoBuffer(sync_client, max_packet_size, max_packet_size);
  QueueRequestsAndWaitForStartTransfer(ep_addr, sync_client, std::move(requests));

  // Deliver the started event to make the endpoint active with resource ID.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    constexpr uint8_t kMockRsrcId = 5;
    Dwc3TestHelper::HandleEpTransferStartedEvent(drv, ep_addr, kMockRsrcId);
  });

  // Verify DEPENDXFER was NOT sent
  auto dependxfer_sent = std::make_shared<std::atomic<bool>>(false);
  auto cleanup_callbacks = DeferClearDepcmdCallbacks(ep_addr);
  dut_.RunInEnvironmentTypeContext([dependxfer_sent, ep_addr](Environment& env) {
    auto& depcmd = env.reg_region()[DEPCMD::Get(ep_addr).addr()];
    depcmd.SetWriteCallback([dependxfer_sent, ep_addr](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      if (DEPCMD::Get(ep_addr).FromValue(val).CMDTYP() == DEPCMD::DEPENDXFER) {
        dependxfer_sent->store(true);
      }
    });
  });

  // Call DisableEndpoint (via UserEpReset in test helper)
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, ep_addr);
    Dwc3TestHelper::UserEpReset(drv, *uep);
  });

  // Verify DEPENDXFER was NOT sent
  EXPECT_FALSE(dependxfer_sent->load());

  // Explicitly dispatch the asynchronous request completion event from the sync client.
  // EpServer::CancelAll completes cancelled requests with ZX_ERR_IO_NOT_PRESENT upon disable.
  ASSERT_TRUE(sync_client.HandleOneEvent(event_handler).ok());
  EXPECT_TRUE(completed);
  EXPECT_EQ(completion_status, ZX_ERR_IO_NOT_PRESENT);

  sync_client = {};
}

INSTANTIATE_TEST_SUITE_P(
    Dwc3EndpointTransferSweep, Dwc3EndpointTransferSweepTest,
    testing::Values(EndpointTransferSweepParams{.ep_addr = 2,
                                                .ep_type = fdescriptor::EndpointType::kBulk,
                                                .max_packet_size = 512},  // Bulk OUT
                    EndpointTransferSweepParams{.ep_addr = 3,
                                                .ep_type = fdescriptor::EndpointType::kBulk,
                                                .max_packet_size = 512},  // Bulk IN
                    EndpointTransferSweepParams{.ep_addr = 2,
                                                .ep_type = fdescriptor::EndpointType::kInterrupt,
                                                .max_packet_size = 64},  // Interrupt OUT
                    EndpointTransferSweepParams{.ep_addr = 3,
                                                .ep_type = fdescriptor::EndpointType::kInterrupt,
                                                .max_packet_size = 64}  // Interrupt IN
                    ),
    [](const testing::TestParamInfo<Dwc3EndpointTransferSweepTest::ParamType>& info) {
      std::stringstream test_name;
      test_name << info.index << "_"
                << (info.param.ep_type == fdescriptor::EndpointType::kBulk ? "BULK" : "INTERRUPT")
                << "_" << (info.param.ep_addr % 2 == 0 ? "OUT" : "IN");
      return test_name.str();
    });

TEST_P(Dwc3EndpointsTest, ShortPacketTransfer) {
  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  ASSERT_TRUE(endpoints.is_ok());

  bool completed = false;
  zx_status_t completion_status = ZX_OK;
  size_t completed_length = 0;

  TestEndpointEventHandler event_handler(completed, completion_status, &completed_length);

  SetUpAndPowerOnEndpoints();

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = fuchsia_hardware_usb_descriptor::EndpointType::kBulk;
    uep->ep.max_packet_size = 512;
    uep->ep.enabled = true;
    uep->ep.got_not_ready = true;
    ASSERT_TRUE(Dwc3TestHelper::InitFifo(drv, 2).is_ok());

    auto* dispatcher = Dwc3TestHelper::GetDispatcher(drv);
    uep->server->Connect(dispatcher, std::move(endpoints->server));
  });

  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> sync_client{
      std::move(endpoints->client)};

  auto requests = CreateVmoBuffer(sync_client, 512, 512, 1);
  QueueRequestsAndWaitForStartTransfer(2, sync_client, std::move(requests));

  // Simulate completion event with short packet (256 bytes)
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    dwc3_trb_t* trb = uep->fifo.current_read();
    trb->control &= ~TRB_HWO;
    // Set remaining size to 256 (so 256 bytes were transferred)
    trb->status = TRB_BUFSIZ(256);
    Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, 2);
  });

  // Wait for request completion
  ASSERT_TRUE(sync_client.HandleOneEvent(event_handler).ok());
  EXPECT_TRUE(completed);
  EXPECT_OK(completion_status);
  EXPECT_EQ(completed_length, 256UL);

  sync_client = {};
}

// TODO(b/509735595): Re-enable once the deferred cancel and reset logic production fixes land.
TEST_P(Dwc3EndpointsTest, DISABLED_ZeroLengthTransfer) {
  SetUpAndPowerOnEndpoints();

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  ASSERT_TRUE(endpoints.is_ok());

  bool completed = false;
  zx_status_t completion_status = ZX_OK;
  TestEndpointEventHandler event_handler(completed, completion_status);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 3);  // Use IN endpoint 3
    ASSERT_NE(uep, nullptr);
    uep->ep.type = fuchsia_hardware_usb_descriptor::EndpointType::kBulk;
    uep->ep.max_packet_size = 512;
    uep->ep.enabled = true;
    uep->ep.got_not_ready = true;
    ASSERT_TRUE(Dwc3TestHelper::InitFifo(drv, 3).is_ok());  // Use IN endpoint 3

    auto* dispatcher = Dwc3TestHelper::GetDispatcher(drv);
    uep->server->Connect(dispatcher, std::move(endpoints->server));
  });

  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> sync_client{
      std::move(endpoints->client)};

  auto requests = CreateVmoBuffer(sync_client, 512, 0, 1);
  QueueRequestsAndWaitForStartTransfer(3, sync_client, std::move(requests));

  // Verify TRB length is 0
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 3);  // Use IN endpoint 3
    dwc3_trb_t* trb = uep->fifo.current_read();
    EXPECT_EQ(TRB_BUFSIZ(trb->status), 0UL);
  });

  // Simulate completion event
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 3);  // Use IN endpoint 3
    dwc3_trb_t* trb = uep->fifo.current_read();
    trb->control &= ~TRB_HWO;
    Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, 3);
  });

  // Wait for request completion
  ASSERT_TRUE(sync_client.HandleOneEvent(event_handler).ok());
  EXPECT_TRUE(completed);
  EXPECT_OK(completion_status);

  sync_client = {};
}

// TODO(b/509735595): Re-enable once the deferred cancel and reset logic production fixes land.
TEST_P(Dwc3EndpointsTest, DISABLED_VerifySiliconBufferingDuringHandshakeReset) {
  SetUpAndPowerOnEndpoints();

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  ASSERT_TRUE(endpoints.is_ok());

  bool completed = false;
  zx_status_t completion_status = ZX_OK;
  TestEndpointEventHandler event_handler(completed, completion_status);

  // 1. Initialize endpoint server, but keep it disabled!
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = fuchsia_hardware_usb_descriptor::EndpointType::kBulk;
    uep->ep.max_packet_size = 512;

    // Explicitly disabled!
    uep->ep.enabled = false;
    uep->ep.got_not_ready = true;

    ASSERT_TRUE(Dwc3TestHelper::InitFifo(drv, 2).is_ok());

    auto* dispatcher = Dwc3TestHelper::GetDispatcher(drv);
    uep->server->Connect(dispatcher, std::move(endpoints->server));
  });

  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> sync_client{
      std::move(endpoints->client)};

  auto requests = CreateVmoBuffer(sync_client, 512, 512, 1);

  // 2. Queue request while endpoint is disabled!
  auto result = sync_client->QueueRequests({std::move(requests)});
  ASSERT_TRUE(result.is_ok()) << "QueueRequests failed: " << result.error_value().status_string();

  dut_.runtime().RunUntilIdle();

  // 3. VERIFY SILICON BUFFERING:
  // - Assert that the request has not completed (completed is false)
  // - Assert that Dwc3 has safely buffered the request in uep->server->queued_reqs!
  EXPECT_FALSE(completed);

  dut_.runtime().RunUntil([&]() {
    size_t size = 0;
    dut_.RunInDriverContext([&](Dwc3& drv) { size = Dwc3TestHelper::GetQueuedReqsSize(drv, 2); });
    return size == 1;
  });

  size_t queued_size = 0;
  dut_.RunInDriverContext(
      [&](Dwc3& drv) { queued_size = Dwc3TestHelper::GetQueuedReqsSize(drv, 2); });
  EXPECT_EQ(queued_size, 1UL) << "Request was not buffered in queued_reqs!";

  // 4. Simulate Host enabling the endpoint (e.g. SET_CONFIGURATION complete)
  // - Enable it in the driver and call UserEpQueueNext()
  auto completion = std::make_shared<libsync::Completion>();
  auto cleanup_callbacks = DeferClearDepcmdCallbacks(2);

  dut_.RunInEnvironmentTypeContext([completion](Environment& env) {
    auto& depcmd = env.reg_region()[DEPCMD::Get(2).addr()];
    depcmd.SetWriteCallback([completion](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      if (DEPCMD::Get(2).FromValue(val).CMDTYP() ==
          DEPCMD::DEPSTRTXFER) {  // DEPSTRTXFER (DMA starts)
        completion->Signal();
      }
    });
    depcmd.SetReadCallback([]() -> uint32_t { return 0; });
  });

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    // Configure and enable! This natively and automatically drains the software queue!
    Dwc3TestHelper::EpSetConfig(drv, uep->ep, true);
  });

  // 5. VERIFY RESUMPTION:
  // - Assert that DMA successfully starts on the buffered request!
  dut_.runtime().RunUntil([&]() { return completion->signaled(); });
  ASSERT_TRUE(completion->signaled());

  // Simulate completion event
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 2);
    dwc3_trb_t* trb = uep->fifo.current_read();
    trb->control &= ~TRB_HWO;
    Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, 2);
  });

  // Wait for completion event to bubble up to client
  ASSERT_TRUE(sync_client.HandleOneEvent(event_handler).ok());
  EXPECT_TRUE(completed);
  EXPECT_OK(completion_status);

  sync_client = {};
}

// Verifies that resetting an endpoint with an active cancel in progress completes outstanding
// requests without leaking FIFO or resource tracking when a trailing completion event arrives.
// TODO(b/509735595): Re-enable once the deferred cancel and reset logic production fixes land.
TEST_P(Dwc3EndpointsTest, DISABLED_DeferredCancelDisableAccountingLeak) {
  SetUpAndPowerOnEndpoints();

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_usb_endpoint::Endpoint>();
  ASSERT_TRUE(endpoints.is_ok());

  bool completed = false;
  zx_status_t completion_status = ZX_OK;
  TestEndpointEventHandler event_handler(completed, completion_status);

  // 1. Initialize and configure a mock Bulk Endpoint 7
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 7);
    ASSERT_NE(uep, nullptr);
    uep->ep.type = fuchsia_hardware_usb_descriptor::EndpointType::kBulk;
    uep->ep.max_packet_size = 512;
    uep->ep.enabled = true;
    uep->ep.got_not_ready = true;
    ASSERT_TRUE(Dwc3TestHelper::InitFifo(drv, 7).is_ok());

    auto* dispatcher = Dwc3TestHelper::GetDispatcher(drv);
    uep->server->Connect(dispatcher, std::move(endpoints->server));
  });

  fidl::SyncClient<fuchsia_hardware_usb_endpoint::Endpoint> sync_client{
      std::move(endpoints->client)};

  auto requests = CreateVmoBuffer(sync_client, 512, 512, 1);
  QueueRequestsAndWaitForStartTransfer(7, sync_client, std::move(requests));

  // 2. Force an EndTransfer operation with a simulated busy hardware return to mark
  // ep->pending_cancel = true. This is done by calling CancelAll before
  // HandleEpTransferStartedEvent, which leaves ep.rsrc_id at kInvalidResourceId.
  auto cancel_result = sync_client->CancelAll();
  ASSERT_TRUE(cancel_result.is_ok())
      << "CancelAll failed: " << cancel_result.error_value().FormatDescription();

  // 3. Forcefully execute the endpoint clear/disable track (UserEpReset)
  // which completes the request with ZX_ERR_IO_NOT_PRESENT and wipes the tracking pointer.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto* uep = Dwc3TestHelper::GetUserEndpoint(drv, 7);
    Dwc3TestHelper::UserEpReset(drv, *uep);
  });

  // Flush completion events to event handler so we are 100% in sync
  ASSERT_TRUE(sync_client.HandleOneEvent(event_handler).ok());
  EXPECT_TRUE(completed);
  EXPECT_EQ(completion_status, ZX_ERR_CANCELED);

  // 4. Fire a mock trailing edge hardware event interrupt (DEPEVT_XFER_COMPLETE) against
  // Endpoint 7.
  dut_.RunInDriverContext(
      [&](Dwc3& drv) { Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, 7); });

  sync_client = {};
}

TEST_P(Dwc3EndpointsTest, CancelAllRequestsOnUnbound) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  const bool enqueue_many = GetParam();
  const TransferState starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  const TransferState active_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  WaitForActiveCount(ep_num, 1);
  WaitForState(ep_num, starting_state);

  // Trigger started event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, active_state);

  // Cancel all requests via client asynchronously.
  auto cancel_replied = std::make_shared<std::atomic<bool>>(false);
  ep_client_->CancelAll().Then([cancel_replied](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
    cancel_replied->store(true);
  });

  WaitForState(ep_num, TransferState::kCanceling);
  EXPECT_FALSE(cancel_replied->load());

  // Verify that cancel_completers has 1 pending completer in the server.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    ASSERT_TRUE(uep.server.has_value());
    EXPECT_EQ(uep.server->cancel_completers.size(), 1u);
  });

  // Close the client endpoint channel to unbind the endpoint server.
  ep_client_ = {};

  // Wait for the driver dispatcher to process OnUnbound and flush completers.
  dut_.runtime().RunUntil([&]() {
    return dut_.RunInDriverContext<bool>([&](Dwc3& drv) {
      auto& uep = GetUserEndpoint(drv, ep_num);
      return uep.server.has_value() && uep.server->cancel_completers.empty();
    });
  });

  // Complete the hardware transfer.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferEnded(drv, ep_num); });
  WaitForState(ep_num, TransferState::kIdle);
}

TEST_P(Dwc3EndpointsTest, CancelAllTransferEndedBeforeUnbound) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  const bool enqueue_many = GetParam();
  const TransferState starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  const TransferState active_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  WaitForActiveCount(ep_num, 1);
  WaitForState(ep_num, starting_state);

  // Trigger started event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, active_state);

  // Cancel all requests via client asynchronously.
  auto cancel_replied = std::make_shared<std::atomic<bool>>(false);
  ep_client_->CancelAll().Then([cancel_replied](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
    cancel_replied->store(true);
  });

  WaitForState(ep_num, TransferState::kCanceling);
  EXPECT_FALSE(cancel_replied->load());

  // Verify that cancel_completers has 1 pending completer in the server.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    ASSERT_TRUE(uep.server.has_value());
    EXPECT_EQ(uep.server->cancel_completers.size(), 1u);
  });

  // Close the client endpoint channel to initiate unbind.
  ep_client_ = {};

  // Fire HandleEpTransferEndedEvent BEFORE driver dispatcher processes OnUnbound.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferEnded(drv, ep_num); });

  // Now process queued events on driver dispatcher including OnUnbound.
  dut_.runtime().RunUntilIdle();

  // Verify state is cleanly Idle and completers vector is empty.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    ASSERT_TRUE(uep.server.has_value());
    EXPECT_EQ(uep.server->cancel_completers.size(), 0u);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kIdle);
  });
}

// Tests that completing a single-transfer request via TransferComplete correctly
// returns the endpoint state machine to kIdle and starts the next queued request.
TEST_P(Dwc3EndpointsTest, SingleTransfer_SequentialCompletionAndNextStart) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  QueueRequest(1, 0, 64, fdescriptor::EndpointType::kInterrupt);
  QueueRequest(1, 64, 64, fdescriptor::EndpointType::kInterrupt);

  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);

  // Complete request 1 via TransferComplete.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });

  // Verify that request 2 is dequeued and begins starting.
  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
    EXPECT_EQ(uep.server->active_reqs.size(), 1u);
    EXPECT_EQ(uep.server->queued_reqs.size(), 0u);
  });

  // Start and complete request 2 via TransferComplete.
  dut_.RunInDriverContext(
      [&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId + 1); });
  WaitForState(ep_num, TransferState::kActiveSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });

  WaitForState(ep_num, TransferState::kIdle);
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(2);
  EXPECT_EQ(completions.size(), 2u);
}

// Tests that when the active queue drains to 0 via TransferComplete, the endpoint
// returns to kIdle and subsequent calls to QueueRequests start transfers cleanly.
TEST_P(Dwc3EndpointsTest, SingleTransfer_ActiveQueueDrainThenRequeue) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  QueueRequest(1, 0, 64, fdescriptor::EndpointType::kInterrupt);
  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);

  // Complete request 1 via TransferComplete so active count drains to 0.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });

  // State should return to kIdle once active requests drain.
  WaitForState(ep_num, TransferState::kIdle);
  std::vector<CompletionResult> completions1 = event_handler_.WaitForCompletions(1);
  EXPECT_EQ(completions1.size(), 1u);

  // Later, client queues Request 2.
  QueueRequest(1, 64, 64, fdescriptor::EndpointType::kInterrupt);
  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext(
      [&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId + 1); });
  WaitForState(ep_num, TransferState::kActiveSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });

  WaitForState(ep_num, TransferState::kIdle);
  std::vector<CompletionResult> completions2 = event_handler_.WaitForCompletions(1);
  EXPECT_EQ(completions2.size(), 1u);
}

// Tests that a 64-byte transfer on a 64-byte max-packet Interrupt IN endpoint with short_bit set
// correctly enqueues and completes 2 TRBs (data TRB via TransferInProgress + ZLP TRB via
// TransferComplete).
TEST_P(Dwc3EndpointsTest, InterruptIn_ZlpTwoTrbCompletion) {
  TriggerConnection();

  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a 64-byte IN request with short_bit = true.
  QueueRequest(1, 0, 64, fdescriptor::EndpointType::kInterrupt, /*short_bit=*/true);
  WaitForState(ep_num, TransferState::kStartingSingle);

  // Verify that 2 TRBs (data TRB + ZLP TRB) were queued in the FIFO.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 2u);
    ASSERT_EQ(uep.server->active_reqs.size(), 1u);
    EXPECT_EQ(uep.server->active_reqs.front().total_trbs, 2u);
  });

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);

  // Complete intermediate data TRB via TransferInProgress.
  // Request should not complete yet and state remains kActiveSingle because ZLP TRB is pending.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  dut_.runtime().RunUntilIdle();
  EXPECT_EQ(event_handler_.completion_count(), 0u);
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kActiveSingle);
    ASSERT_EQ(uep.server->active_reqs.size(), 1u);
    EXPECT_EQ(uep.server->active_reqs.front().completed_trbs, 1u);
    EXPECT_EQ(uep.server->active_reqs.front().completed_bytes, 64u);
  });

  // Complete terminal ZLP TRB via TransferComplete.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
  WaitForState(ep_num, TransferState::kIdle);

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1u);
  EXPECT_EQ(completions[0].status, ZX_OK);
  EXPECT_EQ(completions[0].transfer_size, 64u);
}

// Tests rapid sequential queuing of small notification packets on Interrupt IN (CDC notification
// style).
TEST_P(Dwc3EndpointsTest, InterruptIn_RapidNotificationBurst) {
  TriggerConnection();

  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  constexpr size_t kBurstCount = 6;
  for (size_t i = 0; i < kBurstCount; i++) {
    QueueRequest(1, i * 8, 8, fdescriptor::EndpointType::kInterrupt);
  }

  for (size_t i = 0; i < kBurstCount; i++) {
    WaitForState(ep_num, TransferState::kStartingSingle);
    dut_.RunInDriverContext([&](Dwc3& drv) {
      TriggerEpTransferStarted(drv, ep_num, kResourceId + static_cast<uint32_t>(i));
    });
    WaitForState(ep_num, TransferState::kActiveSingle);
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
  }

  WaitForState(ep_num, TransferState::kIdle);
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(kBurstCount);
  EXPECT_EQ(completions.size(), kBurstCount);
}

// Tests CancelAll while an Interrupt transfer is active, verifying that queued requests
// cancel immediately, active request aborts via DEPENDXFER, and state returns to kIdle.
TEST_P(Dwc3EndpointsTest, Interrupt_CancelAllWhileTransferActive) {
  TriggerConnection();

  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  QueueRequest(1, 0, 16, fdescriptor::EndpointType::kInterrupt);
  QueueRequest(1, 16, 16, fdescriptor::EndpointType::kInterrupt);
  QueueRequest(1, 32, 16, fdescriptor::EndpointType::kInterrupt);

  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);

  auto cancel_replied = std::make_shared<std::atomic<bool>>(false);
  ep_client_->CancelAll().Then([cancel_replied](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
    cancel_replied->store(true);
  });

  // Unstarted requests 2 & 3 should be completed immediately with IO_NOT_PRESENT.
  std::vector<CompletionResult> completions1 = event_handler_.WaitForCompletions(2);
  EXPECT_EQ(completions1.size(), 2u);
  EXPECT_FALSE(cancel_replied->load());

  WaitForState(ep_num, TransferState::kCanceling);

  // Trigger TransferEnded event for active Request 1.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferEnded(drv, ep_num); });

  dut_.runtime().RunUntil([&]() { return cancel_replied->load(); });
  EXPECT_TRUE(cancel_replied->load());
  std::vector<CompletionResult> completions2 = event_handler_.WaitForCompletions(1);
  EXPECT_EQ(completions2.size(), 1u);
  WaitForState(ep_num, TransferState::kIdle);
}

// Tests CancelAll while an Interrupt transfer is in kStartingSingle (before TransferStarted).
TEST_P(Dwc3EndpointsTest, Interrupt_CancelAllWhileTransferStarting) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  QueueRequest(1, 0, 64, fdescriptor::EndpointType::kInterrupt);
  QueueRequest(1, 64, 64, fdescriptor::EndpointType::kInterrupt);

  WaitForState(ep_num, TransferState::kStartingSingle);

  auto cancel_replied = std::make_shared<std::atomic<bool>>(false);
  ep_client_->CancelAll().Then([cancel_replied](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
    cancel_replied->store(true);
  });

  std::vector<CompletionResult> completions1 = event_handler_.WaitForCompletions(1);
  EXPECT_EQ(completions1.size(), 1u);
  WaitForState(ep_num, TransferState::kPendingCancel);
  EXPECT_FALSE(cancel_replied->load());

  // When TransferStarted arrives, it should transition to kCanceling and issue EndTransfer.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kCanceling);

  // TransferEnded completes the cancellation.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferEnded(drv, ep_num); });
  dut_.runtime().RunUntil([&]() { return cancel_replied->load(); });
  EXPECT_TRUE(cancel_replied->load());
  std::vector<CompletionResult> completions2 = event_handler_.WaitForCompletions(1);
  EXPECT_EQ(completions2.size(), 1u);
  WaitForState(ep_num, TransferState::kIdle);
}

// Tests setting stall on an Interrupt endpoint, verifying that queued requests stall,
// and clearing stall cleanly resumes execution.
TEST_P(Dwc3EndpointsTest, Interrupt_StallAndClearHaltResumesQueue) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Request 1 executes and completes.
  QueueRequest(1, 0, 64, fdescriptor::EndpointType::kInterrupt);
  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
  WaitForState(ep_num, TransferState::kIdle);
  std::vector<CompletionResult> completions1 = event_handler_.WaitForCompletions(1);
  EXPECT_EQ(completions1.size(), 1u);

  // Stall endpoint via DCI.
  fidl::WireResult stall_res = dci_->EndpointSetStall(ep_address);
  ASSERT_OK(stall_res.status());
  ASSERT_TRUE(stall_res.value().is_ok());

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_TRUE(uep.ep.stalled);
  });

  // Queue Request 2 while stalled; it should remain in queued_reqs.
  QueueRequest(1, 64, 64, fdescriptor::EndpointType::kInterrupt);
  dut_.runtime().RunUntil([&]() {
    return dut_.RunInDriverContext<bool>(
        [&](Dwc3& drv) { return GetUserEndpoint(drv, ep_num).server->queued_reqs.size() == 1u; });
  });
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kIdle);
  });

  // Clear stall; Request 2 should immediately start.
  fidl::WireResult clear_res = dci_->EndpointClearStall(ep_address);
  ASSERT_OK(clear_res.status());
  ASSERT_TRUE(clear_res.value().is_ok());

  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext(
      [&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId + 1); });
  WaitForState(ep_num, TransferState::kActiveSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });

  WaitForState(ep_num, TransferState::kIdle);
  std::vector<CompletionResult> completions2 = event_handler_.WaitForCompletions(1);
  EXPECT_EQ(completions2.size(), 1u);
}

// Tests handling repeated TransferNotReady events during host polling without dropping requests.
TEST_P(Dwc3EndpointsTest, Interrupt_TransferNotReadyLoop) {
  TriggerConnection();

  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host polls while endpoint is empty (fires NotReady multiple times).
  for (int i = 0; i < 3; i++) {
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  }

  // Queue Request 1 and complete it.
  QueueRequest(1, 0, 16, fdescriptor::EndpointType::kInterrupt);
  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
  WaitForState(ep_num, TransferState::kIdle);

  // Host polls again.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });

  // Queue Request 2 and complete it.
  QueueRequest(1, 16, 16, fdescriptor::EndpointType::kInterrupt);
  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext(
      [&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId + 1); });
  WaitForState(ep_num, TransferState::kActiveSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
  WaitForState(ep_num, TransferState::kIdle);

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(2);
  EXPECT_EQ(completions.size(), 2u);
}

// Tests short packet reception on Interrupt OUT endpoint.
TEST_P(Dwc3EndpointsTest, InterruptOut_ShortPacketHandling) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  QueueRequest(1, 0, 64, fdescriptor::EndpointType::kInterrupt);
  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);

  // Mock hardware receiving a short packet: 16 bytes transferred out of 64 (residual 48 bytes).
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num, 48); });

  WaitForState(ep_num, TransferState::kIdle);
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1u);
  EXPECT_EQ(completions[0].status, ZX_OK);
  EXPECT_EQ(completions[0].transfer_size, 16u);
}

// Tests disabling an active Interrupt endpoint (simulating alternate setting switch).
TEST_P(Dwc3EndpointsTest, Interrupt_ReconfigureAltSettingTeardown) {
  TriggerConnection();

  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kInterrupt, 64);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  QueueRequest(1, 0, 16, fdescriptor::EndpointType::kInterrupt);
  WaitForState(ep_num, TransferState::kStartingSingle);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveSingle);

  // Disable endpoint (simulating alternate setting switch).
  fidl::WireResult disable_res = dci_->DisableEndpoint(ep_address);
  ASSERT_OK(disable_res.status());
  ASSERT_TRUE(disable_res.value().is_ok());

  WaitForState(ep_num, TransferState::kCanceling);

  // Trigger TransferEnded event to simulate hardware completion of EndTransfer.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferEnded(drv, ep_num); });

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1u);
  EXPECT_EQ(completions[0].status, ZX_ERR_IO_NOT_PRESENT);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_FALSE(uep.ep.enabled);
  });
}

// Verifies that calling CancelAll on a stalled/halted endpoint drains in-flight requests with
// ZX_ERR_CANCELED once the hardware EndTransfer sequence completes.
TEST_P(Dwc3EndpointsTest, CancelAllStrandedWhenHalted) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);

  auto expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  // Trigger started event to move to active.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  auto expected_state = enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  // Call SetStall(true) on the endpoint.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    Dwc3TestHelper::EpSetStall(drv, uep.ep, true);
  });

  // Initiate CancelAll
  auto cancel_result =
      std::make_shared<std::optional<fidl::Result<fendpoint::Endpoint::CancelAll>>>();
  auto cancel_completed = std::make_shared<libsync::Completion>();
  ep_client_->CancelAll().Then(
      [cancel_result, cancel_completed](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
        *cancel_result = std::move(res);
        cancel_completed->Signal();
      });

  WaitForState(ep_num, TransferState::kCanceling);
  EXPECT_FALSE(cancel_completed->signaled());

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferEnded(drv, ep_num); });

  WaitForState(ep_num, TransferState::kIdle);
  dut_.runtime().RunUntil([&]() { return cancel_completed->signaled(); });
  ASSERT_TRUE(cancel_completed->signaled());
  ASSERT_TRUE(cancel_result->has_value());
  ASSERT_TRUE((*cancel_result)->is_ok());

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_CANCELED);
}

// Verifies that calling CancelAll on a stalled endpoint while in idle state immediately drains
// queued requests with ZX_ERR_CANCELED without waiting for hardware EndTransfer.
TEST_F(Dwc3EndpointsTestBase, CancelAllStalledWhileIdle) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Stall the endpoint before any transfer starts.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    Dwc3TestHelper::EpSetStall(drv, uep.ep, true);
  });

  // Queue a request.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  WaitForQueuedCount(ep_num, 1);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.server->queued_reqs.size(), 1u);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kIdle);
  });

  // CancelAll should complete immediately using natural async bindings.
  auto cancel_result =
      std::make_shared<std::optional<fidl::Result<fendpoint::Endpoint::CancelAll>>>();
  auto cancel_completed = std::make_shared<libsync::Completion>();
  ep_client_->CancelAll().Then(
      [cancel_result, cancel_completed](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
        *cancel_result = std::move(res);
        cancel_completed->Signal();
      });

  dut_.runtime().RunUntil([&]() { return cancel_completed->signaled(); });
  ASSERT_TRUE(cancel_completed->signaled());
  ASSERT_TRUE(cancel_result->has_value());
  ASSERT_TRUE((*cancel_result)->is_ok());

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_CANCELED);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_TRUE(uep.server->queued_reqs.empty());
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kIdle);
  });
}

// Verifies that requests queued while an endpoint is stalled remain in queued_reqs and resume
// execution automatically once the stall is cleared.
TEST_P(Dwc3EndpointsTest, QueueRequestsWhileStalledResumeOnClearStall) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Stall the endpoint.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    Dwc3TestHelper::EpSetStall(drv, uep.ep, true);
  });

  // Queue request while stalled.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  WaitForQueuedCount(ep_num, 1);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.server->queued_reqs.size(), 1u);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kIdle);
  });

  // Clear stall via ClearStall request.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    Dwc3TestHelper::EpSetStall(drv, uep.ep, false);
    Dwc3TestHelper::UserEpQueueNext(drv, uep);
  });

  auto expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  auto expected_state = enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    if (enqueue_many) {
      TriggerEpTransferInProgress(drv, ep_num);
    } else {
      TriggerEpTransferComplete(drv, ep_num);
    }
  });

  if (enqueue_many) {
    WaitForActiveCount(ep_num, 0u);
  } else {
    WaitForState(ep_num, TransferState::kIdle);
  }

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 512UL);
}

// Verifies that when queuing requests with unregistered VMO IDs alongside valid requests,
// requests with unregistered VMO IDs complete with ZX_ERR_NOT_FOUND without disrupting valid
// requests.
// DISABLED: Requires invalid VMO ID validation and error handling in the driver (addressed in
// subsequent driver CL).
TEST_P(Dwc3EndpointsTest, DISABLED_UnregisteredVmoTransferCompletesWithNotFound) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  // Register a valid VMO ID 2.
  RegisterVmo(2, 4096);

  // Note: we purposely do NOT call RegisterVmo for VMO ID 1,
  // so any request with vmo_id=1 will fail `get_iter` since the VMO ID is unregistered.

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request with an unregistered VMO ID (e.g. 1).
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);

  // Queue a request with a valid VMO ID (2).
  QueueRequest(2, 0, 512, fdescriptor::EndpointType::kBulk);

  // The driver will process these in UserEpQueueNextSingle or UserEpQueueNextOngoing
  // depending on `enqueue_many`. The first should fail `get_iter()` and complete with an
  // error, while the second should succeed.

  // Wait for the valid endpoint transfer to complete.
  if (enqueue_many) {
    WaitForState(ep_num, TransferState::kStartingOngoing);
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
    WaitForState(ep_num, TransferState::kActiveOngoing);
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
    WaitForActiveCount(ep_num, 0u);
  } else {
    // For single transfer, wait for it to be active and then complete it.
    WaitForState(ep_num, TransferState::kStartingSingle);
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
    WaitForState(ep_num, TransferState::kActiveSingle);
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferComplete(drv, ep_num); });
    WaitForState(ep_num, TransferState::kIdle);
  }

  // We should receive 2 completions: one ZX_ERR_NOT_FOUND, one ZX_OK.
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(2);
  ASSERT_EQ(completions.size(), 2UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_NOT_FOUND);
  EXPECT_EQ(completions[0].transfer_size, 0UL);
  EXPECT_OK(completions[1].status);
  EXPECT_EQ(completions[1].transfer_size, 512UL);

  // Verify that the internal software queues correctly popped both requests, leaving a clean
  // state.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_TRUE(uep.server->queued_reqs.empty());
    EXPECT_TRUE(uep.server->active_reqs.empty());
  });

  auto cancel_result =
      std::make_shared<std::optional<fidl::Result<fendpoint::Endpoint::CancelAll>>>();
  auto cancel_completed = std::make_shared<libsync::Completion>();
  ep_client_->CancelAll().Then(
      [cancel_result, cancel_completed](fidl::Result<fendpoint::Endpoint::CancelAll>& res) {
        *cancel_result = std::move(res);
        cancel_completed->Signal();
      });

  if (enqueue_many) {
    WaitForState(ep_num, TransferState::kCanceling);
    dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferEnded(drv, ep_num); });
    WaitForState(ep_num, TransferState::kIdle);
  }

  dut_.runtime().RunUntil([&]() { return cancel_completed->signaled(); });
  ASSERT_TRUE(cancel_completed->signaled());
  ASSERT_TRUE(cancel_result->has_value());
  ASSERT_TRUE((*cancel_result)->is_ok());

  // There should be no stranded completions returned by CancelAll.
  EXPECT_EQ(event_handler_.completion_count(), 0UL);
}

// Verifies that queuing a request with an unregistered VMO ID immediately completes with
// ZX_ERR_NOT_FOUND and does not trigger a hardware kick or leave the endpoint in a non-idle state.
// DISABLED: Requires invalid VMO ID validation and error handling in the driver (addressed in
// subsequent driver CL).
TEST_F(Dwc3EndpointsTestBase, DISABLED_UnregisteredVmoIdImmediatelyCompletesNotFound) {
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);
  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request with an unregistered VMO ID (e.g. 1).
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  dut_.runtime().RunUntilIdle();

  // We should receive 1 completion: ZX_ERR_NOT_FOUND.
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_NOT_FOUND);

  // The transfer state should remain kIdle since enqueued was 0, so no hardware kick.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kIdle);
  });
}

// Verifies that a zero-length transfer on an IN endpoint correctly generates a zero-length TRB
// and completes with ZX_OK and 0 bytes transferred.
// DISABLED: Requires driver support for 0-byte IN TRB generation.
TEST_P(Dwc3EndpointsTest, DISABLED_ZeroLengthInTransfer) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x82;  // IN endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a 0-length request
  QueueRequest(1, 0, 0, fdescriptor::EndpointType::kBulk);

  TransferState expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  // Verify TRB length is 0
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    dwc3_trb_t* trb = uep.fifo.current_read();
    EXPECT_EQ(TRB_BUFSIZ(trb->status), 0UL);
  });

  // Trigger started event to initialize rsrc_id.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  TransferState expected_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  // Complete TRB
  dut_.RunInDriverContext([&](Dwc3& drv) {
    if (enqueue_many) {
      TriggerEpTransferInProgress(drv, ep_num);
    } else {
      TriggerEpTransferComplete(drv, ep_num);
    }
  });

  if (enqueue_many) {
    WaitForActiveCount(ep_num, 0u);
  } else {
    WaitForState(ep_num, TransferState::kIdle);
  }

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 0UL);
}

// Verifies that a request with no data buffer regions specified is treated as a valid zero-length
// transfer on an IN endpoint and completes with ZX_OK and 0 bytes transferred.
// DISABLED: Requires driver support for 0-byte IN TRB generation with empty data fields.
TEST_P(Dwc3EndpointsTest, DISABLED_ZeroLengthTransferNoDataField) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x82;  // IN endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request with NO data field
  frequest::Request req;
  req.defer_completion(false);
  req.information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo{}));
  req.short_(false);

  std::vector<frequest::Request> reqs;
  reqs.reserve(1);
  reqs.push_back(std::move(req));
  // Note: QueueRequests is a one-way FIDL method returning fit::result<fidl::OneWayError>
  // synchronously.
  fit::result result = ep_client_->QueueRequests({std::move(reqs)});
  ASSERT_TRUE(result.is_ok()) << "QueueRequests failed: "
                              << result.error_value().FormatDescription();

  TransferState expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  // Verify TRB length is 0
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    dwc3_trb_t* trb = uep.fifo.current_read();
    EXPECT_EQ(TRB_BUFSIZ(trb->status), 0UL);
  });

  // Trigger started event to initialize rsrc_id.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  TransferState expected_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  // Complete TRB
  dut_.RunInDriverContext([&](Dwc3& drv) {
    if (enqueue_many) {
      TriggerEpTransferInProgress(drv, ep_num);
    } else {
      TriggerEpTransferComplete(drv, ep_num);
    }
  });

  if (enqueue_many) {
    WaitForActiveCount(ep_num, 0u);
  } else {
    WaitForState(ep_num, TransferState::kIdle);
  }

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 0UL);
}

// Verifies that a buffer region with an omitted size field is rejected with ZX_ERR_INVALID_ARGS.
// DISABLED: Requires buffer region size validation in the driver.
TEST_F(Dwc3EndpointsTestBase, DISABLED_ImplicitBufferRegionSize) {
  TriggerConnection();

  const uint8_t ep_address = 0x82;  // IN endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request with explicit VMO ID but omitted region size
  frequest::BufferRegion region;
  region.buffer(frequest::Buffer::WithVmoId(1));
  region.offset(0);

  std::vector<frequest::BufferRegion> regions;
  regions.reserve(1);
  regions.push_back(std::move(region));

  frequest::Request req;
  req.data(std::move(regions));
  req.defer_completion(false);
  req.information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo{}));
  req.short_(false);

  std::vector<frequest::Request> reqs;
  reqs.reserve(1);
  reqs.push_back(std::move(req));
  // Note: QueueRequests is a one-way FIDL method returning fit::result<fidl::OneWayError>
  // synchronously.
  fit::result result = ep_client_->QueueRequests({std::move(reqs)});
  ASSERT_TRUE(result.is_ok()) << "QueueRequests failed: "
                              << result.error_value().FormatDescription();
  dut_.runtime().RunUntilIdle();

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_INVALID_ARGS);
}

// Verifies that multi-region requests combining zero-length and non-zero-length buffer regions
// are rejected with ZX_ERR_NOT_SUPPORTED.
TEST_F(Dwc3EndpointsTestBase, MixedMultiRegionTransfer) {
  TriggerConnection();

  const uint8_t ep_address = 0x82;  // IN endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);
  RegisterVmo(2, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request with [0B, 512B]
  frequest::BufferRegion region1;
  region1.buffer(frequest::Buffer::WithVmoId(1));
  region1.offset(0);
  region1.size(0);

  frequest::BufferRegion region2;
  region2.buffer(frequest::Buffer::WithVmoId(2));
  region2.offset(0);
  region2.size(512);

  std::vector<frequest::BufferRegion> regions;
  regions.reserve(2);
  regions.push_back(std::move(region1));
  regions.push_back(std::move(region2));

  frequest::Request req;
  req.data(std::move(regions));
  req.defer_completion(false);
  req.information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo{}));
  req.short_(false);

  std::vector<frequest::Request> reqs;
  reqs.reserve(1);
  reqs.push_back(std::move(req));
  // Note: QueueRequests is a one-way FIDL method returning fit::result<fidl::OneWayError>
  // synchronously.
  fit::result result = ep_client_->QueueRequests({std::move(reqs)});
  ASSERT_TRUE(result.is_ok()) << "QueueRequests failed: "
                              << result.error_value().FormatDescription();
  dut_.runtime().RunUntilIdle();

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_NOT_SUPPORTED);
}

// Verifies that OUT transfers with sizes that are not multiples of the endpoint's maximum
// packet size are rejected with ZX_ERR_INVALID_ARGS to prevent hardware short packet errors.
TEST_F(Dwc3EndpointsTestBase, InvalidOutTransferSize) {
  TriggerConnection();
  const uint8_t ep_address = 0x02;  // OUT endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);
  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  RegisterVmo(1, 4096);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue 513 bytes (not multiple of 512 max packet size)
  QueueRequest(1, 0, 513, fdescriptor::EndpointType::kBulk);
  dut_.runtime().RunUntilIdle();

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_INVALID_ARGS);
}

// Verifies that transfers requesting offsets and sizes extending beyond the registered VMO's
// bounds are rejected with ZX_ERR_OUT_OF_RANGE.
// DISABLED: Requires registered VMO bounds checking in the driver.
TEST_F(Dwc3EndpointsTestBase, DISABLED_OutOfRangeVmoTransfer) {
  TriggerConnection();
  const uint8_t ep_address = 0x82;  // IN endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);
  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  RegisterVmo(1, 4096);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue request with size exceeding remaining VMO space
  // Offset 4000, size 512 (4000 + 512 = 4512 > 4096)
  QueueRequest(1, 4000, 512, fdescriptor::EndpointType::kBulk);
  dut_.runtime().RunUntilIdle();

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_OUT_OF_RANGE);
}

// Verifies that scatter-gather requests across multiple non-zero buffer regions return
// ZX_ERR_NOT_SUPPORTED since multi-region descriptor chaining is currently unsupported.
TEST_F(Dwc3EndpointsTestBase, ScatterGatherNotImplemented) {
  TriggerConnection();
  const uint8_t ep_address = 0x82;  // IN endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);
  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  RegisterVmo(1, 4096);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue request with multiple non-zero-length regions.
  frequest::BufferRegion region1;
  region1.buffer(frequest::Buffer::WithVmoId(1));
  region1.offset(0);
  region1.size(512);

  frequest::BufferRegion region2;
  region2.buffer(frequest::Buffer::WithVmoId(1));
  region2.offset(512);
  region2.size(512);

  std::vector<frequest::BufferRegion> regions;
  regions.reserve(2);
  regions.push_back(std::move(region1));
  regions.push_back(std::move(region2));

  frequest::Request req;
  req.data(std::move(regions));
  req.defer_completion(false);
  req.information(frequest::RequestInfo::WithBulk(frequest::BulkRequestInfo{}));
  req.short_(false);

  std::vector<frequest::Request> reqs;
  reqs.reserve(1);
  reqs.push_back(std::move(req));
  // Note: QueueRequests is a one-way FIDL method returning fit::result<fidl::OneWayError>
  // synchronously.
  fit::result result = ep_client_->QueueRequests({std::move(reqs)});
  ASSERT_TRUE(result.is_ok()) << "QueueRequests failed: "
                              << result.error_value().FormatDescription();
  dut_.runtime().RunUntilIdle();

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_NOT_SUPPORTED);
}

// Verifies that zero-length requests on OUT (Host-to-Device) endpoints are rejected with
// ZX_ERR_INVALID_ARGS to prevent hardware Babble FIFO overruns.
TEST_F(Dwc3EndpointsTestBase, RejectZeroLengthOutTransfer) {
  TriggerConnection();
  const uint8_t ep_address = 0x02;  // OUT endpoint
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);
  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);

  RegisterVmo(1, 4096);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue zero-length OUT transfer
  QueueRequest(1, 0, 0, fdescriptor::EndpointType::kBulk);
  dut_.runtime().RunUntilIdle();

  // 1. Verify completion status is ZX_ERR_INVALID_ARGS first (guarantees driver processed the
  // request)
  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_EQ(completions[0].status, ZX_ERR_INVALID_ARGS);

  // 2. Request should have been rejected BEFORE active_reqs or queued_reqs were populated
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_TRUE(uep.server->active_reqs.empty());
    EXPECT_TRUE(uep.server->queued_reqs.empty());
  });
}

// Verifies that when an early short packet terminates a multi-TRB transfer, any trailing
// unexecuted TRBs (such as appended ZLPs) are reclaimed and have TRB_HWO cleared.
// DISABLED: Requires trailing TRB reclamation on short packet logic in driver.
TEST_P(Dwc3EndpointsTest, DISABLED_ReapTrailingTrbsOnShortPacket) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue a request with short_bit = true, which will generate 2 TRBs.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk, /*short_bit=*/true);

  TransferState expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);
  WaitForActiveCount(ep_num, 2u);

  dwc3_trb_t* current_trb = nullptr;
  dwc3_trb_t* stranded_trb = nullptr;

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 2u);
    current_trb = uep.fifo.current_read();
    dwc3_trb_t* fifo_base = current_trb - uep.fifo.ReadOffset();
    stranded_trb = fifo_base + ((uep.fifo.ReadOffset() + 1) % uep.fifo.TotalSlots());
  });

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  TransferState expected_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    // Clear ownership of the first TRB and simulate a short packet (e.g. 256 bytes residual)
    current_trb->control &= ~TRB_HWO;
    current_trb->status = TRB_BUFSIZ(256);
    EXPECT_OK(zx_cache_flush(current_trb, sizeof(*current_trb),
                             ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));

    if (enqueue_many) {
      Dwc3TestHelper::HandleEpTransferInProgressEvent(drv, ep_num);
      uep.server->SendCompletions();
    } else {
      Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, ep_num);
    }
  });
  dut_.runtime().RunUntilIdle();

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 256UL);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 0u);
    // Properly flush/invalidate the stranded TRB to fetch from physical memory
    EXPECT_OK(zx_cache_flush(stranded_trb, sizeof(*stranded_trb),
                             ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));

    // TRB_HWO must be cleared from the un-executed TRBs that were reaped back to software
    EXPECT_FALSE(stranded_trb->control & TRB_HWO);
  });
}

// Verifies that when an early short packet terminates a multi-TRB transfer, any trailing
// unexecuted TRBs are reclaimed and the subsequent queued request executes and completes cleanly.
// DISABLED: Requires trailing TRB reclamation on short packet logic in driver.
TEST_P(Dwc3EndpointsTest, DISABLED_ShortPacketReapsTrailingTrbsAndExecutesNextRequest) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);
  RegisterVmo(2, 4096);

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue Request 1 (512B with short_bit = true, generating 2 TRBs).
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk, /*short_bit=*/true);

  // Queue Request 2 (512B, generating 1 TRB).
  QueueRequest(2, 0, 512, fdescriptor::EndpointType::kBulk, /*short_bit=*/false);

  TransferState expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);
  WaitForActiveCount(ep_num, enqueue_many ? 3u : 2u);

  dwc3_trb_t* first_trb = nullptr;
  dwc3_trb_t* stranded_trb = nullptr;
  dwc3_trb_t* second_req_trb = nullptr;

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), enqueue_many ? 3u : 2u);
    first_trb = uep.fifo.current_read();
    dwc3_trb_t* fifo_base = first_trb - uep.fifo.ReadOffset();
    stranded_trb = fifo_base + ((uep.fifo.ReadOffset() + 1) % uep.fifo.TotalSlots());
    if (enqueue_many) {
      second_req_trb = fifo_base + ((uep.fifo.ReadOffset() + 2) % uep.fifo.TotalSlots());
    }
  });

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  TransferState expected_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  // Simulate short packet on Request 1 (256 bytes transferred).
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    first_trb->control &= ~TRB_HWO;
    first_trb->status = TRB_BUFSIZ(256);
    EXPECT_OK(zx_cache_flush(first_trb, sizeof(*first_trb),
                             ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));

    if (enqueue_many) {
      Dwc3TestHelper::HandleEpTransferInProgressEvent(drv, ep_num);
      uep.server->SendCompletions();
    } else {
      Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, ep_num);
    }
  });
  dut_.runtime().RunUntilIdle();

  // Wait for Request 1 completion.
  std::vector<CompletionResult> completions1 = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions1.size(), 1UL);
  EXPECT_OK(completions1[0].status);
  EXPECT_EQ(completions1[0].transfer_size, 256UL);

  if (!enqueue_many) {
    WaitForState(ep_num, TransferState::kStartingSingle);
    dut_.RunInDriverContext(
        [&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId + 1); });
    WaitForState(ep_num, TransferState::kActiveSingle);
    dut_.RunInDriverContext([&](Dwc3& drv) {
      auto& uep = GetUserEndpoint(drv, ep_num);
      second_req_trb = uep.fifo.current_read();
    });
  }

  // Complete Request 2 (512 bytes).
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    second_req_trb->control &= ~TRB_HWO;
    second_req_trb->status = TRB_BUFSIZ(0);
    EXPECT_OK(zx_cache_flush(second_req_trb, sizeof(*second_req_trb),
                             ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));

    if (enqueue_many) {
      Dwc3TestHelper::HandleEpTransferInProgressEvent(drv, ep_num);
      uep.server->SendCompletions();
    } else {
      Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, ep_num);
    }
  });
  dut_.runtime().RunUntilIdle();

  // Wait for Request 2 completion.
  std::vector<CompletionResult> completions2 = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions2.size(), 1UL);
  EXPECT_OK(completions2[0].status);
  EXPECT_EQ(completions2[0].transfer_size, 512UL);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 0u);
    EXPECT_OK(zx_cache_flush(stranded_trb, sizeof(*stranded_trb),
                             ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
    EXPECT_FALSE(stranded_trb->control & TRB_HWO);
  });
}

// Verifies that a multi-TRB transfer where the host terminates with 0 bytes transferred
// reclaims all trailing TRBs and completes with ZX_OK and 0 bytes.
// DISABLED: Requires trailing TRB reclamation on short packet logic in driver.
TEST_P(Dwc3EndpointsTest, DISABLED_ShortPacketZeroBytesTransferredReapsTrailingTrbs) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x82;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue Request 1 (512B with short_bit = true -> 2 TRBs).
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk, /*short_bit=*/true);

  TransferState expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);
  WaitForActiveCount(ep_num, 2u);

  dwc3_trb_t* first_trb = nullptr;
  dwc3_trb_t* stranded_trb = nullptr;

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 2u);
    first_trb = uep.fifo.current_read();
    dwc3_trb_t* first = first_trb - uep.fifo.ReadOffset();
    stranded_trb = first + ((uep.fifo.ReadOffset() + 1) % uep.fifo.TotalSlots());
  });

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  TransferState expected_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  // Simulate 0 bytes transferred (residual = 512).
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    first_trb->control &= ~TRB_HWO;
    first_trb->status = TRB_BUFSIZ(512);
    EXPECT_OK(zx_cache_flush(first_trb, sizeof(*first_trb),
                             ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));

    if (enqueue_many) {
      Dwc3TestHelper::HandleEpTransferInProgressEvent(drv, ep_num);
      uep.server->SendCompletions();
    } else {
      Dwc3TestHelper::HandleEpTransferCompleteEvent(drv, ep_num);
    }
  });
  dut_.runtime().RunUntilIdle();

  std::vector<CompletionResult> completions = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions.size(), 1UL);
  EXPECT_OK(completions[0].status);
  EXPECT_EQ(completions[0].transfer_size, 0UL);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 0u);
    EXPECT_OK(zx_cache_flush(stranded_trb, sizeof(*stranded_trb),
                             ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
    EXPECT_FALSE(stranded_trb->control & TRB_HWO);
  });
}

// Verifies that Bulk OUT TRBs do not set continuous stream flags.
// DISABLED: Requires omitting TRB_CSP on discrete Bulk OUT descriptors in driver.
TEST_P(Dwc3EndpointsTest, DISABLED_VerifyBulkOutTrbFormatting) {
  const bool enqueue_many = GetParam();
  TriggerConnection();

  const uint8_t ep_address = 0x02;  // Bulk OUT
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);

  TransferState expected_starting_state =
      enqueue_many ? TransferState::kStartingOngoing : TransferState::kStartingSingle;
  WaitForState(ep_num, expected_starting_state);

  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  TransferState expected_state =
      enqueue_many ? TransferState::kActiveOngoing : TransferState::kActiveSingle;
  WaitForState(ep_num, expected_state);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_GT(uep.fifo.GetActiveCount(), 0u);
    const auto& trbs = uep.fifo.Read(uep.fifo.GetActiveCount());

    for (const auto& trb : trbs) {
      EXPECT_EQ(trb.control & TRB_CSP, 0u);
    }

    auto* fifo_base = uep.fifo.current_read() - uep.fifo.ReadOffset();
    // TrbFifo::Init decrements last_ by 1 to reserve the final slot of the buffer allocation
    // for the Link TRB, so TotalSlots() (last_ - first_) returns the number of data slots (255).
    // The Link TRB resides at index 255 (fifo_base + TotalSlots()).
    auto* link_trb = fifo_base + uep.fifo.TotalSlots();

    EXPECT_EQ(TRB_TRBCTL(link_trb->control), static_cast<uint32_t>(TRB_TRBCTL_LINK));
    EXPECT_NE(link_trb->control & TRB_CHN, 0u);
  });
}

// Tests continuous multi-request streaming on ongoing Bulk endpoints, verifying that
// TransferInProgress events safely complete individual requests in FIFO order while
// the transfer state remains kActiveOngoing.
TEST_P(Dwc3EndpointsTest, OngoingBulk_MultiRequestStreamingAndTransferInProgress) {
  const bool enqueue_many = GetParam();
  if (!enqueue_many) {
    GTEST_SKIP() << "Ongoing transfers are only applicable for enqueue_many mode.";
  }
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  // Queue 3 requests with distinct sizes (multiples of max packet size for OUT endpoints)
  // to strictly verify FIFO retirement ordering.
  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  QueueRequest(1, 512, 1024, fdescriptor::EndpointType::kBulk);
  QueueRequest(1, 1536, 1536, fdescriptor::EndpointType::kBulk);

  WaitForState(ep_num, TransferState::kStartingOngoing);
  WaitForQueuedCount(ep_num, 2u);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveOngoing);
  WaitForActiveCount(ep_num, 3u);

  // Complete first request via TransferInProgress.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  WaitForActiveCount(ep_num, 2u);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kActiveOngoing);
    EXPECT_EQ(uep.ep.rsrc_id, kResourceId);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 2u);
    EXPECT_EQ(uep.server->active_reqs.size(), 2u);
    EXPECT_TRUE(uep.server->queued_reqs.empty());
  });

  std::vector<CompletionResult> completions1 = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions1.size(), 1u);
  EXPECT_OK(completions1[0].status);
  EXPECT_EQ(completions1[0].transfer_size, 512u);

  // Complete remaining 2 requests.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  WaitForActiveCount(ep_num, 1u);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  WaitForActiveCount(ep_num, 0u);

  std::vector<CompletionResult> completions2 = event_handler_.WaitForCompletions(2);
  ASSERT_EQ(completions2.size(), 2u);
  EXPECT_OK(completions2[0].status);
  EXPECT_EQ(completions2[0].transfer_size, 1024u);
  EXPECT_OK(completions2[1].status);
  EXPECT_EQ(completions2[1].transfer_size, 1536u);

  // State remains kActiveOngoing and resource ID is preserved even when active queue drains to 0.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kActiveOngoing);
    EXPECT_EQ(uep.ep.rsrc_id, kResourceId);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 0u);
    EXPECT_TRUE(uep.server->active_reqs.empty());
    EXPECT_TRUE(uep.server->queued_reqs.empty());
  });
}

// Tests that when active requests in an ongoing Bulk transfer drain to 0, subsequent requests
// are queued and updated via CmdEpUpdateTransfer without resetting to kIdle or re-starting.
TEST_P(Dwc3EndpointsTest, OngoingBulk_ActiveQueueDrainThenRequeueWithUpdateTransfer) {
  const bool enqueue_many = GetParam();
  if (!enqueue_many) {
    GTEST_SKIP() << "Ongoing transfers are only applicable for enqueue_many mode.";
  }
  TriggerConnection();

  const uint8_t ep_address = 0x02;
  const uint8_t ep_num = UsbAddressToEpNum(ep_address);

  SetupEndpoint(ep_address, fdescriptor::EndpointType::kBulk, 512);
  RegisterVmo(1, 4096);

  // Host sends Not Ready event.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferNotReady(drv, ep_num, 0); });
  dut_.runtime().RunUntilIdle();

  QueueRequest(1, 0, 512, fdescriptor::EndpointType::kBulk);
  WaitForState(ep_num, TransferState::kStartingOngoing);
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferStarted(drv, ep_num, kResourceId); });
  WaitForState(ep_num, TransferState::kActiveOngoing);
  WaitForActiveCount(ep_num, 1u);

  // Drain active queue to 0.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  WaitForActiveCount(ep_num, 0u);

  std::vector<CompletionResult> completions1 = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions1.size(), 1u);
  EXPECT_OK(completions1[0].status);
  EXPECT_EQ(completions1[0].transfer_size, 512u);

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kActiveOngoing);
    EXPECT_EQ(uep.ep.rsrc_id, kResourceId);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 0u);
    EXPECT_TRUE(uep.server->active_reqs.empty());
    EXPECT_TRUE(uep.server->queued_reqs.empty());
  });

  auto update_transfer_called = std::make_shared<std::atomic<bool>>(false);
  auto cleanup_callbacks = DeferClearDepcmdCallbacks(ep_num);

  dut_.RunInEnvironmentTypeContext([ep_num, update_transfer_called](Environment& env) {
    auto& depcmd = env.reg_region()[DEPCMD::Get(ep_num).addr()];
    depcmd.SetWriteCallback([ep_num, update_transfer_called](uint64_t val_raw) {
      uint32_t val = static_cast<uint32_t>(val_raw);
      if (DEPCMD::Get(ep_num).FromValue(val).CMDTYP() == DEPCMD::DEPUPDXFER) {
        update_transfer_called->store(true);
      }
    });
  });

  // Re-queue new request while endpoint is still active.
  QueueRequest(1, 512, 512, fdescriptor::EndpointType::kBulk);
  WaitForActiveCount(ep_num, 1u);
  EXPECT_TRUE(update_transfer_called->load());

  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kActiveOngoing);
    EXPECT_EQ(uep.ep.rsrc_id, kResourceId);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 1u);
    EXPECT_EQ(uep.server->active_reqs.size(), 1u);
    EXPECT_TRUE(uep.server->queued_reqs.empty());
  });

  // Complete re-queued request.
  dut_.RunInDriverContext([&](Dwc3& drv) { TriggerEpTransferInProgress(drv, ep_num); });
  WaitForActiveCount(ep_num, 0u);

  std::vector<CompletionResult> completions2 = event_handler_.WaitForCompletions(1);
  ASSERT_EQ(completions2.size(), 1u);
  EXPECT_OK(completions2[0].status);
  EXPECT_EQ(completions2[0].transfer_size, 512u);

  // Verify that the endpoint remains in kActiveOngoing with clean queues at the end.
  dut_.RunInDriverContext([&](Dwc3& drv) {
    auto& uep = GetUserEndpoint(drv, ep_num);
    EXPECT_EQ(uep.ep.transfer_state, TransferState::kActiveOngoing);
    EXPECT_EQ(uep.ep.rsrc_id, kResourceId);
    EXPECT_EQ(uep.fifo.GetActiveCount(), 0u);
    EXPECT_TRUE(uep.server->active_reqs.empty());
    EXPECT_TRUE(uep.server->queued_reqs.empty());
  });
}

namespace {
INSTANTIATE_TEST_SUITE_P(Dwc3EndpointsTestCases, Dwc3EndpointsTest, testing::Bool());
}  // namespace

}  // namespace dwc3
