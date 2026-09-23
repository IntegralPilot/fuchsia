// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fbl/packed_pointer.h>
#include <zxtest/zxtest.h>

namespace {

struct alignas(16) TestStruct {
  int x;
};

TEST(PackedPointerTest, DefaultConstructor) {
  fbl::PackedPointer<TestStruct, 4> ptr;
  EXPECT_NULL(ptr.ptr());
  EXPECT_EQ(0u, ptr.data());
  EXPECT_FALSE(ptr);
}

TEST(PackedPointerTest, NullptrConstructor) {
  fbl::PackedPointer<TestStruct, 4> ptr(nullptr);
  EXPECT_NULL(ptr.ptr());
  EXPECT_EQ(0u, ptr.data());
  EXPECT_FALSE(ptr);
}

TEST(PackedPointerTest, PointerConstructor) {
  TestStruct val;
  fbl::PackedPointer<TestStruct, 4> ptr(&val);
  EXPECT_EQ(&val, ptr.ptr());
  EXPECT_EQ(0u, ptr.data());
  EXPECT_TRUE(ptr);
}

TEST(PackedPointerTest, PointerAndDataConstructor) {
  TestStruct val;
  fbl::PackedPointer<TestStruct, 4> ptr(&val, 0xA);
  EXPECT_EQ(&val, ptr.ptr());
  EXPECT_EQ(0xAu, ptr.data());
  EXPECT_TRUE(ptr);
}

TEST(PackedPointerTest, NullptrAndDataConstructor) {
  fbl::PackedPointer<TestStruct, 4> ptr(nullptr, 0xB);
  EXPECT_NULL(ptr.ptr());
  EXPECT_EQ(0xBu, ptr.data());
  EXPECT_FALSE(ptr);
}

TEST(PackedPointerTest, SetPtr) {
  TestStruct val1, val2;
  fbl::PackedPointer<TestStruct, 4> ptr(&val1, 0x5);
  ptr.set_ptr(&val2);
  EXPECT_EQ(&val2, ptr.ptr());
  EXPECT_EQ(0x5u, ptr.data());
}

TEST(PackedPointerTest, SetData) {
  TestStruct val;
  fbl::PackedPointer<TestStruct, 4> ptr(&val, 0x5);
  ptr.set_data(0xC);
  EXPECT_EQ(&val, ptr.ptr());
  EXPECT_EQ(0xCu, ptr.data());
}

TEST(PackedPointerTest, Reset) {
  TestStruct val;
  fbl::PackedPointer<TestStruct, 4> ptr(&val, 0x5);
  ptr.reset();
  EXPECT_NULL(ptr.ptr());
  EXPECT_EQ(0u, ptr.data());
  EXPECT_FALSE(ptr);
}

TEST(PackedPointerTest, PointerSemantics) {
  TestStruct val;
  val.x = 42;
  fbl::PackedPointer<TestStruct, 4> ptr(&val);
  EXPECT_EQ(42, (*ptr).x);
  EXPECT_EQ(42, ptr->x);
}

TEST(PackedPointerTest, Comparisons) {
  TestStruct val1, val2;
  fbl::PackedPointer<TestStruct, 4> ptr1(&val1, 0x1);
  fbl::PackedPointer<TestStruct, 4> ptr1_again(&val1, 0x1);
  fbl::PackedPointer<TestStruct, 4> ptr1_diff_data(&val1, 0x2);
  fbl::PackedPointer<TestStruct, 4> ptr2(&val2, 0x1);

  EXPECT_TRUE(ptr1 == ptr1_again);
  EXPECT_FALSE(ptr1 != ptr1_again);

  EXPECT_TRUE(ptr1 != ptr1_diff_data);
  EXPECT_FALSE(ptr1 == ptr1_diff_data);

  EXPECT_TRUE(ptr1 != ptr2);
  EXPECT_FALSE(ptr1 == ptr2);

  fbl::PackedPointer<TestStruct, 4> null_ptr;
  EXPECT_TRUE(null_ptr == nullptr);
  EXPECT_FALSE(null_ptr != nullptr);
  EXPECT_TRUE(ptr1 != nullptr);
  EXPECT_FALSE(ptr1 == nullptr);
}

TEST(PackedPointerTest, DisabledAlignmentCheck) {
  struct alignas(16) AlignedStruct {
    char c;
  };
  static_assert(alignof(AlignedStruct) == 16);

  // This would fail static_assert if kCheckAlignment was true because we request 5 bits (32-byte
  // alignment).
  fbl::PackedPointer<AlignedStruct, 5, false> ptr;
  EXPECT_NULL(ptr.ptr());
  EXPECT_EQ(0u, ptr.data());

  alignas(32) char val_buffer[32];
  AlignedStruct* val_ptr = reinterpret_cast<AlignedStruct*>(val_buffer);

  // We don't verify the pointer value here because alignas on the stack
  // might not be respected in all test environments, but we verify
  // that the data is preserved.
  ptr.set_ptr(val_ptr);
  ptr.set_data(0x1F);
  EXPECT_EQ(0x1Fu, ptr.data());
}

}  // namespace
