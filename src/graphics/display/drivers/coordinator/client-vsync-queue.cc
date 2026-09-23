// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/client-vsync-queue.h"

#include <zircon/assert.h>

#include <random>

namespace display_coordinator {

ClientVsyncQueue::ClientVsyncQueue(uint64_t cookie_salt) : cookie_salt_(cookie_salt) {}

ClientVsyncQueue::ClientVsyncQueue() : ClientVsyncQueue(/*cookie_salt=*/std::random_device()()) {}

ClientVsyncQueue::~ClientVsyncQueue() = default;

void ClientVsyncQueue::Push(const Message& message) {
  ZX_DEBUG_ASSERT_MSG(queued_messages_.empty() || IsThrottling(),
                      "DrainUntilThrottled() not called between Push() calls");

  if (queued_messages_.full()) [[unlikely]] {
    queued_messages_.pop();
  }
  queued_messages_.push(message);
}

bool ClientVsyncQueue::Acknowledge(display::VsyncAckCookie ack_cookie) {
  ZX_DEBUG_ASSERT(ack_cookie != display::kInvalidVsyncAckCookie);

  if (ack_cookie != unacknowledged_cookie_) [[unlikely]] {
    return false;
  }

  messages_sent_since_last_acknowledgement_ = 0;
  unacknowledged_cookie_ = display::kInvalidVsyncAckCookie;
  return true;
}

display::VsyncAckCookie ClientVsyncQueue::GenerateCookie() {
  while (true) {
    ++cookie_sequence_;
    display::VsyncAckCookie result(cookie_salt_ ^ cookie_sequence_);

    // When `cookie_salt_` equals `cookie_sequence_`, the generated cookie is
    // invalid (0). In that case, we'll go through the loop one more time.
    if (result != display::kInvalidVsyncAckCookie) [[likely]] {
      return result;
    }
  }
}

}  // namespace display_coordinator
