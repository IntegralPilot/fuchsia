// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/mock-mmio/cpp/globally-ordered-region.h>

#include <gtest/gtest.h>

namespace mock_mmio {

namespace {

class GloballyOrderedRegionTest : public ::testing::Test {
 public:
  void TearDown() override { mmio_range_.CheckAllAccessesReplayed(); }

 protected:
  GloballyOrderedRegion mmio_range_{0x4000, GloballyOrderedRegion::Size::k32};
  fdf::MmioBuffer mmio_buffer_{mmio_range_.GetMmioBuffer()};
};

TEST_F(GloballyOrderedRegionTest, NoOperations) {
  GloballyOrderedRegion mmio_range(0x1000, GloballyOrderedRegion::Size::k32);
  mmio_range_.CheckAllAccessesReplayed();
  SUCCEED();
}

TEST_F(GloballyOrderedRegionTest, ConstructorRangeSize) {
  GloballyOrderedRegion mmio_range1(0x1000, GloballyOrderedRegion::Size::k32);
  GloballyOrderedRegion mmio_range2(0x4000, GloballyOrderedRegion::Size::k32);

  fdf::MmioBuffer mmio_range1_buffer = mmio_range1.GetMmioBuffer();
  fdf::MmioBuffer mmio_range2_buffer = mmio_range2.GetMmioBuffer();

  EXPECT_EQ(0x1000u, mmio_range1_buffer.get_size());
  EXPECT_EQ(0x4000u, mmio_range2_buffer.get_size());
}

TEST_F(GloballyOrderedRegionTest, ConstructorDefaultOperationSize) {
  GloballyOrderedRegion mmio_range1(0x1000, GloballyOrderedRegion::Size::k32);
  GloballyOrderedRegion mmio_range2(0x1000, GloballyOrderedRegion::Size::k16);

  fdf::MmioBuffer mmio_range1_buffer = mmio_range1.GetMmioBuffer();
  fdf::MmioBuffer mmio_range2_buffer = mmio_range2.GetMmioBuffer();

  mmio_range1.Expect({.address = 0x100, .value = 0x01});
  mmio_range2.Expect({.address = 0x100, .value = 0x01});

  EXPECT_EQ(0x01u, mmio_range1_buffer.Read32(0x100));
  EXPECT_EQ(0x01u, mmio_range2_buffer.Read16(0x100));
}

TEST_F(GloballyOrderedRegionTest, ReadOnce) {
  mmio_range_.Expect({.address = 0x100, .value = 0x42434445});
  EXPECT_EQ(0x42434445u, mmio_buffer_.Read32(0x100));
}

TEST_F(GloballyOrderedRegionTest, ReadOnceNonDefaultSize) {
  mmio_range_.Expect(
      {.address = 0x100, .value = 0x42434445, .size = GloballyOrderedRegion::Size::k64});
  EXPECT_EQ(0x42434445u, mmio_buffer_.Read64(0x100));
}

TEST_F(GloballyOrderedRegionTest, ReadOnceExplicitSize) {
  mmio_range_.Expect(
      {.address = 0x100, .value = 0x42434445, .size = GloballyOrderedRegion::Size::k32});
  EXPECT_EQ(0x42434445u, mmio_buffer_.Read32(0x100));
}

TEST_F(GloballyOrderedRegionTest, ReadRepeated) {
  mmio_range_.Expect({.address = 0x100, .value = 0x42});
  mmio_range_.Expect({.address = 0x100, .value = 0x43});
  mmio_range_.Expect({.address = 0x100, .value = 0x44});
  mmio_range_.Expect({.address = 0x100, .value = 0x45});

  EXPECT_EQ(0x42u, mmio_buffer_.Read32(0x100));
  EXPECT_EQ(0x43u, mmio_buffer_.Read32(0x100));
  EXPECT_EQ(0x44u, mmio_buffer_.Read32(0x100));
  EXPECT_EQ(0x45u, mmio_buffer_.Read32(0x100));
}

TEST_F(GloballyOrderedRegionTest, ReadRepeatedFromAccessList) {
  mmio_range_.Expect(GloballyOrderedRegion::AccessList({
      {.address = 0x100, .value = 0x42},
      {.address = 0x100, .value = 0x43},
      {.address = 0x100, .value = 0x44},
      {.address = 0x100, .value = 0x45},
  }));

  EXPECT_EQ(0x42u, mmio_buffer_.Read32(0x100));
  EXPECT_EQ(0x43u, mmio_buffer_.Read32(0x100));
  EXPECT_EQ(0x44u, mmio_buffer_.Read32(0x100));
  EXPECT_EQ(0x45u, mmio_buffer_.Read32(0x100));
}

TEST_F(GloballyOrderedRegionTest, ReadVaryingAddressSize) {
  mmio_range_.Expect({.address = 0x100, .value = 0x42});
  mmio_range_.Expect({.address = 0x104, .value = 0x43, .size = GloballyOrderedRegion::Size::k16});
  mmio_range_.Expect({.address = 0x106, .value = 0x44, .size = GloballyOrderedRegion::Size::k8});
  mmio_range_.Expect({.address = 0x108, .value = 0x45, .size = GloballyOrderedRegion::Size::k64});

  EXPECT_EQ(0x42u, mmio_buffer_.Read32(0x100));
  EXPECT_EQ(0x43u, mmio_buffer_.Read16(0x104));
  EXPECT_EQ(0x44u, mmio_buffer_.Read8(0x106));
  EXPECT_EQ(0x45u, mmio_buffer_.Read64(0x108));
}

TEST_F(GloballyOrderedRegionTest, ReadVaryingAddressSizeFromAccessLists) {
  mmio_range_.Expect(GloballyOrderedRegion::AccessList({
      {.address = 0x100, .value = 0x42},
      {.address = 0x104, .value = 0x43, .size = GloballyOrderedRegion::Size::k16},
  }));
  mmio_range_.Expect(GloballyOrderedRegion::AccessList({
      {.address = 0x106, .value = 0x44, .size = GloballyOrderedRegion::Size::k8},
      {.address = 0x108, .value = 0x45, .size = GloballyOrderedRegion::Size::k64},
  }));

  EXPECT_EQ(0x42u, mmio_buffer_.Read32(0x100));
  EXPECT_EQ(0x43u, mmio_buffer_.Read16(0x104));
  EXPECT_EQ(0x44u, mmio_buffer_.Read8(0x106));
  EXPECT_EQ(0x45u, mmio_buffer_.Read64(0x108));
}

TEST_F(GloballyOrderedRegionTest, WriteOnce) {
  mmio_range_.Expect({.address = 0x100, .value = 0x42434445, .write = true});
  mmio_buffer_.Write32(0x42434445, 0x100);
  mmio_range_.CheckAllAccessesReplayed();
  SUCCEED();
}

TEST_F(GloballyOrderedRegionTest, WriteOnceNonDefaultSize) {
  mmio_range_.Expect(
      {.address = 0x100, .value = 0x42434445, .size = GloballyOrderedRegion::Size::k64});
  EXPECT_EQ(0x42434445u, mmio_buffer_.Read64(0x100));
}

TEST_F(GloballyOrderedRegionTest, WriteOnceExplicitSize) {
  mmio_range_.Expect(
      {.address = 0x100, .value = 0x42434445, .size = GloballyOrderedRegion::Size::k32});
  EXPECT_EQ(0x42434445u, mmio_buffer_.Read32(0x100));
}

TEST_F(GloballyOrderedRegionTest, WriteRepeated) {
  mmio_range_.Expect({.address = 0x100, .value = 0x42, .write = true});
  mmio_range_.Expect({.address = 0x100, .value = 0x43, .write = true});
  mmio_range_.Expect({.address = 0x100, .value = 0x44, .write = true});
  mmio_range_.Expect({.address = 0x100, .value = 0x45, .write = true});

  mmio_buffer_.Write32(0x42, 0x100);
  mmio_buffer_.Write32(0x43, 0x100);
  mmio_buffer_.Write32(0x44, 0x100);
  mmio_buffer_.Write32(0x45, 0x100);
}

TEST_F(GloballyOrderedRegionTest, WriteRepeatedFromAccessList) {
  mmio_range_.Expect(
      GloballyOrderedRegion::AccessList({{.address = 0x100, .value = 0x42, .write = true},
                                         {.address = 0x100, .value = 0x43, .write = true},
                                         {.address = 0x100, .value = 0x44, .write = true},
                                         {.address = 0x100, .value = 0x45, .write = true}}));

  mmio_buffer_.Write32(0x42, 0x100);
  mmio_buffer_.Write32(0x43, 0x100);
  mmio_buffer_.Write32(0x44, 0x100);
  mmio_buffer_.Write32(0x45, 0x100);
}

TEST_F(GloballyOrderedRegionTest, WriteVaryingAddressSize) {
  mmio_range_.Expect({.address = 0x100, .value = 0x42, .write = true});
  mmio_range_.Expect(
      {.address = 0x104, .value = 0x43, .write = true, .size = GloballyOrderedRegion::Size::k16});
  mmio_range_.Expect(
      {.address = 0x106, .value = 0x44, .write = true, .size = GloballyOrderedRegion::Size::k8});
  mmio_range_.Expect(
      {.address = 0x108, .value = 0x45, .write = true, .size = GloballyOrderedRegion::Size::k64});

  mmio_buffer_.Write32(0x42, 0x100);
  mmio_buffer_.Write16(0x43, 0x104);
  mmio_buffer_.Write8(0x44, 0x106);
  mmio_buffer_.Write64(0x45, 0x108);
}

TEST_F(GloballyOrderedRegionTest, WriteVaryingAddressSizeFromAccessLists) {
  mmio_range_.Expect(GloballyOrderedRegion::AccessList({
      {.address = 0x100, .value = 0x42, .write = true},
      {.address = 0x104, .value = 0x43, .write = true, .size = GloballyOrderedRegion::Size::k16},
  }));
  mmio_range_.Expect(GloballyOrderedRegion::AccessList({
      {.address = 0x106, .value = 0x44, .write = true, .size = GloballyOrderedRegion::Size::k8},
      {.address = 0x108, .value = 0x45, .write = true, .size = GloballyOrderedRegion::Size::k64},
  }));

  mmio_buffer_.Write32(0x42, 0x100);
  mmio_buffer_.Write16(0x43, 0x104);
  mmio_buffer_.Write8(0x44, 0x106);
  mmio_buffer_.Write64(0x45, 0x108);
}

TEST_F(GloballyOrderedRegionTest, InterleavedReadAndWrite) {
  mmio_range_.Expect({.address = 0x100, .value = 0x42, .write = true});
  mmio_range_.Expect({.address = 0x104, .value = 0x43, .size = GloballyOrderedRegion::Size::k16});
  mmio_range_.Expect(
      {.address = 0x106, .value = 0x44, .write = true, .size = GloballyOrderedRegion::Size::k8});
  mmio_range_.Expect({.address = 0x108, .value = 0x45, .size = GloballyOrderedRegion::Size::k64});

  mmio_buffer_.Write32(0x42, 0x100);
  EXPECT_EQ(0x43u, mmio_buffer_.Read16(0x104));
  mmio_buffer_.Write8(0x44, 0x106);
  EXPECT_EQ(0x45u, mmio_buffer_.Read64(0x108));
}

TEST_F(GloballyOrderedRegionTest, InterleavedReadAndWriteFromAccessList) {
  mmio_range_.Expect(GloballyOrderedRegion::AccessList({
      {.address = 0x100, .value = 0x42, .write = true},
      {.address = 0x104, .value = 0x43, .size = GloballyOrderedRegion::Size::k16},
      {.address = 0x106, .value = 0x44, .write = true, .size = GloballyOrderedRegion::Size::k8},
      {.address = 0x108, .value = 0x45, .size = GloballyOrderedRegion::Size::k64},
  }));

  mmio_buffer_.Write32(0x42, 0x100);
  EXPECT_EQ(0x43u, mmio_buffer_.Read16(0x104));
  mmio_buffer_.Write8(0x44, 0x106);
  EXPECT_EQ(0x45u, mmio_buffer_.Read64(0x108));
}

}  // namespace

}  // namespace mock_mmio
