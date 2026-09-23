// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <unistd.h>

#include <array>
#include <cstddef>
#include <iostream>
#include <sstream>

#include "src/ui/tests/integration_input_tests/starnix-input/relay-api.h"
#include "third_party/android/platform/bionic/libc/kernel/uapi/linux/input.h"

// Note: This program uses `fprintf()` instead of `std::cerr`, because the latter flushes the writes
// after each token. That flushing causing a single error message to be split over multiple log
// lines.

void fail() {
  std::string packet(relay_api::kFailedMessage);
  write(STDOUT_FILENO, packet.data(), packet.size());
  abort();
}

template <auto F, typename... Args>
auto ensure(size_t caller_lineno, const std::string& callee, Args... args) {
  auto res = F(args...);
  if (errno != 0) {
    fprintf(stderr, "`%s` failed: %s (called from line %zu)", callee.c_str(), strerror(errno),
            caller_lineno);
    fail();
  }
  return res;
}

// Invoke `function`, then `abort()` if `errno` is non-zero.
// In case of failure, logs the caller line number and callee name.
#define ENSURE(function, ...) ensure<function>(__LINE__, #function, __VA_ARGS__)

// Asserts that `val1` equals `val2`.
//
// Written as a macro so that the compiler can verify that
// `format` matches the types of `val1` and `val2`.
//
// Evaluates macro parameters into local variables to ensure
// that any expressions are only evaluated once.
#define ASSERT_EQ(val1, val2, format) \
  do {                                \
    auto v1 = val1;                   \
    auto v2 = val2;                   \
    auto f = format;                  \
    if (v1 != v2) {                   \
      fprintf(stderr, f, v1, v2);     \
      fail();                         \
    }                                 \
  } while (0)

// Asserts that `val1` is greater than or equal to `val2`.
//
// Written as a macro so that the compiler can verify that
// `format` matches the types of `val1` and `val2`.
//
// Evaluates macro parameters into local variables to ensure
// that any expressions are only evaluated once.
#define ASSERT_GE(val1, val2, format) \
  do {                                \
    auto v1 = val1;                   \
    auto v2 = val2;                   \
    auto f = format;                  \
    if (v1 < v2) {                    \
      fprintf(stderr, f, v1, v2);     \
      fail();                         \
    }                                 \
  } while (0)

void relay_events(int epoll_fd, size_t num_of_events) {
  // To account for variable call counts, we subtract each batch from a
  // running count. If we ever read more than the expected amount, something
  // has gone wrong.
  size_t num_remaining = num_of_events;
  constexpr size_t kExpectedEventLen = sizeof(input_event);
  constexpr int kMaxEvents = 1;
  constexpr int kInfiniteTimeout = -1;
  while (num_remaining > 0) {
    // Wait for data.
    std::array<epoll_event, kMaxEvents> event_buf{};
    int n_ready = ENSURE(epoll_wait, epoll_fd, event_buf.data(), kMaxEvents, kInfiniteTimeout);
    ASSERT_EQ(kMaxEvents, n_ready, "expected n_ready=%d, but got %d");
    ASSERT_EQ(EPOLLIN, event_buf[0].events, "expected events_buf[0].events=%u, but got %u");

    // Read the raw data into an `input_event`.
    std::array<unsigned char, static_cast<size_t>(64 * 1024)> data_buf{};
    ssize_t n_read = ENSURE(read, event_buf[0].data.fd, data_buf.data(), data_buf.size());
    size_t num_read = n_read / kExpectedEventLen;
    ASSERT_GE(num_remaining, num_read, "expected %zu events at most but got %zu");
    num_remaining = num_remaining - num_read;

    std::array<char, relay_api::kMaxPacketLen * relay_api::kDownUpNumPackets> text_event_buf{};
    size_t total_formatted_len = 0;

    for (size_t i = 0; i < num_read; i++) {
      struct input_event event{};
      memcpy(&event, &data_buf[i * kExpectedEventLen], kExpectedEventLen);

      // Format the event as a string.
      std::array<char, relay_api::kMaxPacketLen> partial_text_event_buf{};
      size_t formatted_len = snprintf(partial_text_event_buf.data(), partial_text_event_buf.size(),
                                      relay_api::kEventFormat, event.time.tv_sec,
                                      event.time.tv_usec, event.type, event.code, event.value);
      ASSERT_GE(partial_text_event_buf.size(), formatted_len,
                "expected formatted_len < %zu, but got %zu");
      memcpy(&text_event_buf[total_formatted_len], partial_text_event_buf.data(), formatted_len);
      total_formatted_len = total_formatted_len + formatted_len;
    }

    // Write the string to `stdout`.
    ssize_t n_written = ENSURE(write, STDOUT_FILENO, text_event_buf.data(), total_formatted_len);
    ASSERT_EQ(total_formatted_len, static_cast<size_t>(n_written),
              "expected n_written=%zu, but got %zd");
  }
}

