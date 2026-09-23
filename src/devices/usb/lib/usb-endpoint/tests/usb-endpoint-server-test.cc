// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usb-endpoint/usb-endpoint-server.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/fake-bti/bti.h>

#include <atomic>
#include <memory>
#include <thread>

#include <zxtest/zxtest.h>

namespace {

class FakeEndpoint : public usb::EndpointServer {
 public:
  FakeEndpoint(const zx::bti& bti, uint8_t ep_addr,
               fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server,
               usb::ScatterGatherSupport sg_support = usb::ScatterGatherSupport::kSupported)
      : usb::EndpointServer(bti, ep_addr, sg_support) {
    loop_.StartThread("fake-endpoint-loop");
    Connect(loop_.dispatcher(), std::move(server));
  }

  // fuchsia_hardware_usb_new.Endpoint protocol implementation.
  void GetInfo(GetInfoCompleter::Sync& completer) override {
    completer.Reply(fit::as_error(ZX_ERR_NOT_SUPPORTED));
  }
  void QueueRequests(QueueRequestsRequest& request,
                     QueueRequestsCompleter::Sync& completer) override {}
  void CancelAll(CancelAllCompleter::Sync& completer) override {
    completer.Reply(fit::as_error(ZX_ERR_NOT_SUPPORTED));
  }

  void TriggerUnbound(fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end) {
    OnUnbound(fidl::UnbindInfo::Close(ZX_OK), std::move(server_end));
  }

  sync_completion_t unbound_;

 private:
  void OnUnbound(fidl::UnbindInfo info,
                 fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end) override {
    usb::EndpointServer::OnUnbound(info, std::move(server_end));
    sync_completion_signal(&unbound_);
  }

  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

class UsbEndpointServerTest : public zxtest::Test {
 public:
  void SetUp() override {
    client_loop_.StartThread("client-loop");
    for (size_t i = 0; i < std::size(fake_bti_paddrs_); i++) {
      fake_bti_paddrs_[i] = FAKE_BTI_PHYS_ADDR * (i + 1);
    }
    ASSERT_OK(fake_bti_create_with_paddrs(fake_bti_paddrs_, std::size(fake_bti_paddrs_),
                                          fake_bti_.reset_and_get_address()));

    auto endpoints = fidl::Endpoints<fuchsia_hardware_usb_endpoint::Endpoint>::Create();
    ep_ = std::make_unique<FakeEndpoint>(fake_bti_, 0, std::move(endpoints.server));

    client_.Bind(std::move(endpoints.client), client_loop_.dispatcher(), &event_handler_);
  }

  void VerifyRegisteredVmos(size_t count) {
    size_t actual;
    EXPECT_OK(fake_bti_get_pinned_vmos(fake_bti_.get(), nullptr, 0, &actual));
    EXPECT_EQ(actual, count);
  }

 protected:
  async::Loop client_loop_{&kAsyncLoopConfigNeverAttachToThread};
  zx_paddr_t fake_bti_paddrs_[64];
  zx::bti fake_bti_;
  std::unique_ptr<FakeEndpoint> ep_;
  fidl::SharedClient<fuchsia_hardware_usb_endpoint::Endpoint> client_;

  class EventHandler : public fidl::AsyncEventHandler<fuchsia_hardware_usb_endpoint::Endpoint> {
   public:
    void OnCompletion(
        fidl::Event<fuchsia_hardware_usb_endpoint::Endpoint::OnCompletion>& event) override {
      completion_count_++;
      request_count_ += static_cast<uint32_t>(event.completion().size());
      sync_completion_signal(&received_on_completion_);
    }

    sync_completion_t received_on_completion_;
    std::atomic_uint32_t completion_count_;
    std::atomic_uint32_t request_count_;
  };
  EventHandler event_handler_;
};

TEST_F(UsbEndpointServerTest, RegisterVmosTest) {
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(32)));
  sync_completion_t wait;
  client_->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_EQ(result->vmos().size(), 1);
        EXPECT_EQ(result->vmos().at(0).id(), 8);
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());

  VerifyRegisteredVmos(1);
  auto tmp_req = fuchsia_hardware_usb_request::Request();
  tmp_req.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(32);
  auto req_var = usb::RequestVariant(usb::FidlRequest(std::move(tmp_req)));
  auto iters = ep_->get_iter(req_var, zx_system_get_page_size());
  ASSERT_TRUE(iters.is_ok());
  EXPECT_EQ(iters->size(), 1);
  EXPECT_EQ((*iters->at(0).begin()).second, 32);
}

