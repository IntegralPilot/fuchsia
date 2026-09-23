// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zxdump/task.h>

#include "live-memory-cache.h"

namespace zxdump {
namespace {

constexpr auto NotFuchsia() {
  return fit::error{Error{"Try running Fuchsia!", ZX_ERR_NOT_SUPPORTED}};
}

}  // namespace

fit::result<Error, LiveHandle> GetRootJob() { return NotFuchsia(); }

fit::result<Error, LiveHandle> GetInfoResource() { return NotFuchsia(); }

fit::result<Error> TaskHolder::InsertSystem() { return NotFuchsia(); }

// There is never really a LiveMemory object created on not-Fuchsia, but
// the call is still compiled in though live-memory-cache.cc is not.
fit::result<Error, Buffer<>> Process::LiveMemory::ReadLiveMemory(
    uint64_t vaddr, size_t size, ReadMemorySize size_mode, const LiveHandle& handle,
    TaskHolder::LiveMemoryCache& shared_cache) {
  return NotFuchsia();
}

// This is called after set_memory_cache_limit; the limit value is maintained,
// but doesn't do anything.
void TaskHolder::LiveMemoryCache::PruneCache() {}

// The LiveMemoryCache object is harmlessly small but still has a destructor
// for the always-empty Page list so this is called only in dead code paths.
TaskHolder::LiveMemoryCache::Page::~Page() { ZX_PANIC("unreachable on not-Fuchsia"); }

}  // namespace zxdump
