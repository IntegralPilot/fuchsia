// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/dwc3/dwc3-trb-fifo.h"

#include <lib/driver/fake-bti/cpp/fake-bti.h>

#include <gtest/gtest.h>

namespace dwc3 {

class TrbFifoTest : public testing::TestWithParam<bool> {
 public:
  void SetUp() override {
    zx::result bti = fake_bti::CreateFakeBti();
    ASSERT_TRUE(bti.is_ok());
    bti_ = std::move(*bti);

    ASSERT_TRUE(fifo_.Init(bti_, GetParam()).is_ok());
  }

  void TearDown() override {
    fifo_.Release();
    bti_.reset();
  }

  void CheckLinkTRB() {
    zx_paddr_t first_phys = fifo_.GetPhys(fifo_.first_);
    EXPECT_EQ(fifo_.last_->ptr_low, (uint32_t)first_phys);
    EXPECT_EQ(fifo_.last_->ptr_high, (uint32_t)(first_phys >> 32));
    EXPECT_EQ(fifo_.last_->status, 0u);
    EXPECT_EQ(static_cast<uint32_t>(TRB_TRBCTL_LINK | TRB_HWO | TRB_CHN), fifo_.last_->control);
  }

 protected:
  class TTrbFifo : public TrbFifo {
   public:
    using TrbFifo::first_;
    using TrbFifo::GetPhys;
    using TrbFifo::last_;
    using TrbFifo::read_;
    using TrbFifo::write_;
  };

  zx::bti bti_;
  TTrbFifo fifo_;
};

INSTANTIATE_TEST_SUITE_P(TrbFifoTestCases, TrbFifoTest, testing::Bool());

TEST_P(TrbFifoTest, Init) { CheckLinkTRB(); }

TEST_P(TrbFifoTest, WriteAndRead) {
  dwc3_trb_t* trb = fifo_.write_;
  trb->ptr_low = 0x1234;
  trb->ptr_high = 0x5678;
  trb->status = 0xabcd;
  trb->control = 0xef;
  fifo_.AdvanceWrite();

  auto read_trb_res = fifo_.ReadOne();
  ASSERT_TRUE(read_trb_res.is_ok());
  dwc3_trb_t read_trb = *read_trb_res;
  EXPECT_EQ(read_trb.ptr_low, 0x1234u);
  EXPECT_EQ(read_trb.ptr_high, 0x5678u);
  EXPECT_EQ(read_trb.status, 0xabcdu);
  EXPECT_EQ(read_trb.control, 0xefu);

  fifo_.AdvanceRead();
  EXPECT_EQ(fifo_.read_, fifo_.write_);
}

TEST_P(TrbFifoTest, Wrap) {
  const size_t size = kBufferSize / sizeof(dwc3_trb_t);
  for (size_t i = 0; i < size - 1; i++) {
    fifo_.AdvanceWrite();
  }
  EXPECT_EQ(fifo_.write_, fifo_.first_);
}

TEST_P(TrbFifoTest, AvailableSlots) {
  const size_t size = kBufferSize / sizeof(dwc3_trb_t);
  EXPECT_EQ(fifo_.AvailableSlots(), size - 2);

  fifo_.AdvanceWrite();
  EXPECT_EQ(fifo_.AvailableSlots(), size - 3);

  for (size_t i = 0; i < size - 3; i++) {
    fifo_.AdvanceWrite();
  }
  EXPECT_EQ(fifo_.AvailableSlots(), 0u);

  fifo_.AdvanceRead();
  EXPECT_EQ(fifo_.AvailableSlots(), 1u);
}

TEST_P(TrbFifoTest, ReInitTest) {
  fifo_.AdvanceWrite();
  ASSERT_NE(fifo_.write_, fifo_.first_);

  fifo_.Release();
  ASSERT_TRUE(fifo_.Init(bti_, GetParam()).is_ok());

  ASSERT_EQ(fifo_.write_, fifo_.first_);
  ASSERT_EQ(fifo_.read_, fifo_.first_);

  CheckLinkTRB();
}

}  // namespace dwc3
