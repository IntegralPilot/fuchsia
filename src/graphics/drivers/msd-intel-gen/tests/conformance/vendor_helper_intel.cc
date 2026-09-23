// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include "src/graphics/drivers/msd-intel-gen/include/magma_intel_gen_defs.h"
#include "src/graphics/magma/tests/integration/vendor_helper_generic.h"

class VendorHelperServerIntel : public VendorHelperServerGeneric {
 public:
  void GetConfig(GetConfigCompleter::Sync& completer) override {
    fidl::Arena arena;
    auto builder = ::fuchsia_gpu_magma_test::wire::VendorHelperGetConfigResponse::Builder(arena);
    completer.Reply(
        builder
            .execute_command_no_resources_type(
                ::fuchsia_gpu_magma_test::wire::ExecuteCommandNoResourcesType::kInvalid)
            .get_device_timestamp_type(
                ::fuchsia_gpu_magma_test::wire::GetDeviceTimestampType::kSupported)
            .get_device_timestamp_query_id(kMagmaIntelGenQueryTimestamp)
            .buffer_map_features(::fuchsia_gpu_magma_test::wire::BufferMapFeatures::kSupported)
            .buffer_unmap_type(::fuchsia_gpu_magma_test::wire::BufferUnmapType::kNotImplemented)
            .connection_perform_buffer_op_type(
                ::fuchsia_gpu_magma_test::wire::ConnectionPerformBufferOpType::kNotImplemented)
            .Build());
  }

  void ValidateCalibratedTimestamps(
      ::fuchsia_gpu_magma_test::wire::VendorHelperValidateCalibratedTimestampsRequest* request,
      ValidateCalibratedTimestampsCompleter::Sync& completer) override {
    struct magma_intel_gen_timestamp_query intel_timestamp_return;

    FX_CHECK(request->query_buffer.size() >= sizeof(intel_timestamp_return));
    memcpy(&intel_timestamp_return, request->query_buffer.data(), sizeof(intel_timestamp_return));

    if (request->before_ns >= intel_timestamp_return.monotonic_raw_timestamp[0]) {
      completer.Reply(false);
    } else if (intel_timestamp_return.monotonic_raw_timestamp[0] >=
               intel_timestamp_return.monotonic_raw_timestamp[1]) {
      completer.Reply(false);
    } else if (intel_timestamp_return.monotonic_raw_timestamp[1] >= request->after_ns) {
      completer.Reply(false);
    } else {
      completer.Reply(true);
    }
  }

  void OnClosed(fidl::UnbindInfo info) {}
};

int main() {
  async::Loop async_loop(&kAsyncLoopConfigNeverAttachToThread);

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(async_loop.dispatcher());

  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  VendorHelperServerIntel server;
  fidl::ServerBindingGroup<fuchsia_gpu_magma_test::VendorHelper> bindings;

  result =
      outgoing.AddUnmanagedProtocol<fuchsia_gpu_magma_test::VendorHelper>(bindings.CreateHandler(
          &server, async_loop.dispatcher(), std::mem_fn(&VendorHelperServerIntel::OnClosed)));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add VendorHelper protocol: " << result.status_string();
    return -1;
  }

  async_loop.Run();

  return 0;
}
