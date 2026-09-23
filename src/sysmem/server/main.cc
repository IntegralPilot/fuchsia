// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/trace-provider/provider.h>

#include "src/sysmem/server/app.h"
#include "src/sysmem/server/sysmem_config.h"

int main(int argc, const char** argv) {
  // kAsyncLoopConfigAttachToCurrentThread is currently required by
  // component::Outgoing() which can currently only construct using
  // async_get_default_dispatcher().
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"sysmem"}).BuildAndInitialize();
  trace::TraceProviderWithFdio trace_provider(loop.dispatcher());
  App app(loop.dispatcher());
  loop.Run();
  // sysmem should never exit (it should continue serving its out dir forever) unless it aborts
  // or crashes, so return -1 to indicate an error has occurred and trigger the
  // `on_terminate: "reboot"` component setting.
  return -1;
}
