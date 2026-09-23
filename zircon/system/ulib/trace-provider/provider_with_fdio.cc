// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/trace-provider/fdio_connect.h>
#include <lib/trace-provider/provider.h>
#include <lib/zx/process.h>
#include <stdio.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include "export.h"
#include "utils.h"

EXPORT trace_provider_t* trace_provider_create_with_name_fdio(async_dispatcher_t* dispatcher,
                                                              const char* name) {
  ZX_DEBUG_ASSERT(dispatcher);

  zx_handle_t to_service;
  auto status = trace_provider_connect_with_fdio(&to_service);
  if (status != ZX_OK) {
    fprintf(stderr, "TraceProvider: connection failed: status=%d(%s)\n", status,
            zx_status_get_string(status));
    return nullptr;
  }

  return trace_provider_create_with_name(to_service, dispatcher, name);
}

EXPORT trace_provider_t* trace_provider_create_with_fdio(async_dispatcher_t* dispatcher) {
  ZX_DEBUG_ASSERT(dispatcher);

  return trace_provider_create_with_name_fdio(dispatcher, nullptr);
}

EXPORT trace_provider_t* trace_provider_create_synchronously_with_fdio(
    async_dispatcher_t* dispatcher, const char* name, bool* out_manager_is_tracing_already) {
  ZX_DEBUG_ASSERT(dispatcher);

  zx_handle_t to_service;
  auto status = trace_provider_connect_with_fdio(&to_service);
  if (status != ZX_OK) {
    fprintf(stderr, "TraceProvider: connection failed: status=%d(%s)\n", status,
            zx_status_get_string(status));
    return nullptr;
  }

  return trace_provider_create_synchronously(to_service, dispatcher, name,
                                             out_manager_is_tracing_already);
}
