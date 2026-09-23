// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_CONSOLE_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_CONSOLE_H_

#include <map>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/zxdb/console/command.h"
#include "src/developer/debug/zxdb/console/console_context.h"
#include "src/lib/fxl/macros.h"
#include "src/lib/fxl/observer_list.h"
#include "src/lib/line_input/modal_line_input.h"

namespace zxdb {

class AsyncOutputBuffer;
class ConsoleSuspendToken;
class OutputBuffer;
class Session;

class Console : debug::LogBackend {
 public:
  class OutputObserver {
   public:
    virtual void OnOutput(const OutputBuffer& output) {}
  };

  explicit Console(Session* session);
  virtual ~Console();

  static Console* get() { return singleton_; }

  ConsoleContext& context() { return context_; }

  fxl::ObserverList<OutputObserver>& output_observers() { return output_observers_; }

  fxl::WeakPtr<Console> GetWeakPtr();

  // Initializes |context_| and does any work that should happen before the console is ready to
  // accept input from the user. |context_| is responsible for enabling and disabling input and
  // output as appropriate after this is called.
  virtual void Init() = 0;

  // Causes the message loop to exit the next time through.
  virtual void Quit() = 0;

  // Prints the buffer/string to the console.
  void Output(const OutputBuffer& output, bool add_newline = true);
  void Output(const std::string& s);
  void Output(const Err& err);

  // Writes the given output to the console if streaming is enabled.
  void Stream(const OutputBuffer& output);

  // Synchronously prints the output if the async buffer is complete. Otherwise adds a listener and
  // prints the output to the console when it is complete.
  void Output(fxl::RefPtr<AsyncOutputBuffer> output);

  // Writes the given output to the console.
  virtual void Write(const OutputBuffer& output, bool add_newline = true) = 0;

  // Clears the contents of the console.
  virtual void Clear() = 0;

  // Asks the user a question. The possible answers are stored in the options struct.
  //
  // Callers should pass anything they want to print above the prompt in the |message|. It's
  // important to do this instead of calling Output() followed by ModalGetOption() because there
  // can theoretically be multiple prompts pending (in case they're triggered by async events)
  // and the message passed here will always get printed above the prompt when its turn comes.
  virtual void ModalGetOption(const line_input::ModalPromptOptions& options, OutputBuffer message,
                              const std::string& prompt,
                              line_input::ModalLineInput::ModalCompletionCallback cb) = 0;

  // Parses and dispatches the given line of input. By providing different |cmd_context|, the output
  // could be redirected or outputted.
  //
  // When posting programmatic commands, set add_to_history = false or the command will confusingly
  // appear as the "last command" (when they hit enter again) and in the "up" history.
  virtual void ProcessInputLine(const std::string& line,
                                fxl::RefPtr<CommandContext> cmd_context = nullptr,
                                bool add_to_history = true) = 0;

  virtual void EnableInput() = 0;
  virtual void DisableInput() = 0;

  void EnableOutput();
  void DisableOutput();

  void EnableStreaming();
  void DisableStreaming();

  // Implements |LogBackend|.
  void WriteLog(debug::LogSeverity severity, const debug::FileLineFunction& location,
                std::string log) override;

 protected:
  static Console* singleton_;
  ConsoleContext context_;

  fxl::ObserverList<OutputObserver> output_observers_;

 private:
  bool OutputEnabled();

  // Similarly, returns true if streaming is enabled. False means streaming is
  // disabled.
  bool StreamingEnabled() const { return streaming_enabled_ > 0; }

  // Track all asynchronous output pending. We want to store a reference and lookup by pointer, so
  // the object is duplicated here (RefPtr doesn't like to be put in a set).
  //
  // These pointers own the tree of async outputs for each async operation. We need to keep owning
  // pointers to the roots of every AsyncOutputBuffer we've installed ourselves as a completion
  // callback for to keep them in scope until they're completed.
  std::map<AsyncOutputBuffer*, fxl::RefPtr<AsyncOutputBuffer>> async_output_;

  // A counter for whether the `Output` functions should actually write to the console.
  //
  // Output is enabled if this value is greater than zero and disabled if the value is zero
  // or lower. Using a counter lets clients balance calls to EnableOutput and DisableOutput
  // without needing to coordinate with each other.
  int output_enabled_ = 0;

  // Similar to the above counter for output, we enable streaming when this value is greater than 0
  // and disable streaming when this value is less than or equal to zero. This setting is very
  // closely tied, but not equivalent to the Embedded* variety of Console modes. The transition
  // from Embedded to Interactive and vice-versa will toggle the counter in the respective
  // directions (i.e. Embedded->Interactive will call DisableStreaming and Interactive->Embedded
  // will call EnableStreaming) but other parts of the system may need to interact with streaming
  // directly without changing the state of ConsoleMode.
  int streaming_enabled_ = 0;

  fxl::WeakPtrFactory<Console> weak_factory_;

  FXL_DISALLOW_COPY_AND_ASSIGN(Console);
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_CONSOLE_H_
