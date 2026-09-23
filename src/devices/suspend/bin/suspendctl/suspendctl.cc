// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "suspendctl.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>

#include <algorithm>
#include <cctype>
#include <cstdint>
#include <filesystem>
#include <iostream>
#include <sstream>
#include <string>

namespace suspendctl {

namespace fh_suspend = fuchsia_hardware_power_suspend;

constexpr char kSuspenderServicePath[] = "/svc/fuchsia.hardware.power.suspend.SuspendService";

// For now we assume that the system only supports one suspend state.
constexpr int kDefaultSuspendState = 0;

constexpr char kUsageSummary[] = R"""(
Fuchsia Suspend HAL Control

Usage:
  suspendctl [subcmd]

Subcommands:
  suspend - Suspends the system.
     help - Print this message.
)""";

void usage() { std::cout << kUsageSummary << std::endl; }

std::string format_vector(const std::vector<uint64_t>& v) {
  std::ostringstream os;

  os << "{ ";
  for (const uint64_t u : v) {
    os << u << " ";
  }
  os << "}";
  return os.str();
}

zx::result<fidl::SyncClient<fh_suspend::Suspender>> open_suspend_client() {
  std::vector<std::string> entries;
  for (const auto& entry : std::filesystem::directory_iterator(kSuspenderServicePath)) {
    entries.push_back(entry.path());
  }

  if (entries.size() != 1) {
    std::cerr << "Expected to find exactly 1 entry in " << kSuspenderServicePath << ", found "
              << entries.size() << " instead." << std::endl;
    return zx::error(ZX_ERR_INTERNAL);
  }

  std::string target_path = entries[0] + "/suspender";

  zx::result client_end = component::Connect<fh_suspend::Suspender>(target_path);
  if (client_end.is_error()) {
    std::cerr << "Failed to open service at " << target_path
              << ", status = " << client_end.status_string() << std::endl;
    return client_end.take_error();
  }

  fidl::SyncClient<fh_suspend::Suspender> client(std::move(client_end.value()));
  if (!client.is_valid()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(client));
}

int DoSuspend(fidl::SyncClient<fh_suspend::Suspender> client, std::ostream& out,
              std::ostream& err) {
  fh_suspend::SuspenderSuspendRequest req;
  req.state_index(kDefaultSuspendState);
  auto suspend_result = client->Suspend(req);
  if (suspend_result.is_error()) {
    err << "Failed to suspend, reason: " << suspend_result.error_value().FormatDescription()
        << std::endl;
    return -1;
  }

  out << "Resumed. Stats:\n"
      << "  Suspend Duration: " << suspend_result->suspend_duration().value_or(0) << "ns\n"
      << "  Suspend Overhead: " << suspend_result->suspend_overhead().value_or(0) << "ns\n";

  if (suspend_result->reason().has_value()) {
    out << "       Wake Reason:\n";

    if (suspend_result->reason()->soft_wake_vectors()) {
      out << "   Soft Wake Vectors:"
          << format_vector(*suspend_result->reason()->soft_wake_vectors());
    }
    if (suspend_result->reason()->wake_vectors()) {
      out << "   Soft Wake Vectors:" << format_vector(*suspend_result->reason()->wake_vectors());
    }
  }

  return 0;
}

Action ParseArgs(const std::vector<std::string>& argv) {
  if (argv.size() < 2) {
    usage();
    return Action::Error;
  }

  if (argv[1] == "help") {
    usage();
    return Action::PrintHelp;
  }

  if (argv[1] == "suspend") {
    return Action::Suspend;
  }

  usage();
  return Action::Error;
}

int run(int argc, char* argv[]) {
  std::vector<std::string> args;
  args.reserve(argc);
  for (int i = 0; i < argc; i++) {
    args.emplace_back(argv[i]);
  }

  Action action = ParseArgs(args);

  switch (action) {
    case Action::Error:
      return -1;
    case Action::PrintHelp:
      return 0;
    case Action::Suspend:
      break;
  }

  std::cout << "suspending..." << std::endl;

  auto client = open_suspend_client();
  if (client.is_error()) {
    std::cerr << "Failed to open suspend client, st = " << client.status_string() << std::endl;
    return -1;
  }

  return DoSuspend(std::move(*client), std::cout, std::cerr);
}

}  // namespace suspendctl
