// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_PLATFORM_TRACE_PROVIDER_H
#define ZIRCON_PLATFORM_TRACE_PROVIDER_H

#if MAGMA_ENABLE_TRACING
#include <lib/async-loop/cpp/loop.h>
#include <lib/trace-provider/provider.h>
#endif

#include <lib/magma/platform/platform_trace_provider.h>

#include <memory>
#include <vector>

namespace magma {

#if MAGMA_ENABLE_TRACING
class ZirconPlatformTraceProvider : public PlatformTraceProvider {
 public:
  ZirconPlatformTraceProvider();
  ~ZirconPlatformTraceProvider() override;

  bool Initialize(uint32_t channel) override;
  bool IsInitialized() override { return bool(trace_provider_); }

 private:
  async::Loop loop_;
  std::unique_ptr<trace::TraceProvider> trace_provider_;
};

#endif

}  // namespace magma

#endif  // ZIRCON_PLATFORM_TRACE_H
