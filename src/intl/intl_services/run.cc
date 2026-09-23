// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/intl/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include "lib/inspect/cpp/health.h"
#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/lib/intl/intl_property_provider_impl/intl_property_provider_impl.h"
#include "src/lib/intl/time_zone_info/time_zone_info_service.h"

using intl::IntlPropertyProviderImpl;
using intl::TimeZoneInfoService;

namespace intl {

namespace {

// Use this as the name of the health node monitoring this set of services.
constexpr char kHealthNodeName[] = "fuchsia.intl.PropertyProvider";

void init(int argc, const char** argv) {
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  if (!fxl::SetLogSettingsFromCommandLine(command_line)) {
    exit(EXIT_FAILURE);
  }
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"intl_services"}).BuildAndInitialize();
}

}  // namespace

zx_status_t serve_intl_profile_provider(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  init(argc, argv);
  std::unique_ptr<sys::ComponentContext> context =
      sys::ComponentContext::CreateAndServeOutgoingDirectory();

  auto inspector = inspect::ComponentInspector(loop.dispatcher(), {});
  inspect::Node node = inspector.root().CreateChild(kHealthNodeName);
  inspect::NodeHealth health = inspect::NodeHealth(&node);
  health.Ok();

  std::unique_ptr<IntlPropertyProviderImpl> intl =
      IntlPropertyProviderImpl::Create(context->svc(), std::move(health));
  const auto intl_status = context->outgoing()->AddPublicService(intl->GetHandler());
  if (intl_status != ZX_OK) {
    FX_LOGS(FATAL) << "could not start intl_property_provider_impl";
  }

  FX_LOGS(INFO) << "Started.";

  return loop.Run();
}

// I don't think that it is worth the effort to try and merge the mostly-similar
// function above.
zx_status_t serve_fuchsia_intl_services(int argc, const char** argv) {
  init(argc, argv);
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  std::unique_ptr<sys::ComponentContext> context =
      sys::ComponentContext::CreateAndServeOutgoingDirectory();

  auto inspector = inspect::ComponentInspector(loop.dispatcher(), {});
  inspect::Node node = inspector.root().CreateChild(kHealthNodeName);
  inspect::NodeHealth health = inspect::NodeHealth(&node);
  health.Ok();

  std::unique_ptr<TimeZoneInfoService> info = TimeZoneInfoService::Create();
  // Required by the startup protocol of TimeZoneInfoService.
  info->Start();
  const auto info_status = context->outgoing()->AddPublicService(info->GetHandler());
  if (info_status != ZX_OK) {
    FX_LOGS(FATAL) << "could not start time_zone_info_service";
  }

  std::unique_ptr<IntlPropertyProviderImpl> intl =
      IntlPropertyProviderImpl::Create(context->svc(), std::move(health));
  const auto intl_status = context->outgoing()->AddPublicService(intl->GetHandler());
  if (intl_status != ZX_OK) {
    FX_LOGS(FATAL) << "could not start intl_property_provider_impl";
  }

  FX_LOGS(INFO) << "Started.";

  return loop.Run();
}

}  // namespace intl
