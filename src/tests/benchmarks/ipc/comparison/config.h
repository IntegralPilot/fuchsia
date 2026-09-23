// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_BENCHMARKS_IPC_COMPARISON_CONFIG_H_
#define SRC_TESTS_BENCHMARKS_IPC_COMPARISON_CONFIG_H_

#include <cstddef>

struct BatchConfig {
  size_t messages_to_send;
  size_t batch_size;
  size_t message_size;
};

struct StreamConfig {
  size_t messages_to_send;
  size_t batch_size = 1;
  size_t message_size;
};

struct SequentialConfig {
  size_t messages_to_send;
  size_t message_size;
  size_t channel_capacity;
};

struct VmoConfig {
  size_t messages_to_send;
  size_t message_size;
  size_t batch_size;
  size_t per_vmo_batch;
};

#endif  // SRC_TESTS_BENCHMARKS_IPC_COMPARISON_CONFIG_H_
