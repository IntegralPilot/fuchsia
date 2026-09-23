// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/xhci/xhci-transfer-ring.h"

#include <fake-dma-buffer/fake-dma-buffer.h>

#include "src/devices/usb/drivers/xhci/tests/test-env.h"
#include "src/devices/usb/drivers/xhci/xhci-event-ring.h"

namespace usb_xhci {

class TransferRingHarness : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_TRUE(driver_test()
                    .StartDriverWithCustomStartArgs([&](fdf::DriverStartArgs& args) {
                      xhci_config::Config fake_config;
                      fake_config.enable_suspend() = false;
                      args.config(fake_config.ToVmo());
                    })
                    .is_ok());
    ASSERT_OK(driver_test().driver()->TestInit(this));
  }

  void TearDown() override { ASSERT_TRUE(driver_test().StopDriver().is_ok()); }

  fdf_testing::ForegroundDriverTest<EmptyTestConfig>& driver_test() { return driver_test_; }
  TransferRing* ring() { return ring_; }
  void SetRing(TransferRing* ring) { ring_ = ring; }
  EventRing& event_ring() { return event_ring_; }

 private:
  fdf_testing::ForegroundDriverTest<EmptyTestConfig> driver_test_;
  EventRing event_ring_;
  TransferRing* ring_;
};

// UsbXhci Methods
zx::result<> UsbXhci::Init(std::unique_ptr<dma_buffer::BufferFactory> buffer_factory) {
  buffer_factory_ = std::move(buffer_factory);

  fbl::AllocChecker ac;
  interrupters_ = fbl::MakeArray<Interrupter>(&ac, 1);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  max_slots_ = 32;
  device_state_ = fbl::MakeArray<fbl::RefPtr<DeviceState>>(&ac, max_slots_);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  for (size_t i = 0; i < max_slots_; i++) {
    device_state_[i] = fbl::MakeRefCounted<DeviceState>(static_cast<uint32_t>(i), this);
    fbl::AutoLock l(&device_state_[i]->transaction_lock());
    for (size_t c = 0; c < max_slots_; c++) {
      zx_status_t status = device_state_[i]->InitEndpoint(
          static_cast<uint8_t>(c), &GetTestHarness<TransferRingHarness>()->event_ring(), nullptr);
      if (status != ZX_OK) {
        return zx::error(status);
      }
    }
  }

  fbl::AutoLock l(&device_state_[0]->transaction_lock());
  GetTestHarness<TransferRingHarness>()->SetRing(&device_state_[0]->GetEndpoint(0).transfer_ring());
  return zx::ok();
}

void UsbXhci::UsbHciRequestQueue(usb_request_t* usb_request,
                                 const usb_request_complete_callback_t* complete_cb) {}

zx_status_t UsbXhci::UsbHciCancelAll(uint32_t device_id, uint8_t ep_address) {
  return ZX_ERR_NOT_SUPPORTED;
}

// EventRing Methods
zx_status_t EventRing::AddSegmentIfNone() { return ZX_OK; }

void EventRing::RemovePressure() {}

TEST_F(TransferRingHarness, EmptyShortTransferTest) {
  auto ring = this->ring();
  ASSERT_EQ(ring->HandleShortPacket(nullptr, 0, nullptr), ZX_ERR_IO);
}

TEST_F(TransferRingHarness, CorruptedTransferRingShortTransferTest) {
  auto ring = this->ring();
  Normal trb;
  {
    auto context = ring->AllocateContext();
    ASSERT_OK(ring->AddTRB(trb, std::move(context)));
  }
  ASSERT_EQ(ring->HandleShortPacket(nullptr, 0, nullptr), ZX_ERR_IO);
  ring->TakePendingTRBs();
}

TEST_F(TransferRingHarness, MultiPageShortTransferTest) {
  constexpr size_t kNumTrbs = 510;
  constexpr size_t kTrbLength = 20;
  constexpr size_t kShortLength = 4;

  auto ring = this->ring();
  TRB* first = nullptr;
  TRB* last;
  for (size_t i = 0; i < kNumTrbs; i++) {
    TRB* ptr;
    ASSERT_OK(ring->AllocateTRB(&ptr, nullptr));
    if (first == nullptr) {
      first = ptr;
    }
    Control::FromTRB(ptr).set_Type(Control::Normal).ToTrb(ptr);
    static_cast<Normal*>(ptr)->set_LENGTH(kTrbLength);
    last = ptr;
  }

  ASSERT_OK(ring->AssignContext(last, ring->AllocateContext(), first));

  TRB* last_trb;
  ASSERT_OK(ring->HandleShortPacket(last - 1, kShortLength, &last_trb));
  EXPECT_EQ(last_trb, last);

  std::unique_ptr<TRBContext> context;
  ASSERT_OK(ring->CompleteTRB(last, &context));
  EXPECT_EQ(context->short_transfer_len.value(), ((kNumTrbs - 1) * kTrbLength) - kShortLength);
  EXPECT_EQ(context->first_trb, first);
  EXPECT_EQ(context->trb, last);
}

TEST_F(TransferRingHarness, SetStall) {
  auto ring = this->ring();
  ASSERT_FALSE(ring->stalled());
  ring->set_stall(true);
  ASSERT_TRUE(ring->stalled());
  ring->set_stall(false);
  ASSERT_FALSE(ring->stalled());
}

