// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/linux_process_handle.h"

#include <fcntl.h>
#include <inttypes.h>
#include <signal.h>
#include <sys/ptrace.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <zircon/errors.h>

#include <fbl/unique_fd.h>

#include "src/developer/debug/debug_agent/elf_utils.h"
#include "src/developer/debug/debug_agent/linux_exception_handle.h"
#include "src/developer/debug/debug_agent/linux_suspend_handle.h"
#include "src/developer/debug/debug_agent/linux_thread_handle.h"
#include "src/developer/debug/debug_agent/linux_utils.h"
#include "src/developer/debug/debug_agent/process_handle_observer.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace debug_agent {

namespace {

// Word size for memory operations with ptrace.
using PtraceWord = uintptr_t;
constexpr size_t kPtraceWordSize = sizeof(PtraceWord);

// Wrapper around ptrace PEEK/POKE for LinuxProcessHandle's WriteMemoryByWord().
debug::Status PtraceReadWrapper(pid_t pid, uintptr_t word_address, uintptr_t* data_out) {
  FX_DCHECK(word_address % kPtraceWordSize == 0);

  errno = 0;
  uint64_t word = ptrace(PTRACE_PEEKTEXT, pid, word_address, 0);
  if (errno)
    return debug::ErrnoStatus(errno);

  *data_out = word;
  return debug::Status();
}

debug::Status PtraceWriteWrapper(pid_t pid, uintptr_t word_address, uintptr_t data) {
  if (ptrace(PTRACE_POKETEXT, pid, word_address, data) == -1)
    return debug::ErrnoStatus(errno);
  return debug::Status();
}

}  // namespace

LinuxProcessHandle::LinuxProcessHandle(fxl::RefPtr<LinuxTask> task) : task_(std::move(task)) {
  task_->SetObserver(this);
}

LinuxProcessHandle::~LinuxProcessHandle() { task_->SetObserver(nullptr); }

std::string LinuxProcessHandle::GetName() const {
  // Use only the module name. This process name will be used as the module name for the main
  // module so must not include the command line.
  if (auto cmdline = linux::GetCommandLine(task_->pid()); !cmdline.empty())
    return cmdline[0];
  return linux::GetExe(task_->pid());
}

std::vector<std::unique_ptr<ThreadHandle>> LinuxProcessHandle::GetChildThreads() const {
  std::vector<std::unique_ptr<ThreadHandle>> result;

  // Child child threads are all stored in the "task" subdirectory of the process.
  for (int child_pid : linux::GetDirectoryPids(fxl::StringPrintf("/proc/%d/task", task_->pid()))) {
    if (child_pid == task_->pid()) {
      // Duplicate of the main PID. Take care to return the same task object to keep the object's
      // state in sync.
      result.push_back(std::make_unique<LinuxThreadHandle>(task_));
    } else {
      result.push_back(
          std::make_unique<LinuxThreadHandle>(fxl::MakeRefCounted<LinuxTask>(child_pid)));
    }
  }

  return result;
}

debug::Status LinuxProcessHandle::Kill() {
  // TODO(brettw) this causes a Zombie. Somehow we don't get an FDReady call after this.
  if (kill(task_->pid(), SIGKILL) == 0)
    return debug::Status();
  return debug::ErrnoStatus(errno);
}

debug::Status LinuxProcessHandle::Attach(ProcessHandleObserver* observer, AttachConfig config) {
  observer_ = observer;
  return task_->Attach();
}

bool LinuxProcessHandle::IsAttached() const { return task_->is_attached(); }

void LinuxProcessHandle::Detach() { task_->Detach(); }

uint64_t LinuxProcessHandle::GetLoaderBreakpointAddress() {
  return linux::GetLdSoBreakpointAddress(task_->pid());
}

std::vector<debug_ipc::AddressRegion> LinuxProcessHandle::GetAddressSpace(uint64_t address) const {
  std::vector<debug_ipc::AddressRegion> result;
  for (auto& entry : linux::GetMaps(task_->pid())) {
    if (address && !entry.range.InRange(address))
      continue;  // Asking for a specific range this address isn't in.

    debug_ipc::AddressRegion& region = result.emplace_back();
    region.name = std::move(entry.path);
    region.base = entry.range.begin();
    region.size = entry.range.size();
    region.depth = 0;  // Linux mappings aren't nested.
    region.vmo_offset = entry.offset;
    region.read = entry.read;
    region.write = entry.write;
    region.execute = entry.execute;
    region.shared = entry.shared;
  }
  return result;
}

