// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

int main() {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"test_program"}).BuildAndInitialize();
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  auto inspector =
      std::make_unique<inspect::ComponentInspector>(loop.dispatcher(), inspect::PublishOptions{});

  inspector->root().RecordString("version", "v1");
  auto option_a = inspector->root().CreateChild("option_a");
  option_a.RecordInt("value", 10);

  auto option_b = inspector->root().CreateChild("option_b");
  option_b.RecordInt("value", 20);

  FX_LOG_KV(INFO, "I'm an info log", FX_KV("tag", "hello"), FX_KV("foo", 100));
  FX_LOG_KV(WARNING, "I'm a warn log", FX_KV("bar", "baz"));
  FX_LOG_KV(ERROR, "I'm an error log");

  loop.Run();
  return 0;
}
