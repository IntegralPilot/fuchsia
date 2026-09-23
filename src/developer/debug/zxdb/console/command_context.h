// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_COMMAND_CONTEXT_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_COMMAND_CONTEXT_H_

#include <lib/fit/defer.h>
#include <lib/fit/function.h>

#include <map>

#include "src/developer/debug/zxdb/client/analytics_event.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/format/async_output_buffer.h"
#include "src/developer/debug/zxdb/format/output_buffer.h"
#include "src/lib/fxl/memory/ref_counted.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class Command;
class Console;
class ConsoleContext;

// This object collects the output and errors from a command and tracks its completion.
//
// The command implementation must keep this object alive for as long as the command executes (which
// could be asynchronously). When the CommandContext is destroyed, the callbacks will executed and
// the command will be considered complete.
class CommandContext : public fxl::RefCountedThreadSafe<CommandContext> {
 public:
  // Writes the given buffer to the output.
  virtual void Output(const OutputBuffer& output) = 0;

  // Convenience wrapper to output the given string.
  void Output(const std::string& s) { Output(OutputBuffer(s)); }

  // Synchronously prints the output if the async buffer is complete. Otherwise adds a listener and
  // prints the output to the console when it is complete.
  //
  // This call takes a reference to the CommandContext (keeping the command in a non-completed
  // state) for as long as the AsyncOutputBuffer remains incomplete.
  void Output(fxl::RefPtr<AsyncOutputBuffer> output);

  // Reports that the command failed with the given error. The error will be printed to the screen.
  virtual void ReportError(const Err& err) = 0;

  // The Console/ConsoleContext may be null if this object has outlived the Console object. In
  // production this probably won't happen but can be triggered in tests more easily.
  //
  // If the code calling this function is being used in a synchronous context (i.e. called directly
  // from a command handler and not from a callback), these pointers are guaranteed non-null.
  Console* console() { return weak_console_.get(); }
  ConsoleContext* GetConsoleContext() const;

  // Returns true if this command context has encountered any error.
  bool has_error() const;

  // Sets the completion observer used by the console to tell when the command is done. This is used
  // for enabling and disabling input. The callback passed into the constructors of some derived
  // classes are instead for the creator of the CommandContext (which may not necessarily be the
  // console).
  //
  // Since this is currently used only for console integration, it's called the "Console" completion
  // observer and there can be only one of them. If we have a need for more than one, we can
  // generalize this in the future.
  void SetConsoleCompletionObserver(fit::deferred_callback observer);

  // Sets the command report for this command. This is called after a command has been successfully
  // parsed. If the command fails parsing, we use the default constructed |report_| with the error
  // given to |SetError| by the derived class.
  void SetCommandReport(CommandReport other);
  const CommandReport& GetCommandReport() const { return report_; }

 protected:
  FRIEND_REF_COUNTED_THREAD_SAFE(CommandContext);

  // Console may be null.
  explicit CommandContext(Console* console);
  virtual ~CommandContext();

  // Used by derived classes to set the offending error.
  void SetError(const Err& err);

 private:
  fxl::WeakPtr<Console> weak_console_;

  // Track all asynchronous output pending. We want to store a reference and lookup by pointer, so
  // the object is duplicated here (RefPtr doesn't like to be put in a set).
  //
  // These pointers own the tree of async outputs for each async operation. We need to keep owning
  // pointers to the roots of every AsyncOutputBuffer we've installed ourselves as a completion
  // callback for to keep them in scope until they're completed.
  std::map<AsyncOutputBuffer*, fxl::RefPtr<AsyncOutputBuffer>> async_output_;

  // A report with all of the basic information for the command associated with this context. If
  // analytics are enabled, this will contain all of the information sent for any particular
  // command. This value is always valid. A default constructed report is made at construction time,
  // which can be used to report parsing errors for something that was typed on the command line.
  // Once a command has been parsed and filled in an instance of a Command, the default report is
  // replaced with one created from that Command instance.
  CommandReport report_;

  fit::deferred_callback console_completion_observer_;
};

// This is the normal implementation that just outputs everything to the console.
class ConsoleCommandContext : public CommandContext {
 public:
  // A completion callback is issued when this object goes out of scope. It is passed the first
  // error that was output (if any) which allows the caller to determine success or failure of the
  // operation.
  //
  // This error (along with any subsequent ones) will have already been printed so does not need
  // further processing in the common case.
  using CompletionCallback = fit::callback<void(const Err& first_error)>;

  using CommandContext::Output;
  void Output(const OutputBuffer& output) override;
  void ReportError(const Err& err) override;

 private:
  FRIEND_REF_COUNTED_THREAD_SAFE(ConsoleCommandContext);
  FRIEND_MAKE_REF_COUNTED(ConsoleCommandContext);

  // Console may be null.
  explicit ConsoleCommandContext(Console* console, CompletionCallback done = CompletionCallback());
  ~ConsoleCommandContext() override;

  CompletionCallback done_;  // Possibly null.

  Err first_error_;
};

// This command context implementation collects all the output
class OfflineCommandContext : public CommandContext {
 public:
  // A completion callback is issued when this object goes out of scope. It is passed all output
  // buffers and errors that have been generated.
  using CompletionCallback = fit::callback<void(OutputBuffer output, std::vector<Err> errors)>;

  using CommandContext::Output;
  void Output(const OutputBuffer& output) override;
  void ReportError(const Err& err) override;

 private:
  FRIEND_REF_COUNTED_THREAD_SAFE(OfflineCommandContext);
  FRIEND_MAKE_REF_COUNTED(OfflineCommandContext);

  // Console may be null.
  explicit OfflineCommandContext(Console* console, CompletionCallback done = CompletionCallback());
  ~OfflineCommandContext() override;

  CompletionCallback done_;  // Possibly null.

  OutputBuffer output_;
  std::vector<Err> errors_;
};

// This command context forwards everything to an underlying command context. It allows multiple
// commands to be sequenced (since each NestedCommandContext represents one step) while gathering
// the output into one place.
class NestedCommandContext : public CommandContext {
 public:
  // This completion callback represents just the error from this step.
  using CompletionCallback = fit::callback<void(const Err& first_error)>;

  using CommandContext::Output;
  void Output(const OutputBuffer& output) override;
  void ReportError(const Err& err) override;

 private:
  FRIEND_REF_COUNTED_THREAD_SAFE(NestedCommandContext);
  FRIEND_MAKE_REF_COUNTED(NestedCommandContext);

  explicit NestedCommandContext(fxl::RefPtr<CommandContext> parent,
                                CompletionCallback cb = CompletionCallback());
  ~NestedCommandContext() override;

  fxl::RefPtr<CommandContext> parent_;
  CompletionCallback done_;  // Possibly null.

  Err first_error_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CONSOLE_COMMAND_CONTEXT_H_
