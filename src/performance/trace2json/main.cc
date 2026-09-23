// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>

#include <iostream>
#include <map>
#include <set>
#include <vector>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/performance/trace2json/convert.h"

namespace {

const char kHelp[] = "help";
const char kInputFile[] = "input-file";
const char kOutputFile[] = "output-file";
const char kSystemEventOutputFile[] = "system-event-output-file";
const char kPattern[] = "pattern";
const char kCategory[] = "category";

std::set<std::string> kKnownOptions = {
    kHelp, kInputFile, kOutputFile, kSystemEventOutputFile, kPattern, kCategory,
};

void PrintHelpMessage() {
  std::map<std::string, std::string> options = {
      {"help", "Print this help message."},
      {"input-file=[]", "Read trace from the specified file."},
      {"output-file=[]", "Write the converted trace to the specified file."},
      {"system-event-output-file=[]",
       "Optional. Write the system trace events to the specified file in jsonlines "
       "format. If provided, the main output file will not contain system trace events."},
      {"pattern=[]",
       "A regular expression to match against event names. Events that do not "
       "match will be dropped. This parameter may be specified multiple times."},
      {"category=[]",
       "An exact string to match against event categories. Events that do not "
       "match will be dropped. This parameter may be specified multiple times."},
  };

  std::cerr
      << "trace2json [options]: Convert a trace from fxt (Fuchsia trace format) to json (Chrome "
         "trace format)."
      << "\n"
      << "Fuchsia trace format: "
         "https://fuchsia.googlesource.com/fuchsia/+/HEAD/docs/development/tracing/"
         "trace-format/"
      << "\n"
      << "Chrome trace format: "
         "https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU/edit"
      << "\n";
  for (const auto& option : options) {
    std::cerr << "  --" << option.first << ": " << option.second << "\n";
  }
}

}  // namespace

int main(int argc, char** argv) {
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  if (!fxl::SetLogSettingsFromCommandLine(command_line)) {
    return 1;
  }

  if (command_line.HasOption(kHelp)) {
    PrintHelpMessage();
    return 0;
  }

  bool invalid_options = false;
  for (const auto& option : command_line.options()) {
    if (kKnownOptions.count(option.name) == 0) {
      FX_LOGS(ERROR) << "Unknown option: " << option.name;
      invalid_options = true;
    }
  }

  if (command_line.positional_args().size() > 0) {
    FX_LOGS(ERROR) << "Unexpected positional arg";
    invalid_options = true;
  }

  if (invalid_options) {
    PrintHelpMessage();
    return 1;
  }

  ConvertSettings settings;
  if (command_line.HasOption(kInputFile)) {
    command_line.GetOptionValue(kInputFile, &settings.input_file_name);
  } else {
    FX_LOGS(ERROR) << "Reading from stdin is no longer supported, please pass --" << kInputFile;
    PrintHelpMessage();
    return 1;
  }
  if (command_line.HasOption(kOutputFile)) {
    command_line.GetOptionValue(kOutputFile, &settings.output_file_name);
  } else {
    FX_LOGS(ERROR) << "Writing to stdout is no longer supported, please pass --" << kOutputFile;
    PrintHelpMessage();
    return 1;
  }
  if (command_line.HasOption(kSystemEventOutputFile)) {
    command_line.GetOptionValue(kSystemEventOutputFile, &settings.system_event_output_file_name);
  }
  auto patterns_sv = command_line.GetOptionValues(kPattern);
  settings.patterns.assign(patterns_sv.begin(), patterns_sv.end());

  auto categories_sv = command_line.GetOptionValues(kCategory);
  settings.categories.assign(categories_sv.begin(), categories_sv.end());

  if (!ConvertTrace(settings)) {
    return 1;
  }

  return 0;
}
