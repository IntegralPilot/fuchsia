// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_MEMORY_SAMPLER_INSTRUMENTATION_RECORDER_H_
#define SRC_PERFORMANCE_MEMORY_SAMPLER_INSTRUMENTATION_RECORDER_H_

#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(29)
#include <fidl/fuchsia.memory.sampler/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/socket.h>

#include <atomic>
#include <unordered_set>

#include <fbl/macros.h>
#include <fbl/mutex.h>

#include "poisson_sampler.h"

namespace memory_sampler {

// Allocation recorder. This class is designed to be initialized in
// static storage, to be thread safe, and to be suitable for use
// during an allocation or a deallocation (by being safe to call
// from within the scudo allocation hooks).
//
// It establishes a FIDL connection to a sampling profiler on
// startup, reports allocations and deallocations, and relevant
// information to support symbolization of the collected profiles.
class Recorder {
 public:
  DISALLOW_COPY_ASSIGN_AND_MOVE(Recorder);
  // Returns a reference to the singleton object, initializing (and
  // blocking) it if necessary.
  static Recorder *Get();
  // Returns a pointer to the singleton object if it is initialized;
  // returns nullptr otherwise.
  static Recorder *GetIfReady();
  // Decides whether to discard or sample this allocation, and acts
  // appropriately.
  void MaybeRecordAllocation(void *address, size_t size) __TA_EXCLUDES(&lock_);
  // Decides whether to discard or sample this deallocation, and acts
  // appropriately.
  void MaybeForgetAllocation(void *address) __TA_EXCLUDES(&lock_);
  // Collects the module layout of the current process and
  // communicates it to the profiler.
  void SetModulesInfo();

  // Convenience factory method for use in tests. This eschews the
  // singleton interface, and supports providing a custom FIDL
  // client. It does not perform any of the initialisations handled by
  // the singleton interface. This should not be used outside of tests.
  static Recorder CreateRecorderForTesting(fidl::SyncClient<fuchsia_memory_sampler::Sampler> client,
                                           std::function<PoissonSampler &()> get_poisson_sampler,
                                           bool use_socket = true);

  // The average count of bytes allocated between two samples.
  static constexpr size_t kSamplingIntervalBytes = static_cast<size_t>(128 * 1024);

  // Returns true if the recorder has disconnected from the profiler.
  bool is_disabled() const { return is_disabled_.load(std::memory_order_relaxed); }

  // Disconnects the recorder from the profiler and disables all subsequent recording.
  void Disconnect() __TA_EXCLUDES(&lock_);

 private:
  // Grants tests access to internal state. Unlike
  // `CreateRecorderForTesting`, which only constructs an instance, the state
  // tests need to manipulate here is an implementation detail that should not
  // become part of this class' API.
  friend struct RecorderTestPeer;

  Recorder(fidl::SyncClient<fuchsia_memory_sampler::Sampler> client, zx::socket socket,
           std::function<PoissonSampler &()> get_poisson_sampler);
  // Initializes the singleton into statically-allocated storage.
  static void InitSingletonOnce();
  fbl::Mutex lock_;
  fidl::SyncClient<fuchsia_memory_sampler::Sampler> client_ __TA_GUARDED(&lock_);
  zx::socket socket_;
  std::atomic<bool> is_disabled_{false};
  std::atomic<bool> peer_signaled_{false};
  std::unordered_set<void *> recorded_allocations_ __TA_GUARDED(&lock_);
  std::function<PoissonSampler &()> GetPoissonSampler;

  // Records an allocation's address and size and communicates it to
  // the profiler. Returns true if the allocation was successfully recorded.
  bool RecordAllocation(void *address, size_t size) __TA_EXCLUDES(&lock_);
  // Records a deallocation's address and communicates it to the
  // profiler.
  void ForgetAllocation(void *address) __TA_EXCLUDES(&lock_);
};
}  // namespace memory_sampler

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(29)
#endif  // SRC_PERFORMANCE_MEMORY_SAMPLER_INSTRUMENTATION_RECORDER_H_
