// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.process.lifecycle/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fdf/dispatcher.h>
#include <stdint.h>
#include <string.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <atomic>

#include "src/lib/dso/cpp/async.h"

namespace {

// Use an atomic because we expect threads to run in parallel in this test.
std::atomic_uint32_t run_counter{0};

class LifecycleHandler : public fidl::Server<fuchsia_process_lifecycle::Lifecycle> {
 public:
  static void Create(zx_handle_t lifecycle, async_dispatcher_t* dispatcher) {
    fidl::ServerEnd server_end =
        fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle>{zx::channel(lifecycle)};
    ZX_ASSERT(server_end.is_valid());
    new LifecycleHandler(std::move(server_end), dispatcher);
  }

 private:
  LifecycleHandler(fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> server_end,
                   async_dispatcher_t* dispatcher)
      : binding_(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure) {}

  void Stop(StopCompleter::Sync& completer) override {
    binding_.Close(ZX_OK);
    delete this;
  }

  fidl::ServerBinding<fuchsia_process_lifecycle::Lifecycle> binding_;
};

}  // namespace

__EXPORT
extern "C" uint32_t waiting_async_read_run_counter() { return run_counter.load(); }

int dso_main_async(int argc, const char** argv, const char** envp, zx_handle_t _svc,
                   zx_handle_t _pkg, zx_handle_t _directory_request, zx_handle_t lifecycle,
                   zx_handle_t _config, fdf_dispatcher_t* fdf_dispatcher) {
  run_counter.fetch_add(1);

  async_dispatcher_t* const dispatcher = fdf_dispatcher_get_async_dispatcher(fdf_dispatcher);
  async::PostTask(dispatcher,
                  [dispatcher, lifecycle] { LifecycleHandler::Create(lifecycle, dispatcher); });

  return 0;
}
