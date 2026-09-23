// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_SCRIPT_RUNNER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_SCRIPT_RUNNER_H_

#include <cstdint>
#include <filesystem>
#include <fstream>
#include <string>

#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class Session;

class ScriptRunner : public Console::OutputObserver {
 public:
  // |completion_callback| is called when the script finishes (either EOF or error).
  // The bool parameter indicates if it was successful.
  using CompletionCallback = fit::callback<void(bool success)>;

  ScriptRunner(Session* session, Console* console);
  virtual ~ScriptRunner();

  // Runs the script at |path|.
  void Run(const std::filesystem::path& path, CompletionCallback cb);

  void set_timeout_s(uint64_t timeout_s) { timeout_s_ = timeout_s; }

  // Implements |Console::OutputObserver|.
  void OnOutput(const OutputBuffer& output) override;

  // Returns an OutputBuffer with the collected output surrounded by "BEGIN OUTPUT" and "END OUTPUT"
  // markers preceded by the current expected output and script line number.
  OutputBuffer GetFailureContext();

 private:
  // Process the script until the next command or line of output. When the next command is reached,
  void ProcessScriptLines();

  // Dispatches the given |command| after the output of the currently executing has been completely
  // processed. This is always issued asynchronously, but may be more than one tick of the message
  // loop, depending on the amount of output being matched against for a given command.
  void DispatchNextCommandWhenReady(const std::string& command);

  void Fail(const std::string& message);
  void Complete(bool success);

  Console* console_;

  std::filesystem::path script_path_;
  std::ifstream script_file_;
  CompletionCallback completion_cb_;

  // The pattern of a single line that |OnOutput| is expecting.
  std::string expected_output_pattern_;

  // This is passed to a FuzzyMatcher object to communicate that it should not expect the order of
  // strings to be exact.
  bool allow_out_of_order_output_ = false;

  // The output collected from all output events since the last dispatched command. When a timeout
  // is reached without matching a particular output, this will be displayed in an error message.
  std::string collected_output_;

  // Useful for debugging when timeout.
  int line_number_ = 0;

  uint64_t timeout_s_ = 10;
  bool has_failed_ = false;

  fxl::WeakPtrFactory<ScriptRunner> weak_factory_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_SCRIPT_RUNNER_H_
