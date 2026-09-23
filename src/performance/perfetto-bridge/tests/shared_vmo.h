// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_PERFETTO_BRIDGE_TESTS_SHARED_VMO_H_
#define SRC_PERFORMANCE_PERFETTO_BRIDGE_TESTS_SHARED_VMO_H_

#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <memory>

#include <perfetto/ext/tracing/core/shared_memory.h>

/// An adapter class that allows a Fuchsia VMO to be passed to Perfetto APIs in a format they can
/// handle.
class SharedVmo : public perfetto::SharedMemory {
 public:
  SharedVmo(zx::vmo vmo, zx::vmar vmar, void* addr, size_t size);
  class Factory : public SharedMemory::Factory {
   public:
    ~Factory() override = default;
    std::unique_ptr<SharedMemory> CreateSharedMemory(size_t) override;
  };

  static std::unique_ptr<SharedVmo> AdoptVmo(zx::vmo vmo);
  // The transport layer is expected to tear down the resource associated to
  // this object region when destroyed.
  ~SharedVmo() override;
  const void* start() const override;
  size_t size() const override;

 private:
  zx::vmo vmo_;
  zx::vmar vmar_;
  void* addr_;
  size_t size_;
};
#endif  // SRC_PERFORMANCE_PERFETTO_BRIDGE_TESTS_SHARED_VMO_H_