TEST_F(UsbEndpointServerTest, RegisterMultipleVmosTest) {
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(32)));
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(5).size(16)));
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(48)));
  sync_completion_t wait;
  client_->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_EQ(result->vmos().size(), 2);
        EXPECT_EQ(result->vmos().at(0).id(), 8);
        EXPECT_EQ(result->vmos().at(1).id(), 5);
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());

  VerifyRegisteredVmos(2);
  auto tmp_req = fuchsia_hardware_usb_request::Request();
  tmp_req.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(32);
  tmp_req.data()
      ->emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(5))
      .offset(0)
      .size(16);
  auto req_var = usb::RequestVariant(usb::FidlRequest(std::move(tmp_req)));
  auto iters = ep_->get_iter(req_var, zx_system_get_page_size());
  ASSERT_TRUE(iters.is_ok());
  EXPECT_EQ(iters->size(), 2);
  EXPECT_EQ((*iters->at(0).begin()).second, 32);
  EXPECT_EQ((*iters->at(1).begin()).second, 16);
}

TEST_F(UsbEndpointServerTest, RegisterVmoWithOffsetTest) {
  const size_t page_size = zx_system_get_page_size();
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(
      std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(page_size * 4)));
  vmo_info.emplace_back(
      std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(9).size(page_size / 2)));
  sync_completion_t wait;
  client_->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        EXPECT_EQ(result->vmos().size(), 2);
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());

  VerifyRegisteredVmos(2);

  // Request with offset 0
  auto req0 = fuchsia_hardware_usb_request::Request();
  req0.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(64);
  auto req0_var = usb::RequestVariant(usb::FidlRequest(std::move(req0)));
  auto iters0 = ep_->get_iter(req0_var, page_size);
  ASSERT_TRUE(iters0.is_ok());
  auto [phys0, size0] = *iters0->at(0).begin();
  EXPECT_EQ(size0, 64);

  // Request with non-zero page-aligned offset
  auto req1 = fuchsia_hardware_usb_request::Request();
  req1.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(page_size)
      .size(128);
  auto req1_var = usb::RequestVariant(usb::FidlRequest(std::move(req1)));
  auto iters1 = ep_->get_iter(req1_var, page_size);
  ASSERT_TRUE(iters1.is_ok());
  auto [phys1, size1] = *iters1->at(0).begin();
  EXPECT_EQ(size1, 128);
  EXPECT_NE(phys0, phys1);

  // Request with non-page-aligned offset
  auto req2 = fuchsia_hardware_usb_request::Request();
  req2.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(page_size + 64)
      .size(128);
  auto req2_var = usb::RequestVariant(usb::FidlRequest(std::move(req2)));
  auto iters2 = ep_->get_iter(req2_var, page_size);
  ASSERT_TRUE(iters2.is_ok());
  auto [phys2, size2] = *iters2->at(0).begin();
  EXPECT_EQ(size2, 128);
  EXPECT_EQ(phys2, phys1 + 64);

  // Request spanning page boundary (64 bytes in page 0, 64 bytes in page 1)
  auto req3 = fuchsia_hardware_usb_request::Request();
  req3.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(page_size - 64)
      .size(128);
  auto req3_var = usb::RequestVariant(usb::FidlRequest(std::move(req3)));
  auto iters3 = ep_->get_iter(req3_var, page_size);
  ASSERT_TRUE(iters3.is_ok());
  auto it3 = iters3->at(0).begin();
  auto [phys3_0, size3_0] = *it3;
  EXPECT_EQ(size3_0, 64);
  ++it3;
  auto [phys3_1, size3_1] = *it3;
  EXPECT_EQ(size3_1, 64);
  EXPECT_EQ(phys3_1, phys1);

  // Request on single-page VMO (id 9) with non-zero offset
  auto req4 = fuchsia_hardware_usb_request::Request();
  req4.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(9))
      .offset(32)
      .size(64);
  auto req4_var = usb::RequestVariant(usb::FidlRequest(std::move(req4)));
  auto iters4 = ep_->get_iter(req4_var, page_size);
  ASSERT_TRUE(iters4.is_ok());
  auto [phys4, size4] = *iters4->at(0).begin();
  EXPECT_EQ(size4, 64);
}

