// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "../rolling_buffer.h"

#include <lib/zx/fifo.h>

#include <algorithm>
#include <barrier>
#include <iostream>
#include <thread>
#include <vector>

#include <gtest/gtest.h>

namespace {

void print_buff(size_t idx, std::span<uint8_t> data) {
  size_t begin = idx & ~size_t{0x7};
  if (begin > 256) {
    begin -= 256;
  } else {
    begin = 0;
  }
  size_t end = begin + std::min(size_t{512}, data.size());
  for (size_t i = begin; i < end; i++) {
    fprintf(stderr, "%02x", data[i]);
    if (i % 8 == 7) {
      fprintf(stderr, ": %lx\n", i - 7);
    }
  }
}

bool verify_buffer(std::span<uint8_t> data) {
  for (size_t i = 0; i < data.size(); i++) {
    switch (data[i]) {
      case 0x11: {
        // The next 7 bytes should also be 0x11
        for (size_t j = 1; j < 7; j++) {
          if (data[i + j] != 0x11) {
            print_buff(i + j, data);
            fprintf(stderr, "ERROR AT OFFSET %lx\n", i + j);
            return false;
          }
        }
        i += 7;
        break;
      }
      case 0x22: {
        // The next 15 bytes should also be 0x22
        for (size_t j = 1; j < 15; j++) {
          if (data[i + j] != 0x22) {
            print_buff(i + j, data);
            fprintf(stderr, "ERROR AT OFFSET %lx\n", i + j);
            return false;
          }
        }
        i += 15;
        break;
      }
      case 0x33: {
        // The next 23 bytes should also be 0x33
        for (size_t j = 1; j < 23; j++) {
          if (data[i + j] != 0x33) {
            print_buff(i + j, data);
            fprintf(stderr, "ERROR AT OFFSET %lx\n", i + j);
            return false;
          }
        }
        i += 23;
        break;
      }
      default:
        // Invalid data
        print_buff(i, data);
        fprintf(stderr, "ERROR AT OFFSET %lx\n", i);
        return false;
    }
  }
  return true;
}

constexpr size_t kTestBufferSize = 128;

TEST(RollingBufferTest, InitialState) {
  RollingBuffer buffer;
  std::vector<uint8_t> backing_buffer1(kTestBufferSize);
  std::vector<uint8_t> backing_buffer2(kTestBufferSize);
  buffer.SetDoubleBuffers(std::span(backing_buffer1), std::span(backing_buffer2));

  EXPECT_EQ(buffer.BytesAllocated(), size_t{0});
  EXPECT_EQ(buffer.BufferSize(BufferNumber::kZero), kTestBufferSize);
  EXPECT_EQ(buffer.BufferSize(BufferNumber::kOne), kTestBufferSize);
  EXPECT_EQ(buffer.MaxBufferSize(), kTestBufferSize * 2);

  RollingBufferState state = buffer.State();
  EXPECT_EQ(state.GetBufferOffset(), uint32_t{0});
  EXPECT_EQ(state.GetWrappedCount(), uint32_t{0});
  EXPECT_EQ(state.GetBufferNumber(), BufferNumber::kZero);

  EXPECT_FALSE(state.IsBufferFilled(BufferNumber::kZero));
  EXPECT_FALSE(state.IsBufferFilled(BufferNumber::kOne));
}

TEST(RollingBufferTest, AllocRecordSimple) {
  RollingBuffer buffer;
  std::vector<uint8_t> backing_buffer1(kTestBufferSize);
  std::vector<uint8_t> backing_buffer2(kTestBufferSize);
  buffer.SetDoubleBuffers(std::span(backing_buffer1), std::span(backing_buffer2));

  AllocationResult result = buffer.AllocRecord(16);
  ASSERT_TRUE(std::holds_alternative<Allocation>(result));
  Allocation alloc = std::get<Allocation>(result);
  ASSERT_NE(alloc.ptr, nullptr);
  EXPECT_EQ(reinterpret_cast<uint8_t*>(alloc.ptr), backing_buffer1.data());

  EXPECT_EQ(buffer.BytesAllocated(), size_t{16});
  RollingBufferState state = buffer.State();
  EXPECT_EQ(state.GetBufferOffset(), uint32_t{16});
  EXPECT_EQ(state.GetBufferNumber(), BufferNumber::kZero);
}

TEST(RollingBufferTest, AllocRecordFillsBuffer) {
  RollingBuffer buffer;
  std::vector<uint8_t> backing_buffer1(kTestBufferSize);
  std::vector<uint8_t> backing_buffer2(kTestBufferSize);
  buffer.SetDoubleBuffers(std::span(backing_buffer1), std::span(backing_buffer2));

  // Fill up most of the first buffer
  ASSERT_TRUE(std::holds_alternative<Allocation>(buffer.AllocRecord(kTestBufferSize)));
  EXPECT_EQ(buffer.BytesAllocated(), kTestBufferSize);

  // This allocation should fill the first buffer
  AllocationResult result = buffer.AllocRecord(8);
  ASSERT_TRUE(std::holds_alternative<SwitchingAllocation>(result));
  SwitchingAllocation filled_result = std::get<SwitchingAllocation>(result);
  EXPECT_EQ(filled_result.prev_state.GetBufferOffset(), kTestBufferSize);
  EXPECT_EQ(filled_result.prev_state.GetBufferNumber(), BufferNumber::kZero);
  // This gives use the previous state, so we shouldn't see the buffer filled.
  EXPECT_FALSE(filled_result.prev_state.IsBufferFilled(BufferNumber::kZero));

  // But if we re-query the state, we should now see that it's full for buffer 0
  RollingBufferState current_state = buffer.State();
  EXPECT_TRUE(current_state.IsBufferFilled(BufferNumber::kZero));
  EXPECT_FALSE(current_state.IsBufferFilled(BufferNumber::kOne));
}

TEST(RollingBufferTest, SetBufferServiced) {
  RollingBuffer buffer;
  std::vector<uint8_t> backing_buffer1(kTestBufferSize);
  std::vector<uint8_t> backing_buffer2(kTestBufferSize);
  buffer.SetDoubleBuffers(std::span(backing_buffer1), std::span(backing_buffer2));

  // Fill and switch buffer 0
  ASSERT_TRUE(std::holds_alternative<Allocation>(buffer.AllocRecord(kTestBufferSize)));
  AllocationResult result = buffer.AllocRecord(8);
  ASSERT_TRUE(std::holds_alternative<SwitchingAllocation>(result));
  RollingBufferState filled_state = std::get<SwitchingAllocation>(result).prev_state;

  // Buffer 0 should be marked full
  RollingBufferState current_state = buffer.State();
  EXPECT_TRUE(current_state.IsBufferFilled(BufferNumber::kZero));
  EXPECT_FALSE(current_state.IsBufferFilled(BufferNumber::kOne));

  ASSERT_TRUE(std::holds_alternative<BufferFull>(buffer.AllocRecord(kTestBufferSize)));

  // Service buffer 0
  buffer.SetBufferServiced(filled_state.GetWrappedCount());
  current_state = buffer.State();
  EXPECT_FALSE(current_state.IsBufferFilled(BufferNumber::kZero));
  EXPECT_FALSE(current_state.IsBufferFilled(BufferNumber::kOne));

  ASSERT_TRUE(std::holds_alternative<SwitchingAllocation>(buffer.AllocRecord(kTestBufferSize)));
}

TEST(BufferTest, ConcurrentOneshot) {
  size_t buffer_size = size_t{1024} * 1024;
  std::vector<uint8_t> data1(buffer_size);
  RollingBuffer b;
  b.SetOneshotBuffer(data1);

  std::barrier barrier(3, []() {});
  auto writer = [&](std::span<const uint64_t> data) {
    barrier.arrive_and_wait();
    for (;;) {
      AllocationResult record = b.AllocRecord(data.size_bytes());
      if (std::holds_alternative<Allocation>(record)) {
        std::ranges::copy(data, std::get<Allocation>(record).ptr);
      } else {
        break;
      }
    }
  };

  const uint64_t data1s[1] = {0x1111'1111'1111'1111};
  const uint64_t data2s[2] = {0x2222'2222'2222'2222, 0x2222'2222'2222'2222};
  const uint64_t data3s[3] = {0x3333'3333'3333'3333, 0x3333'3333'3333'3333, 0x3333'3333'3333'3333};
  std::thread t1(writer, std::span{data1s, 1});
  std::thread t2(writer, std::span{data2s, 2});
  std::thread t3(writer, std::span{data3s, 3});

  t1.join();
  t2.join();
  t3.join();
  size_t bytes_written = b.BytesAllocated();
  // Our largest record size is 24 words, so there must be less than that amount of space left
  EXPECT_GE(bytes_written, (size_t{1024} * 1024) - 24);
  EXPECT_TRUE(verify_buffer(std::span{data1.begin(), bytes_written}));
}

TEST(BufferTest, ConcurrentCircular) {
  size_t buffer_size = size_t{512};
  std::vector<uint8_t> data[2] = {std::vector<uint8_t>(buffer_size),
                                  std::vector<uint8_t>(buffer_size)};
  RollingBuffer b;
  b.SetDoubleBuffers(data[0], data[1]);

  std::optional<uint32_t> fill_amount[2] = {std::nullopt, std::nullopt};
  std::barrier barrier(3, []() {});

  std::atomic<uint8_t> writes_in_flight = 0;
  std::atomic<RollingBufferState> pending_service = RollingBufferState();

  auto WriteRecord = [&](std::span<const uint64_t> data) -> bool {
    // We track writes in flights so we know when it's safe to free the filled buffer.
    //
    // Once the buffer fills, we don't immediately know that it's safe to free it. There could still
    // be writers finishing up their writes.
    //
    // We wait until the were 0 writes in flights. Then, if we see that there is a pending service,
    // we know that there can be no more writers in the buffer that requires service.
    writes_in_flight.fetch_add(1);
    AllocationResult record = b.AllocRecord(data.size_bytes());
    bool written = false;
    if (std::holds_alternative<Allocation>(record)) {
      std::ranges::copy(data, std::get<Allocation>(record).ptr);
      written = true;
    } else if (std::holds_alternative<SwitchingAllocation>(record)) {
      RollingBufferState prev_state = std::get<SwitchingAllocation>(record).prev_state;
      // The buffer itself doesn't track how much data is in the spare buffer. We're responsible for
      // recording that.
      fill_amount[static_cast<uint8_t>(prev_state.GetBufferNumber())] =
          prev_state.GetBufferOffset();
      fill_amount[1 - (static_cast<uint8_t>(prev_state.GetBufferNumber()))] = std::nullopt;
      pending_service.store(prev_state, std::memory_order_relaxed);

      std::ranges::copy(data, std::get<SwitchingAllocation>(record).ptr);
      written = true;
    }
    if (writes_in_flight.fetch_sub(1, std::memory_order_acq_rel) != 1) {
      return written;
    }

    // We're the last one out. It's now safe to possibly service a buffer, so we should check if any
    // need servicing.
    RollingBufferState expected_service = pending_service.load(std::memory_order_relaxed);
    if (expected_service == RollingBufferState()) {
      return written;
    }
    //  There's a pending service and we were the last ones out. We need to try to switch the
    //  buffer.
    if (pending_service.compare_exchange_weak(expected_service, RollingBufferState(),
                                              std::memory_order_relaxed,
                                              std::memory_order_relaxed)) {
      b.SetBufferServiced(expected_service.GetWrappedCount());
    }
    return written;
  };

  std::atomic<uint64_t> bytes_written{0};
  auto writer = [&](std::span<const uint64_t> data) {
    barrier.arrive_and_wait();
    while (bytes_written.load() < size_t{1024} * 1024) {
      bool wrote = WriteRecord(data);
      if (wrote) {
        bytes_written.fetch_add(data.size_bytes());
      }
    }
  };

  const uint64_t data1[1] = {0x1111'1111'1111'1111};
  const uint64_t data2[2] = {0x2222'2222'2222'2222, 0x2222'2222'2222'2222};
  const uint64_t data3[3] = {0x3333'3333'3333'3333, 0x3333'3333'3333'3333, 0x3333'3333'3333'3333};
  std::thread t1(writer, std::span{data1, 1});
  std::thread t2(writer, std::span{data2, 2});
  std::thread t3(writer, std::span{data3, 3});

  t1.join();
  t2.join();
  t3.join();
  for (size_t buffer = 0; buffer < 2; buffer++) {
    uint32_t bytes_written = fill_amount[buffer].value_or(b.State().GetBufferOffset());
    EXPECT_TRUE(verify_buffer(std::span<uint8_t>{data[buffer].begin(), bytes_written}));
  }
}

TEST(BufferTest, ConcurrentStreaming) {
  size_t buffer_size = size_t{1024};
  std::vector<uint8_t> data[2] = {std::vector<uint8_t>(buffer_size),
                                  std::vector<uint8_t>(buffer_size)};
  RollingBuffer b;
  b.SetDoubleBuffers(data[0], data[1]);

  std::barrier barrier(4, []() {});
  std::vector<uint8_t> streamed_data;

  std::atomic<uint8_t> writes_in_flight = 0;
  std::atomic<RollingBufferState> pending_service = RollingBufferState();

  std::atomic<uint64_t> bytes_streamed{0};
  zx::fifo reader, writer;
  zx::fifo::create(3, sizeof(RollingBufferState), 0, &reader, &writer);

  auto WriteRecord = [&](std::span<const uint64_t> data) {
    writes_in_flight.fetch_add(1, std::memory_order_acq_rel);
    AllocationResult record = b.AllocRecord(data.size_bytes());
    if (std::holds_alternative<Allocation>(record)) {
      std::ranges::copy(data, std::get<Allocation>(record).ptr);
    } else if (std::holds_alternative<SwitchingAllocation>(record)) {
      RollingBufferState prev_state = std::get<SwitchingAllocation>(record).prev_state;
      pending_service.store(prev_state, std::memory_order_relaxed);
      std::ranges::copy(data, std::get<SwitchingAllocation>(record).ptr);
    }
    // If we're not the last one out, we don't need to do anything else.
    if (writes_in_flight.fetch_sub(1, std::memory_order_acq_rel) != 1) {
      return;
    }
    RollingBufferState expected_service = pending_service.load(std::memory_order_relaxed);
    if (expected_service != RollingBufferState()) {
      //  There's a pending service and we were the last ones out. We need to try to switch the
      //  buffer.
      if (pending_service.compare_exchange_weak(expected_service, RollingBufferState(),
                                                std::memory_order_relaxed,
                                                std::memory_order_relaxed)) {
        size_t written = 0;
        while (written == 0) {
          writer.write(sizeof(RollingBufferState), &expected_service, 1, &written);
        }
      }
    }
  };

  const size_t kBytesToStream = size_t{1024} * 1024;
  auto ReaderThread = [&]() {
    barrier.arrive_and_wait();
    size_t expected_wrapped = 0;
    while (bytes_streamed.load() < kBytesToStream) {
      zx_signals_t actual;
      reader.wait_one(ZX_FIFO_READABLE, zx::deadline_after(zx::duration::infinite()), &actual);
      RollingBufferState request;
      size_t count;
      reader.read(sizeof(RollingBufferState), &request, 1, &count);
      if (count == 0) {
        continue;
      }
      ASSERT_EQ(request.GetWrappedCount(), expected_wrapped);
      expected_wrapped += 1;

      std::span<uint8_t> to_copy{data[static_cast<uint8_t>(request.GetBufferNumber())].begin(),
                                 request.GetBufferOffset()};
      bool verified = verify_buffer(to_copy);
      if (!verified) {
        std::cerr << "Wrapped Count: " << request.GetWrappedCount() << "\n";
        std::cerr << "BufferNumber: " << static_cast<uint8_t>(request.GetBufferNumber()) << "\n";
        std::cerr << "Bytes: " << request.GetBufferOffset() << "\n";
        std::cerr << "Service needed this buff: "
                  << request.IsBufferFilled(request.GetBufferNumber());
        std::cerr << "Service needed other buff: "
                  << request.IsBufferFilled(
                         RollingBufferState::ToBufferNumber(request.GetWrappedCount() + 1));
      }
      std::ranges::copy(to_copy, std::back_inserter(streamed_data));
      bytes_streamed.fetch_add(request.GetBufferOffset());
      b.SetBufferServiced(request.GetWrappedCount());
    }
  };

  auto data_writer = [&](std::span<const uint64_t> data) {
    barrier.arrive_and_wait();
    while (bytes_streamed.load() < kBytesToStream) {
      WriteRecord(data);
    }
  };

  const uint64_t data1[1] = {0x1111'1111'1111'1111};
  const uint64_t data2[2] = {0x2222'2222'2222'2222, 0x2222'2222'2222'2222};
  const uint64_t data3[3] = {0x3333'3333'3333'3333, 0x3333'3333'3333'3333, 0x3333'3333'3333'3333};
  std::thread t1(data_writer, std::span{data1, 1});
  std::thread t2(data_writer, std::span{data2, 2});
  std::thread t3(data_writer, std::span{data3, 3});
  std::thread reader_thread(ReaderThread);

  t1.join();
  t2.join();
  t3.join();
  reader_thread.join();
  EXPECT_TRUE(verify_buffer(streamed_data));
}

TEST(RollingBufferTest, AllocRecordNextBufferTooSmall) {
  RollingBuffer buffer;
  std::vector<uint8_t> backing_buffer1(kTestBufferSize);
  // Next buffer is smaller, e.g. 16 bytes
  std::vector<uint8_t> backing_buffer2(16);
  buffer.SetDoubleBuffers(std::span(backing_buffer1), std::span(backing_buffer2));

  // Fill up the first buffer
  ASSERT_TRUE(std::holds_alternative<Allocation>(buffer.AllocRecord(kTestBufferSize)));

  // Try to allocate 32 bytes. It doesn't fit in backing_buffer1 (which is full).
  // It rolls to backing_buffer2, which is only 16 bytes.
  // This should return BufferFull, NOT a SwitchingAllocation.
  AllocationResult result = buffer.AllocRecord(32);
  EXPECT_TRUE(std::holds_alternative<BufferFull>(result));
}

TEST(RollingBufferTest, OneshotAllocationTooLarge) {
  RollingBuffer buffer;
  std::vector<uint8_t> backing_buffer(kTestBufferSize);
  buffer.SetOneshotBuffer(std::span(backing_buffer));

  // Try to allocate more than kTestBufferSize
  AllocationResult result = buffer.AllocRecord(kTestBufferSize + 8);
  EXPECT_TRUE(std::holds_alternative<BufferFull>(result));
}

}  // namespace
