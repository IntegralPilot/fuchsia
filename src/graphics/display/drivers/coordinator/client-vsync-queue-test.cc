// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/client-vsync-queue.h"

#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/time.h>

#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/vsync-ack-cookie.h"

namespace display_coordinator {

namespace {

class ClientVsyncQueueTest : public ::testing::Test {
 private:
  fdf_testing::ScopedGlobalLogger logger_;
};

constexpr ClientVsyncQueue::Message CreateTestMessage(int sequence_number) {
  return {
      .display_id = display::DisplayId(1000 + sequence_number),
      .timestamp = zx::time_monotonic(2000 + sequence_number),
      .config_stamp = display::ConfigStamp(3000 + sequence_number),
  };
}

TEST_F(ClientVsyncQueueTest, PushSingleMessage) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  static constexpr ClientVsyncQueue::Message kTestMessage = CreateTestMessage(0);
  queue.Push(kTestMessage);

  std::vector<ClientVsyncQueue::Message> sent_messages;
  queue.DrainUntilThrottled(
      [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
        static_assert(ClientVsyncQueue::kRequestAckWatermark > 1);
        EXPECT_EQ(ack_cookie, display::kInvalidVsyncAckCookie);

        sent_messages.push_back(message);
      });

  EXPECT_THAT(sent_messages, ::testing::ElementsAre(kTestMessage));
}

TEST_F(ClientVsyncQueueTest, PushUpToRequestWatermark) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  for (int index = 0; index < ClientVsyncQueue::kRequestAckWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(index);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
          EXPECT_EQ(ack_cookie, display::kInvalidVsyncAckCookie);
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::ElementsAre(test_message));
  }
}

TEST_F(ClientVsyncQueueTest, PushGeneratesCookieAtRequestWatermark) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  for (int index = 0; index < ClientVsyncQueue::kRequestAckWatermark; ++index) {
    queue.Push(CreateTestMessage(index));
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
          ASSERT_EQ(ack_cookie, display::kInvalidVsyncAckCookie);
        });
  }

  static constexpr ClientVsyncQueue::Message kMessageWithCookie =
      CreateTestMessage(ClientVsyncQueue::kRequestAckWatermark);
  queue.Push(kMessageWithCookie);
  std::vector<ClientVsyncQueue::Message> sent_messages;
  queue.DrainUntilThrottled(
      [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
        EXPECT_NE(ack_cookie, display::kInvalidVsyncAckCookie);
        sent_messages.push_back(message);
      });
  EXPECT_THAT(sent_messages, ::testing::ElementsAre(kMessageWithCookie));
}

TEST_F(ClientVsyncQueueTest, PushWithoutAckUpToThrottleWatermark) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  for (int index = 0; index < ClientVsyncQueue::kThrottleWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(index);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
          if (index == ClientVsyncQueue::kRequestAckWatermark) {
            EXPECT_NE(ack_cookie, display::kInvalidVsyncAckCookie);
          } else {
            EXPECT_EQ(ack_cookie, display::kInvalidVsyncAckCookie);
          }
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::ElementsAre(test_message));
  }
}

TEST_F(ClientVsyncQueueTest, PushWithoutAckPastBufferSize) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  for (int index = 0; index < ClientVsyncQueue::kThrottleWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(index);
    queue.Push(test_message);

    queue.DrainUntilThrottled([&](const ClientVsyncQueue::Message&, display::VsyncAckCookie) {});
  }

  // Go past the buffer size to ensure that replacing entries in the
  // circular buffer does not result in crashes.
  for (int index = 0; index < ClientVsyncQueue::kThrottleBufferSize * 2; ++index) {
    const ClientVsyncQueue::Message test_message =
        CreateTestMessage(ClientVsyncQueue::kThrottleWatermark + index);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::IsEmpty());
  }
}

TEST_F(ClientVsyncQueueTest, AcknowledgeBeforeAckCookieSent) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  static constexpr display::VsyncAckCookie kInvalidCookie(1);
  EXPECT_FALSE(queue.Acknowledge(kInvalidCookie));

  std::vector<ClientVsyncQueue::Message> sent_messages;
  queue.DrainUntilThrottled(
      [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
        sent_messages.push_back(message);
      });
  EXPECT_THAT(sent_messages, ::testing::IsEmpty());
}