std::optional<debug_ipc::AddressRegion> LinuxProcessHandle::GetSharedAddressSpace() const {
  // Not implemented.
  return std::nullopt;
}

std::vector<debug_ipc::Module> LinuxProcessHandle::GetModules() const {
  return GetElfModulesForProcess(*this, linux::GetLdSoDebugAddress(task_->pid()));
}

fit::result<debug::Status, std::vector<debug_ipc::InfoHandle>> LinuxProcessHandle::GetHandles()
    const {
  // This does not exist on linux.
  return fit::error(debug::Status("No handles on Linux"));
}

debug::Status LinuxProcessHandle::ReadMemory(uintptr_t address, void* buffer, size_t len,
                                             size_t* actual) const {
  iovec local_iov;
  local_iov.iov_base = buffer;
  local_iov.iov_len = len;

  iovec remote_iov;
  remote_iov.iov_base = reinterpret_cast<void*>(address);
  remote_iov.iov_len = len;

  auto read_result = process_vm_readv(task_->pid(), &local_iov, 1, &remote_iov, 1, 0);
  if (read_result < 0)
    return debug::ErrnoStatus(errno);

  *actual = static_cast<size_t>(read_result);
  return debug::Status();
}

debug::Status LinuxProcessHandle::WriteMemory(uintptr_t address, const void* buffer, size_t len,
                                              size_t* actual) {
  *actual = 0;

  // These ptrace functions require a stopped process which isn't guaranteed by our infrastructure.
  // Linux suspensions are synchronous so we don't need to wait for this to complete.
  LinuxSuspendHandle suspension(task_);

  // Unlike for reading, we can't use process_vm_writev() because that won't let us write to
  // read-only memory which is needed for breakpoints. PTRACE_POKETEXT allows us to do this but the
  // API is much less convenient.
  return WriteMemoryByWord(task_->pid(), address, buffer, len, &PtraceReadWrapper,
                           &PtraceWriteWrapper, actual);
}

std::vector<debug_ipc::MemoryBlock> LinuxProcessHandle::ReadMemoryBlocks(uint64_t address,
                                                                         uint32_t size) const {
  // TODO(brettw) figure out how this should work with respect to invalid memory regions.
  debug_ipc::MemoryBlock block;
  block.address = address;
  block.size = size;
  block.valid = true;
  block.data.resize(size);

  if (size > 0) {
    size_t actual;
    if (ReadMemory(address, &block.data[0], size, &actual).has_error()) {
      block.valid = false;
      block.data.clear();
    }
  }

  return {std::move(block)};
}

debug::Status LinuxProcessHandle::SaveMinidump(const std::vector<DebuggedThread*>& threads,
                                               std::vector<uint8_t>* core_data) {
  return debug::Status("SaveMinidump is unimplemented on Linux.");
}

