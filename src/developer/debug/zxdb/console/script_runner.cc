// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/script_runner.h"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/common/fuzzy_matcher.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/lib/fxl/strings/trim.h"

namespace zxdb {

ScriptRunner::ScriptRunner(Session* session, Console* console)
    : console_(console), weak_factory_(this) {}

ScriptRunner::~ScriptRunner() = default;

void ScriptRunner::Run(const std::filesystem::path& path, CompletionCallback cb) {
  script_path_ = path;
  completion_cb_ = std::move(cb);

  script_file_ = std::ifstream(script_path_);
  if (!script_file_) {
    Fail("Failed to open " + script_path_.string());
    return;
  }

  console_->output_observers().AddObserver(this);

  debug::MessageLoop::Current()->PostTimer(
      FROM_HERE, timeout_s_ * 1000, [weak_this = weak_factory_.GetWeakPtr()]() {
        if (weak_this && !weak_this->has_failed_ && weak_this->completion_cb_) {
          std::string error_msg = "Timeout waiting for pattern \"" +
                                  weak_this->expected_output_pattern_ + "\" in the output:\n";
          weak_this->Fail(error_msg);
        }
      });

  ProcessScriptLines();
}

void ScriptRunner::OnOutput(const OutputBuffer& output) {
  if (has_failed_) {
    return;
  }

  std::string output_str = output.AsString();
  collected_output_.append(output_str);

  if (!collected_output_.empty() && collected_output_.back() != '\n') {
    collected_output_.push_back('\n');
  }

  FuzzyMatcher matcher(collected_output_);

  while (!expected_output_pattern_.empty() &&
         matcher.MatchesLine(expected_output_pattern_, allow_out_of_order_output_)) {
    expected_output_pattern_.clear();
    ProcessScriptLines();
  }

  if (script_file_.eof() && expected_output_pattern_.empty()) {
    console_->output_observers().RemoveObserver(this);
    Complete(true);
  }
}

void ScriptRunner::ProcessScriptLines() {
  if (!expected_output_pattern_.empty())
    return;

  std::string line;
  while (std::getline(script_file_, line)) {
    line_number_++;

    if (line.empty()) {
      continue;
    }

    // Inputs.
    if (debug::StringStartsWith(line, "[zxdb]")) {
      std::string command = std::string(fxl::TrimString(line.substr(6), " "));
      DispatchNextCommandWhenReady(command);
      return;
    } else if (debug::StringStartsWith(line, "##")) {
      // Inline directives.
      std::string directive = std::string(fxl::TrimString(line.substr(2), " "));
      if (debug::StringStartsWith(directive, "allow-out-of-order-output")) {
        allow_out_of_order_output_ = true;
      }
      continue;
    } else if (debug::StringStartsWith(line, "#")) {
      // Comment.
      continue;
    }

    // Expected outputs.
    expected_output_pattern_ = line;
    return;
  }
}

void ScriptRunner::DispatchNextCommandWhenReady(const std::string& command) {
  if (!expected_output_pattern_.empty()) {
    // Still waiting on output from the last dispatched command.
    debug::MessageLoop::Current()->PostTask(FROM_HERE,
                                            [weak_this = weak_factory_.GetWeakPtr(), command]() {
                                              if (weak_this)
                                                weak_this->DispatchNextCommandWhenReady(command);
                                            });
    return;
  }

  debug::MessageLoop::Current()->PostTask(FROM_HERE,
                                          [weak_this = weak_factory_.GetWeakPtr(), command]() {
                                            if (!weak_this)
                                              return;

                                            weak_this->collected_output_.clear();
                                            weak_this->allow_out_of_order_output_ = false;

                                            // Fetch the first line of expected output.
                                            weak_this->ProcessScriptLines();

                                            weak_this->console_->ProcessInputLine(command);
                                          });
}

OutputBuffer ScriptRunner::GetFailureContext() {
  return "Expected: \"" + expected_output_pattern_ + "\" on script line #" +
         std::to_string(line_number_) + " but got output:\n" +
         "============================= BEGIN OUTPUT =============================\n" +
         collected_output_ +
         "============================== END OUTPUT ==============================\n" +
         "You may want to check the output order in the script matches the generated "
         "output, or use ## allow-out-of-order-output. See https://fuchsia.dev/fuchsia-src/development/debugger/scripting "
         "for more examples.";
}

void ScriptRunner::Fail(const std::string& message) {
  has_failed_ = true;

  // We cannot defer unregistering ourselves as an observer until |Complete| since we will begin
  // receiving output events from ourselves.
  console_->output_observers().RemoveObserver(this);

  console_->Output(Err(message + "\n"));
  console_->Output(GetFailureContext());
  Complete(false);
}

void ScriptRunner::Complete(bool success) {
  if (completion_cb_) {
    auto cb = std::move(completion_cb_);
    cb(success);
  }
}

}  // namespace zxdb