int open_device(int epoll_fd, const std::string& device_path) {
  int device_fd = ENSURE(open, device_path.c_str(), O_RDONLY);
  epoll_event epoll_params = {.events = EPOLLIN, .data = {.fd = device_fd}};
  ENSURE(epoll_ctl, epoll_fd, EPOLL_CTL_ADD, device_fd, &epoll_params);
  return device_fd;
}

void close_device(int device_fd, int epoll_fd) {
  ENSURE(epoll_ctl, epoll_fd, EPOLL_CTL_DEL, device_fd, nullptr);
  ENSURE(close, device_fd);
}

void write_message_to_stdout(const std::string& message) {
  auto n_written = ENSURE(write, STDOUT_FILENO, message.data(), message.size());
  ASSERT_EQ(message.size(), static_cast<size_t>(n_written), "expected n_written=%zu, but got %zd");
}

constexpr std::string kTouchDevice = "/dev/input/event0";
constexpr std::string kMouseDevice = "/dev/input/event2";

int main() {
  int epoll_fd = ENSURE(epoll_create, 1);  // Per manual page, must be >0.

  char input_buffer[128];

  std::string device;

  while (true) {
    // Let starnix test fixture know that we are ready for an input message.
    write_message_to_stdout(relay_api::kWaitForStdinMessage);

    std::string cmd;

    // Read input from STDIN_FILENO
    ssize_t bytes_read = read(STDIN_FILENO, input_buffer, sizeof(input_buffer) - 1);
    if (bytes_read <= 0) {
      write_message_to_stdout(relay_api::kFailedMessage);
      return 0;
    }
    // Null-terminate the string
    input_buffer[bytes_read] = '\0';

    std::stringstream ss(input_buffer);

    ss >> cmd;
    if (cmd == relay_api::kQuitCmd) {
      return 0;
    }

    if (cmd == relay_api::kDeviceDelimiter) {
      ss >> device;
      if (device == relay_api::kTouchDev) {
        device = kTouchDevice;
      } else if (device == relay_api::kMouseDev) {
        device = kMouseDevice;
      }
      continue;
    }

    if (cmd == relay_api::kEventCmd) {
      // open the device, handle X events, then close.
      size_t num_of_events;
      ss >> num_of_events;

      int device_fd = open_device(epoll_fd, device);

      // Let starnix test fixture know that we're ready for it to inject input events.
      write_message_to_stdout(relay_api::kReadyMessage);

      // Now just copy events from `evdev` to stdout.
      // We expect two taps received across two to four calls to `read`, depending
      // on the state of what's been written at the time that we read.
      relay_events(epoll_fd, num_of_events);

      // Close file.
      close_device(device_fd, epoll_fd);

    } else {
      // Unknown command.
      write_message_to_stdout(relay_api::kFailedMessage);
      return 0;
    }
  }
}
