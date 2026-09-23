// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/validate/logs/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include "lib/syslog/cpp/log_level.h"
#include "lib/syslog/cpp/log_settings.h"

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

class Puppet : public fuchsia::validate::logs::LogSinkPuppet {
 public:
  explicit Puppet(std::unique_ptr<sys::ComponentContext> context) : context_(std::move(context)) {
    context_->outgoing()->AddPublicService(sink_bindings_.GetHandler(this));
  }

  void GetInfo(GetInfoCallback callback) override {
    fuchsia::validate::logs::PuppetInfo info;
    info.pid = GetKoid(zx_process_self());
    info.tid = GetKoid(zx_thread_self());
    callback(info);
  }

  void EmitLog(fuchsia::validate::logs::RecordSpec spec, EmitLogCallback callback) override {
    if (fuchsia_logging::IsSeverityEnabled(spec.record.severity)) {
      auto builder = fuchsia_logging::LogBufferBuilder(static_cast<uint8_t>(spec.record.severity));
      auto buffer = builder.WithFile(spec.file, spec.line).Build();
      for (auto& arg : spec.record.arguments) {
        switch (arg.value.Which()) {
          case fuchsia::validate::logs::Value::kUnknown:
          case fuchsia::validate::logs::Value::Invalid:
            break;
          case fuchsia::validate::logs::Value::kFloating:
            buffer.WriteKeyValue(arg.name, arg.value.floating());
            break;
          case fuchsia::validate::logs::Value::kSignedInt:
            buffer.WriteKeyValue(arg.name, arg.value.signed_int());
            break;
          case fuchsia::validate::logs::Value::kUnsignedInt:
            buffer.WriteKeyValue(arg.name, arg.value.unsigned_int());
            break;
          case fuchsia::validate::logs::Value::kText:
            buffer.WriteKeyValue(arg.name, arg.value.text().data());
            break;
          case fuchsia::validate::logs::Value::kBoolean:
            buffer.WriteKeyValue(arg.name, arg.value.boolean());
            break;
        }
      }
      [[maybe_unused]] zx::result<> result = fuchsia_logging::FlushToGlobalLogger(buffer);
    }
    callback();
  }

 private:
  std::unique_ptr<sys::ComponentContext> context_;
  fidl::BindingSet<fuchsia::validate::logs::LogSinkPuppet> sink_bindings_;
};

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  fuchsia_logging::LogSettingsBuilder log_settings;
  log_settings.WithDispatcher(loop.dispatcher())
      .WithSeverityChangedListener(
          nullptr, +[](void*, fuchsia_logging::RawLogSeverity severity) {
            auto builder = fuchsia_logging::LogBufferBuilder(severity);
            auto buffer = builder.WithFile(__FILE__, __LINE__).WithMsg("Changed severity").Build();
            [[maybe_unused]] zx::result<> result = fuchsia_logging::FlushToGlobalLogger(buffer);
          });
  log_settings.BuildAndInitialize();
  Puppet puppet(sys::ComponentContext::CreateAndServeOutgoingDirectory());
  // Note: This puppet is ran by a runner that
  // uses --test-invalid-unicode, which isn't directly passed to this puppet
  // but tells the Rust puppet runner that we are sending invalid UTF-8.
  FX_LOGS(INFO) << "Puppet started.\xc3\x28";
  loop.Run();
}
