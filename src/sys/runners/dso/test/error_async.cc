// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/dso/cpp/async.h"

int dso_main_async(int argc, const char** argv, const char** envp, zx_handle_t svc, zx_handle_t pkg,
                   zx_handle_t directory_request, zx_handle_t lifecycle, zx_handle_t config,
                   fdf_dispatcher_t* dispatcher) {
  // Returning a non-zero code here will cause the runner to immediately terminate the component,
  // there is no need to close the lifecycle channel here.
  return 1;
}
