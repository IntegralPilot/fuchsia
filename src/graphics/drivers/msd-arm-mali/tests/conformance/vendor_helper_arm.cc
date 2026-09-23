// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include "src/graphics/drivers/msd-arm-mali/include/magma_arm_mali_types.h"
#include "src/graphics/drivers/msd-arm-mali/include/magma_vendor_queries.h"
#include "src/graphics/magma/tests/integration/vendor_helper_generic.h"

class VendorHelperServerArm : public VendorHelperServerGeneric {
 public:
  void GetConfig(GetConfigCompleter::Sync& completer) override {
    fidl::Arena arena;
    auto builder = ::fuchsia_gpu_magma_test::wire::VendorHelperGetConfigResponse::Builder(arena);
    completer.Reply(
        builder
            .execute_command_no_resources_type(
                ::fuchsia_gpu_magma_test::wire::ExecuteCommandNoResourcesType::kNotImplemented)
            .get_device_timestamp_type(
                ::fuchsia_gpu_magma_test::wire::GetDeviceTimestampType::kSupported)
            .get_device_timestamp_query_id(kMsdArmVendorQueryDeviceTimestamp)
            .buffer_map_features(
                ::fuchsia_gpu_magma_test::wire::BufferMapFeatures::kSupported |
                ::fuchsia_gpu_magma_test::wire::BufferMapFeatures::kSupportsGrowable)
            .buffer_unmap_type(::fuchsia_gpu_magma_test::wire::BufferUnmapType::kSupported)
            .connection_perform_buffer_op_type(
                ::fuchsia_gpu_magma_test::wire::ConnectionPerformBufferOpType::kSupported)
            .Build());
  }

  void ValidateCalibratedTimestamps(
      ::fuchsia_gpu_magma_test::wire::VendorHelperValidateCalibratedTimestampsRequest* request,
      ValidateCalibratedTimestampsCompleter::Sync& completer) override {
    struct magma_arm_mali_device_timestamp_return arm_timestamp_return;

    FX_CHECK(request->query_buffer.size() >= sizeof(arm_timestamp_return));
    memcpy(&arm_timestamp_return, request->query_buffer.data(), sizeof(arm_timestamp_return));

    if (request->before_ns >= arm_timestamp_return.monotonic_raw_timestamp_before) {
      completer.Reply(false);
    } else if (arm_timestamp_return.monotonic_raw_timestamp_before >=
               arm_timestamp_return.monotonic_raw_timestamp_after) {
      completer.Reply(false);
    } else if (arm_timestamp_return.monotonic_raw_timestamp_after >= request->after_ns) {
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

  VendorHelperServerArm server;
  fidl::ServerBindingGroup<fuchsia_gpu_magma_test::VendorHelper> bindings;

  result =
      outgoing.AddUnmanagedProtocol<fuchsia_gpu_magma_test::VendorHelper>(bindings.CreateHandler(
          &server, async_loop.dispatcher(), std::mem_fn(&VendorHelperServerArm::OnClosed)));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add VendorHelper protocol: " << result.status_string();
    return -1;
  }

  async_loop.Run();

  return 0;
}
