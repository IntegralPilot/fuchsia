// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/console/command_context.h"

#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/analytics_reporter.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/common/ref_ptr_to.h"
#include "src/developer/debug/zxdb/console/console.h"

namespace zxdb {

CommandContext::CommandContext(Console* console)
    : weak_console_(console ? console->GetWeakPtr() : nullptr) {}

CommandContext::~CommandContext() {
  if (weak_console_) {
    weak_console_->context().session()->analytics().ReportCommand(report_);
  }
}

void CommandContext::Output(fxl::RefPtr<AsyncOutputBuffer> output) {
  if (output->is_complete()) {
    // Synchronously available.
    Output(output->DestructiveFlatten());
  } else {
    // Listen for completion.
    AsyncOutputBuffer* output_ptr = output.get();
    output->SetCompletionCallback([this_ref = RefPtrTo(this), output_ptr = output.get()]() {
      this_ref->Output(output_ptr->DestructiveFlatten());

      auto found = this_ref->async_output_.find(output_ptr);
      FX_DCHECK(found != this_ref->async_output_.end());
      this_ref->async_output_.erase(found);
    });
    async_output_[output_ptr] = std::move(output);
  }
}

ConsoleContext* CommandContext::GetConsoleContext() const {
  if (weak_console_)
    return &weak_console_->context();
  return nullptr;
}

void CommandContext::SetConsoleCompletionObserver(fit::deferred_callback observer) {
  FX_DCHECK(!console_completion_observer_);
  console_completion_observer_ = std::move(observer);
}

void CommandContext::SetCommandReport(CommandReport other) { report_ = std::move(other); }

bool CommandContext::has_error() const { return report_.err.has_error(); }

void CommandContext::SetError(const Err& err) { report_.err = err; }

// ConsoleCommandContext ---------------------------------------------------------------------------

ConsoleCommandContext::ConsoleCommandContext(Console* console, CompletionCallback done)
    : CommandContext(console), done_(std::move(done)) {}

ConsoleCommandContext::~ConsoleCommandContext() {
  if (done_)
    done_(first_error_);
}

void ConsoleCommandContext::Output(const OutputBuffer& output) {
  if (console())
    console()->Output(output);
}

void ConsoleCommandContext::ReportError(const Err& err) {
  if (!first_error_.has_error()) {
    SetError(err);
    first_error_ = err;
  }

  OutputBuffer out;
  out.Append(err);
  Output(out);
}

// OfflineCommandContext ---------------------------------------------------------------------------

OfflineCommandContext::OfflineCommandContext(Console* console, CompletionCallback done)
    : CommandContext(console), done_(std::move(done)) {}

OfflineCommandContext::~OfflineCommandContext() {
  if (done_)
    done_(std::move(output_), std::move(errors_));
}

void OfflineCommandContext::Output(const OutputBuffer& output) { output_.Append(output); }

void OfflineCommandContext::ReportError(const Err& err) {
  // Set the first one to be reported.
  if (errors_.empty())
    SetError(err);

  errors_.push_back(err);

  OutputBuffer out;
  out.Append(err);
  if (!debug::StringEndsWith(err.msg(), "\n"))
    out.Append("\n");
  Output(out);
}

// NestedCommandContext ----------------------------------------------------------------------------

NestedCommandContext::NestedCommandContext(fxl::RefPtr<CommandContext> parent,
                                           CompletionCallback cb)
    : CommandContext(parent->console()), parent_(std::move(parent)), done_(std::move(cb)) {}

NestedCommandContext::~NestedCommandContext() {
  if (done_)
    done_(first_error_);
}

void NestedCommandContext::Output(const OutputBuffer& output) { parent_->Output(output); }

void NestedCommandContext::ReportError(const Err& err) {
  if (!first_error_.has_error()) {
    SetError(err);
    first_error_ = err;
  }
  parent_->ReportError(err);
}

}  // namespace zxdb