TEST_F(UsbEndpointServerTest, GetIterInvalidVmoIdTest) {
  const size_t page_size = zx_system_get_page_size();
  auto req = fuchsia_hardware_usb_request::Request();
  req.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(999))
      .offset(0)
      .size(64);
  auto req_var = usb::RequestVariant(usb::FidlRequest(std::move(req)));
  auto iters = ep_->get_iter(req_var, page_size);
  ASSERT_TRUE(iters.is_error());
  EXPECT_EQ(iters.status_value(), ZX_ERR_NOT_FOUND);
}

TEST_F(UsbEndpointServerTest, GetIterOutOfBoundsOffsetTest) {
  const size_t page_size = zx_system_get_page_size();
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(page_size)));
  sync_completion_t wait;
  client_->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());

  // Offset equal to VMO size
  auto req0 = fuchsia_hardware_usb_request::Request();
  req0.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(page_size)
      .size(64);
  auto req0_var = usb::RequestVariant(usb::FidlRequest(std::move(req0)));
  auto iters0 = ep_->get_iter(req0_var, page_size);
  ASSERT_TRUE(iters0.is_error());
  EXPECT_EQ(iters0.status_value(), ZX_ERR_OUT_OF_RANGE);

  // Offset exceeding VMO size
  auto req1 = fuchsia_hardware_usb_request::Request();
  req1.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(page_size * 2)
      .size(64);
  auto req1_var = usb::RequestVariant(usb::FidlRequest(std::move(req1)));
  auto iters1 = ep_->get_iter(req1_var, page_size);
  ASSERT_TRUE(iters1.is_error());
  EXPECT_EQ(iters1.status_value(), ZX_ERR_OUT_OF_RANGE);
}

TEST_F(UsbEndpointServerTest, GetIterOutOfBoundsRangeTest) {
  const size_t page_size = zx_system_get_page_size();
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(page_size)));
  sync_completion_t wait;
  client_->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());

  // Offset + size > VMO size
  auto req = fuchsia_hardware_usb_request::Request();
  req.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(page_size - 32)
      .size(64);
  auto req_var = usb::RequestVariant(usb::FidlRequest(std::move(req)));
  auto iters = ep_->get_iter(req_var, page_size);
  ASSERT_TRUE(iters.is_error());
  EXPECT_EQ(iters.status_value(), ZX_ERR_OUT_OF_RANGE);
}

TEST_F(UsbEndpointServerTest, GetIterInvalidSizeTest) {
  const size_t page_size = zx_system_get_page_size();
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(page_size)));
  sync_completion_t wait;
  client_->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());

  // Size 0
  auto req = fuchsia_hardware_usb_request::Request();
  req.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(0);
  auto req_var = usb::RequestVariant(usb::FidlRequest(std::move(req)));
  auto iters = ep_->get_iter(req_var, page_size);
  ASSERT_TRUE(iters.is_error());
  EXPECT_EQ(iters.status_value(), ZX_ERR_INVALID_ARGS);
}

TEST_F(UsbEndpointServerTest, UnregisterVmosTest) {
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(32)));
  sync_completion_t wait;
  client_->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());
  sync_completion_reset(&wait);

  VerifyRegisteredVmos(1);

  client_->UnregisterVmos(std::vector<uint64_t>{4})
      .Then(
          [&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::UnregisterVmos>& result) {
            ASSERT_TRUE(result.is_ok());
            EXPECT_EQ(result->failed_vmo_ids().size(), 1);
            EXPECT_EQ(result->failed_vmo_ids().at(0), 4);
            sync_completion_signal(&wait);
          });
  sync_completion_wait(&wait, zx::time::infinite().get());
  sync_completion_reset(&wait);

  VerifyRegisteredVmos(1);

  client_->UnregisterVmos(std::vector<uint64_t>{8, 3})
      .Then(
          [&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::UnregisterVmos>& result) {
            ASSERT_TRUE(result.is_ok());
            EXPECT_EQ(result->failed_vmo_ids().size(), 1);
            EXPECT_EQ(result->failed_vmo_ids().at(0), 3);
            sync_completion_signal(&wait);
          });
  sync_completion_wait(&wait, zx::time::infinite().get());
  sync_completion_reset(&wait);

  VerifyRegisteredVmos(0);
}

