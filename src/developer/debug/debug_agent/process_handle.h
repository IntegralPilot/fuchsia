// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_PROCESS_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_PROCESS_HANDLE_H_

#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <zircon/types.h>

#include <memory>
#include <vector>

#include "src/developer/debug/shared/status.h"

#if defined(__Fuchsia__)
#include <lib/zx/process.h>
#elif defined(__linux__)
#include "src/developer/debug/debug_agent/linux_task.h"
#endif

namespace debug_ipc {
struct AddressRegion;
struct MemoryBlock;
struct Module;
struct InfoHandle;
}  // namespace debug_ipc

namespace debug_agent {

class DebuggedThread;
class ProcessHandleObserver;
class ThreadHandle;

// LINT.IfChange
// DEBUGGER INTERFACE IN DYNAMIC LOADER
//
// Unlike other libcs that use standard debugger interface (https://gbenson.net/r_debug/,
// https://sourceware.org/gdb/wiki/LinkerInterface), Fuchsia and its libc are more cooperative for
// debuggers in that
//   * ZX_PROP_PROCESS_DEBUG_ADDR is used instead of DT_DEBUG in the dynamic table.
//   * ZX_PROP_PROCESS_BREAK_ON_LOAD is used to ask the dynamic loader to issue a breakpoint on
//     module changes proactively instead of requiring debuggers to install a breakpoint on r_brk.
//
// The overall process looks like
//   * When a process starts, it'll set the value of ZX_PROP_PROCESS_DEBUG_ADDR to the r_debug
//     struct and read the value of ZX_PROP_PROCESS_BREAK_ON_LOAD.
//   * If the value of ZX_PROP_PROCESS_BREAK_ON_LOAD is non-zero, it means a debugger is attached
//     and the process should issue a breakpoint upon
//     * The first time ZX_PROP_PROCESS_DEBUG_ADDR is set.
//     * Each dlopen() and dlclose() that changes the module list.
//   * To distinguish the above dynamic loading breakpoint from other user-provided breakpoints
//     (e.g., __buildin_debugtrap()), the process also set the value of
//     ZX_PROP_PROCESS_BREAK_ON_LOAD to the address of the breakpoint instruction before the
//     exception is issued, so that the debugger could compare the address of an exception with
//     this value.
//
// When a debugger attaches to a process
//   * It should first check whether ZX_PROP_PROCESS_BREAK_ON_LOAD is set. If so it should refuse
//     to attach because another debugger has already attached. It's not possible today because
//     there can be at most one debugger channel for each process.
//   * It should set ZX_PROP_PROCESS_BREAK_ON_LOAD to a non-zero value, e.g., 1.
//   * It should check whether ZX_PROP_PROCESS_DEBUG_ADDR is set and read the module list from it.
//
// When a debugger handles a software breakpoint, it should check whether the breakpoint address
// matches the value of ZX_PROP_PROCESS_BREAK_ON_LOAD. If so, it should update the module list and
// continue the execution.
// LINT.ThenChange(//src/starnix/kernel/execution/shared.rs)
#if defined(__Fuchsia__)
using NativeProcessHandle = zx::process;
#elif defined(__linux__)
using NativeProcessHandle = fxl::RefPtr<LinuxTask>;
#else
#error Unknown platform
#endif

class ProcessHandle {
 public:
  virtual ~ProcessHandle() = default;

  // Access to the underlying native process object. This is for porting purposes, ideally this
  // object would encapsulate all details about the process for testing purposes and this getter
  // would be removed. In testing situations, the returned value may be an empty object,
  // TODO(brettw) Remove this.
  virtual const NativeProcessHandle& GetNativeHandle() const = 0;
  virtual NativeProcessHandle& GetNativeHandle() = 0;

  virtual zx_koid_t GetKoid() const = 0;
  virtual std::string GetName() const = 0;

  virtual std::vector<std::unique_ptr<ThreadHandle>> GetChildThreads() const = 0;

  // Get the Koid of the enclosing job.
  virtual zx_koid_t GetJobKoid() const = 0;

  // Terminates the process. The actually termination will normally happen asynchronously.
  virtual debug::Status Kill() = 0;

  // Retrieves the return code for an exited process. Returns some default value if the process is
  // still running (as defined by the kernel).
  virtual int64_t GetReturnCode() const = 0;

  struct AttachConfig {
    AttachConfig() = default;
    bool claim_exception_channel = true;
  };

  // Registers for process notifications on the given interface. The pointer must outlive this class
  // or until Detach() is called. The observer must not be null (use Detach() instead). Calling
  // multiple times will replace the observer pointer.
  virtual debug::Status Attach(ProcessHandleObserver* observer, AttachConfig config) = 0;

  virtual bool IsAttached() const = 0;

  // Unregisters for process notifications. See Attach(). It is legal to call Detach() multiple
  // times or when not already attached.
  virtual void Detach() = 0;

  // Get the address of the dynamic loader's special breakpoint that notifies a module list change.
  // See "DEBUGGER INTERFACE IN DYNAMIC LOADER" above.
  virtual uint64_t GetLoaderBreakpointAddress() = 0;

  // Returns the address space information. If the address is non-null, only the regions covering
  // that address will be returned. Otherwise all regions will be returned.
  virtual std::vector<debug_ipc::AddressRegion> GetAddressSpace(uint64_t address) const = 0;

  // Returns the address region corresponding to the shared address space if this process has
  // multiple instances of ZX_INFO_MAPS_TYPE_ASPACE map types. This is only possible today via
  // processes created with the zx_process_create_shared syscall in conjunction with the
  // ZX_PROCESS_SHARED option to zx_process_create, which can be controlled by options to the ELF
  // runner in a component's CML file or via calling the syscall directly. The process created via
  // zx_process_create with ZX_PROCESS_SHARED is considered the "prototype" process, from which
  // other shared processes are created.
  //
  // This prototype process contains exactly one address space, which is then shared with other
  // processes created via zx_process_create_shared. The shared process, which contains both a
  // private address space and the shared one from the prototype. In either case, this function
  // returns the shared address space from the process, if present.
  virtual std::optional<debug_ipc::AddressRegion> GetSharedAddressSpace() const = 0;

  // Returns the modules (shared libraries and the main binary) for the process. Will be empty on
  // failure.
  //
  // Prefer this version to calling the elf_utils variant because this one allows mocking.
  virtual std::vector<debug_ipc::Module> GetModules() const = 0;

  // Returns the handles opened by the process.
  virtual fit::result<debug::Status, std::vector<debug_ipc::InfoHandle>> GetHandles() const = 0;

  virtual debug::Status ReadMemory(uintptr_t address, void* buffer, size_t len,
                                   size_t* actual) const = 0;
  virtual debug::Status WriteMemory(uintptr_t address, const void* buffer, size_t len,
                                    size_t* actual) = 0;

  // Does a mapped-memory-aware read of the process memory. The result can contain holes which
  // the normal ReadMemory call above can't handle. On failure, there will be one block returned
  // covering the requested size, marked invalid.
  virtual std::vector<debug_ipc::MemoryBlock> ReadMemoryBlocks(uint64_t address,
                                                               uint32_t size) const = 0;

  virtual debug::Status SaveMinidump(const std::vector<DebuggedThread*>& threads,
                                     std::vector<uint8_t>* core_data) = 0;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_PROCESS_HANDLE_H_
