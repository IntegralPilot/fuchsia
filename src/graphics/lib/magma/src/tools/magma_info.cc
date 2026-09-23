// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>

#include <filesystem>

#include "src/lib/fxl/command_line.h"

const char* kGpuClassPath = "/svc/fuchsia.gpu.magma.Service";

int main(int argc, char** argv) {
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);

  static const char kGpuDeviceFlag[] = "gpu-device";
  static const char kDumpTypeFlag[] = "dump-type";

  std::string gpu_device_value;
  if (!command_line.GetOptionValue(kGpuDeviceFlag, &gpu_device_value)) {
    for (auto& p : std::filesystem::directory_iterator(kGpuClassPath)) {
      gpu_device_value = static_cast<std::string>(p.path()) + "/device";
      break;
    }
    if (gpu_device_value.empty()) {
      fprintf(stderr, "No magma device found\n");
      return -1;
    }
  }
  printf("Opening magma device: %s\n", gpu_device_value.c_str());
  zx::result client_end = component::Connect<fuchsia_gpu_magma::DiagnosticDevice>(gpu_device_value);
  if (client_end.is_error()) {
    printf("Failed to open magma device %s: %s\n", gpu_device_value.c_str(),
           client_end.status_string());
    return -1;
  }

  uint32_t dump_type = 0;
  std::string dump_type_string;
  if (command_line.GetOptionValue(kDumpTypeFlag, &dump_type_string)) {
    dump_type = atoi(dump_type_string.c_str());
  }

  const fidl::OneWayStatus result = fidl::WireCall(client_end.value())->DumpState(dump_type);
  if (!result.ok()) {
    printf("magma_DeviceDumpStatus failed: %d", result.status());
    return -1;
  }
  printf("Dumping system driver status to system log\n");

  return 0;
}