TEST_F(UsbEndpointServerTest, UnboundTest) {
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(32)));
  sync_completion_t wait;
  client_->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());

  VerifyRegisteredVmos(1);

  // Trigger unbind
  {
    auto unused = std::move(client_);
  }

  sync_completion_wait(&ep_->unbound_, zx::time::infinite().get());
  VerifyRegisteredVmos(0);
}

TEST_F(UsbEndpointServerTest, RequestCompleteFidlTest) {
  ep_->RequestComplete(
      ZX_OK, 0,
      usb::FidlRequest(std::move(fuchsia_hardware_usb_request::Request().defer_completion(false))));
  sync_completion_wait(&event_handler_.received_on_completion_, zx::time::infinite().get());
  EXPECT_EQ(event_handler_.completion_count_.load(), 1);
  EXPECT_EQ(event_handler_.request_count_.load(), 1);
  sync_completion_reset(&event_handler_.received_on_completion_);

  ep_->RequestComplete(
      ZX_OK, 0,
      usb::FidlRequest(std::move(fuchsia_hardware_usb_request::Request().defer_completion(false))));
  sync_completion_wait(&event_handler_.received_on_completion_, zx::time::infinite().get());
  EXPECT_EQ(event_handler_.completion_count_.load(), 2);
  EXPECT_EQ(event_handler_.request_count_.load(), 2);
  sync_completion_reset(&event_handler_.received_on_completion_);
}

TEST_F(UsbEndpointServerTest, RequestCompleteDeferredFidlTest) {
  ep_->RequestComplete(
      ZX_OK, 5,
      usb::FidlRequest(std::move(fuchsia_hardware_usb_request::Request().defer_completion(true))));

  ep_->RequestComplete(
      ZX_OK, 9,
      usb::FidlRequest(std::move(fuchsia_hardware_usb_request::Request().defer_completion(true))));

  ep_->RequestComplete(
      ZX_ERR_INTERNAL, 0,
      usb::FidlRequest(std::move(fuchsia_hardware_usb_request::Request().defer_completion(true))));

  sync_completion_wait(&event_handler_.received_on_completion_, zx::time::infinite().get());
  EXPECT_EQ(event_handler_.completion_count_.load(), 1);
  EXPECT_EQ(event_handler_.request_count_.load(), 3);
  sync_completion_reset(&event_handler_.received_on_completion_);

  ep_->RequestComplete(
      ZX_OK, 15,
      usb::FidlRequest(std::move(fuchsia_hardware_usb_request::Request().defer_completion(true))));

  ep_->RequestComplete(
      ZX_OK, 3,
      usb::FidlRequest(std::move(fuchsia_hardware_usb_request::Request().defer_completion(false))));

  sync_completion_wait(&event_handler_.received_on_completion_, zx::time::infinite().get());
  EXPECT_EQ(event_handler_.completion_count_.load(), 2);
  EXPECT_EQ(event_handler_.request_count_.load(), 5);
  sync_completion_reset(&event_handler_.received_on_completion_);
}

TEST_F(UsbEndpointServerTest, RequestCompleteBatchSplittingTest) {
  // Queue 70 deferred completions.
  for (size_t i = 0; i < 70; ++i) {
    ep_->RequestComplete(ZX_OK, 1,
                         usb::FidlRequest(std::move(
                             fuchsia_hardware_usb_request::Request().defer_completion(true))));
  }

  // Queue 1 non-deferred completion to trigger the flush of all 71 accumulated completions.
  ep_->RequestComplete(
      ZX_OK, 1,
      usb::FidlRequest(std::move(fuchsia_hardware_usb_request::Request().defer_completion(false))));

  // Wait until all 71 completions are received by the client.
  while (event_handler_.request_count_.load() < 71) {
    ASSERT_OK(sync_completion_wait(&event_handler_.received_on_completion_, zx::sec(60).get()));
    sync_completion_reset(&event_handler_.received_on_completion_);
  }

  EXPECT_EQ(event_handler_.request_count_.load(), 71);
  // With kMaxBatchSize = 64 (fendpoint::kMaxCompletionsPerEvent), 71 completions must be split
  // across 2 events (64, 7).
  EXPECT_EQ(event_handler_.completion_count_.load(), 2);
}

