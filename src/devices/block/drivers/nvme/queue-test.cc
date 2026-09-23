// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/nvme/queue.h"

#include <lib/fake-bti/bti.h>
#include <zircon/syscalls.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace nvme {

constexpr uint32_t kQueueMagic = 0xabbacaba;

class QueueTest : public ::testing::Test {
 public:
  void SetUp() { ASSERT_OK(fake_bti_create(fake_bti_.reset_and_get_address())); }

 protected:
  zx::bti fake_bti_;
};

TEST_F(QueueTest, CappedToPageSize) {
  auto queue = Queue::Create(fake_bti_.borrow(), /*queue_id=*/1, /*max_entries=*/100,
                             /*entry_size=*/zx_system_get_page_size());
  ASSERT_OK(queue);
  ASSERT_EQ(1u, queue->entry_count());
}

TEST_F(QueueTest, WrapsAround) {
  // Create a queue with two elements.
  auto queue = Queue::Create(fake_bti_.borrow(), /*queue_id=*/1, /*max_entries=*/100,
                             /*entry_size=*/zx_system_get_page_size() / 2);
  ASSERT_OK(queue);

  // To start with, the next item in the queue should be the first item in the queue.
  ASSERT_EQ(0u, queue->NextIndex());
  // Set the first item in the queue to |kQueueMagic| and move forward.
  *static_cast<uint32_t*>(queue->Next()) = kQueueMagic;
  // The next index in the queue should now be 1 (the second item).
  ASSERT_EQ(1u, queue->NextIndex());
  // Set the second item in the queue to 0 and move forward.
  *static_cast<uint32_t*>(queue->Next()) = 0;
  // We should have wrapped around to the start of the queue.
  ASSERT_EQ(0u, queue->NextIndex());
  // Check that the first item in the queue is still |kQueueMagic|.
  ASSERT_EQ(*static_cast<uint32_t*>(queue->Next()), kQueueMagic);
}

TEST_F(QueueTest, CappedToMaxEntries) {
  auto queue =
      Queue::Create(fake_bti_.borrow(), /*queue_id=*/1, /*max_entries=*/100, /*entry_size=*/1);
  ASSERT_OK(queue);
  ASSERT_EQ(100u, queue->entry_count());
}

TEST_F(QueueTest, PhysicalAddress) {
  auto queue =
      Queue::Create(fake_bti_.borrow(), /*queue_id=*/1, /*max_entries=*/100, /*entry_size=*/1);
  ASSERT_OK(queue);
  ASSERT_EQ(zx_paddr_t{FAKE_BTI_PHYS_ADDR}, queue->GetDeviceAddress());
}

}  // namespace nvme
