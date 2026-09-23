// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/compiler.h>

#include <cstdint>

#include <fbl/unaligned.h>
#include <zxtest/zxtest.h>

namespace {

TEST(UnalignedLoad, Basic) {
  int32_t aligned_source = 3;
  int32_t dest = fbl::UnalignedLoad<int32_t>(&aligned_source);
  EXPECT_EQ(dest, aligned_source);

  char container[] = {0x01, 0x11, 0x21, 0x31, 0x41};
  void* unaligned_int = &container[1];
  dest = fbl::UnalignedLoad<int32_t>(unaligned_int);
  EXPECT_EQ(dest, 0x41312111);
}

TEST(UnalignedStore, Basic) {
  int32_t value = 3;
  int32_t aligned_dest;
  fbl::UnalignedStore(reinterpret_cast<void*>(&aligned_dest), value);
  EXPECT_EQ(aligned_dest, 3);

  char container[] = {0, 0, 0, 0, 0};
  void* unaligned_dest = reinterpret_cast<void*>(&container[1]);
  fbl::UnalignedStore(unaligned_dest, 0x11213141);
  EXPECT_EQ(container[1], 0x41);
  EXPECT_EQ(container[2], 0x31);
  EXPECT_EQ(container[3], 0x21);
  EXPECT_EQ(container[4], 0x11);
}

struct PackedStruct {
  uint8_t padding;
  int64_t unaligned_value;
} __PACKED;

TEST(UnalignedLoad, PackedStruct) {
  PackedStruct ps = {
      .padding = 0,
      .unaligned_value = 42,
  };

  int64_t loaded = fbl::UnalignedLoad<int64_t>(&ps.unaligned_value);
  EXPECT_EQ(loaded, 42);
}

TEST(UnalignedStore, PackedStruct) {
  PackedStruct ps = {
      .padding = 0,
      .unaligned_value = 0,
  };

  fbl::UnalignedStore(&ps.unaligned_value, int64_t{0x0102030405060708});

  EXPECT_EQ(0x08, reinterpret_cast<const char*>(&ps.unaligned_value)[0]);
  EXPECT_EQ(0x07, reinterpret_cast<const char*>(&ps.unaligned_value)[1]);
  EXPECT_EQ(0x06, reinterpret_cast<const char*>(&ps.unaligned_value)[2]);
  EXPECT_EQ(0x05, reinterpret_cast<const char*>(&ps.unaligned_value)[3]);
  EXPECT_EQ(0x04, reinterpret_cast<const char*>(&ps.unaligned_value)[4]);
  EXPECT_EQ(0x03, reinterpret_cast<const char*>(&ps.unaligned_value)[5]);
  EXPECT_EQ(0x02, reinterpret_cast<const char*>(&ps.unaligned_value)[6]);
  EXPECT_EQ(0x01, reinterpret_cast<const char*>(&ps.unaligned_value)[7]);
}

struct NotDefaultConstructable {
  NotDefaultConstructable() = delete;
  explicit NotDefaultConstructable(int32_t value_) : value(value_) {}

  int32_t value;
};

TEST(UnalignedLoad, NotDefaultConstructable) {
  NotDefaultConstructable source(42);
  NotDefaultConstructable loaded = fbl::UnalignedLoad<NotDefaultConstructable>(&source);
  EXPECT_EQ(loaded.value, 42);
}

}  // anonymous namespace
