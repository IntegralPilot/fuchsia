// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/service.h>
#include <lib/fdio/directory.h>

#include <filesystem>
#include <print>

#include "resetctl.h"

int main(int argc, const char* argv[]) {
  if (argc < 2 || (argc == 2 && (strcmp(argv[1], "--help") == 0 || strcmp(argv[1], "-h") == 0))) {
    resetctl::PrintUsage(argv[0]);
    return 0;
  }

  if (argc < 3) {
    resetctl::PrintUsage(argv[0]);
    return -1;
  }

  const char* instance_name = argv[1];

  std::string svc_path = "/svc/fuchsia.hardware.reset.Service/" + std::string(instance_name);
  std::string out_svc_path =
      "/out/svc/fuchsia.hardware.reset.Service/" + std::string(instance_name);

  std::string chosen_root;
  if (std::filesystem::exists(svc_path)) {
    chosen_root = "/svc";
  } else if (std::filesystem::exists(out_svc_path)) {
    chosen_root = "/out/svc";
  } else {
    std::println(stderr, "Reset service instance '{}' not found in /svc or /out/svc",
                 instance_name);
    return -1;
  }

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> svc_dir =
      component::OpenServiceRoot(chosen_root);
  if (svc_dir.is_error()) {
    std::println(stderr, "Failed to open service root {}: {}", chosen_root,
                 svc_dir.status_string());
    return -1;
  }

  zx::result<fidl::ClientEnd<fuchsia_hardware_reset::Reset>> client_end =
      component::ConnectAtMember<fuchsia_hardware_reset::Service::Reset>(svc_dir.value(),
                                                                         instance_name);
  if (client_end.is_error()) {
    std::println(stderr, "Failed to connect to reset protocol for instance {}: {}", instance_name,
                 client_end.status_string());
    return -1;
  }

  auto result = resetctl::Run(argc - 1, argv + 1, std::move(client_end.value()));
  if (result.is_error()) {
    std::println(stderr, "Failed to run command: {}", result.status_string());
    return -1;
  }

  return 0;
}
