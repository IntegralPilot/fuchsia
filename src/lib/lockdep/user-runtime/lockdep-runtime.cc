// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <atomic>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <mutex>
#include <thread>

#include <lockdep/lock_class_state.h>
#include <lockdep/runtime_api.h>
#include <lockdep/thread_lock_state.h>

#if defined(__Fuchsia__)
#include "src/lib/debug/backtrace-request.h"  // nogncheck
#elif defined(__linux__) || defined(__APPLE__)
#include <execinfo.h>
#include <unistd.h>
#endif

namespace lockdep {

namespace {

std::atomic<bool> loop_detection_graph_is_dirty{false};
std::mutex loop_detector_mutex;
std::condition_variable loop_detector_cv;
std::once_flag init_thread_once;

void LockDepThread() {
  while (true) {
    std::unique_lock<std::mutex> lock(loop_detector_mutex);
    loop_detector_cv.wait_for(lock, std::chrono::seconds(2),
                              [] { return loop_detection_graph_is_dirty.load(); });

    if (loop_detection_graph_is_dirty.exchange(false)) {
      lockdep::LoopDetectionPass();
    }
  }
}

}  // namespace

// System-defined hook to report detected lock validation failures.
void SystemLockValidationError(AcquiredLockEntry* lock_entry, AcquiredLockEntry* conflicting_entry,
                               ThreadLockState* state, void* caller_address, void* caller_frame,
                               LockResult result) {
  std::fprintf(stderr, "====================================================\n");
  std::fprintf(stderr, "LOCK DEP ERROR: Lock validation failed!\n");
  std::fprintf(stderr, "Reason: %s\n", ToString(result));
  std::fprintf(stderr, "Bad lock: name=%s order=%lu address=%p\n",
               ValidatorLockClassState::GetName(lock_entry->id()),
               static_cast<unsigned long>(lock_entry->order()), lock_entry->address());
  std::fprintf(stderr, "Conflict: name=%s order=%lu address=%p\n",
               ValidatorLockClassState::GetName(conflicting_entry->id()),
               static_cast<unsigned long>(conflicting_entry->order()),
               conflicting_entry->address());
  std::fprintf(stderr, "caller=%p frame=%p\n", caller_address, caller_frame);
  std::fprintf(stderr, "====================================================\n");

#if defined(__Fuchsia__)
  backtrace_request_current_thread();
#elif defined(__linux__) || defined(__APPLE__)
  void* array[128];
  int size = backtrace(array, 128);
  std::fprintf(stderr, "Backtrace:\n");
  backtrace_symbols_fd(array, size, STDERR_FILENO);
#endif
}

// System-defined hook to abort the program due to a fatal lock violation.
void SystemLockValidationFatal(AcquiredLockEntry* lock_entry, ThreadLockState* state,
                               void* caller_address, void* caller_frame, LockResult result) {
  std::fprintf(stderr, "====================================================\n");
  std::fprintf(stderr, "LOCK DEP FATAL ERROR: Fatal lock violation detected!\n");
  std::fprintf(stderr, "Reason: %s\n", ToString(result));
  std::fprintf(stderr, "Lock: name=%s order=%lu address=%p\n",
               ValidatorLockClassState::GetName(lock_entry->id()),
               static_cast<unsigned long>(lock_entry->order()), lock_entry->address());
  std::fprintf(stderr, "pc=%p stack frame=%p\n", caller_address, caller_frame);
  std::fprintf(stderr, "====================================================\n");

#if defined(__Fuchsia__)
  backtrace_request_current_thread();
#elif defined(__linux__) || defined(__APPLE__)
  void* array[128];
  int size = backtrace(array, 128);
  std::fprintf(stderr, "Backtrace:\n");
  backtrace_symbols_fd(array, size, STDERR_FILENO);
#endif

  std::abort();
}

// System-defined hook to report detection of a circular lock dependency.
void SystemCircularLockDependencyDetected(ValidatorLockClassState* connected_set_root) {
  std::fprintf(stderr, "====================================================\n");
  std::fprintf(stderr, "LOCK DEP ERROR: Circular lock dependency detected:\n");

  for (auto& node : ValidatorLockClassState::Iter()) {
    if (node.connected_set() == connected_set_root) {
      for (LockClassId dependency_id : node.dependency_set()) {
        ValidatorLockClassState* dependency = ValidatorLockClassState::Get(dependency_id);
        if (dependency->connected_set() == connected_set_root) {
          std::fprintf(stderr, "  %s -> %s\n", dependency->name(), node.name());
        }
      }
    }
  }
  std::fprintf(stderr, "====================================================\n");
}

// System-defined hook that returns the ThreadLockState instance for the current
// thread.
ThreadLockState* SystemGetThreadLockState(LockFlags lock_flags) {
  thread_local ThreadLockState thread_lock_state{};
  return &thread_lock_state;
}

// System-defined hook that initializes the ThreadLockState for the current thread.
void SystemInitThreadLockState(ThreadLockState* state) {}

// System-defined hook that triggers a loop detection pass.
void SystemTriggerLoopDetection() {
  loop_detection_graph_is_dirty.store(true);
  std::call_once(init_thread_once, [] { std::thread(LockDepThread).detach(); });
  loop_detector_cv.notify_one();
}

}  // namespace lockdep