// static
debug::Status LinuxProcessHandle::WriteMemoryByWord(pid_t pid, uintptr_t address,
                                                    const void* buffer, size_t len,
                                                    ReadWordFunc read_fn, WriteWordFunc write_fn,
                                                    size_t* actual) {
  if (len == 0)
    return debug::Status();

  const uint8_t* source_byte_buffer = static_cast<const uint8_t*>(buffer);

  uintptr_t word_first = address / kPtraceWordSize * kPtraceWordSize;
  uintptr_t word_last = (address + len - 1) / kPtraceWordSize * kPtraceWordSize;

  size_t begin_offset_in_word = address % kPtraceWordSize;

  // Using this buffer + memcpy is not as fast as doing bitwise register operations but it's easier
  // to write correctly and avoids any endian issues or undefined compiler behavior.

  // copy_target_to_word_buffer and copy_word_buffer_to_target transfers between the target memory
  // and our word buffer, converted through a register word as used in ptrace.
  uint8_t word_buffer[kPtraceWordSize];
  auto copy_target_to_word_buffer = [pid, word_buffer = word_buffer,
                                     read_fn](uintptr_t word_address) mutable {
    PtraceWord word;
    if (debug::Status status = read_fn(pid, word_address, &word); status.has_error())
      return status;
    memcpy(word_buffer, &word, kPtraceWordSize);
    return debug::Status();
  };
  auto copy_word_buffer_to_target = [pid, word_buffer = word_buffer,
                                     write_fn](uintptr_t word_address) {
    PtraceWord word;
    memcpy(&word, word_buffer, kPtraceWordSize);
    return write_fn(pid, word_address, word);
  };

  if (word_first == word_last) {
    // Handle masking on both ends.
    if (debug::Status status = copy_target_to_word_buffer(word_first); status.has_error())
      return status;
    memcpy(&word_buffer[begin_offset_in_word], source_byte_buffer, len);
    if (debug::Status status = copy_word_buffer_to_target(word_first); status.has_error())
      return status;

    *actual = len;
    return debug::Status();
  }

  // Writing more than one word.

  // Left side.
  size_t source_offset = 0;  // Index of the byte we're reading, updated as we go.
  uintptr_t first_full_word_address;
  if (address % kPtraceWordSize == 0) {
    // Beginning is word aligned.
    first_full_word_address = word_first;
  } else {
    // Need to read and combine the bytes on the left size.
    // Example of which bytes to write for begin_offset_in_word = 5:
    //               0 1 2 3 4 5 6 7
    //             [ . . . . . X X X ]
    if (debug::Status status = copy_target_to_word_buffer(word_first); status.has_error())
      return status;
    size_t left_bytes_to_write = kPtraceWordSize - begin_offset_in_word;  // = 3 for the example.
    memcpy(&word_buffer[begin_offset_in_word], source_byte_buffer, left_bytes_to_write);
    if (debug::Status status = copy_word_buffer_to_target(word_first); status.has_error())
      return status;

    (*actual) += left_bytes_to_write;
    source_offset += left_bytes_to_write;
    first_full_word_address = word_first + kPtraceWordSize;
  }

  // Set of full words.
  uintptr_t last_full_word_address = (address + len - 8) / kPtraceWordSize * kPtraceWordSize;
  if (last_full_word_address >= first_full_word_address) {
    size_t num_full_words =
        (last_full_word_address - first_full_word_address) / kPtraceWordSize + 1;
    for (size_t word_i = 0; word_i < num_full_words; word_i++) {
      PtraceWord word;
      memcpy(&word, &source_byte_buffer[source_offset], kPtraceWordSize);
      if (debug::Status status =
              write_fn(pid, first_full_word_address + word_i * kPtraceWordSize, word);
          status.has_error()) {
        return status;
      }

      (*actual) += kPtraceWordSize;
      source_offset += kPtraceWordSize;
    }
  }

  // Right side.
  if (source_offset < len) {
    FX_DCHECK(len - source_offset < kPtraceWordSize);  // Should have less than one word remaining.
    if (debug::Status status = copy_target_to_word_buffer(word_last); status.has_error())
      return status;
    memcpy(word_buffer, &source_byte_buffer[source_offset], len - source_offset);
    if (debug::Status status = copy_word_buffer_to_target(word_last); status.has_error())
      return status;

    (*actual) += len - source_offset;
  }

  return debug::Status();
}

void LinuxProcessHandle::OnExited(LinuxTask* task,
                                  std::unique_ptr<LinuxExceptionHandle> exception) {
  // This indicates thread termination, which can also be process termination for the main thread.
  if (task->pid() == task_->pid()) {
    // Thread is the main thread of this process.
    observer_->OnProcessTerminated();
  } else {
    observer_->OnThreadExiting(std::move(exception));
  }
}

void LinuxProcessHandle::OnProcessStarting(std::unique_ptr<LinuxExceptionHandle> exception) {
  observer_->OnProcessStarting(std::make_unique<LinuxProcessHandle>(exception->task()));

  // Letting the exception handle go out of scope here may resume the new process. If the
  // observer wants it suspended it will have taken a new suspend handle to it.
}

void LinuxProcessHandle::OnThreadStarting(std::unique_ptr<LinuxExceptionHandle> exception) {
  // Register outselves as the watcher for the new task. All thread notifications are routed through
  // the process.
  exception->task()->SetObserver(this);
  observer_->OnThreadStarting(std::move(exception));
}

void LinuxProcessHandle::OnTermSignal(int pid, int signal_number) {
  observer_->OnProcessTerminated();
}

void LinuxProcessHandle::OnStopSignal(LinuxTask* task,
                                      std::unique_ptr<LinuxExceptionHandle> exception) {
  observer_->OnException(std::move(exception));
}

void LinuxProcessHandle::OnContinued(int pid) {}

}  // namespace debug_agent
