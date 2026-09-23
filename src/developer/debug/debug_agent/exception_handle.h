// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_EXCEPTION_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_EXCEPTION_HANDLE_H_

#include <lib/fit/result.h>

#include <memory>

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/status.h"
#include "src/lib/fxl/macros.h"

namespace debug_agent {

class ProcessHandle;
class ThreadHandle;

// ExceptionHandle abstracts zx::exception, allowing for a more straightforward implementation in
// tests in overrides of this class.
class ExceptionHandle {
 public:
  // How this exception should be resolved when closed.
  enum class Resolution { kTryNext, kHandled };

  ExceptionHandle() = default;
  virtual ~ExceptionHandle() = default;

  // Returns a handle to the excepting thread. Will return a null pointer on failure.
  virtual std::unique_ptr<ThreadHandle> GetThreadHandle() const = 0;

  virtual std::unique_ptr<ProcessHandle> GetProcessHandle() const = 0;

  // Returns the type of the exception for this and the current thread state.
  //
  // This requires getting the debug registers for the thread so the thread handle is passed in.
  // This could be implemented without the parameter because this object can create thread handles,
  // but that would be less efficient and all callers currently have existing ThreadHandles.
  virtual debug_ipc::ExceptionType GetType(const ThreadHandle& thread) const = 0;

  // Returns the current resolution for the exception.
  virtual fit::result<debug::Status, Resolution> GetResolution() const = 0;

  virtual debug::Status SetResolution(Resolution resolution) = 0;

  // Returns the associated the exception handling strategy.
  virtual fit::result<debug::Status, debug_ipc::ExceptionStrategy> GetStrategy() const = 0;

  // Sets the handling strategy.
  virtual debug::Status SetStrategy(debug_ipc::ExceptionStrategy strategy) = 0;

  // Returns this exception info for IPC.
  //
  // Race conditions or other errors can conspire to mean the exception records are not valid. In
  // order to differentiate this case from "0" addresses, ExceptionRecord.valid will be set to
  // false on failure.
  virtual debug_ipc::ExceptionRecord GetRecord() const = 0;

  // Returns the raw process id from the low level exception object.
  virtual uint64_t GetPid() const = 0;

  // Returns the raw thread id from the low level exception object.
  virtual uint64_t GetTid() const = 0;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_EXCEPTION_HANDLE_H_
