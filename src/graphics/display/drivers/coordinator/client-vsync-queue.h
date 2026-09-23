// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_VSYNC_QUEUE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_VSYNC_QUEUE_H_

#include <lib/zx/time.h>
#include <zircon/assert.h>

#include <concepts>
#include <cstdint>

#include <fbl/ring_buffer.h>

#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/vsync-ack-cookie.h"

namespace display_coordinator {

// Traffic shaper for VSync messages sent to a Coordinator client.
//
// Under normal operation, VSync messages are generated at a high frequency.
// Clients designed for throughput rather than latency may occasionally take a
// long time to process a VSync. Without traffic shaping, this would result in a
// long queue of unprocessed FIDL messages, creating memory pressure.
//
// This traffic shaper avoids the situation described above. Clients are
// occasionally asked to acknowledge VSync messages, to provide a backpressure
// signal. When a client falls behind, a fixed number of VSync messages are
// buffered. If the buffer fills up, older messages are discarded in favor of
// newer messages.
//
// Instances are not thread-safe. Concurrent access must be synchronized
// externally.
class ClientVsyncQueue {
 public:
  // The information in one VSync message.
  struct Message {
    display::DisplayId display_id;
    zx::time_monotonic timestamp;
    display::ConfigStamp config_stamp;

    friend constexpr bool operator==(const Message&, const Message&) = default;
  };

  // Throttling starts when the unacknowledged message count exceeds this.
  static constexpr int32_t kThrottleWatermark = 120;

  // Ack cookie requested when the unacknowledged message count exceeds this.
  static constexpr int32_t kRequestAckWatermark = kThrottleWatermark / 2;

  // Throttled messages are buffered when
  static constexpr int32_t kThrottleBufferSize = 10;

  // Creates a queue that does not deliver received VSync events.
  ClientVsyncQueue();

  // Provided to make tests deterministic.
  explicit ClientVsyncQueue(uint64_t cookie_salt);

  ClientVsyncQueue(const ClientVsyncQueue&) = delete;
  ClientVsyncQueue& operator=(const ClientVsyncQueue&) = delete;

  ~ClientVsyncQueue();

  // Registers a VSync message to be sent to the Coordinator client.
  //
  // If the queue is full, the oldest VSync message will be overwritten.
  //
  // `DrainUntilThrottled()` must be called before any other method is called.
  void Push(const Message& message);

  // Called when a client acknowledges a VSync.
  //
  // `ack_cookie` must not be invalid.
  //
  // Returns false if `ack_cookie` is not a correct acknowledgement cookie.
  //
  // If this method succeeds, `DrainUntilThrottled()` must be called before any
  // other method is called.
  bool Acknowledge(display::VsyncAckCookie ack_cookie);

  // Removes all the messages eligible to be sent to the Coordinator client.
  //
  // `send_message` must take two arguments, a `const
  // ClientVsyncQueue::Message&` and a `display::VsyncAckCookie`, and must
  // attempt to send the VSync message described by the arguments.
  //
  // `send_message` must not make any method calls on this queue.
  //
  // `send_message` receives messages in the same order that they were given to
  // `Push()`.
  template <typename Callable>
  void DrainUntilThrottled(Callable send_message)
    requires(std::invocable<Callable, const ClientVsyncQueue::Message&, display::VsyncAckCookie>);

 private:
  // False if `Acknowledge()` cannot succeed.
  bool IsWaitingForAcknowledgement() const {
    return unacknowledged_cookie_ != display::kInvalidVsyncAckCookie;
  }

  // False if the next `Push()` will let its message through.
  bool IsThrottling() const {
    static_assert(
        kThrottleWatermark > kRequestAckWatermark,
        "Current implementation assumes the throttle watermark is above the request watermark");
    return messages_sent_since_last_acknowledgement_ == kThrottleWatermark;
  }

  // Returns a new valid `VSyncAckCookie`.
  //
  // This method is not idempotent. The generated value is expected to be used.
  [[nodiscard]] display::VsyncAckCookie GenerateCookie();

  // Stores throttled VSync messages.
  fbl::RingBuffer<Message, kThrottleBufferSize> queued_messages_;

  const uint64_t cookie_salt_ = 0;
  uint64_t cookie_sequence_ = 0;

  // Unthrottled messages since the last successful `Acknowledge()`.
  int32_t messages_sent_since_last_acknowledgement_ = 0;

  // Invalid if we don't have a cookie waiting to be acknowledged.
  display::VsyncAckCookie unacknowledged_cookie_ = display::kInvalidVsyncAckCookie;
};

template <typename Callable>
void ClientVsyncQueue::DrainUntilThrottled(Callable send_message)
  requires(std::invocable<Callable, const ClientVsyncQueue::Message&, display::VsyncAckCookie>)
{
  while (!queued_messages_.empty()) {
    if (IsThrottling()) [[unlikely]] {
      break;
    }

    display::VsyncAckCookie cookie = display::kInvalidVsyncAckCookie;
    if (messages_sent_since_last_acknowledgement_ == kRequestAckWatermark) [[unlikely]] {
      ZX_DEBUG_ASSERT(unacknowledged_cookie_ == display::kInvalidVsyncAckCookie);
      cookie = GenerateCookie();

      // This state change technically takes effect after the `send_message()`
      // call below completes. We can get away with making the state change now
      // because `send_message()` is not allowed to make method calls on this
      // instance.
      unacknowledged_cookie_ = cookie;
    }

    const Message& message = queued_messages_.front();
    send_message(message, cookie);
    queued_messages_.pop();
    ++messages_sent_since_last_acknowledgement_;
  }

  ZX_DEBUG_ASSERT(queued_messages_.empty() || IsThrottling());
}

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_VSYNC_QUEUE_H_