TEST_F(UsbEndpointServerTest, OnUnboundBatchSplittingTest) {
  // Queue 71 deferred completions.
  for (size_t i = 0; i < 71; ++i) {
    ep_->RequestComplete(ZX_OK, 1,
                         usb::FidlRequest(std::move(
                             fuchsia_hardware_usb_request::Request().defer_completion(true))));
  }

  auto endpoints = fidl::Endpoints<fuchsia_hardware_usb_endpoint::Endpoint>::Create();
  // OnUnbound() consumes the server end, so the client observes PEER_CLOSED and tears down
  // asynchronously on client_loop_. Co-own the handler with the client so that it outlives the
  // teardown callback, which runs after this test body returns.
  auto unbound_handler = std::make_shared<EventHandler>();
  fidl::SharedClient<fuchsia_hardware_usb_endpoint::Endpoint> unbound_client(
      std::move(endpoints.client), client_loop_.dispatcher(), unbound_handler.get(),
      fidl::ShareUntilTeardown(unbound_handler));

  ep_->TriggerUnbound(std::move(endpoints.server));

  while (unbound_handler->request_count_.load() < 71) {
    ASSERT_OK(sync_completion_wait(&unbound_handler->received_on_completion_, zx::sec(60).get()));
    sync_completion_reset(&unbound_handler->received_on_completion_);
  }

  EXPECT_EQ(unbound_handler->request_count_.load(), 71);
  EXPECT_EQ(unbound_handler->completion_count_.load(), 2);
}

TEST_F(UsbEndpointServerTest, RequestCompleteBanjoTest) {
  constexpr size_t kBaseReqSize = sizeof(usb_request_t);
  constexpr size_t kFirstLayerReqSize = usb::Request<void>::RequestSize(kBaseReqSize);

  bool called = false;
  auto callback = [](void* ctx, usb_request_t* request) {
    *static_cast<bool*>(ctx) = true;
    // We take ownership.
    usb::Request<void> unused(request, kBaseReqSize);
  };
  usb_request_complete_callback_t complete_cb = {
      .callback = callback,
      .ctx = &called,
  };

  std::optional<usb::Request<void>> request;
  ASSERT_EQ(usb::Request<void>::Alloc(&request, 0, 0, kFirstLayerReqSize), ZX_OK);

  ep_->RequestComplete(ZX_OK, 0,
                       usb::BorrowedRequest<void>(request->take(), complete_cb, kBaseReqSize));
  EXPECT_TRUE(called);
}

TEST_F(UsbEndpointServerTest, ConcurrentRequestCompleteAndUnbind) {
  std::atomic<bool> stop{false};
  std::thread complete_thread([&]() {
    while (!stop.load()) {
      ep_->RequestComplete(ZX_OK, 0,
                           usb::FidlRequest(std::move(
                               fuchsia_hardware_usb_request::Request().defer_completion(false))));
      zx::nanosleep(zx::deadline_after(zx::usec(10)));
    }
  });

  // Let it run for a bit
  zx::nanosleep(zx::deadline_after(zx::msec(10)));

  // Trigger unbind
  {
    auto unused = std::move(client_);
  }

  sync_completion_wait(&ep_->unbound_, zx::time::infinite().get());

  stop.store(true);
  complete_thread.join();
}

