// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/dso/cpp/sync.h"

#include <zircon/compiler.h>
#include <zircon/types.h>

extern "C" struct dso_sync_input {
  uint32_t handle_count;
  zx_handle_t* handle;
  uint32_t* handle_info;
  uint32_t name_count;
  const char** names;
  int argc;
  const char** argv;
  const char** envp;
};

typedef struct dso_sync_input dso_sync_input_t;

__EXPORT
extern "C" int _dso_start(dso_sync_input_t input) {
  // TODO(https://fxbug.dev/403545512): Add thread-local support to fdio and libc initialization
  // so that every synchronous DSO component can have its own virtual namespace.

  //__libc_extensions_init(input.handle_count, input.handle, input.handle_info, input.name_count,
  // input.names);
  // Give any unclaimed handles to fdio_take_startup_handle(). This function
  // takes ownership of the data, but not the memory: it assumes that the
  // arrays are valid as long as the component is alive.
  // fdio_startup_handles_init_tls(input.handle_count, input.handle, input.handle_info);
  const int code = dso_main(input.argc, input.argv, input.envp);
  //__libc_extensions_fini();
  return code;
}
