// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_MEMORY_THRASHER_LIB_H_
#define SRC_PERFORMANCE_MEMORY_THRASHER_LIB_H_

#include <fidl/fuchsia.fxfs/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>

#include <cstdint>
#include <limits>
#include <memory>
#include <string>
#include <vector>

struct ThrashStatus {
  std::string thrasher_type;
  uint64_t total_memory_bytes;
  uint64_t touches_delta;
  uint64_t total_touches;
  uint64_t distinct_pages_delta;
  zx::duration time_delta;
  zx::duration total_time;
};

using ThrashCallback = fit::function<void(std::vector<zx::vmo>)>;
using StatusCallback = fit::function<void(const ThrashStatus&)>;

struct ThrashConfig {
  int bursts_per_second = 100;
  int run_for_seconds = 60;
  int num_threads = 1;
  int pages_per_read = 1;
  int consecutive_pages_per_read = 1;
  async_dispatcher_t* dispatcher;
  bool verbose = false;
  int status_interval_ms = 1000;
  fit::function<void(zx::duration)> teardown_sleep_fn;
};

// Abstract base class for all thrashers.
class Thrasher {
 public:
  virtual ~Thrasher() = default;

  // Starts asynchronous initialization. Calls |on_initialized| when ready to start.
  // |on_initialized| will be called with ZX_OK on success, or an error status on failure.
  virtual void Initialize(fit::function<void(zx_status_t)> on_initialized) = 0;

  // Starts the actual thrashing workload. Must be called after successful Initialize().
  virtual void Start(std::shared_ptr<ThrashCallback> callback,
                     std::shared_ptr<StatusCallback> status_callback) = 0;
};

// Factory functions for creating specific thrashers.
std::shared_ptr<Thrasher> CreateAnonThrasher(ThrashConfig config, size_t buffer_size_bytes);
std::shared_ptr<Thrasher> CreateMmapThrasher(ThrashConfig config, std::string filename);
std::shared_ptr<Thrasher> CreateDirThrasher(ThrashConfig config, std::string dirname);
std::shared_ptr<Thrasher> CreateBlobThrasher(ThrashConfig config,
                                             const std::vector<std::string>& merkle_roots,
                                             size_t max_blob_size_bytes);

// Exposed for testing
std::shared_ptr<Thrasher> CreateBlobThrasherWithClient(
    ThrashConfig config, fidl::ClientEnd<fuchsia_fxfs::BlobReader> blob_reader_client,
    const std::vector<std::string>& merkle_roots, size_t max_blob_size_bytes);
void LogVmos(const std::vector<zx::vmo>& vmos, bool verbose);

#endif  // SRC_PERFORMANCE_MEMORY_THRASHER_LIB_H_
