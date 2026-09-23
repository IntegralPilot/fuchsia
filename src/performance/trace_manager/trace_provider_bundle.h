// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_MANAGER_TRACE_PROVIDER_BUNDLE_H_
#define SRC_PERFORMANCE_TRACE_MANAGER_TRACE_PROVIDER_BUNDLE_H_

#include <fidl/fuchsia.tracing.provider/cpp/fidl.h>
#include <stdint.h>

#include <iosfwd>
#include <map>
#include <string>

namespace tracing {

struct TraceProviderBundle : public fidl::AsyncEventHandler<fuchsia_tracing_provider::Provider> {
  TraceProviderBundle(fidl::ClientEnd<fuchsia_tracing_provider::Provider> provider, uint32_t id,
                      zx_koid_t pid, const std::string& name, async_dispatcher_t* dispatcher);
  ~TraceProviderBundle() override = default;

  TraceProviderBundle(const TraceProviderBundle& value) = delete;
  TraceProviderBundle& operator=(const TraceProviderBundle&) = delete;

  TraceProviderBundle(TraceProviderBundle&& value) = default;
  TraceProviderBundle& operator=(TraceProviderBundle&&) = default;

  void on_fidl_error(fidl::UnbindInfo info) override;

  void SetOnUnbound(fit::function<void(fidl::UnbindInfo)> on_unbound) {
    on_unbound_ = std::move(on_unbound);
  }

  std::string ToString() const;

  fidl::Client<fuchsia_tracing_provider::Provider> provider;

  uint32_t id;
  zx_koid_t pid;
  std::string name;

 private:
  fit::function<void(fidl::UnbindInfo)> on_unbound_;
};

struct TraceProviderSpec {
  std::optional<uint32_t> buffer_size_megabytes;
  std::vector<std::string> categories;
};

using TraceProviderSpecMap = std::map<std::string, TraceProviderSpec>;

std::ostream& operator<<(std::ostream& out, const TraceProviderBundle& bundle);

}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_MANAGER_TRACE_PROVIDER_BUNDLE_H_
