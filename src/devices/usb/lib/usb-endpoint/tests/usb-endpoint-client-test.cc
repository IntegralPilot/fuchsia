// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/lib/usb-endpoint/include/usb-endpoint/usb-endpoint-client.h"

#include <fidl/fuchsia.hardware.usb/cpp/fidl.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>

#include <zxtest/zxtest.h>

#include "src/devices/usb/lib/usb-endpoint/testing/fake-usb-endpoint-server.h"

namespace {

using UsbProtocolType = fuchsia_hardware_usb::Usb;
constexpr uint8_t kEpAddr = 1;

class FakeUsbEndpoint : public fake_usb_endpoint::FakeEndpoint {
 public:
  ~FakeUsbEndpoint() {
    EXPECT_EQ(expected_register_vmos_.load(), 0);
    EXPECT_EQ(expected_unregister_vmos_.load(), 0);
  }

  void ExpectRegisterVmos(uint32_t count) { expected_register_vmos_ += count; }
  void RegisterVmos(RegisterVmosRequest& request, RegisterVmosCompleter::Sync& completer) override {
    ASSERT_TRUE(expected_register_vmos_ > 0);
    expected_register_vmos_ -= static_cast<uint32_t>(request.vmo_ids().size());
    fake_usb_endpoint::FakeEndpoint::RegisterVmos(request, completer);
  }

  void ExpectUnregisterVmos(uint32_t count) { expected_unregister_vmos_ += count; }
  void UnregisterVmos(UnregisterVmosRequest& request,
                      UnregisterVmosCompleter::Sync& completer) override {
    ASSERT_TRUE(expected_unregister_vmos_ > 0);
    expected_unregister_vmos_ -= static_cast<uint32_t>(request.vmo_ids().size());
    fake_usb_endpoint::FakeEndpoint::UnregisterVmos(request, completer);
  }

  void QueueRequests(QueueRequestsRequest& request,
                     QueueRequestsCompleter::Sync& completer) override {
    fake_usb_endpoint::FakeEndpoint::QueueRequests(request, completer);
    if (on_queue_requests_) {
      on_queue_requests_(*this);
    }
  }

  void set_on_queue_requests(std::function<void(FakeUsbEndpoint&)> cb) {
    on_queue_requests_ = std::move(cb);
  }

 private:
  std::atomic_uint32_t expected_register_vmos_ = 0;
  std::atomic_uint32_t expected_unregister_vmos_ = 0;
  std::function<void(FakeUsbEndpoint&)> on_queue_requests_ = nullptr;
};

class FakeUsbServer
    : public fake_usb_endpoint::FakeUsbFidlProvider<UsbProtocolType, FakeUsbEndpoint> {
 public:
  FakeUsbServer(async_dispatcher_t* dispatcher, fidl::ServerEnd<UsbProtocolType> server)
      : fake_usb_endpoint::FakeUsbFidlProvider<UsbProtocolType, FakeUsbEndpoint>(dispatcher),
        binding_ref_(fidl::BindServer(dispatcher, std::move(server), this)) {}

 private:
  const std::optional<fidl::ServerBindingRef<UsbProtocolType>> binding_ref_;
};

class UsbEndpointClientTest : public zxtest::Test {
 public:
  void SetUp() override {
    server_loop_.StartThread("usb-endpoint-client-test-server-thread");
    client_loop_.StartThread("usb-endpoint-client-test-client-thread");
    client_ = std::make_unique<usb::EndpointClient<UsbEndpointClientTest>>(
        usb::EndpointType::BULK, this, std::mem_fn(&UsbEndpointClientTest::Complete));

    auto endpoints = fidl::Endpoints<UsbProtocolType>::Create();
    server_ =
        std::make_unique<FakeUsbServer>(server_loop_.dispatcher(), std::move(endpoints.server));
    ASSERT_NOT_NULL(server_);

    server_->ExpectConnectToEndpoint(kEpAddr);
    EXPECT_OK(client_->Init(kEpAddr, endpoints.client, client_loop_.dispatcher()));
  }

