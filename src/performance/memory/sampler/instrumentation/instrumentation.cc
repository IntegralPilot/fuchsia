// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(29)

#include "recorder.h"
#include "scoped_reentrancy_guard.h"

namespace {
// Static variable only used to initialize the `Recorder` at startup
// time as a global constructor. This variable should not be read.
struct RecorderInitializer {
  RecorderInitializer() { memory_sampler::Recorder::Get(); }
} recorder_initializer;
}  // namespace

extern "C" {
// Registers a callback to be called by the Scudo allocator whenever
// it serves an allocation.
//
// Note: this function is technically forbidden to allocate per
// Scudo's contract. We use a `ScopedReentrancyGuard` to prevent
// unbounded recursion when allocating, which seems sufficient in
// practice.
//
// Also disable name lint because the symbol name is load-bearing and is imposed by
// the allocator.
// NOLINTNEXTLINE(bugprone-reserved-identifier)
__attribute__((visibility("default"))) void __scudo_allocate_hook(void* ptr, size_t size) {
  if (memory_sampler::ScopedReentrancyGuard::WouldReenter())
    return;
  memory_sampler::ScopedReentrancyGuard guard;

  auto* recorder = memory_sampler::Recorder::GetIfReady();
  if (recorder == nullptr || recorder->is_disabled())
    return;
  recorder->MaybeRecordAllocation(ptr, size);
}
// Registers a callback to be called by the Scudo allocator whenever
// it serves a deallocation.
//
// Note: this function is technically forbidden to allocate per
// Scudo's contract. We use a `ScopedReentrancyGuard` to prevent
// unbounded recursion, which seems sufficient in practice.
//
// Also disable name lint because the symbol name is load-bearing and is imposed by
// the allocator.
// NOLINTNEXTLINE(bugprone-reserved-identifier)
__attribute__((visibility("default"))) void __scudo_deallocate_hook(void* ptr) {
  if (memory_sampler::ScopedReentrancyGuard::WouldReenter())
    return;
  memory_sampler::ScopedReentrancyGuard guard;

  auto* recorder = memory_sampler::Recorder::GetIfReady();
  if (recorder == nullptr || recorder->is_disabled())
    return;
  recorder->MaybeForgetAllocation(ptr);
}
}

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(29)