TEST_F(UsbEndpointServerTest, GetIterUnsupportedScatterGatherTest) {
  const size_t page_size = zx_system_get_page_size();
  auto endpoints = fidl::Endpoints<fuchsia_hardware_usb_endpoint::Endpoint>::Create();
  auto unsupported_ep = std::make_unique<FakeEndpoint>(fake_bti_, 1, std::move(endpoints.server),
                                                       usb::ScatterGatherSupport::kUnsupported);
  fidl::SharedClient<fuchsia_hardware_usb_endpoint::Endpoint> client{std::move(endpoints.client),
                                                                     client_loop_.dispatcher()};

  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info;
  vmo_info.emplace_back(
      std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(page_size * 2)));
  sync_completion_t wait;
  client->RegisterVmos({std::move(vmo_info)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        sync_completion_signal(&wait);
      });
  sync_completion_wait(&wait, zx::time::infinite().get());

  // Single-page request should succeed
  auto req_single = fuchsia_hardware_usb_request::Request();
  req_single.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(page_size);
  auto req_single_var = usb::RequestVariant(usb::FidlRequest(std::move(req_single)));
  auto iters_single = unsupported_ep->get_iter(req_single_var, page_size);
  ASSERT_TRUE(iters_single.is_ok());

  // Multi-page request should return ZX_ERR_NOT_SUPPORTED
  auto req_multi = fuchsia_hardware_usb_request::Request();
  req_multi.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(page_size * 2);
  auto req_multi_var = usb::RequestVariant(usb::FidlRequest(std::move(req_multi)));
  auto iters_multi = unsupported_ep->get_iter(req_multi_var, page_size);
  ASSERT_TRUE(iters_multi.is_error());
  EXPECT_EQ(iters_multi.status_value(), ZX_ERR_NOT_SUPPORTED);

  // Multi-buffer request should return ZX_ERR_NOT_SUPPORTED
  auto req_multi_buf = fuchsia_hardware_usb_request::Request();
  req_multi_buf.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(page_size);
  req_multi_buf.data()
      ->emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(page_size)
      .size(page_size);
  auto req_multi_buf_var = usb::RequestVariant(usb::FidlRequest(std::move(req_multi_buf)));
  auto iters_multi_buf = unsupported_ep->get_iter(req_multi_buf_var, page_size);
  ASSERT_TRUE(iters_multi_buf.is_error());
  EXPECT_EQ(iters_multi_buf.status_value(), ZX_ERR_NOT_SUPPORTED);

  // Supported endpoint should succeed with multi-page requests
  auto endpoints_supp = fidl::Endpoints<fuchsia_hardware_usb_endpoint::Endpoint>::Create();
  auto supported_ep = std::make_unique<FakeEndpoint>(fake_bti_, 1, std::move(endpoints_supp.server),
                                                     usb::ScatterGatherSupport::kSupported);
  fidl::SharedClient<fuchsia_hardware_usb_endpoint::Endpoint> client_supp{
      std::move(endpoints_supp.client), client_loop_.dispatcher()};
  std::vector<fuchsia_hardware_usb_endpoint::VmoInfo> vmo_info_supp;
  vmo_info_supp.emplace_back(
      std::move(fuchsia_hardware_usb_endpoint::VmoInfo().id(8).size(page_size * 2)));
  sync_completion_t wait_supp;
  client_supp->RegisterVmos({std::move(vmo_info_supp)})
      .Then([&](const fidl::Result<fuchsia_hardware_usb_endpoint::Endpoint::RegisterVmos>& result) {
        ASSERT_TRUE(result.is_ok());
        sync_completion_signal(&wait_supp);
      });
  sync_completion_wait(&wait_supp, zx::time::infinite().get());

  auto req_supp_multi = fuchsia_hardware_usb_request::Request();
  req_supp_multi.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(page_size * 2);
  auto req_supp_var = usb::RequestVariant(usb::FidlRequest(std::move(req_supp_multi)));
  auto iters_supp = supported_ep->get_iter(req_supp_var, page_size);
  ASSERT_TRUE(iters_supp.is_ok());
  EXPECT_EQ(iters_supp->size(), 1);

  // Supported endpoint should also succeed with multi-buffer requests
  auto req_supp_multi_buf = fuchsia_hardware_usb_request::Request();
  req_supp_multi_buf.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(0)
      .size(page_size);
  req_supp_multi_buf.data()
      ->emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(8))
      .offset(page_size)
      .size(page_size);
  auto req_supp_multi_buf_var =
      usb::RequestVariant(usb::FidlRequest(std::move(req_supp_multi_buf)));
  auto iters_supp_multi_buf = supported_ep->get_iter(req_supp_multi_buf_var, page_size);
  ASSERT_TRUE(iters_supp_multi_buf.is_ok());
  EXPECT_EQ(iters_supp_multi_buf->size(), 2);

  // Zero-length request on unsupported endpoint should succeed
  auto req_zero = fuchsia_hardware_usb_request::Request();
  auto req_zero_var = usb::RequestVariant(usb::FidlRequest(std::move(req_zero)));
  auto iters_zero = unsupported_ep->get_iter(req_zero_var, page_size);
  ASSERT_TRUE(iters_zero.is_ok());
  EXPECT_EQ(iters_zero->size(), 0);
}

}  // namespace
