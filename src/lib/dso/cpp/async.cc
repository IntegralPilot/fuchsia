// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/dso/cpp/async.h"

#include <string.h>
#include <zircon/compiler.h>
#include <zircon/processargs.h>

extern "C" struct dso_async_input {
  uint32_t handle_count;
  zx_handle_t* handle;
  uint32_t* handle_info;
  uint32_t name_count;
  const char** names;
  int argc;
  const char** argv;
  const char** envp;
  fdf_dispatcher_t* dispatcher;
};

typedef struct dso_async_input dso_async_input_t;

#define TAKE_HANDLE(_name_)                 \
  _name_ = input.handle[n];                 \
  input.handle_info[n] = ZX_HANDLE_INVALID; \
  input.handle[n] = ZX_HANDLE_INVALID;

__EXPORT
extern "C" int _dso_start_async(dso_async_input_t input) {
  // Capture handles that we want to pass to dso_main() directly.
  zx_handle_t svc = ZX_HANDLE_INVALID;
  zx_handle_t pkg = ZX_HANDLE_INVALID;
  zx_handle_t directory_request = ZX_HANDLE_INVALID;
  zx_handle_t lifecycle = ZX_HANDLE_INVALID;
  zx_handle_t config = ZX_HANDLE_INVALID;
  for (uint32_t n = 0; n < input.handle_count; ++n) {
    const unsigned arg = PA_HND_ARG(input.handle_info[n]);
    switch (PA_HND_TYPE(input.handle_info[n])) {
      case PA_NS_DIR:
        if (arg < input.name_count) {
          if (strcmp(input.names[arg], "/svc") == 0) {
            TAKE_HANDLE(svc)
          }
          if (strcmp(input.names[arg], "/pkg") == 0) {
            TAKE_HANDLE(pkg)
          }
        }
        break;
      case PA_DIRECTORY_REQUEST:
        TAKE_HANDLE(directory_request)
        break;
      case PA_LIFECYCLE:
        TAKE_HANDLE(lifecycle)
        break;
      case PA_VMO_COMPONENT_CONFIG:
        TAKE_HANDLE(config)
        break;
      default:
        continue;
    }
  }

  return dso_main_async(input.argc, input.argv, input.envp, svc, pkg, directory_request, lifecycle,
                        config, input.dispatcher);
}
