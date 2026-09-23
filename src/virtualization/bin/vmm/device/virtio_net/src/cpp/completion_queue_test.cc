// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/virtualization/bin/vmm/device/virtio_net/src/cpp/completion_queue.h"

#include <lib/driver/testing/cpp/driver_runtime.h>

#include <latch>

#include <gmock/gmock.h>
#include <src/lib/testing/loop_fixture/test_loop_fixture.h>

namespace {

// To simplify the tests, buffer IDs and buffer lengths will sequentially increase across and
// between each batch.
constexpr uint32_t kFirstBufferId = 1;
constexpr uint32_t kFirstBufferLength = 128;

constexpr uint8_t kPort = 5;

class FakeNetDevice : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceIfc> {
 public:
  fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceIfc> GetClient() {
    auto [client, server] =
        fdf::Endpoints<fuchsia_hardware_network_driver::NetworkDeviceIfc>::Create();
    fdf::BindServer(fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher()->get(),
                    std::move(server), this);
    return std::move(client);
  }

  void CompleteRx(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest* request,
                  fdf::Arena& arena, CompleteRxCompleter::Sync& completer) override {
    std::vector<fuchsia_hardware_network_driver::wire::RxBufferPart> batch;
    for (const auto& buffer : request->rx) {
      // Static values which should be the same for every completed buffer.
      ASSERT_EQ(buffer.data.size(), 1u);
      ASSERT_EQ(buffer.data[0].offset, 0u);
      ASSERT_EQ(buffer.meta.port, kPort);
      ASSERT_EQ(buffer.meta.frame_type, fuchsia_hardware_network::wire::FrameType::kEthernet);

      batch.push_back(buffer.data[0]);
    }
    rx_batches_.push_back(batch);

    OnCompleteRx();
  }

  MOCK_METHOD(void, OnCompleteRx, ());

  void CompleteTx(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request,
                  fdf::Arena& arena, CompleteTxCompleter::Sync& completer) override {
    std::vector<fuchsia_hardware_network_driver::wire::TxResult> batch(request->tx.begin(),
                                                                       request->tx.end());
    tx_batches_.push_back(batch);

    OnCompleteTx();
  }

  MOCK_METHOD(void, OnCompleteTx, ());

  void PortStatusChanged(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
      fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) override {
    FAIL() << "Not supported by the FakeNetDevice";
  }
  void AddPort(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcAddPortRequest* request,
               fdf::Arena& arena, AddPortCompleter::Sync& completer) override {
    []() { FAIL() << "Not supported by the FakeNetDevice"; }();
    completer.buffer(arena).Reply(ZX_ERR_NOT_SUPPORTED);
  }
  void RemovePort(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRemovePortRequest* request,
                  fdf::Arena& arena, RemovePortCompleter::Sync& completer) override {
    FAIL() << "Not supported by the FakeNetDevice";
  }
  void DelegateRxLease(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcDelegateRxLeaseRequest* request,
      fdf::Arena& arena, DelegateRxLeaseCompleter::Sync& completer) override {
    FAIL() << "Not supported by the FakeNetDevice";
  }
  void UpdateRxBufferParams(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcUpdateRxBufferParamsRequest* request,
      fdf::Arena& arena, UpdateRxBufferParamsCompleter::Sync& completer) override {
    FAIL() << "Not supported by the FakeNetDevice";
    completer.buffer(arena).Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
  }
  void RequestRxSpace(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRequestRxSpaceRequest* request,
      fdf::Arena& arena, RequestRxSpaceCompleter::Sync& completer) override {
    FAIL() << "Not supported by the FakeNetDevice";
  }

  std::vector<std::vector<fuchsia_hardware_network_driver::wire::TxResult>> tx_batches_;
  std::vector<std::vector<fuchsia_hardware_network_driver::wire::RxBufferPart>> rx_batches_;
};

class CompletionQueueTest : public gtest::TestLoopFixture {
 public:
  void SetUp() override {
    TestLoopFixture::SetUp();
    client_.Bind(device_.GetClient(), driver_runtime_.StartBackgroundDispatcher()->get());
  }

