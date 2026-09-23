// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/time.h>

#include <cstdint>
#include <string_view>

#include <zxtest/zxtest.h>

namespace {

// The host test attaches uart_test.message_size=XXXX where XXXXX is the expected number of bytes.
// This allows us to continue reading until we read everything. Worst case this test will time out
// if the interrupts are never fired.
class UartInterruptTest : public zxtest::Test {
 public:
  static void SetUpTestSuite() {
    standalone::Option option = {.prefix = "uart_test.message_size="};
    standalone::GetOptions({option});

    // The host side will add a line break character \n.
    std::string value = option.option.substr(option.prefix.length());
    message_size_ = atoi(value.c_str()) + 1;
  }

  void SetUp() final { message_.resize(message_size_, '\0'); }

  cpp20::span<char> message() { return message_; }
  size_t max_message_size() const { return message_size_; }

 private:
  static size_t message_size_;

  std::vector<char> message_;
};

size_t UartInterruptTest::message_size_ = 0;

// Read everything from the UART then write it back so it can be read by the serial listener.
TEST_F(UartInterruptTest, KernelEchoTest) {
  // Magic string telling the host interaction test it should start pushing characters into the
  // UART.
  printf("uart-interrupt-test: Uart Ready\n");
  // Let the RX Buffer fill up.
  zx::nanosleep(zx::deadline_after(zx::sec(2)));
  size_t total = 0;
  size_t empty_count = 0;

  auto system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debug_resource = std::move(result.value());

  while (total < max_message_size()) {
    size_t read = 0;
    ASSERT_OK(zx_debug_read(debug_resource.get(), message().data() + total,
                            max_message_size() - total, &read));
    total += read;

    if (read == 0) {
      empty_count++;
      // If we did not find anything in the uart, go to sleep.
      zx::nanosleep(zx::deadline_after(zx::sec(1)));
    } else {
      empty_count = 0;
    }

    // We retried 5 times and failed to read a single character.
    if (empty_count >= 5) {
      FAIL("Reached maximum retries while trying to read characters from UART.");
      return;
    }
  }

  // zx_debug_write has a maximum write chunk of 256.
  constexpr size_t kChunkSize = 256;
  size_t written = 0;
  size_t tail = total % kChunkSize;
  while (written < total - tail) {
    ASSERT_OK(zx_debug_write(message().data() + written, kChunkSize));
    written += kChunkSize;
  }
  // write trailing bytes.
  if (tail != 0) {
    ASSERT_OK(zx_debug_write(message().data() + total - tail, tail));
  }
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
}

}  // namespace
