// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.process.lifecycle/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/process.h>
#include <zircon/processargs.h>

#include <atomic>

#include "src/lib/dso/cpp/sync.h"

namespace {

// Use an atomic because we expect threads to run in parallel in this test.
std::atomic_uint32_t run_counter{0};

class LifecycleHandler : public fidl::Server<fuchsia_process_lifecycle::Lifecycle> {
 public:
  LifecycleHandler(fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle> server_end,
                   async::Loop& loop)
      : loop_(loop),
        binding_(loop.dispatcher(), std::move(server_end), this, fidl::kIgnoreBindingClosure) {}

 private:
  void Stop(StopCompleter::Sync& completer) override {
    binding_.Close(ZX_OK);
    loop_.Quit();
  }

  async::Loop& loop_;
  fidl::ServerBinding<fuchsia_process_lifecycle::Lifecycle> binding_;
};

}  // namespace

__EXPORT
extern "C" uint32_t waiting_sync_read_run_counter() { return run_counter.load(); }

int dso_main(int argc, const char** argv, const char** envp) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  run_counter.fetch_add(1);

  fidl::ServerEnd lifecycle = fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle>{
      zx::channel(zx_take_startup_handle(PA_LIFECYCLE))};
  ZX_ASSERT(lifecycle.is_valid());
  LifecycleHandler handler(std::move(lifecycle), loop);
  loop.Run();

  return 0;
}
