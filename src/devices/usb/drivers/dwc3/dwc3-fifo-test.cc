// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/dwc3/dwc3-fifo.h"

#include <lib/driver/fake-bti/cpp/fake-bti.h>

#include <gtest/gtest.h>

namespace dwc3 {

template <typename T>
class FifoTest : public testing::TestWithParam<bool> {
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

 protected:
  class TFifo : public Fifo<T> {
   public:
    using Fifo<T>::first_;
    using Fifo<T>::read_;
    using Fifo<T>::write_;
  };

  zx::bti bti_;
  TFifo fifo_;
};

using FifoTestU32 = FifoTest<uint32_t>;
INSTANTIATE_TEST_SUITE_P(FifoTestU32Cases, FifoTestU32, testing::Bool());

TEST_P(FifoTestU32, WriteAndRead) {
  fifo_.Write(fifo_.write_);
  fifo_.Advance(fifo_.write_);

  std::vector<uint32_t> values = fifo_.Read(fifo_.read_, 1);
  ASSERT_EQ(values.size(), 1u);
  fifo_.Advance(fifo_.read_);

  ASSERT_EQ(fifo_.write_, fifo_.read_);
}

TEST_P(FifoTestU32, Wrap) {
  const size_t size = kBufferSize / sizeof(uint32_t);
  for (size_t i = 0; i < size; i++) {
    fifo_.Advance(fifo_.write_);
  }
  ASSERT_EQ(fifo_.write_, fifo_.first_);
}

TEST_P(FifoTestU32, MultiElementWriteAndRead) {
  constexpr size_t kCount = 5;
  fifo_.Write(fifo_.write_, kCount);
  fifo_.Advance(fifo_.write_, kCount);

  std::vector<uint32_t> values = fifo_.Read(fifo_.read_, kCount);
  ASSERT_EQ(values.size(), kCount);
  fifo_.Advance(fifo_.read_, kCount);

  ASSERT_EQ(fifo_.write_, fifo_.read_);
}

TEST_P(FifoTestU32, WrappingWriteAndRead) {
  const size_t size = kBufferSize / sizeof(uint32_t);
  // Advance to near the end of the buffer.
  fifo_.Advance(fifo_.write_, size - 2);
  fifo_.read_ = fifo_.write_;

  constexpr size_t kCount = 4;
  fifo_.Write(fifo_.write_, kCount);
  fifo_.Advance(fifo_.write_, kCount);

  std::vector<uint32_t> values = fifo_.Read(fifo_.read_, kCount);
  ASSERT_EQ(values.size(), kCount);
  fifo_.Advance(fifo_.read_, kCount);

  ASSERT_EQ(fifo_.write_, fifo_.read_);
}

TEST_P(FifoTestU32, ReInitTest) {
  fifo_.Write(fifo_.write_);
  fifo_.Advance(fifo_.write_);
  ASSERT_NE(fifo_.write_, fifo_.first_);

  fifo_.Release();
  ASSERT_TRUE(fifo_.Init(bti_, GetParam()).is_ok());

  ASSERT_EQ(fifo_.write_, fifo_.first_);
  ASSERT_EQ(fifo_.read_, fifo_.first_);
}

using FifoTestU8 = FifoTest<uint8_t>;
INSTANTIATE_TEST_SUITE_P(FifoTestU8Cases, FifoTestU8, testing::Bool());

TEST_P(FifoTestU8, WriteAndRead) {
  fifo_.Write(fifo_.write_);
  fifo_.Advance(fifo_.write_);

  std::vector<uint8_t> values = fifo_.Read(fifo_.read_, 1);
  ASSERT_EQ(values.size(), 1u);
  fifo_.Advance(fifo_.read_);

  ASSERT_EQ(fifo_.write_, fifo_.read_);
}

}  // namespace dwc3
