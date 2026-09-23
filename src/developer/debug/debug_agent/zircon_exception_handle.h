// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_EXCEPTION_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_EXCEPTION_HANDLE_H_

#include <lib/zx/exception.h>
#include <lib/zx/process.h>
#include <zircon/syscalls/exception.h>

#include <optional>
#include <utility>

#include "src/developer/debug/debug_agent/exception_handle.h"
#include "src/lib/fxl/macros.h"

namespace debug_agent {

// Wraps a zx::exception, which is expected to be valid for the lifetime of an instance of this
// class.
class ZirconExceptionHandle : public ExceptionHandle {
 public:
  // This is used for normal exceptions as well as thread start and stop events (delivered as
  // exceptions) so the exception report (which is used only for what you would normally think of as
  // "exceptions") will be nullopt in those cases.
  ZirconExceptionHandle(zx::exception exception, const zx_exception_info_t& info,
                        std::optional<zx_exception_report_t> report)
      : exception_(std::move(exception)), info_(info), report_(report) {}

  ~ZirconExceptionHandle() = default;

  std::unique_ptr<ThreadHandle> GetThreadHandle() const override;
  std::unique_ptr<ProcessHandle> GetProcessHandle() const override;
  debug_ipc::ExceptionType GetType(const ThreadHandle& thread) const override;
  fit::result<debug::Status, Resolution> GetResolution() const override;
  debug::Status SetResolution(Resolution state) override;
  fit::result<debug::Status, debug_ipc::ExceptionStrategy> GetStrategy() const override;
  debug::Status SetStrategy(debug_ipc::ExceptionStrategy strategy) override;
  debug_ipc::ExceptionRecord GetRecord() const override;
  uint64_t GetPid() const override;
  uint64_t GetTid() const override;

 private:
  zx::exception exception_;
  zx_exception_info_t info_;
  std::optional<zx_exception_report_t> report_;

  FXL_DISALLOW_COPY_ASSIGN_AND_MOVE(ZirconExceptionHandle);
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_EXCEPTION_HANDLE_H_
