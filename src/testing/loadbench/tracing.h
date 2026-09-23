// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTING_LOADBENCH_TRACING_H_
#define SRC_TESTING_LOADBENCH_TRACING_H_

#include <lib/zircon-internal/ktrace.h>

#include <array>
#include <iterator>
#include <map>
#include <string>
#include <vector>

#include <trace-reader/reader.h>
#include <trace-reader/records.h>

#include "utility.h"

class Tracing {
 public:
  Tracing();

  Tracing(const Tracing&) = delete;
  Tracing& operator=(const Tracing&) = delete;

  Tracing(Tracing&&) = delete;
  Tracing& operator=(Tracing&&) = delete;

  virtual ~Tracing() {
    Stop();
    buffer_.reset();
    buffer_capacity_ = 0;
    buffer_size_ = 0;
  }

  struct DurationStats {
    uint64_t begin_ts_ns = 0;
    uint64_t end_ts_ns = 0;
    uint64_t wall_duration_ns = 0;
    std::vector<trace::Argument> arguments;

    explicit DurationStats(uint64_t begin) : begin_ts_ns(begin) {}
    DurationStats(uint64_t begin, uint64_t end, uint64_t wall_duration_ns,
                  std::vector<trace::Argument> arguments)
        : begin_ts_ns(begin),
          end_ts_ns(end),
          wall_duration_ns(wall_duration_ns),
          arguments(std::move(arguments)) {}
  };

  struct QueuingStats {
    uint64_t begin_ts_ns = 0;
    uint64_t end_ts_ns = 0;
    uint64_t queuing_time_ns = 0;
    uint64_t associated_thread = 0;

    QueuingStats(uint64_t begin, uint64_t thread) {
      begin_ts_ns = begin;
      associated_thread = thread;
    }
  };

  // Rewinds kernel trace buffer.
  void Rewind();

  // Starts kernel tracing.
  void Start(uint32_t group_mask);

  // Stops kernel tracing.
  void Stop();

  // Reads trace buffer and converts output into human-readable format. Stores in location defined
  // by <filepath>. Will overwrite any existing files with same name.
  bool WriteHumanReadable(std::ostream& filepath);

  // Picks out traces pertaining to name in string_ref and runs stats on them. Returns false if name
  // not found.
  bool PopulateDurationStats(std::string string_ref, std::vector<DurationStats>* duration_stats,
                             std::map<uint64_t, QueuingStats>* queuing_stats);

  bool running() const { return running_; }

 private:
  enum EventState {
    kBegin,
    kEnd,
    kNone,
  };

  // Performs same action as zx_ktrace_read and does necessary checks.
  virtual void ReadKernelBuffer(zx_handle_t handle, void* data_buf, uint32_t offset, size_t len,
                                size_t* bytes_read);
  size_t ReadKernelRecords(trace::TraceReader::RecordConsumer consumer,
                           trace::TraceReader::ErrorHandler error_handler);

  zx_handle_t tracing_resource_ = GetTracingResource()->get();
  bool running_ = false;

  // An intermediate heap allocated buffer used to store ktrace data.
  std::unique_ptr<uint8_t[]> buffer_{nullptr};
  // The capacity of buffer_.
  size_t buffer_capacity_{0};
  // The amount of data in the buffer.
  size_t buffer_size_{0};
};

#endif  // SRC_TESTING_LOADBENCH_TRACING_H_