  void ValidateTxBatches(std::vector<uint32_t> expected) {
    ASSERT_EQ(device_.tx_batches_.size(), expected.size());
    uint32_t buffer_id = kFirstBufferId;
    for (uint32_t i = 0; i < expected.size(); i++) {
      ASSERT_EQ(device_.tx_batches_[i].size(), expected[i]);
      for (const auto& result : device_.tx_batches_[i]) {
        ASSERT_EQ(result.id, buffer_id++);
        ASSERT_EQ(result.status, ZX_OK);
      }
    }
  }

  void ValidateRxBatches(std::vector<uint32_t> expected) {
    ASSERT_EQ(device_.rx_batches_.size(), expected.size());
    uint32_t buffer_id = kFirstBufferId;
    uint32_t buffer_length = kFirstBufferLength;
    for (uint32_t i = 0; i < expected.size(); i++) {
      ASSERT_EQ(device_.rx_batches_[i].size(), expected[i]);
      for (const auto& result : device_.rx_batches_[i]) {
        ASSERT_EQ(result.id, buffer_id++);
        ASSERT_EQ(result.length, buffer_length++);
      }
    }
  }

  fdf_testing::DriverRuntime driver_runtime_;
  FakeNetDevice device_;
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> client_;
};

TEST_F(CompletionQueueTest, TxCompleteFewerThanMaxDepth) {
  HostToGuestCompletionQueue queue(dispatcher(), &client_);
  queue.Complete(kFirstBufferId, ZX_OK);

  // Dispatch loop hasn't run the task yet.
  ASSERT_TRUE(device_.tx_batches_.empty());

  // Expect one CompleteTx call as per the comment below.
  std::latch completed_tx(1);
  EXPECT_CALL(device_, OnCompleteTx).WillOnce([&] { completed_tx.count_down(); });

  RunLoopUntilIdle();

  completed_tx.wait();

  // Single element in a single batch.
  ASSERT_NO_FATAL_FAILURE(ValidateTxBatches({1}));
}

TEST_F(CompletionQueueTest, TxCompleteMoreThanMaxDepth) {
  HostToGuestCompletionQueue queue(dispatcher(), &client_);
  uint32_t buffer_id = kFirstBufferId;
  for (uint32_t i = 0; i < HostToGuestCompletionQueue::kMaxDepth + 1; i++) {
    queue.Complete(buffer_id++, ZX_OK);
  }

  // Dispatch loop hasn't run the task yet.
  ASSERT_TRUE(device_.tx_batches_.empty());

  // Expect two CompleteTx calls as per the comment below.
  std::latch completed_tx(2);
  EXPECT_CALL(device_, OnCompleteTx).Times(2).WillRepeatedly([&] { completed_tx.count_down(); });

  RunLoopUntilIdle();

  completed_tx.wait();

  // Two batches, one full and one with one element.
  ASSERT_NO_FATAL_FAILURE(ValidateTxBatches({HostToGuestCompletionQueue::kMaxDepth, 1}));
}

TEST_F(CompletionQueueTest, TxCompleteMoreThanQueueSize) {
  HostToGuestCompletionQueue queue(dispatcher(), &client_);
  uint32_t buffer_id = kFirstBufferId;
  for (uint32_t i = 0; i < HostToGuestCompletionQueue::kQueueDepth + 3; i++) {
    queue.Complete(buffer_id++, ZX_OK);
  }

  // Dispatch loop hasn't run the task yet.
  ASSERT_TRUE(device_.tx_batches_.empty());

  // Expect six CompleteTx calls as per the comment below.
  std::latch completed_tx(6);
  EXPECT_CALL(device_, OnCompleteTx).Times(6).WillRepeatedly([&] { completed_tx.count_down(); });

  RunLoopUntilIdle();

  // Stick some more completions into the now empty queue.
  for (uint32_t i = 0; i < HostToGuestCompletionQueue::kMaxDepth / 2; i++) {
    queue.Complete(buffer_id++, ZX_OK);
  }

  RunLoopUntilIdle();

  completed_tx.wait();

  // Six batches. The first two are batches from the completion queue, and the next 3 are single
  // element overflows, and the last is another iteration from the completion queue.
  ASSERT_NO_FATAL_FAILURE(ValidateTxBatches({HostToGuestCompletionQueue::kMaxDepth,
                                             HostToGuestCompletionQueue::kMaxDepth, 1, 1, 1,
                                             HostToGuestCompletionQueue::kMaxDepth / 2}));
}

TEST_F(CompletionQueueTest, RxCompleteFewerThanMaxDepth) {
  GuestToHostCompletionQueue queue(kPort, dispatcher(), &client_);
  queue.Complete(kFirstBufferId, kFirstBufferLength);

  // Dispatch loop hasn't run the task yet.
  ASSERT_TRUE(device_.rx_batches_.empty());

  // Expect one CompleteRx call as per the comment below.
  std::latch completed_rx(1);
  EXPECT_CALL(device_, OnCompleteRx).WillOnce([&] { completed_rx.count_down(); });

  RunLoopUntilIdle();

  completed_rx.wait();

  // Single element in a single batch.
  ASSERT_NO_FATAL_FAILURE(ValidateRxBatches({1}));
}

TEST_F(CompletionQueueTest, RxCompleteMoreThanMaxDepth) {
  GuestToHostCompletionQueue queue(kPort, dispatcher(), &client_);
  uint32_t buffer_id = kFirstBufferId;
  uint32_t buffer_length = kFirstBufferLength;
  for (uint32_t i = 0; i < GuestToHostCompletionQueue::kMaxDepth + 1; i++) {
    queue.Complete(buffer_id++, buffer_length++);
  }

  // Expect three CompleteRx calls as per the comment below.
  std::latch completed_rx(3);
  EXPECT_CALL(device_, OnCompleteRx).Times(3).WillRepeatedly([&] { completed_rx.count_down(); });

  RunLoopUntilIdle();

  for (uint32_t i = 0; i < GuestToHostCompletionQueue::kMaxDepth / 2; i++) {
    queue.Complete(buffer_id++, buffer_length++);
  }

  RunLoopUntilIdle();

  completed_rx.wait();

  // Three batches, one full, one with one element, and then the last half full.
  ASSERT_NO_FATAL_FAILURE(ValidateRxBatches(
      {GuestToHostCompletionQueue::kMaxDepth, 1, GuestToHostCompletionQueue::kMaxDepth / 2}));
}

TEST_F(CompletionQueueTest, RxCompleteMoreThanQueueSize) {
  GuestToHostCompletionQueue queue(kPort, dispatcher(), &client_);
  uint32_t buffer_id = kFirstBufferId;
  uint32_t buffer_length = kFirstBufferLength;
  for (uint32_t i = 0; i < GuestToHostCompletionQueue::kQueueDepth + 2; i++) {
    queue.Complete(buffer_id++, buffer_length++);
  }

  // Expect four CompleteRx calls as per the comment below.
  std::latch completed_rx(4);
  EXPECT_CALL(device_, OnCompleteRx).Times(4).WillRepeatedly([&] { completed_rx.count_down(); });

  RunLoopUntilIdle();

  completed_rx.wait();

  // Four batches, two from the completion queue and two single element overflows.
  ASSERT_NO_FATAL_FAILURE(ValidateRxBatches(
      {GuestToHostCompletionQueue::kMaxDepth, GuestToHostCompletionQueue::kMaxDepth, 1, 1}));
}

}  // namespace
