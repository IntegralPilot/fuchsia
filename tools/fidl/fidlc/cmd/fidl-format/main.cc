// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <stdio.h>
#include <unistd.h>

#include <iostream>
#include <memory>
#include <string>
#include <vector>

#include "tools/fidl/fidlc/src/experimental_flags.h"
#include "tools/fidl/fidlc/src/formatter.h"
#include "tools/fidl/fidlc/src/lexer.h"
#include "tools/fidl/fidlc/src/source_manager.h"
#include "tools/fidl/fidlc/src/utils.h"

namespace {

void Usage(std::string_view argv0) {
  std::cout << "usage: " << argv0
            << " <options> [<files>]\n"
               "\n"
               " * `-i, --in-place` Formats file in place\n"
               "\n"
               " * `-h, --help` Prints this help, and exit immediately.\n"
               "\n"
               " If no files are specified it formats code from standard input.\n"
               "\n";
  std::cout.flush();
}

[[noreturn]] void FailWithUsage(std::string_view argv0, const char* message, ...) {
  va_list args;
  va_start(args, message);
  vfprintf(stderr, message, args);
  va_end(args);
  Usage(argv0);
  exit(1);
}

[[noreturn]] void Fail(const char* message, ...) {
  va_list args;
  va_start(args, message);
  vfprintf(stderr, message, args);
  va_end(args);
  exit(1);
}

bool Format(const fidlc::SourceFile& source_file, fidlc::Reporter* reporter, std::string& output) {
  fidlc::Lexer lexer(source_file, reporter);
  fidlc::ExperimentalFlagSet experimental_flags;
  // The formatter is run directly by fx format-code, not as part of the build,
  // so we can't rely on having accurate experimental flags. Instead, we just
  // set all the flags which could block parsing if disabled.
  experimental_flags.Enable(fidlc::ExperimentalFlag::kZxCTypes);

  auto formatter = fidlc::Formatter(100, reporter);
  auto result = formatter.Format(source_file, experimental_flags);
  if (!result.has_value()) {
    return false;
  }
  output = result.value();

  std::string source_file_str(source_file.data());
  if (fidlc::RemoveWhitespace(source_file_str) != fidlc::RemoveWhitespace(output)) {
    // Note that this is only useful as long as we do not have the formatter do
    // things that affect non-whitespace characters, like sort using statements
    // or coalesce consts into const blocks.  If / when this happens, this check
    // may need to be more nuanced (or, those things could happen in a separate pass).
    std::string filename(source_file.filename());
    Fail(
        "Internal formatter failure: output is not the same as input processing file %s. "
        "Please report a bug.\n",
        filename.c_str());
  }

  return true;
}

}  // namespace

int main(int argc, char* argv[]) {
  // Construct the args vector from the argv array.
  std::vector<std::string> args(argv, argv + argc);
  bool in_place = false;
  size_t pos = 1;
  // Process options
  while (pos < args.size() && args[pos] != "--" && args[pos].find('-') == 0) {
    if (args[pos] == "-i" || args[pos] == "--in-place") {
      in_place = true;
    } else if (args[pos] == "-h" || args[pos] == "--help") {
      Usage(args[0]);
      exit(0);
    } else {
      FailWithUsage(args[0], "Unknown argument: %s\n", args[pos].c_str());
    }
    pos++;
  }

  fidlc::SourceManager source_manager;

  // Is this formatting stdin to stdout?
  bool pipe = (pos == args.size());

  if (in_place && pipe) {
    Fail("-i not accepted when formatting standard input.");
  }

  // Process filenames.
  if (pipe) {
    std::string input(std::istreambuf_iterator<char>(std::cin >> std::noskipws),
                      std::istreambuf_iterator<char>());
    source_manager.AddSourceFile(std::make_unique<fidlc::SourceFile>("stdin", std::move(input)));
  } else {
    for (size_t i = pos; i < args.size(); i++) {
      const char* reason;
      if (!source_manager.CreateSource(args[i], &reason)) {
        Fail("Couldn't read in source data from %s: %s\n", args[i].c_str(), reason);
      }
    }
  }

  fidlc::Reporter reporter;
  for (const auto& source_file : source_manager.sources()) {
    std::string output;
    if (!Format(*source_file, &reporter, output)) {
      // In the formatter, we do not print the report if there are only
      // warnings.
      bool enable_color = !std::getenv("NO_COLOR") && isatty(fileno(stderr));
      reporter.PrintReports(enable_color);
      return 1;
    }
    FILE* out_file;
    if (in_place && !pipe) {
      const char* filename = source_file->filename().data();
      out_file = fopen(filename, "w+");
      if (out_file == nullptr) {
        std::string error = "Fail: cannot open file: ";
        error.append(filename);
        error.append(":\n");
        error.append(strerror(errno));
        Fail(error.c_str());
      }
    } else {
      out_file = stdout;
    }
    fprintf(out_file, "%s", output.c_str());
  }
}