TEST_F(ClientVsyncQueueTest, AcknowledgeBeforeThrottle) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  display::VsyncAckCookie ack_cookie = display::kInvalidVsyncAckCookie;

  for (int index = 0; index < ClientVsyncQueue::kThrottleWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(index);
    queue.Push(test_message);

    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          if (cookie != display::kInvalidVsyncAckCookie) {
            ASSERT_EQ(ack_cookie, display::kInvalidVsyncAckCookie)
                << "New VSync cookie generated before old cookie acknowledged";
            ack_cookie = cookie;
          }
        });
  }
  ASSERT_NE(ack_cookie, display::kInvalidVsyncAckCookie) << "No VSync cookie generated";

  ASSERT_TRUE(queue.Acknowledge(ack_cookie));

  {
    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
          sent_messages.push_back(message);
        });
    ASSERT_THAT(sent_messages, ::testing::IsEmpty())
        << "Messages queued before throttling was expected";
  }

  for (int index = 0; index < ClientVsyncQueue::kRequestAckWatermark; ++index) {
    const ClientVsyncQueue::Message test_message =
        CreateTestMessage(ClientVsyncQueue::kThrottleWatermark + index);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::ElementsAre(test_message))
        << "Incorrect queue state after cookie acknowledged";
  }
}

TEST_F(ClientVsyncQueueTest, AcknowledgeAfterThrottleBeforeDrop) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  display::VsyncAckCookie ack_cookie = display::kInvalidVsyncAckCookie;

  for (int index = 0; index < ClientVsyncQueue::kThrottleWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(index);
    queue.Push(test_message);

    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          if (cookie != display::kInvalidVsyncAckCookie) {
            ASSERT_EQ(ack_cookie, display::kInvalidVsyncAckCookie)
                << "New VSync cookie generated before old cookie acknowledged";
            ack_cookie = cookie;
          }
        });
  }
  ASSERT_NE(ack_cookie, display::kInvalidVsyncAckCookie) << "No VSync cookie generated";

  {
    std::vector<ClientVsyncQueue::Message> throttled_messages;
    for (int index = 0; index < ClientVsyncQueue::kThrottleBufferSize; ++index) {
      const ClientVsyncQueue::Message test_message =
          CreateTestMessage(ClientVsyncQueue::kThrottleWatermark + index);
      queue.Push(test_message);
      throttled_messages.push_back(test_message);

      std::vector<ClientVsyncQueue::Message> sent_messages;
      queue.DrainUntilThrottled(
          [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
            EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);
            sent_messages.push_back(message);
          });
      EXPECT_THAT(sent_messages, ::testing::IsEmpty())
          << "Message sent after throttling criteria met";
    }

    ASSERT_TRUE(queue.Acknowledge(ack_cookie));
    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled([&](const ClientVsyncQueue::Message& message,
                                  display::VsyncAckCookie cookie) {
      static_assert(ClientVsyncQueue::kThrottleBufferSize < ClientVsyncQueue::kRequestAckWatermark);
      EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);

      sent_messages.push_back(message);
    });
    EXPECT_THAT(sent_messages, ::testing::ElementsAreArray(throttled_messages))
        << "Throttled messages not sent after cookie acknowledged";
  }

  static constexpr int kMessagesLeftBeforeRequestWatermark =
      ClientVsyncQueue::kRequestAckWatermark - ClientVsyncQueue::kThrottleBufferSize;
  for (int index = 0; index < kMessagesLeftBeforeRequestWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(
        ClientVsyncQueue::kThrottleWatermark + ClientVsyncQueue::kThrottleBufferSize + index);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::ElementsAre(test_message))
        << "Incorrect queue state after cookie acknowledged";
  }

  {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(
        ClientVsyncQueue::kThrottleWatermark + ClientVsyncQueue::kRequestAckWatermark);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          EXPECT_NE(cookie, display::kInvalidVsyncAckCookie);
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::ElementsAre(test_message))
        << "Incorrect queue state after cookie acknowledged";
  }
}

TEST_F(ClientVsyncQueueTest, AcknowledgeWithInvalidCookie) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  display::VsyncAckCookie ack_cookie = display::kInvalidVsyncAckCookie;

  for (int index = 0; index < ClientVsyncQueue::kThrottleWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(index);
    queue.Push(test_message);

    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          if (cookie != display::kInvalidVsyncAckCookie) {
            ASSERT_EQ(ack_cookie, display::kInvalidVsyncAckCookie)
                << "New VSync cookie generated before old cookie acknowledged";
            ack_cookie = cookie;
          }
        });
  }
  ASSERT_NE(ack_cookie, display::kInvalidVsyncAckCookie) << "No VSync cookie generated";

  {
    std::vector<ClientVsyncQueue::Message> throttled_messages;
    for (int index = 0; index < ClientVsyncQueue::kThrottleBufferSize; ++index) {
      const ClientVsyncQueue::Message test_message =
          CreateTestMessage(ClientVsyncQueue::kThrottleWatermark + index);
      queue.Push(test_message);
      throttled_messages.push_back(test_message);

      std::vector<ClientVsyncQueue::Message> sent_messages;
      queue.DrainUntilThrottled(
          [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
            EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);
            sent_messages.push_back(message);
          });
      EXPECT_THAT(sent_messages, ::testing::IsEmpty())
          << "Message sent after throttling criteria met";
    }

    const display::VsyncAckCookie invalid_cookie(~ack_cookie.value());
    EXPECT_FALSE(queue.Acknowledge(invalid_cookie));
    {
      std::vector<ClientVsyncQueue::Message> sent_messages;
      queue.DrainUntilThrottled(
          [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
            sent_messages.push_back(message);
          });
      EXPECT_THAT(sent_messages, ::testing::IsEmpty())
          << "Throttled messages sent after incorrect cookie acknowledgement";
    }

    ASSERT_TRUE(queue.Acknowledge(ack_cookie));
    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled([&](const ClientVsyncQueue::Message& message,
                                  display::VsyncAckCookie cookie) {
      static_assert(ClientVsyncQueue::kThrottleBufferSize < ClientVsyncQueue::kRequestAckWatermark);
      EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);

      sent_messages.push_back(message);
    });
    EXPECT_THAT(sent_messages, ::testing::ElementsAreArray(throttled_messages))
        << "Throttled messages not sent after correct cookie acknowledged";
  }
}

