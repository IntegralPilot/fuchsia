// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../buffer.h"

#include <barrier>
#include <thread>

#include <gtest/gtest.h>

namespace {
TEST(BufferTest, Alloc) {
  std::vector<uint8_t> data(size_t{1024});
  DurableBuffer b;
  b.Set(data);
  ASSERT_EQ(b.MaxSize(), size_t{1024});
  uint64_t* record = b.AllocRecord(16);
  ASSERT_EQ(record, reinterpret_cast<uint64_t*>(data.data()));
  ASSERT_EQ(b.BytesAllocated(), size_t{16});

  uint64_t* record2 = b.AllocRecord(32);
  ASSERT_EQ(record2, reinterpret_cast<uint64_t*>(data.data() + 16));
  ASSERT_EQ(b.BytesAllocated(), size_t{16 + 32});

  b.Reset();
  ASSERT_EQ(b.BytesAllocated(), size_t{0});
}

TEST(BufferTest, Overflow) {
  std::vector<uint8_t> data(size_t{128});
  DurableBuffer b;
  b.Set(data);
  uint64_t* record1 = b.AllocRecord(64);
  uint64_t* record2 = b.AllocRecord(56);
  uint64_t* record3 = b.AllocRecord(16);
  ASSERT_NE(nullptr, record1);
  ASSERT_NE(nullptr, record2);
  ASSERT_EQ(nullptr, record3);
  ASSERT_EQ(b.BytesAllocated(), size_t{120});
  uint64_t* record4 = b.AllocRecord(8);
  ASSERT_NE(nullptr, record4);
  ASSERT_EQ(b.BytesAllocated(), size_t{128});
}

TEST(BufferTest, Concurrent) {
  size_t buffer_size = size_t{1024} * 1024;
  std::vector<uint8_t> data(buffer_size);
  DurableBuffer b;
  b.Set(data);
  std::barrier barrier(3, []() {});

  auto T3 = [&]() {
    barrier.arrive_and_wait();
    const uint64_t data[3] = {0x3333'3333'3333'3333, 0x3333'3333'3333'3333, 0x3333'3333'3333'3333};
    while (uint64_t* record = b.AllocRecord(sizeof(data))) {
      if (record == nullptr) {
        break;
      }
      memcpy(record, data, sizeof(data));
    }
  };

  auto T2 = [&]() {
    barrier.arrive_and_wait();
    const uint64_t data[2] = {0x2222'2222'2222'2222, 0x2222'2222'2222'2222};
    while (uint64_t* record = b.AllocRecord(sizeof(data))) {
      if (record == nullptr) {
        break;
      }
      memcpy(record, data, sizeof(data));
    }
  };

  auto T1 = [&]() {
    barrier.arrive_and_wait();
    const uint64_t data[1] = {0x1111'1111'1111'1111};
    while (uint64_t* record = b.AllocRecord(sizeof(data))) {
      if (record == nullptr) {
        break;
      }
      memcpy(record, data, sizeof(data));
    }
  };

  std::thread t1(T1);
  std::thread t2(T2);
  std::thread t3(T3);

  t1.join();
  t2.join();
  t3.join();
  // Verify the buffer
  for (size_t i = 0; i < buffer_size; i++) {
    switch (data[i]) {
      case 0x11: {
        // The next 7 bytes should also be 0x11
        for (size_t j = 1; j < 7; j++) {
          ASSERT_EQ(data[i + j], 0x11);
        }
        i += 7;
        break;
      }
      case 0x22: {
        // The next 15 bytes should also be 0x22
        for (size_t j = 1; j < 15; j++) {
          ASSERT_EQ(data[i + j], 0x22);
        }
        i += 15;
        break;
      }
      case 0x33: {
        // The next 23 bytes should also be 0x33
        for (size_t j = 1; j < 23; j++) {
          ASSERT_EQ(data[i + j], 0x33);
        }
        i += 23;
        break;
      }
      default:
        // Invalid data
        ASSERT_EQ(true, false);
    }
  }
}

}  // namespace
