// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.ipc/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmo.h>

#include <thread>

#include <perftest/results.h>

#include "batch_case.h"
#include "sequential_case.h"
#include "stream_case.h"
#include "timer.h"
#include "vmo_case.h"

const size_t MESSAGES_TO_SEND = 90000;
const size_t MESSAGE_SIZE = 2000;
const size_t BATCH_SIZE = 30;
const size_t RUNS_PER_CONFIG = 10;
const size_t CHANNEL_CAPACITY = 128;
const size_t PER_VMO_BATCH = 64;
static_assert(MESSAGES_TO_SEND % BATCH_SIZE == 0, "Expect that we fill each batch exactly");

int main(int argc, const char** argv) {
  // Create the main async event loop.
  // Note: The async loop is created and managed within each recv_* function.

  const char* output_file = "/custom_artifacts/fuchsiaperf.json";
  if (argc > 1) {
    if (strcmp(argv[1], "--out") != 0 || argc < 3) {
      FX_LOGS(ERROR) << "This program expects only one argument called --out.";
      return 1;
    }
    output_file = argv[2];
  }

  Timing batch_timings[RUNS_PER_CONFIG];
  Timing stream_timings[RUNS_PER_CONFIG];
  Timing batch_stream_timings[RUNS_PER_CONFIG];
  Timing sequential_timings[RUNS_PER_CONFIG];
  Timing vmo_timings[RUNS_PER_CONFIG];

  if (MESSAGE_SIZE * BATCH_SIZE < 63 * 1024) {
    for (size_t i = 0; i < RUNS_PER_CONFIG; ++i) {
      FX_LOGS(INFO) << "Running batch...";
      zx::channel x1, x2;
      ZX_ASSERT(zx::channel::create(0, &x1, &x2) == ZX_OK);
      BatchCase batch_case(BatchConfig{.messages_to_send = MESSAGES_TO_SEND,
                                       .batch_size = BATCH_SIZE,
                                       .message_size = MESSAGE_SIZE});
      auto t1 = std::thread(&BatchCase::send, &batch_case, std::move(x1), &batch_timings[i]);
      auto t2 = std::thread(&BatchCase::recv, &batch_case, std::move(x2), &batch_timings[i]);
      t1.join();
      t2.join();
      FX_LOGS(INFO) << "Done batch.";
    }
  } else {
    FX_LOGS(INFO) << "Skipping batch, payload too big.";
  }

  for (size_t i = 0; i < RUNS_PER_CONFIG; ++i) {
    FX_LOGS(INFO) << "Running stream...";
    zx::channel x1, x2;
    ZX_ASSERT(zx::channel::create(0, &x1, &x2) == ZX_OK);
    StreamCase stream_case(StreamConfig{
        .messages_to_send = MESSAGES_TO_SEND, .batch_size = 1, .message_size = MESSAGE_SIZE});
    auto t1 = std::thread(&StreamCase::send, &stream_case, std::move(x1), &stream_timings[i]);
    auto t2 = std::thread(&StreamCase::recv, &stream_case, std::move(x2), &stream_timings[i]);
    t1.join();
    t2.join();
    FX_LOGS(INFO) << "Done stream.";
  }

  for (size_t i = 0; i < RUNS_PER_CONFIG; ++i) {
    FX_LOGS(INFO) << "Running batch stream...";
    zx::channel x1, x2;
    ZX_ASSERT(zx::channel::create(0, &x1, &x2) == ZX_OK);
    StreamCase stream_case(StreamConfig{.messages_to_send = MESSAGES_TO_SEND,
                                        .batch_size = BATCH_SIZE,
                                        .message_size = MESSAGE_SIZE});
    auto t1 = std::thread(&StreamCase::send, &stream_case, std::move(x1), &batch_stream_timings[i]);
    auto t2 = std::thread(&StreamCase::recv, &stream_case, std::move(x2), &batch_stream_timings[i]);
    t1.join();
    t2.join();
    FX_LOGS(INFO) << "Done batch stream.";
  }

  if (MESSAGE_SIZE < 63 * 1024) {
    for (size_t i = 0; i < RUNS_PER_CONFIG; ++i) {
      FX_LOGS(INFO) << "Running sequential...";
      zx::channel x1, x2;
      ZX_ASSERT(zx::channel::create(0, &x1, &x2) == ZX_OK);
      SequentialCase sequential_case(SequentialConfig{.messages_to_send = MESSAGES_TO_SEND,
                                                      .message_size = MESSAGE_SIZE,
                                                      .channel_capacity = CHANNEL_CAPACITY});
      auto t1 = std::thread(&SequentialCase::send, &sequential_case, std::move(x1),
                            &sequential_timings[i]);
      auto t2 = std::thread(&SequentialCase::recv, &sequential_case, std::move(x2),
                            &sequential_timings[i]);
      t1.join();
      t2.join();
      FX_LOGS(INFO) << "Done sequential.";
    }
  } else {
    FX_LOGS(INFO) << "Skipping sequential, payload too big.";
  }

  for (size_t i = 0; i < RUNS_PER_CONFIG; ++i) {
    FX_LOGS(INFO) << "Running VMO...";
    zx::channel x1, x2;
    ZX_ASSERT(zx::channel::create(0, &x1, &x2) == ZX_OK);
    VmoCase vmo_case(VmoConfig{.messages_to_send = MESSAGES_TO_SEND,
                               .message_size = MESSAGE_SIZE,
                               .batch_size = BATCH_SIZE,
                               .per_vmo_batch = PER_VMO_BATCH});
    auto t1 = std::thread(&VmoCase::send, &vmo_case, std::move(x1), &vmo_timings[i]);
    auto t2 = std::thread(&VmoCase::recv, &vmo_case, std::move(x2), &vmo_timings[i]);
    t1.join();
    t2.join();
    FX_LOGS(INFO) << "Done VMO.";
  }

  const char* suite = "fuchsia.ipc.comparison";
  struct BenchData {
    const char* name;
    const Timing* timings;

    BenchData(const char* name, const Timing* timings) : name(name), timings(timings) {}
  };

  BenchData bench_data[] = {
      BenchData("Batch/recv", batch_timings), BenchData("Stream/recv", stream_timings),
      BenchData("BatchStream/recv", batch_stream_timings),
      BenchData("Sequential/recv", sequential_timings), BenchData("VMO/recv", vmo_timings)};

  perftest::ResultsSet results;
  for (const auto& data : bench_data) {
    if (data.timings->send_duration.load() == 0) {
      continue;
    }
    auto* test_case = results.AddTestCase(suite, data.name, "nanoseconds");
    for (size_t i = 0; i < RUNS_PER_CONFIG; ++i) {
      test_case->AppendValue(data.timings[i].recv_seconds() * 1e9 / MESSAGES_TO_SEND);
    }
  };

  results.PrintSummaryStatistics(stdout);
  ZX_ASSERT(results.WriteJSONFile(output_file));
  FX_LOGS(INFO) << "Wrote results to " << output_file;

  // Run the loop until it is shutdown.
  return 0;
}
