// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/commands/verb_stack_usage.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/mock_frame.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/console/console_test.h"

namespace zxdb {

namespace {

class VerbStackUsage : public ConsoleTest {};

}  // namespace

TEST_F(VerbStackUsage, GetThreadStackUsage) {
  const uint64_t kPageSize = session().arch_info().page_size();

  const uint64_t kStackBase = 0x100000;  // HIGH address of stack region.
  const uint64_t kStackSizeInPages = 20;
  const uint64_t kStackSize = kStackSizeInPages * kPageSize;

  // This test expects the page size to be larger than the used bytes for the below computations.
  const uint64_t kUsedBytes = 0x5e;
  ASSERT_GE(kPageSize, kUsedBytes);
  const uint64_t kStackPointer = kStackBase - kUsedBytes;

  // Same values for the unsafe stack.
  const uint64_t kUnsafeStackBase = 0x9000000;
  const uint64_t kUnsafeStackSizeInPages = 20;
  const uint64_t kUnsafeStackSize = kUnsafeStackSizeInPages * kPageSize;
  const uint64_t kUnsafeUsedBytes = 0x103;
  ASSERT_GE(kPageSize, kUnsafeUsedBytes);
  const uint64_t kUnsafeStackPointer = kUnsafeStackBase - kUnsafeUsedBytes;

  // Set up the thread to be stopped with the given stack pointer.
  std::vector<std::unique_ptr<Frame>> frames;
  Location location(Location::State::kSymbolized, 0x1000);
  frames.push_back(std::make_unique<MockFrame>(&session(), thread(), location, kStackPointer));
  InjectExceptionWithStack(ConsoleTest::kProcessKoid, ConsoleTest::kThreadKoid,
                           debug_ipc::ExceptionType::kSingleStep, std::move(frames), true);

  std::vector<debug_ipc::AddressRegion> aspace;
  debug_ipc::AddressRegion& containing_region = aspace.emplace_back();
  containing_region.base = 0;
  containing_region.size = 0x100000000000ull;
  containing_region.depth = 0;

  debug_ipc::AddressRegion& other_region = aspace.emplace_back();
  other_region.base = 0x1000;
  other_region.size = 0x1000;
  other_region.depth = 1;
  other_region.vmo_koid = 1234;

  // Safe stack region.
  debug_ipc::AddressRegion& stack_region = aspace.emplace_back();
  stack_region.base = kStackBase - kStackSize;
  stack_region.size = kStackSize;
  stack_region.depth = 1;
  stack_region.vmo_koid = 45678;
  const uint64_t kCommittedBytes = 3 * kPageSize;
  stack_region.committed_bytes = kCommittedBytes;

  // Unused region.
  debug_ipc::AddressRegion& other_region2 = aspace.emplace_back();
  other_region2.base = kStackBase + 0x1000;
  other_region2.size = 0x1000;
  other_region2.depth = 1;
  other_region2.vmo_koid = 99990;

  // Unsafe stack region.
  debug_ipc::AddressRegion& unsafe_region = aspace.emplace_back();
  unsafe_region.base = kUnsafeStackBase - kUnsafeStackSize;
  unsafe_region.size = kUnsafeStackSize;
  unsafe_region.depth = 1;
  unsafe_region.vmo_koid = 64291;
  const uint64_t kUnsafeCommittedBytes = 5 * kPageSize;
  unsafe_region.committed_bytes = kUnsafeCommittedBytes;

  // Good query.
  ThreadStackUsage usage =
      GetThreadStackUsage(&console().context(), aspace, thread(), kUnsafeStackPointer);
  ASSERT_TRUE(usage.safe_stack.ok());
  EXPECT_EQ(usage.safe_stack.value().total, kStackSize);
  EXPECT_EQ(usage.safe_stack.value().used, kUsedBytes);
  EXPECT_EQ(usage.safe_stack.value().committed, kCommittedBytes);
  EXPECT_EQ(usage.safe_stack.value().wasted, kCommittedBytes - kUsedBytes);

  ASSERT_TRUE(usage.unsafe_stack.ok());
  EXPECT_EQ(usage.unsafe_stack.value().total, kUnsafeStackSize);
  EXPECT_EQ(usage.unsafe_stack.value().used, kUnsafeUsedBytes);
  EXPECT_EQ(usage.unsafe_stack.value().committed, kUnsafeCommittedBytes);
  EXPECT_EQ(usage.unsafe_stack.value().wasted, kUnsafeCommittedBytes - kUnsafeUsedBytes);
}

}  // namespace zxdb