TEST_F(TransferRingHarness, AllocateContiguousFailsIfNotEnoughContiguousPhysicalMemoryExists) {
  constexpr auto kOverAllocateAmount = 9001;
  auto ring = this->ring();
  ASSERT_EQ(ring->AllocateContiguous(kOverAllocateAmount).error_value(), ZX_ERR_NO_MEMORY);
}

TEST_F(TransferRingHarness, AllocateContiguousAllocatesContiguousBlocks) {
  auto ring = this->ring();
  constexpr size_t kContiguousCount = 42;
  constexpr size_t kIterationCount = 512;
  for (size_t i = 0; i < kIterationCount; i++) {
    auto result = ring->AllocateContiguous(kContiguousCount);
    ASSERT_TRUE(result.is_ok());
    ASSERT_EQ(result->trbs.size(), kContiguousCount);
    auto trb_start = result->trbs.data();
    for (size_t c = 0; c < kContiguousCount; c++) {
      ASSERT_NE(Control::FromTRB(trb_start + c).Type(), Control::Link);
    }
  }
}

TEST_F(TransferRingHarness, CanHandleConsecutiveLinks) {
  auto ring = this->ring();
  const size_t trb_per_segment = zx_system_get_page_size() / sizeof(TRB);
  // 1 TRB is the link TRB, and TransferRing::AllocInternal() will allocate if there's not 2
  // available TRBs.
  const size_t trb_per_segment_no_alloc = trb_per_segment - 3;
  std::deque<TRB*> pending_trbs;
  // Fill up a segment.
  for (size_t i = 0; i < trb_per_segment_no_alloc; i++) {
    auto context = ring->AllocateContext();
    TRBContext* ref = context.get();
    ASSERT_OK(ring->AddTRB(TRB(), std::move(context)));

    pending_trbs.emplace_back(ref->trb);
  }

  // Move the dequeue pointer forward two steps, to TRB 2.
  for (size_t i = 0; i < 3; i++) {
    std::unique_ptr<TRBContext> ctx;
    ring->CompleteTRB(pending_trbs.front(), &ctx);
    pending_trbs.pop_front();
  }

  // Finish filling up the segment. This will allocate a new link TRB (TRB 0) and we'll start
  // filling up a new segment.
  for (size_t i = 0; i < 4; i++) {
    auto context = ring->AllocateContext();
    TRBContext* ref = context.get();
    ASSERT_OK(ring->AddTRB(TRB(), std::move(context)));

    pending_trbs.emplace_back(ref->trb);
  }

  // Move the dequeue pointer forward again - to TRB 3.
  for (size_t i = 0; i < 1; i++) {
    std::unique_ptr<TRBContext> ctx;
    ring->CompleteTRB(pending_trbs.front(), &ctx);
    pending_trbs.pop_front();
  }

  // This will allocate a new link TRB at 1.
  for (size_t i = 0; i < trb_per_segment_no_alloc; i++) {
    auto context = ring->AllocateContext();
    TRBContext* ref = context.get();
    ASSERT_OK(ring->AddTRB(TRB(), std::move(context)));

    pending_trbs.emplace_back(ref->trb);
  }

  // At this stage, we have 3 TRB segments.
  // Segment 0 looks like this:
  // 0 // link to seg#1,0
  // 1 // link to seg#2,0
  // ...
  // 255 // link to seg#0,0
  //
  // Segment 1 looks like this:
  // 0
  // ...
  // 255 // link to seg#0,1
  //
  // Segment 2 looks like this:
  // 0
  // ...
  // 255 // link to seg#1,2
  //
  // Notice that there are two consecutive links here - one between seg#1,255 -> seg#0,1 and then
  // seg#0,1 -> seg#2,0.

  // Clean up all the pending TRBs.
  while (!pending_trbs.empty()) {
    std::unique_ptr<TRBContext> ctx;
    ASSERT_OK(ring->CompleteTRB(pending_trbs.front(), &ctx));
    pending_trbs.pop_front();
  }

  // Move through, allocating and deallocating TRBs.
  // This will eventually hit the consecutive links.
  for (size_t i = 0; i < 3 * trb_per_segment; i++) {
    auto context = ring->AllocateContext();
    TRBContext* ref = context.get();
    ASSERT_OK(ring->AddTRB(TRB(), std::move(context)));
    std::unique_ptr<TRBContext> ctx;
    ASSERT_OK(ring->CompleteTRB(ref->trb, &ctx));
  }
}

TEST_F(TransferRingHarness, Peek) {
  auto ring = this->ring();
  TRB* trb;
  ring->AllocateTRB(&trb, nullptr);
  auto result = ring->PeekCommandRingControlRegister(0);
  ASSERT_EQ(trb + 1, ring->PhysToVirt(result->PTR()));
  ASSERT_TRUE(result->RCS());
}

TEST_F(TransferRingHarness, First) {
  ContiguousTRBInfo info;
  TRB a;
  TRB b;
  info.trbs = cpp20::span(&a, 1);
  ASSERT_EQ(info.first().data(), &a);
  info.nop = cpp20::span(&b, 1);
  ASSERT_EQ(info.first().data(), &b);
}

}  // namespace usb_xhci
