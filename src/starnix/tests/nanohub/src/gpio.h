// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_NANOHUB_SRC_GPIO_H_
#define SRC_STARNIX_TESTS_NANOHUB_SRC_GPIO_H_

#include <sys/ioctl.h>

#include <cstdint>

#define MOCK_GPIO_MAX_NAME_SIZE 32
#define MOCK_GPIO_V2_LINES_MAX 64
#define MOCK_GPIO_V2_LINE_NUM_ATTRS_MAX 10

enum mock_gpio_v2_line_flag {
  MOCK_GPIO_V2_LINE_FLAG_USED = 1ULL << 0,
  MOCK_GPIO_V2_LINE_FLAG_ACTIVE_LOW = 1ULL << 1,
  MOCK_GPIO_V2_LINE_FLAG_INPUT = 1ULL << 2,
  MOCK_GPIO_V2_LINE_FLAG_OUTPUT = 1ULL << 3,
  MOCK_GPIO_V2_LINE_FLAG_EDGE_RISING = 1ULL << 4,
  MOCK_GPIO_V2_LINE_FLAG_EDGE_FALLING = 1ULL << 5,
  MOCK_GPIO_V2_LINE_FLAG_OPEN_DRAIN = 1ULL << 6,
  MOCK_GPIO_V2_LINE_FLAG_OPEN_SOURCE = 1ULL << 7,
  MOCK_GPIO_V2_LINE_FLAG_BIAS_PULL_UP = 1ULL << 8,
  MOCK_GPIO_V2_LINE_FLAG_BIAS_PULL_DOWN = 1ULL << 9,
  MOCK_GPIO_V2_LINE_FLAG_BIAS_DISABLED = 1ULL << 10,
  MOCK_GPIO_V2_LINE_FLAG_EVENT_CLOCK_REALTIME = 1ULL << 11,
  MOCK_GPIO_V2_LINE_FLAG_EVENT_CLOCK_HTE = 1ULL << 12,
};

struct mock_gpio_v2_line_attribute {
  uint32_t id;
  uint32_t padding;
  union {
    uint64_t flags;
    uint64_t values;
    uint32_t debounce_period_us;
  };
};

struct mock_gpio_v2_line_config_attribute {
  struct mock_gpio_v2_line_attribute attr;
  uint64_t mask;
};

struct mock_gpio_v2_line_config {
  uint64_t flags;
  uint32_t num_attrs;
  uint32_t padding[5];
  struct mock_gpio_v2_line_config_attribute attrs[MOCK_GPIO_V2_LINE_NUM_ATTRS_MAX];
};

struct mock_gpio_v2_line_request {
  uint32_t offsets[MOCK_GPIO_V2_LINES_MAX];
  char consumer[MOCK_GPIO_MAX_NAME_SIZE];
  struct mock_gpio_v2_line_config config;
  uint32_t num_lines;
  uint32_t event_buffer_size;
  uint32_t padding[5];
  int32_t fd;
};

#ifndef MOCK_GPIO_V2_GET_LINE_IOCTL
#define MOCK_GPIO_V2_GET_LINE_IOCTL _IOWR(0xB4, 0x07, struct mock_gpio_v2_line_request)
#endif

#endif  // SRC_STARNIX_TESTS_NANOHUB_SRC_GPIO_H_
