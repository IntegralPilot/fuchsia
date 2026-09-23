// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/cmdline/status.h>
#include <stdio.h>
#include <unistd.h>
#include <zircon/assert.h>

#include <iostream>
#include <memory>
#include <string>
#include <vector>

#include "tools/fidl/fidlc/cmd/fidl-lint/command_line_options.h"
#include "tools/fidl/fidlc/src/findings.h"
#include "tools/fidl/fidlc/src/findings_json.h"
#include "tools/fidl/fidlc/src/lexer.h"
#include "tools/fidl/fidlc/src/linter.h"
#include "tools/fidl/fidlc/src/parser.h"
#include "tools/fidl/fidlc/src/source_manager.h"
#include "tools/fidl/fidlc/src/tree_visitor.h"

namespace {

[[noreturn]] void FailWithUsage(const std::string& argv0, const char* message, ...) {
  va_list args;
  va_start(args, message);
  vfprintf(stderr, message, args);
  va_end(args);
  std::cerr << fidlc::Usage(argv0) << '\n';
  exit(2);  // Exit code 1 is reserved to indicate lint findings
}

[[noreturn]] void Fail(const char* message, ...) {
  va_list args;
  va_start(args, message);
  vfprintf(stderr, message, args);
  va_end(args);
  exit(2);  // Exit code 1 is reserved to indicate lint findings
}

fidlc::Finding DiagnosticToFinding(const fidlc::Diagnostic& diag) {
  const char* check_id = nullptr;
  switch (diag.def.kind) {
    case fidlc::DiagnosticKind::kError:
      check_id = "parse-error";
      break;
    case fidlc::DiagnosticKind::kWarning:
      check_id = "parse-warning";
      break;
    case fidlc::DiagnosticKind::kRetired:
      ZX_PANIC("should never emit a retired diagnostic");
  }
  return fidlc::Finding(diag.span, check_id, diag.Format());
}

void Lint(const fidlc::SourceFile& source_file, fidlc::ExperimentalFlagSet experimental_flags,
          fidlc::Findings* findings, const std::set<std::string>& included_checks,
          const std::set<std::string>& excluded_checks, bool exclude_by_default,
          std::set<std::string>* excluded_checks_not_found) {
  fidlc::Reporter reporter;
  fidlc::Lexer lexer(source_file, &reporter);
  fidlc::Parser parser(&lexer, &reporter, experimental_flags);
  std::unique_ptr<fidlc::File> ast = parser.Parse();
  for (auto* diag : reporter.Diagnostics()) {
    findings->push_back(DiagnosticToFinding(*diag));
  }
  if (!parser.Success()) {
    return;
  }

  fidlc::Linter linter;

  linter.set_included_checks(included_checks);
  linter.set_excluded_checks(excluded_checks);
  linter.set_exclude_by_default(exclude_by_default);

  linter.Lint(ast, findings, excluded_checks_not_found);
}

}  // namespace

int main(int argc, char* argv[]) {
  fidlc::CommandLineOptions options;
  std::vector<std::string> filepaths;
  cmdline::Status status =
      fidlc::ParseCommandLine(argc, const_cast<const char**>(argv), &options, &filepaths);
  if (status.has_error()) {
    Fail("%s\n", status.error_message().c_str());
  }

  if (filepaths.empty()) {
    FailWithUsage(argv[0], "No files provided\n");
  }

  fidlc::SourceManager source_manager;

  // Process filenames.
  for (const auto& filepath : filepaths) {
    const char* reason;
    if (!source_manager.CreateSource(filepath, &reason)) {
      Fail("Couldn't read in source data from %s: %s\n", filepath.c_str(), reason);
    }
  }

  std::set<std::string> excluded_checks_not_found;
  if (options.must_find_excluded_checks) {
    // copy excluded checks specified in command line options, and the linter will remove each one
    // encountered during linting.
    excluded_checks_not_found =
        std::set<std::string>(options.excluded_checks.begin(), options.excluded_checks.end());
  }

  bool exclude_by_default = !options.included_checks.empty() && options.excluded_checks.empty();

  // Convert command line vectors to sets, and add internally-disabled checks to excluded
  auto included_checks =
      std::set<std::string>(options.included_checks.begin(), options.included_checks.end());
  auto excluded_checks =
      std::set<std::string>(options.excluded_checks.begin(), options.excluded_checks.end());

  // Add experimental checks to included checks. Experimental checks don't count
  // for enabling exclude_by_default, but do get added added to included_checks
  // to turn them on. Merging included-checks and experimental-checks allows
  // experimental checks to be enabled through either the --include-checks flag
  // or the --experimental-checks flag, which makes it possible to use
  // exclude-by-default mode even if you only want to turn on experimental
  // checks, by passing them through --include-checks rather than
  // --experimental-checks.
  //
  // Note that this works in reverse as well; it is possible to enable a normal
  // check via --experimental-checks, however this has no effect unless the
  // check is also being excluded via --exclude-checks or exclude-by-default in
  // being used because some other check was passed with --include-checks.
  // Allowing non-experimental checks to be enabled via --experimental-checks
  // ensures forward compatibility when a previously-experimental check is
  // officially released an so no-longer experimental.
  included_checks.insert(options.experimental_checks.begin(), options.experimental_checks.end());

  fidlc::ExperimentalFlagSet experimental_flags;
  for (const auto& flag_str : options.experimental_flags) {
    auto it = fidlc::kAllExperimentalFlags.find(flag_str);
    if (it == fidlc::kAllExperimentalFlags.end()) {
      Fail("Unknown experimental flag %s\n", flag_str.c_str());
    }
    experimental_flags.Enable(it->second);
  }

  fidlc::Findings findings;
  bool enable_color = !std::getenv("NO_COLOR") && isatty(fileno(stderr));
  for (const auto& source_file : source_manager.sources()) {
    Lint(*source_file, experimental_flags, &findings, included_checks, excluded_checks,
         exclude_by_default, &excluded_checks_not_found);
  }

  if (options.format == "text") {
    auto lints = fidlc::FormatFindings(findings, enable_color);
    for (const auto& lint : lints) {
      fprintf(stderr, "%s\n", lint.c_str());
    }
  } else {
    ZX_ASSERT(options.format == "json");  // should never be false
    std::cout << fidlc::FindingsJson(findings).Produce().str();
  }

  if (!excluded_checks_not_found.empty()) {
    std::ostringstream os;
    os << "The following checks were excluded but were never encountered:\n";
    for (auto& check_id : excluded_checks_not_found) {
      os << "  * " << check_id << '\n';
    }
    os << "Please remove these checks from your excluded_checks list and try again.\n";
    Fail(os.str().c_str());
  }

  // Exit with a status of '1' if there were any findings (at least one file was not "lint-free")
  return findings.empty() ? 0 : 1;
}
