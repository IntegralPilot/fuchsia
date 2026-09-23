// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/exceptions/handler/component_lookup.h"

#include <fidl/fuchsia.driver.crash/cpp/fidl.h>
#include <fuchsia/sys2/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fpromise/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/fidl_oneshot.h"
#include "src/developer/forensics/utils/promise_timeout.h"
#include "src/lib/fidl/contrib/fpromise/client.h"

namespace forensics {
namespace exceptions {
namespace handler {
namespace {

// LINT.IfChange
constexpr std::string_view kDriverHostPattern = "bootstrap/driver-hosts:";
// LINT.ThenChange(//src/devices/bin/driver_framework/meta/driver_framework.bootstrap_shard.cml)

::fpromise::promise<ComponentInfo> GetInfo(async_dispatcher_t* dispatcher,
                                           const std::shared_ptr<sys::ServiceDirectory>& services,
                                           zx::duration timeout, zx_koid_t thread_koid) {
  namespace sys = fuchsia::sys2;
  return OneShotCall<sys::CrashIntrospect, &sys::CrashIntrospect::FindComponentByThreadKoid>(
             dispatcher, services, timeout, thread_koid)
      .or_else([](Error& error) { return ::fpromise::error(); })
      .and_then([](const sys::CrashIntrospect_FindComponentByThreadKoid_Result& result)
                    -> ::fpromise::result<ComponentInfo> {
        if (result.is_err()) {
          // RESOURCE_NOT_FOUND most likely means a thread from a process outside a component,
          // which is not an error.
          if (result.err() != fuchsia::component::Error::RESOURCE_NOT_FOUND) {
            FX_LOGS(WARNING) << "Failed FindComponentByThreadKoid, error: "
                             << static_cast<int>(result.err());
          }
          return ::fpromise::error();
        }

        const sys::ComponentCrashInfo& info = result.response().info;
        std::string moniker = (info.has_moniker()) ? info.moniker() : "";
        if (!moniker.empty() && moniker[0] == '/') {
          moniker = moniker.substr(1);
        }
        return ::fpromise::ok(ComponentInfo{
            .url = (info.has_url()) ? info.url() : "",
            .realm_path = "",
            .moniker = moniker,
        });
      });
}

::fpromise::promise<ComponentInfo> GetDriverInfo(
    async_dispatcher_t* dispatcher,
    fidl::Client<fuchsia_driver_crash::CrashIntrospect>& driver_crash_introspect,
    zx::duration timeout, zx_koid_t process_koid, zx_koid_t thread_koid, ComponentInfo& fallback) {
  using FindDriverCrashErrors =
      fidl::ErrorsIn<fuchsia_driver_crash::CrashIntrospect::FindDriverCrash>;

  ::fpromise::promise<ComponentInfo, Error> find_driver_crash_promise =
      fidl_fpromise::as_promise(
          driver_crash_introspect->FindDriverCrash({process_koid, thread_koid}))
          .or_else([](FindDriverCrashErrors& error) {
            FX_LOGS(INFO) << "Failed FindDriverCrash for driver: " << error;
            return fpromise::error(Error::kConnectionError);
          })
          .and_then([](const fuchsia_driver_crash::CrashIntrospectFindDriverCrashResponse& result) {
            return fpromise::ok(ComponentInfo{
                .url = result.info().url().value_or(""),
                .realm_path = "",
                .moniker = result.info().node_moniker().value_or(""),
            });
          });

  return MakePromiseTimeout<ComponentInfo>(std::move(find_driver_crash_promise), dispatcher,
                                           timeout)
      .or_else([fallback](Error& result) {
        FX_LOGS(INFO) << "Falling back to component attribution. Error: " << ToString(result);
        return fpromise::ok(fallback);
      });
}

}  // namespace

::fpromise::promise<ComponentInfo> GetComponentInfo(
    async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
    fidl::Client<fuchsia_driver_crash::CrashIntrospect>& driver_crash_introspect,
    const zx::duration timeout, zx_koid_t process_koid, zx_koid_t thread_koid) {
  return GetInfo(dispatcher, services, timeout, thread_koid)
      .or_else([]() {
        FX_LOGS(INFO) << "Failed FindComponentByThreadKoid, crash will lack component attribution";
        return ::fpromise::error();
      })
      .and_then(
          [dispatcher, services, timeout, process_koid, thread_koid,
           &driver_crash_introspect](ComponentInfo& info) -> ::fpromise::promise<ComponentInfo> {
            // Drivers will crash under the driver host, we can attribute to the specific driver
            // that crashed using the driver specific CrashIntrospect.
            if (info.moniker.starts_with(kDriverHostPattern) && driver_crash_introspect) {
              return GetDriverInfo(dispatcher, driver_crash_introspect, timeout, process_koid,
                                   thread_koid, info);
            }

            return ::fpromise::make_ok_promise(info);
          });
}

}  // namespace handler
}  // namespace exceptions
}  // namespace forensics