  void TearDown() override {
    client_.reset();
    client_loop_.Shutdown();
    server_loop_.Shutdown();
  }

 protected:
  void RequestTest(fuchsia_hardware_usb_request::Buffer::Tag type, size_t req_count);

  std::unique_ptr<usb::EndpointClient<UsbEndpointClientTest>> client_;
  std::unique_ptr<FakeUsbServer> server_;

 private:
  void Complete(std::vector<fuchsia_hardware_usb_endpoint::Completion> completion) {
    if (on_completion_) {
      on_completion_(std::move(completion));
    }
  }

  std::function<void(std::vector<fuchsia_hardware_usb_endpoint::Completion>)> on_completion_ =
      nullptr;

 protected:
  void set_on_completion(
      std::function<void(std::vector<fuchsia_hardware_usb_endpoint::Completion>)> on_completion) {
    on_completion_ = std::move(on_completion);
  }

 private:
  async::Loop client_loop_{&kAsyncLoopConfigNeverAttachToThread};
  async::Loop server_loop_{&kAsyncLoopConfigNeverAttachToThread};
};

void UsbEndpointClientTest::RequestTest(fuchsia_hardware_usb_request::Buffer::Tag type,
                                        size_t req_count) {
  const size_t kVmoSize = 32;

  auto actual = client_->AddRequests(req_count, kVmoSize, type);
  EXPECT_EQ(actual, req_count);
  EXPECT_TRUE(client_->RequestsFull());

  std::vector<usb::FidlRequest> requests;
  size_t count = 0;
  while (auto req = client_->GetRequest()) {
    EXPECT_EQ(req->request().information()->Which(),
              fuchsia_hardware_usb_request::RequestInfo::Tag::kBulk);
    EXPECT_EQ(req->request().data()->size(), 1);
    EXPECT_EQ(req->request().data()->at(0).buffer()->Which(), type);

    count++;

    requests.emplace_back(std::move(*req));
  }
  EXPECT_TRUE(client_->RequestsEmpty());

  EXPECT_EQ(count, req_count);

  // Put requests back in queue for teardown
  for (auto& req : requests) {
    client_->PutRequest(std::move(req));
  }
  EXPECT_TRUE(client_->RequestsFull());
}

TEST_F(UsbEndpointClientTest, VmoIdRequests) {
  const size_t kRequestCount = 3;
  server_->fake_endpoint(kEpAddr).ExpectRegisterVmos(kRequestCount);
  server_->fake_endpoint(kEpAddr).ExpectUnregisterVmos(kRequestCount);
  RequestTest(fuchsia_hardware_usb_request::Buffer::Tag::kVmoId, kRequestCount);
}

TEST_F(UsbEndpointClientTest, DataRequests) {
  const size_t kRequestCount = 3;
  RequestTest(fuchsia_hardware_usb_request::Buffer::Tag::kData, kRequestCount);
}

TEST_F(UsbEndpointClientTest, Copy) {
  const size_t kVmoSize = 32;

  server_->fake_endpoint(kEpAddr).ExpectRegisterVmos(1);
  auto actual =
      client_->AddRequests(1, kVmoSize, fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  EXPECT_EQ(actual, 1);
  EXPECT_TRUE(client_->RequestsFull());
  actual = client_->AddRequests(1, kVmoSize, fuchsia_hardware_usb_request::Buffer::Tag::kData);
  EXPECT_EQ(actual, 1);
  EXPECT_TRUE(client_->RequestsFull());

  std::vector<usb::FidlRequest> requests;
  uint8_t in_buffer[] = {0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7};
  while (auto req = client_->GetRequest()) {
    req->clear_buffers();
    ASSERT_TRUE(req.has_value());
    {
      auto actual = req->CopyTo(0, in_buffer, sizeof(in_buffer), client_->GetMapped());
      EXPECT_EQ(actual.size(), 1);
      EXPECT_EQ(actual[0], sizeof(in_buffer));
      (*req)->data()->at(0).size(actual[0]);
    }

    uint8_t out_buffer[sizeof(in_buffer) + 4] = {0};
    {
      auto actual = req->CopyFrom(0, out_buffer, sizeof(out_buffer), client_->GetMapped());
      EXPECT_EQ(actual.size(), 1);
      EXPECT_EQ(actual[0], sizeof(in_buffer));
      EXPECT_BYTES_EQ(out_buffer, in_buffer, actual[0]);
    }

    requests.emplace_back(std::move(*req));
  }

  for (auto& req : requests) {
    client_->PutRequest(std::move(req));
  }
  EXPECT_TRUE(client_->RequestsFull());

  server_->fake_endpoint(kEpAddr).ExpectUnregisterVmos(1);
}

TEST_F(UsbEndpointClientTest, RxRequests) {
  const size_t kVmoSize = 32;

  // Add a data request.
  size_t actual_data =
      client_->AddRequests(1, kVmoSize, fuchsia_hardware_usb_request::Buffer::Tag::kData);
  ASSERT_EQ(actual_data, 1);

  // Add a VMO request.
  server_->fake_endpoint(kEpAddr).ExpectRegisterVmos(1);
  size_t actual_vmo =
      client_->AddRequests(1, kVmoSize, fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
  ASSERT_EQ(actual_vmo, 1);

  std::optional<usb::FidlRequest> req_data = client_->GetRequest();
  ASSERT_TRUE(req_data.has_value());

  std::optional<usb::FidlRequest> req_vmo = client_->GetRequest();
  ASSERT_TRUE(req_vmo.has_value());

  std::vector<fuchsia_hardware_usb_request::Request> f_reqs;
  f_reqs.push_back(req_data->take_request());
  f_reqs.push_back(req_vmo->take_request());

  fit::result<fidl::OneWayError> res = (*client_)->QueueRequests(
      fuchsia_hardware_usb_endpoint::EndpointQueueRequestsRequest{std::move(f_reqs)});
  ASSERT_TRUE(res.is_ok(), "%s", res.error_value().FormatDescription().c_str());

  libsync::Completion completion;
  std::vector<fuchsia_hardware_usb_endpoint::Completion> completed_reqs;

  set_on_completion([&](std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
    for (auto& comp : completions) {
      completed_reqs.push_back(std::move(comp));
      completion.Signal();
    }
  });

  std::vector<uint8_t> test_data1 = {0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f};
  std::vector<uint8_t> test_data2 = {0x10, 0x11, 0x12, 0x13, 0x14, 0x15};

  // Complete data request.
  server_->fake_endpoint(kEpAddr).RequestComplete(ZX_OK, test_data1);
  completion.Wait();
  completion.Reset();

  ASSERT_EQ(completed_reqs.size(), 1);
  {
    EXPECT_EQ(completed_reqs[0].transfer_size(), test_data1.size());
    usb::FidlRequest ret_req{std::move(completed_reqs[0].request().value())};
    std::vector<uint8_t> read_buf(test_data1.size());
    ret_req.CopyFrom(0, read_buf.data(), read_buf.size(), client_->GetMapped());
    EXPECT_BYTES_EQ(read_buf.data(), test_data1.data(), test_data1.size());
    client_->PutRequest(std::move(ret_req));
  }
  completed_reqs.clear();

  // Complete VMO request.
  server_->fake_endpoint(kEpAddr).RequestComplete(ZX_OK, test_data2);
  completion.Wait();

  ASSERT_EQ(completed_reqs.size(), 1);
  {
    EXPECT_EQ(completed_reqs[0].transfer_size(), test_data2.size());
    usb::FidlRequest ret_req{std::move(completed_reqs[0].request().value())};
    std::vector<uint8_t> read_buf(test_data2.size());
    ret_req.CopyFrom(0, read_buf.data(), read_buf.size(), client_->GetMapped());
    EXPECT_BYTES_EQ(read_buf.data(), test_data2.data(), test_data2.size());
    client_->PutRequest(std::move(ret_req));
  }

  server_->fake_endpoint(kEpAddr).ExpectUnregisterVmos(1);
}

TEST_F(UsbEndpointClientTest, TxRequests) {
  const size_t kVmoSize = 32;

  libsync::Completion read_completion;
  libsync::Completion event_completion;
  std::vector<fuchsia_hardware_usb_endpoint::Completion> completed_reqs;

  set_on_completion([&](std::vector<fuchsia_hardware_usb_endpoint::Completion> completions) {
    for (auto& comp : completions) {
      completed_reqs.push_back(std::move(comp));
      event_completion.Signal();
    }
  });

  // 1. Data Request
  {
    size_t actual =
        client_->AddRequests(1, kVmoSize, fuchsia_hardware_usb_request::Buffer::Tag::kData);
    ASSERT_EQ(actual, 1);

    std::optional<usb::FidlRequest> req = client_->GetRequest();
    ASSERT_TRUE(req.has_value());

    std::vector<uint8_t> test_data = {0x1, 0x2, 0x3, 0x4};
    req->CopyTo(0, test_data.data(), test_data.size(), client_->GetMapped());

    std::vector<fuchsia_hardware_usb_request::Request> f_reqs;
    f_reqs.push_back(req->take_request());

    server_->fake_endpoint(kEpAddr).set_on_queue_requests([&](FakeUsbEndpoint& ep) {
      auto complete = fit::defer([&]() { read_completion.Signal(); });
      zx::result read_res = ep.ReadPendingRequestData();
      ASSERT_OK(read_res.status_value());
      EXPECT_BYTES_EQ(read_res.value().data(), test_data.data(), test_data.size());
    });

    fit::result<fidl::OneWayError> res = (*client_)->QueueRequests(
        fuchsia_hardware_usb_endpoint::EndpointQueueRequestsRequest{std::move(f_reqs)});
    ASSERT_TRUE(res.is_ok(), "%s", res.error_value().FormatDescription().c_str());

    read_completion.Wait();
    read_completion.Reset();

    // Complete to clear it from server
    server_->fake_endpoint(kEpAddr).RequestComplete(ZX_OK, 0);
    event_completion.Wait();
    event_completion.Reset();

    ASSERT_EQ(completed_reqs.size(), 1);
    usb::FidlRequest ret_req{std::move(completed_reqs[0].request().value())};
    client_->PutRequest(std::move(ret_req));
    completed_reqs.clear();
  }

  // 2. VMO Request
  {
    server_->fake_endpoint(kEpAddr).ExpectRegisterVmos(1);
    size_t actual =
        client_->AddRequests(1, kVmoSize, fuchsia_hardware_usb_request::Buffer::Tag::kVmoId);
    ASSERT_EQ(actual, 1);

    std::optional<usb::FidlRequest> req = client_->GetRequest();
    ASSERT_TRUE(req.has_value());

    std::vector<uint8_t> test_data = {0x5, 0x6, 0x7, 0x8};
    req->CopyTo(0, test_data.data(), test_data.size(), client_->GetMapped());

    std::vector<fuchsia_hardware_usb_request::Request> f_reqs;
    f_reqs.push_back(req->take_request());

    server_->fake_endpoint(kEpAddr).set_on_queue_requests([&](FakeUsbEndpoint& ep) {
      auto read_res = ep.ReadPendingRequestData();
      ASSERT_OK(read_res.status_value());
      EXPECT_BYTES_EQ(read_res.value().data(), test_data.data(), test_data.size());
      read_completion.Signal();
    });

    fit::result<fidl::OneWayError> res = (*client_)->QueueRequests(
        fuchsia_hardware_usb_endpoint::EndpointQueueRequestsRequest{std::move(f_reqs)});
    ASSERT_TRUE(res.is_ok(), "%s", res.error_value().FormatDescription().c_str());

    read_completion.Wait();
    read_completion.Reset();

    // Complete to clear it from server.
    server_->fake_endpoint(kEpAddr).RequestComplete(ZX_OK, 0);
    event_completion.Wait();

    ASSERT_EQ(completed_reqs.size(), 1);
    usb::FidlRequest ret_req{std::move(completed_reqs[0].request().value())};
    client_->PutRequest(std::move(ret_req));
    server_->fake_endpoint(kEpAddr).ExpectUnregisterVmos(1);
  }
}

}  // namespace
