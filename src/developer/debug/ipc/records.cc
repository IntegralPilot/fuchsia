// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/ipc/records.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/syscalls/object.h>

#include <algorithm>

namespace debug_ipc {

bool IsDebug(ExceptionType type) {
  switch (type) {
    // There's an argument to be had about whether these belong here.
    case ExceptionType::kThreadStarting:
    case ExceptionType::kThreadExiting:
    case ExceptionType::kProcessStarting:

    case ExceptionType::kHardwareBreakpoint:
    case ExceptionType::kWatchpoint:
    case ExceptionType::kSingleStep:
    case ExceptionType::kSoftwareBreakpoint:
    case ExceptionType::kSynthetic:
      return true;
    default:
      return false;
  }
}

const char* ExceptionTypeToString(ExceptionType type) {
  switch (type) {
    case ExceptionType::kNone:
      return "None";
    case ExceptionType::kGeneral:
      return "General";
    case ExceptionType::kPageFault:
      return "Fatal Page Fault";
    case ExceptionType::kUndefinedInstruction:
      return "Undefined Instruction";
    case ExceptionType::kUnalignedAccess:
      return "Unaligned Access";
    case ExceptionType::kPolicyError:
      return "Policy Error";
    case ExceptionType::kThreadStarting:
      return "Thread Starting";
    case ExceptionType::kThreadExiting:
      return "Thread Exiting";
    case ExceptionType::kProcessStarting:
      return "Process Starting";
    case ExceptionType::kHardwareBreakpoint:
      return "Hardware Breakpoint";
    case ExceptionType::kWatchpoint:
      return "Watchpoint";
    case ExceptionType::kSingleStep:
      return "Single Step";
    case ExceptionType::kSoftwareBreakpoint:
      return "Software Breakpoint";
    case ExceptionType::kSynthetic:
      return "Synthetic";
    case ExceptionType::kUnknown:
      return "Unknown";
    case ExceptionType::kLast:
      return "kLast";
  }
  FX_NOTREACHED();
  return nullptr;
}

const char* ExceptionStrategyToString(ExceptionStrategy strategy) {
  switch (strategy) {
    case ExceptionStrategy::kNone:
      return "None";
    case ExceptionStrategy::kFirstChance:
      return "First-Chance";
    case ExceptionStrategy::kSecondChance:
      return "Second-Chance";
    case ExceptionStrategy::kLast:
      return "kLast";
  }
  FX_NOTREACHED();
  return nullptr;
}

std::optional<ExceptionStrategy> ToExceptionStrategy(uint32_t raw_value) {
  switch (raw_value) {
    case ZX_EXCEPTION_STRATEGY_FIRST_CHANCE:
      return ExceptionStrategy::kFirstChance;
    case ZX_EXCEPTION_STRATEGY_SECOND_CHANCE:
      return ExceptionStrategy::kSecondChance;
    default:
      return {};
  }
}

std::optional<uint32_t> ToRawValue(ExceptionStrategy strategy) {
  switch (strategy) {
    case ExceptionStrategy::kNone:
    case ExceptionStrategy::kLast:
      return {};
    case ExceptionStrategy::kFirstChance:
      return ZX_EXCEPTION_STRATEGY_FIRST_CHANCE;
    case ExceptionStrategy::kSecondChance:
      return ZX_EXCEPTION_STRATEGY_SECOND_CHANCE;
  };
}

const char* ThreadRecord::StateToString(ThreadRecord::State state) {
  switch (state) {
    case ThreadRecord::State::kNew:
      return "New";
    case ThreadRecord::State::kRunning:
      return "Running";
    case ThreadRecord::State::kSuspended:
      return "Suspended";
    case ThreadRecord::State::kBlocked:
      return "Blocked";
    case ThreadRecord::State::kDying:
      return "Dying";
    case ThreadRecord::State::kDead:
      return "Dead";
    case ThreadRecord::State::kCoreDump:
      return "Core Dump";
    case ThreadRecord::State::kLast:
    default:
      return "Unknown";
  }
}

const char* ThreadRecord::BlockedReasonToString(BlockedReason reason) {
  switch (reason) {
    case ThreadRecord::BlockedReason::kNotBlocked:
      return "Not blocked";
    case ThreadRecord::BlockedReason::kException:
      return "Exception";
    case ThreadRecord::BlockedReason::kSleeping:
      return "Sleeping";
    case ThreadRecord::BlockedReason::kFutex:
      return "Futex";
    case ThreadRecord::BlockedReason::kPort:
      return "Port";
    case ThreadRecord::BlockedReason::kChannel:
      return "Channel";
    case ThreadRecord::BlockedReason::kWaitOne:
      return "Wait one";
    case ThreadRecord::BlockedReason::kWaitMany:
      return "Wait many";
    case ThreadRecord::BlockedReason::kInterrupt:
      return "Interrupt";
    case ThreadRecord::BlockedReason::kPager:
      return "Pager";
    case ThreadRecord::BlockedReason::kLast:
      break;
  }

  FX_NOTREACHED();
  return "";
}

const char* BreakpointTypeToString(BreakpointType type) {
  switch (type) {
    case BreakpointType::kSoftware:
      return "Software";
    case BreakpointType::kHardware:
      return "Hardware";
    case BreakpointType::kReadWrite:
      return "ReadWrite";
    case BreakpointType::kWrite:
      return "Write";
    case BreakpointType::kLast:
      return "Last";
  }

  FX_NOTREACHED();
  return nullptr;
}

bool IsWatchpointType(debug_ipc::BreakpointType type) {
  // clang-format off
  return type == BreakpointType::kReadWrite ||
         type == BreakpointType::kWrite;
  // clang-format on
}

bool AttachConfig::ShouldDeferModules(const AttachConfig& config) {
  switch (config.priority) {
    case debug_ipc::AttachConfig::Priority::kMinimal:
    case debug_ipc::AttachConfig::Priority::kWeak:
      return true;
    case debug_ipc::AttachConfig::Priority::kStrong:
      // Sending modules for a job never makes sense, for processes defer to the relevant options.
      return config.target == debug_ipc::TaskType::kJob;
  }
}

const char* AttachPriorityToString(AttachConfig::Priority priority) {
  switch (priority) {
    case AttachConfig::Priority::kStrong:
      return "Strong";
    case AttachConfig::Priority::kWeak:
      return "Weak";
    case AttachConfig::Priority::kMinimal:
      return "Minimal";
  }

  FX_NOTREACHED();
  return nullptr;
}

AttachConfig FilterConfig::ToAttachConfig(const FilterConfig& filter_config) {
  AttachConfig ret;

  // A never_attach filter will be overridden by anything else.
  if (filter_config.never_attach) {
    ret.priority = AttachConfig::Priority::kMinimal;
  }

  if (filter_config.weak) {
    ret.priority = AttachConfig::Priority::kWeak;
  }

  ret.target = filter_config.job_only ? debug_ipc::TaskType::kJob : debug_ipc::TaskType::kProcess;

  return ret;
}

const char* Filter::TypeToString(Type type) {
  switch (type) {
    case Type::kUnset:
      return "(unset)";
    case Type::kProcessNameSubstr:
      return "process name substr";
    case Type::kProcessName:
      return "process name";
    case Type::kComponentName:
      return "component name";
    case Type::kComponentUrl:
      return "component url";
    case Type::kComponentMoniker:
      return "component moniker";
    case Type::kComponentMonikerSuffix:
      return "component moniker suffix";
    case Type::kComponentMonikerPrefix:
      return "component moniker prefix";
    case Type::kLast:
      break;
  }

  FX_NOTREACHED();
  return nullptr;
}

const char* StackFrame::TrustToString(Trust trust) {
  switch (trust) {
    case debug_ipc::StackFrame::Trust::kScan:
      return "Scan";
    case debug_ipc::StackFrame::Trust::kSigReturn:
      return "SigReturn";
    case debug_ipc::StackFrame::Trust::kSCS:
      return "SCS";
    case debug_ipc::StackFrame::Trust::kFP:
      return "FP";
    case debug_ipc::StackFrame::Trust::kPLT:
      return "PLT";
    case debug_ipc::StackFrame::Trust::kArmEhAbi:
      return "ArmEhAbi";
    case debug_ipc::StackFrame::Trust::kCFI:
      return "CFI";
    case debug_ipc::StackFrame::Trust::kContext:
      return "Context";
    case debug_ipc::StackFrame::Trust::kUnknown:
      return "Unknown";
  }

  FX_NOTREACHED();
  return nullptr;
}

}  // namespace debug_ipc
