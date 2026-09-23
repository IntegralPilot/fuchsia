// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace_manager/trace_provider_bundle.h"

#include <iostream>

#include <src/lib/fxl/strings/string_printf.h>

namespace tracing {

TraceProviderBundle::TraceProviderBundle(
    fidl::ClientEnd<fuchsia_tracing_provider::Provider> provider, uint32_t id, zx_koid_t pid,
    const std::string& name, async_dispatcher_t* dispatcher)
    : provider(std::move(provider), dispatcher, this), id(id), pid(pid), name(name) {}

void TraceProviderBundle::on_fidl_error(fidl::UnbindInfo info) {
  if (on_unbound_) {
    on_unbound_(info);
  }
}

std::string TraceProviderBundle::ToString() const {
  // The pid and name should be present, so we don't try to get fancy with
  // the formatting if it turns out they're not.
  return fxl::StringPrintf("#%u {%lu:%s}", id, pid, name.c_str());
}

std::ostream& operator<<(std::ostream& out, const TraceProviderBundle& bundle) {
  return out << bundle.ToString();
}

}  // namespace tracing
