// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "resetctl.h"

#include <print>

namespace resetctl {

void PrintUsage(const char* binary_name) {
  std::println(stderr, "Usage: {} <instance_name> <subcommand> [args]", binary_name);
  std::println(stderr, "Subcommands:");
  std::println(stderr, "  assert   - Assert the reset line");
  std::println(stderr, "             Example: {} <instance_name> assert", binary_name);
  std::println(stderr, "  deassert - Deassert the reset line");
  std::println(stderr, "             Example: {} <instance_name> deassert", binary_name);
  std::println(stderr, "  toggle   - Toggle the reset line (optional timeout in ns)");
  std::println(stderr, "             Example: {} <instance_name> toggle", binary_name);
  std::println(stderr, "             Example: {} <instance_name> toggle 1000", binary_name);
  std::println(stderr, "  status   - Get the reset status");
  std::println(stderr, "             Example: {} <instance_name> status", binary_name);
}

zx::result<> Run(int argc, const char** argv,
                 fidl::ClientEnd<fuchsia_hardware_reset::Reset> client_end) {
  if (argc < 2) {
    std::println(stderr, "Subcommand missing.");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fidl::WireSyncClient client(std::move(client_end));
  const char* subcommand = argv[1];

  if (strcmp(subcommand, "assert") == 0) {
    auto result = client->Assert();
    if (!result.ok()) {
      return zx::error(result.status());
    }
    if (result->is_error()) {
      return zx::error(result->error_value());
    }
    return zx::ok();
  } else if (strcmp(subcommand, "deassert") == 0) {
    auto result = client->Deassert();
    if (!result.ok()) {
      return zx::error(result.status());
    }
    if (result->is_error()) {
      return zx::error(result->error_value());
    }
    return zx::ok();
  } else if (strcmp(subcommand, "toggle") == 0) {
    if (argc > 2) {
      uint64_t timeout_ns = strtoull(argv[2], nullptr, 10);
      auto result = client->ToggleWithTimeout(timeout_ns);
      if (!result.ok()) {
        return zx::error(result.status());
      }
      if (result->is_error()) {
        return zx::error(result->error_value());
      }
    } else {
      auto result = client->Toggle();
      if (!result.ok()) {
        return zx::error(result.status());
      }
      if (result->is_error()) {
        return zx::error(result->error_value());
      }
    }
    return zx::ok();
  } else if (strcmp(subcommand, "status") == 0) {
    auto result = client->Status();
    if (!result.ok()) {
      return zx::error(result.status());
    }
    if (result->is_error()) {
      return zx::error(result->error_value());
    }
    std::println("Asserted: {}", result->value()->asserted ? "true" : "false");
    return zx::ok();
  } else {
    std::println(stderr, "Unknown subcommand: {}", subcommand);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
}

}  // namespace resetctl