TEST_F(ClientVsyncQueueTest, AcknowledgeAfterDrop) {
  ClientVsyncQueue queue(/*cookie_salt=*/0x11000);

  display::VsyncAckCookie ack_cookie = display::kInvalidVsyncAckCookie;

  for (int index = 0; index < ClientVsyncQueue::kThrottleWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(index);
    queue.Push(test_message);

    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          if (cookie != display::kInvalidVsyncAckCookie) {
            ASSERT_EQ(ack_cookie, display::kInvalidVsyncAckCookie)
                << "New VSync cookie generated before old cookie acknowledged";
            ack_cookie = cookie;
          }
        });
  }
  ASSERT_NE(ack_cookie, display::kInvalidVsyncAckCookie) << "No VSync cookie generated";

  for (int index = 0; index < ClientVsyncQueue::kThrottleWatermark; ++index) {
    const ClientVsyncQueue::Message test_message =
        CreateTestMessage(ClientVsyncQueue::kThrottleWatermark + index);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::IsEmpty())
        << "Message sent after throttling criteria met";
  }

  {
    std::vector<ClientVsyncQueue::Message> bufferred_messages;
    for (int index = 0; index < ClientVsyncQueue::kThrottleBufferSize; ++index) {
      const ClientVsyncQueue::Message test_message =
          CreateTestMessage((2 * ClientVsyncQueue::kThrottleWatermark) + index);
      queue.Push(test_message);
      bufferred_messages.push_back(test_message);

      std::vector<ClientVsyncQueue::Message> sent_messages;
      queue.DrainUntilThrottled(
          [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
            EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);
            sent_messages.push_back(message);
          });
      EXPECT_THAT(sent_messages, ::testing::IsEmpty())
          << "Message sent after throttling criteria met";
    }

    ASSERT_TRUE(queue.Acknowledge(ack_cookie));
    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled([&](const ClientVsyncQueue::Message& message,
                                  display::VsyncAckCookie cookie) {
      static_assert(ClientVsyncQueue::kThrottleBufferSize < ClientVsyncQueue::kRequestAckWatermark);
      EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);

      sent_messages.push_back(message);
    });
    EXPECT_THAT(sent_messages, ::testing::ElementsAreArray(bufferred_messages))
        << "Bufferred messages not sent after cookie acknowledged";
  }

  static constexpr int kMessagesLeftBeforeRequestWatermark =
      ClientVsyncQueue::kRequestAckWatermark - ClientVsyncQueue::kThrottleBufferSize;
  for (int index = 0; index < kMessagesLeftBeforeRequestWatermark; ++index) {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(
        (2 * ClientVsyncQueue::kThrottleWatermark) + ClientVsyncQueue::kThrottleBufferSize + index);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          EXPECT_EQ(cookie, display::kInvalidVsyncAckCookie);
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::ElementsAre(test_message))
        << "Incorrect queue state after cookie acknowledged";
  }

  {
    const ClientVsyncQueue::Message test_message = CreateTestMessage(
        (2 * ClientVsyncQueue::kThrottleWatermark) + ClientVsyncQueue::kRequestAckWatermark);
    queue.Push(test_message);

    std::vector<ClientVsyncQueue::Message> sent_messages;
    queue.DrainUntilThrottled(
        [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie cookie) {
          EXPECT_NE(cookie, display::kInvalidVsyncAckCookie);
          sent_messages.push_back(message);
        });
    EXPECT_THAT(sent_messages, ::testing::ElementsAre(test_message))
        << "Incorrect queue state after cookie acknowledged";
  }
}

}  // namespace

}  // namespace display_coordinator
