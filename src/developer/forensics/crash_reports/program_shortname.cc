// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/program_shortname.h"

#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <optional>
#include <string>

namespace forensics::crash_reports {
namespace {

// Shorten |program_name| into a shortname by removing the "fuchsia-pkg://" prefix if present and
// replacing all '/' with ':'.
//
// For example `fuchsia-pkg://fuchsia.com/foo-bar#meta/foo_bar.cm` becomes
// `fuchsia.com:foo-bar#meta:foo_bar.cm`.
std::string Shorten(std::string program_name) {
  // Remove leading whitespace.
  const size_t first_non_whitespace = program_name.find_first_not_of(' ');
  if (first_non_whitespace == std::string::npos) {
    return "";
  }
  program_name = program_name.substr(first_non_whitespace);

  // Remove the "fuchsia-pkg://" prefix if present.
  const std::string fuchsia_pkg_prefix("fuchsia-pkg://");
  if (program_name.find(fuchsia_pkg_prefix) == 0) {
    program_name.erase(/*pos=*/0u, /*len=*/fuchsia_pkg_prefix.size());
  }
  std::replace(program_name.begin(), program_name.end(), '/', ':');

  // Remove all repeating ':'.
  for (size_t idx = program_name.find("::"); idx != std::string::npos;
       idx = program_name.find("::")) {
    program_name.erase(idx, 1);
  }

  // Remove trailing white space.
  const size_t last_non_whitespace = program_name.find_last_not_of(' ');
  return (last_non_whitespace == std::string::npos)
             ? ""
             : program_name.substr(0, last_non_whitespace + 1);
}

}  // namespace

std::optional<ProgramShortname> ProgramShortname::Create(std::string program_name) {
  const std::string shortened = Shorten(std::move(program_name));
  if (shortened.empty() || shortened == "." || shortened == "..") {
    FX_LOGS(ERROR) << "Invalid shortened program name: '" << shortened << "'";
    return std::nullopt;
  }

  return ProgramShortname(shortened);
}

std::string ProgramShortname::Logname() const {
  std::string name = value_;

  // Find the last colon in |name|.
  const size_t last_colon = name.find_last_of(":");
  if (last_colon == std::string::npos) {
    return name;
  }

  // Remove everything leading up to the last colon.
  name.erase(name.begin(), name.begin() + last_colon + 1);

  // Determine if there's a ".cm" suffix in |name|.
  const size_t cm_suffix = name.rfind(".cm");
  if (cm_suffix == std::string::npos) {
    return name;
  }

  // Erase the ".cm" and everything after it.
  name.erase(name.begin() + cm_suffix, name.end());
  return name;
}

}  // namespace forensics::crash_reports
