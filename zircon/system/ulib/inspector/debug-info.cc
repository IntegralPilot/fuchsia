// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <inttypes.h>
#include <lib/zx/process.h>
#include <lib/zx/suspend_token.h>
#include <lib/zx/thread.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/policy.h>
#include <zircon/types.h>

#include <string>
#include <vector>

#include <inspector/inspector.h>

#include "gwp-asan.h"
#include "src/lib/debug/backtrace-request-utils.h"
#include "zircon/system/ulib/inspector/backtrace.h"

namespace {

#if defined(__x86_64__)
const char* kArch = "x86_64";
#elif defined(__aarch64__)
const char* kArch = "aarch64";
#elif defined(__riscv)
const char* kArch = "riscv64";
#else
#error unsupported architecture
#endif

const char* excp_type_to_str(const zx_excp_type_t type) {
  switch (type) {
    case ZX_EXCP_GENERAL:
      return "general fault";
    case ZX_EXCP_FATAL_PAGE_FAULT:
      return "fatal page fault";
    case ZX_EXCP_UNDEFINED_INSTRUCTION:
      return "undefined instruction";
    case ZX_EXCP_SW_BREAKPOINT:
      return "sw breakpoint";
    case ZX_EXCP_HW_BREAKPOINT:
      return "hw breakpoint";
    case ZX_EXCP_UNALIGNED_ACCESS:
      return "alignment fault";
    case ZX_EXCP_POLICY_ERROR:
      return "policy error";
    default:
      // Note: To get a compilation failure when a new exception type has
      // been added without having also updated this function, compile with
      // -Wswitch-enum.
      return "<unknown fault>";
  }
}

const char* policy_exception_code_to_str(uint32_t policy_exception_code) {
  switch (policy_exception_code) {
    case ZX_EXCP_POLICY_CODE_BAD_HANDLE:
      return "BAD_HANDLE";
    case ZX_EXCP_POLICY_CODE_WRONG_OBJECT:
      return "WRONG_OBJECT";
    case ZX_EXCP_POLICY_CODE_VMAR_WX:
      return "VMAR_WX";
    case ZX_EXCP_POLICY_CODE_NEW_ANY:
      return "NEW_ANY";
    case ZX_EXCP_POLICY_CODE_NEW_VMO:
      return "NEW_VMO";
    case ZX_EXCP_POLICY_CODE_NEW_CHANNEL:
      return "NEW_CHANNEL";
    case ZX_EXCP_POLICY_CODE_NEW_EVENT:
      return "NEW_EVENT";
    case ZX_EXCP_POLICY_CODE_NEW_EVENTPAIR:
      return "NEW_EVENTPAIR";
    case ZX_EXCP_POLICY_CODE_NEW_PORT:
      return "NEW_PORT";
    case ZX_EXCP_POLICY_CODE_NEW_SOCKET:
      return "NEW_SOCKET";
    case ZX_EXCP_POLICY_CODE_NEW_FIFO:
      return "NEW_FIFO";
    case ZX_EXCP_POLICY_CODE_NEW_TIMER:
      return "NEW_TIMER";
    case ZX_EXCP_POLICY_CODE_NEW_PROCESS:
      return "NEW_PROCESS";
    case ZX_EXCP_POLICY_CODE_NEW_PROFILE:
      return "NEW_PROFILE";
    case ZX_EXCP_POLICY_CODE_AMBIENT_MARK_VMO_EXEC:
      return "AMBIENT_MARK_VMO_EXEC";
    case ZX_EXCP_POLICY_CODE_CHANNEL_FULL_WRITE:
      return "CHANNEL_FULL_WRITE";
    case ZX_EXCP_POLICY_CODE_PORT_TOO_MANY_PACKETS:
      return "PORT_TOO_MANY_PACKETS";
    case ZX_EXCP_POLICY_CODE_BAD_SYSCALL:
      return "BAD_SYSCALL";
    case ZX_EXCP_POLICY_CODE_PORT_TOO_MANY_OBSERVERS:
      return "PORT_TOO_MANY_OBSERVERS";
    default:
      return "<unknown policy code>";
  }
}

// Globs the general registers and the interpretation of them as IP, SP, FP, etc.
struct decoded_registers_t {
  zx_vaddr_t pc = 0;
  zx_vaddr_t sp = 0;
  zx_vaddr_t fp = 0;
};

decoded_registers_t decode_registers(const zx_thread_state_general_regs_t* regs) {
  decoded_registers_t decoded;
#if defined(__x86_64__)
  decoded.pc = regs->rip;
  decoded.sp = regs->rsp;
  decoded.fp = regs->rbp;
#elif defined(__aarch64__)
  decoded.pc = regs->pc;
  decoded.sp = regs->sp;
  decoded.fp = regs->r[29];
#elif defined(__riscv)
  decoded.pc = regs->pc;
  decoded.sp = regs->sp;
  decoded.fp = regs->s0;
#else
#error unsupported architecture
#endif

  return decoded;
}

// How much memory to dump, in bytes.
constexpr size_t kMemoryDumpSize = 256;

zx_koid_t get_koid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    printf("failed to get koid\n");
    return ZX_HANDLE_INVALID;
  }
  return info.koid;
}

void get_name(zx_handle_t handle, char* buf, size_t size) {
  zx_status_t status = zx_object_get_property(handle, ZX_PROP_NAME, buf, size);
  if (status != ZX_OK) {
    strlcpy(buf, "<unknown>", size);
  }
}

void print_exception_report(FILE* out, const zx_exception_report_t& report,
                            const zx_thread_state_general_regs_t* regs,
                            const inspector_excp_data_t* excp_data) {
  decoded_registers_t decoded = decode_registers(regs);

  if (report.header.type == ZX_EXCP_FATAL_PAGE_FAULT) {
    const char* access_type;
    const char* violation;
    zx_vaddr_t fault_addr;
#if defined(__x86_64__)
    static constexpr uint32_t kErrCodeInstrFetch = (1 << 4);
    static constexpr uint32_t kErrCodeWrite = (1 << 1);
    static constexpr uint32_t kErrCodeProtectionViolation = (1 << 0);
    if (report.context.arch.u.x86_64.err_code & kErrCodeInstrFetch) {
      access_type = "execute";
    } else if (report.context.arch.u.x86_64.err_code & kErrCodeWrite) {
      access_type = "write";
    } else {
      access_type = "read";
    }

    if (report.context.arch.u.x86_64.err_code & kErrCodeProtectionViolation) {
      violation = "protection";
    } else {
      violation = "not-present";
    }
    fault_addr = excp_data->cr2;
#elif defined(__aarch64__)
    // The one ec bit that's different between a data and instruction abort
    static constexpr uint32_t kEcDataAbortBit = (1 << 28);
    static constexpr uint32_t kIssCacheOp = (1 << 8);
    static constexpr uint32_t kIssWrite = (1 << 6);
    static constexpr uint32_t kDccNoLvlMask = 0b111100;
    static constexpr uint32_t kDccPermissionFault = 0b001100;
    static constexpr uint32_t kDccTranslationFault = 0b000100;
    static constexpr uint32_t kDccAddressSizeFault = 0b000000;
    static constexpr uint32_t kDccAccessFlagFault = 0b001000;
    static constexpr uint32_t kDccSynchronousExternalFault = 0b010000;

    if (report.context.arch.u.arm_64.esr & kEcDataAbortBit) {
      if (report.context.arch.u.arm_64.esr & kIssWrite &&
          !(report.context.arch.u.arm_64.esr & kIssCacheOp)) {
        access_type = "write";
      } else {
        access_type = "read";
      }
    } else {
      access_type = "execute";
    }

    switch ((report.context.arch.u.arm_64.esr & kDccNoLvlMask)) {
      case kDccPermissionFault:
        violation = "protection";
        break;
      case kDccTranslationFault:
        violation = "not-present";
        break;
      case kDccAddressSizeFault:
        violation = "address-size";
        break;
      case kDccAccessFlagFault:
        violation = "access-flag";
        break;
      case kDccSynchronousExternalFault:
        violation = "external-abort";
        break;
      default:
        violation = "undecoded";
        break;
    }

    fault_addr = excp_data->far;
#elif defined(__riscv)
    static constexpr uint32_t RISCV64_EXCEPTION_INS_PAGE_FAULT = 12;
    static constexpr uint32_t RISCV64_EXCEPTION_LOAD_PAGE_FAULT = 13;
    static constexpr uint32_t RISCV64_EXCEPTION_STORE_PAGE_FAULT = 15;

    switch (report.context.arch.u.riscv_64.cause) {
      case RISCV64_EXCEPTION_INS_PAGE_FAULT:
        access_type = "execute";
        violation = "not-present";
        break;
      case RISCV64_EXCEPTION_LOAD_PAGE_FAULT:
        access_type = "read";
        violation = "not-present";
        break;
      case RISCV64_EXCEPTION_STORE_PAGE_FAULT:
        access_type = "write";
        violation = "not-present";
        break;
      default:
        access_type = "unknown";
    }
    switch (static_cast<zx_status_t>(report.context.synth_code)) {
      case ZX_ERR_NOT_FOUND:
        violation = "not-present";
        break;
      case ZX_ERR_ACCESS_DENIED:
        violation = "protection";
        break;
      default:
        violation = "unknown";
    }
    fault_addr = report.context.arch.u.riscv_64.tval;
#else
#error unsupported architecture
#endif
    fprintf(out, "<== %s %s page fault (error %s) at %#" PRIxPTR ", PC at %#" PRIxPTR "\n",
            access_type, violation,
            zx_status_get_string(static_cast<zx_status_t>(report.context.synth_code)), fault_addr,
            decoded.pc);
  } else if (report.header.type == ZX_EXCP_POLICY_ERROR) {
    switch (report.context.synth_code) {
      case ZX_EXCP_POLICY_CODE_BAD_SYSCALL:
        fprintf(out, "<== policy error: %s (%d, syscall %d), PC at %#" PRIxPTR "\n",
                policy_exception_code_to_str(report.context.synth_code), report.context.synth_code,
                report.context.synth_data, decoded.pc);
        break;

      default:
        fprintf(out, "<== policy error: %s (%d), PC at %#" PRIxPTR "\n",
                policy_exception_code_to_str(report.context.synth_code), report.context.synth_code,
                decoded.pc);
        break;
    }
  } else {
    fprintf(out, "<== %s, PC at %#" PRIxPTR "\n", excp_type_to_str(report.header.type), decoded.pc);
  }
}

// Print the GWP-ASan information if the thread is on a GWP-ASan exception. Do nothing otherwise.
// The process and the thread must be readable.
void print_gwp_asan_info(FILE* out, const zx::process& process,
                         const zx_exception_report_t& exception_report) {
  inspector::GwpAsanInfo info;
  if (inspector::inspector_get_gwp_asan_info(process, exception_report, &info).is_ok() &&
      info.error_type) {
    fprintf(out, "GWP-ASan Error: %s at %#lx\n", info.error_type, info.faulting_addr);
    fprintf(out, "Allocated at %lu with size %lu here:\n", info.allocation_address,
            info.allocation_size);
    for (size_t i = 0; i < info.allocation_trace.size(); i++) {
      fprintf(out, "{{{bt:%lu:%#lx}}}\n", i, info.allocation_trace[i]);
    }
    if (!info.deallocation_trace.empty()) {
      fprintf(out, "Freed here:\n");
      for (size_t i = 0; i < info.deallocation_trace.size(); i++) {
        fprintf(out, "{{{bt:%lu:%#lx}}}\n", i, info.deallocation_trace[i]);
      }
    }
  }
}

// - |skip_markup_context| avoids printing dso list repeatedly for multiple threads in a process.
// - |only_modules_in_stack| will only print a module's info if a frame from the stack is in the
// module. This can be useful for making sure long module lists don't blow out the logs.
void inspector_print_debug_info_impl(FILE* out, zx_handle_t process_handle,
                                     zx_handle_t thread_handle, bool skip_markup_context,
                                     bool only_modules_in_stack) {
  zx_status_t status;
  // If the caller didn't supply |regs| use a local copy.
  zx_thread_state_general_regs_t regs;

  zx::unowned<zx::process> process(process_handle);
  zx_koid_t pid = get_koid(process->get());
  char process_name[ZX_MAX_NAME_LEN];
  get_name(process->get(), process_name, sizeof(process_name));

  zx::unowned<zx::thread> thread(thread_handle);
  zx_koid_t tid = get_koid(thread->get());
  char thread_name[ZX_MAX_NAME_LEN];
  get_name(thread->get(), thread_name, sizeof(thread_name));

  // Attempt to obtain the registers. If this fails, it means that the thread wasn't provided in a
  // valid state.
  status = inspector_read_general_regs(thread->get(), &regs);
  if (status != ZX_OK) {
    printf("[Process %s, Thread %s] Could not get general registers: %s.\n", process_name,
           thread_name, zx_status_get_string(status));
    return;
  }
  decoded_registers_t decoded = decode_registers(&regs);

  // Backtrace requests are special software breakpoints that get resumed. They need to be clearly
  // differentiable from other exceptions.
  bool backtrace_requested = false;

  // Check if the thread is on an exception.
  zx_exception_report_t report;
  bool on_exception = false;
  if (thread->get_info(ZX_INFO_THREAD_EXCEPTION_REPORT, &report, sizeof(report), nullptr,
                       nullptr) == ZX_OK) {
    // The thread is in a valid exception state.
    if (!ZX_EXCP_IS_ARCH(report.header.type) && report.header.type != ZX_EXCP_POLICY_ERROR) {
      return;
    }

    on_exception = true;
    backtrace_requested = is_backtrace_request(report.header.type, &regs);
    if (backtrace_requested) {
      fprintf(out, "<== BACKTRACE REQUEST: process %s[%" PRIu64 "] thread %s[%" PRIu64 "]\n",
              process_name, pid, thread_name, tid);
    } else {
      // Normal exception.
      fprintf(out, "<== CRASH: process %s[%" PRIu64 "] thread %s[%" PRIu64 "]\n", process_name, pid,
              thread_name, tid);

#if defined(__x86_64__)
      inspector_excp_data_t* excp_data = &report.context.arch.u.x86_64;
      zx_vaddr_t tp_dump = regs.fs_base;
#elif defined(__aarch64__)
      inspector_excp_data_t* excp_data = &report.context.arch.u.arm_64;
      zx_vaddr_t tp_dump = regs.tpidr;
#elif defined(__riscv)
      inspector_excp_data_t* excp_data = &report.context.arch.u.riscv_64;
      zx_vaddr_t tp_dump = regs.tp;
#else
#error unsupported architecture
#endif

      print_exception_report(out, report, &regs, excp_data);
      inspector_print_general_regs(out, &regs, excp_data);
      // Print the common stack part of the thread.
      fprintf(out, "bottom of user stack:\n");
      inspector_print_memory(out, process->get(), decoded.sp, kMemoryDumpSize,
                             inspector_print_memory_format::Hex32);

      // Print the bytes "around" the PC to assist in debugging "undefined instruction" exceptions
      // or other cases where it's helpful to know what instruction was executed.
      fprintf(out, "memory dump near pc:\n");
      // Try to capture bytes before and after the PC to ensure we get the full instruction.  Round
      // down to the nearest multiple of 16, then back it up by 32.
      zx_vaddr_t dumpAddr = decoded.pc & ~static_cast<zx_vaddr_t>(16);
      if (dumpAddr >= 32) {
        dumpAddr -= 32;
      }
      constexpr size_t kPcDumpSize = 64;
      inspector_print_memory(out, process->get(), dumpAddr, kPcDumpSize,
                             inspector_print_memory_format::Hex8);

      fprintf(out, "memory dump near thread pointer:\n");
      tp_dump &= -zx_vaddr_t{8};
      tp_dump = std::max<zx_vaddr_t>(tp_dump, 32) - 32;
      constexpr size_t kTpDumpSize = 64;
      inspector_print_memory(out, process->get(), tp_dump, kTpDumpSize,
                             inspector_print_memory_format::Hex32);

      fprintf(out, "arch: %s\n", kArch);
    }
  } else {
    // Print minimal information for threads not on an exception, e.g., other threads in a backtrace
    // request.
    fprintf(out, "<== process %s[%" PRIu64 "] thread %s[%" PRIu64 "]\n", process_name, pid,
            thread_name, tid);
  }

  const std::vector<unwinder::Frame> frames =
      inspector::get_frames(out, process->get(), thread->get());

  // Leave |pcs| empty to print context for all modules.
  std::vector<uint64_t> pcs;
  if (only_modules_in_stack) {
    for (const unwinder::Frame& frame : frames) {
      if (uint64_t pc; frame.regs.GetPC(pc).ok() && pc != 0) {
        pcs.push_back(pc);
      }
    }
  }

  if (!skip_markup_context)
    inspector::print_markup_context(out, process->get(), std::move(pcs));

  inspector_print_backtrace_markup(out, process->get(), thread->get());

  if (on_exception)
    print_gwp_asan_info(out, *process, report);
}

}  // namespace

__EXPORT void inspector_print_debug_info(FILE* out, zx_handle_t process_handle,
                                         zx_handle_t thread_handle) {
  fprintf(out, "{{{reset:begin}}}\n");
  inspector_print_debug_info_impl(out, process_handle, thread_handle, false,
                                  /*only_modules_in_stack=*/true);
  fprintf(out, "{{{reset:end}}}\n");
}

__EXPORT void inspector_print_debug_info_for_thread(FILE* out, zx_handle_t process_handle,
                                                    zx_koid_t thread_koid) {
  zx::unowned<zx::process> process(process_handle);
  zx::thread thread;
  zx_status_t status = process->get_child(thread_koid, ZX_RIGHT_SAME_RIGHTS, &thread);
  if (status != ZX_OK) {
    fprintf(out, "crashsvc: failed to get thread handle for koid %lu: %s\n", thread_koid,
            zx_status_get_string(status));
    return;
  }

  // Suspend the thread to be able to read its registers.
  zx::suspend_token suspend_token;
  status = thread.suspend(&suspend_token);
  if (status != ZX_OK) {
    fprintf(out, "crashsvc: failed to suspend thread for koid %lu: %s\n", thread_koid,
            zx_status_get_string(status));
    return;
  }

  // Wait for the thread to be suspended.
  zx_signals_t observed;
  status = thread.wait_one(ZX_THREAD_SUSPENDED, zx::deadline_after(zx::sec(1)), &observed);
  if (status != ZX_OK) {
    fprintf(out,
            "crashsvc: timed out waiting for thread suspension for koid %lu: %s. Trying to print "
            "anyway.\n",
            thread_koid, zx_status_get_string(status));
  }

  inspector_print_debug_info(out, process_handle, thread.get());
}

// The approach of |inspector_print_debug_info_for_all_threads| is to suspend the process, obtain
// all threads, go over the ones in an exception first and print them and only then print all the
// other threads. This permits to have a clearer view between logs and the crash report.
__EXPORT void inspector_print_debug_info_for_all_threads(FILE* out, zx_handle_t process_handle) {
  zx_status_t status = ZX_ERR_NOT_SUPPORTED;

  zx::unowned<zx::process> process(process_handle);
  char process_name[ZX_MAX_NAME_LEN];
  get_name(process->get(), process_name, sizeof(process_name));
  zx_koid_t process_koid = get_koid(process->get());

  // Suspend the process so that each thread is suspended and no more threads get spawned.
  // NOTE: A process cannot suspend itself, so this could fail on some environments (like calling
  //       this function on your process). To support that usecase, this logic will also try to
  //       suspend each thread individually.
  //
  //       The advantages of suspending the process vs each thread individually are:
  //       1. Threads get suspended at a single point in time, which gives a more accurate
  //          representation of what the process is doing at the moment of printing.
  //       2. When a process is suspended, no more threads will be spawned.
  zx::suspend_token process_suspend_token;
  status = process->suspend(&process_suspend_token);
  if (status != ZX_OK) {
    printf("[Process %s (%" PRIu64 ")] Could not suspend process: %s. Continuing anyway.\n ",
           process_name, process_koid, zx_status_get_string(status));
  }

  // Get the thread list.
  // NOTE: This could be skipping threads being created at the moment of this call.
  //       This is an inherent race between suspending a process and a thread being created.
  size_t actual, avail;
  // This is an outrageous amount of threads to output. We mark them all as invalid first.
  constexpr size_t kMaxThreadHandles = 128;
  std::vector<zx_koid_t> thread_koids(kMaxThreadHandles);
  status = process->get_info(ZX_INFO_PROCESS_THREADS, thread_koids.data(),
                             thread_koids.size() * sizeof(zx_koid_t), &actual, &avail);
  if (status != ZX_OK) {
    printf("[Process %s (%" PRIu64 ")] Could not get list of threads: %s.\n", process_name,
           process_koid, zx_status_get_string(status));
    return;
  }

  std::vector<zx::thread> thread_handles(actual);
  std::vector<std::string> thread_names(actual);
  std::vector<zx_info_thread_t> thread_infos(actual);

  // Get the thread associated data.
  for (size_t i = 0; i < actual; i++) {
    // Get the handles.
    zx::thread& child = thread_handles[i];
    status = process->get_child(thread_koids[i], ZX_RIGHT_SAME_RIGHTS, &child);
    if (status != ZX_OK) {
      printf("[Process %s (%" PRIu64 ")] Could not obtain thread handle: %s.\n", process_name,
             process_koid, zx_status_get_string(status));
      continue;
    }

    // Get the name.
    char thread_name[ZX_MAX_NAME_LEN];
    get_name(child.get(), thread_name, sizeof(thread_name));
    thread_names[i] = thread_name;

    // Get the thread infos.
    zx_info_thread_t thread_info = {};
    status = child.get_info(ZX_INFO_THREAD, &thread_info, sizeof(thread_info), nullptr, nullptr);
    if (status != ZX_OK) {
      printf("[Process %s (%" PRIu64 "), Thread %s (%" PRIu64 ")] Could not obtain info: %s\n",
             process_name, process_koid, thread_names[i].c_str(), thread_koids[i],
             zx_status_get_string(status));
      continue;
    }

    thread_infos[i] = std::move(thread_info);
  }

  // Ensure only the first thread has markup context printed.
  bool skip_markup_context = false;

  fprintf(out, "{{{reset:begin}}}\n");

  // Print the threads in an exception first.
  for (size_t i = 0; i < actual; i++) {
    zx::thread& child = thread_handles[i];
    if (!child.is_valid())
      continue;

    // If the thread is not in an exception, it will be printed on the next loop.
    if (thread_infos[i].state != ZX_THREAD_STATE_BLOCKED_EXCEPTION)
      continue;

    // We print the thread and then mark this koid as empty, so that it won't be printed on the
    // suspended pass. This means we can free the handle after this.
    inspector_print_debug_info_impl(out, process->get(), child.get(), skip_markup_context,
                                    /*only_modules_in_stack=*/false);
    skip_markup_context = true;
    thread_handles[i].reset();
  }

  // Go over each thread and print them.
  for (size_t i = 0; i < actual; i++) {
    if (!thread_handles[i].is_valid())
      continue;

    // If the thread is in an exception, it was already printed by the previous loop.
    if (thread_infos[i].state == ZX_THREAD_STATE_BLOCKED_EXCEPTION) {
      continue;
    }

    zx::thread& child = thread_handles[i];

    // Wait for the thread to be suspended.
    // We do this regardless of the process suspension. There are legitimate cases where the process
    // suspension would fail, like trying to suspend one's own process. If the process suspension
    // was successful, this is a no-op.
    zx::suspend_token suspend_token;
    status = child.suspend(&suspend_token);
    if (status != ZX_OK) {
      printf("[Process %s (%" PRIu64 "), Thread %s (%" PRIu64 ")] Could not suspend thread: %s.\n",
             process_name, process_koid, thread_names[i].c_str(), thread_koids[i],
             zx_status_get_string(status));
      continue;
    }

    status = child.wait_one(ZX_THREAD_SUSPENDED, zx::deadline_after(zx::msec(500)), nullptr);
    if (status != ZX_OK) {
      printf("[Process %s (%" PRIu64 "), Thread %s (%" PRIu64 ")] Didn't get suspend signal: %s.\n",
             process_name, process_koid, thread_names[i].c_str(), thread_koids[i],
             zx_status_get_string(status));
      continue;
    }

    // We can now print the thread.
    inspector_print_debug_info_impl(out, process->get(), child.get(), skip_markup_context,
                                    /*only_modules_in_stack=*/false);
    skip_markup_context = true;
  }

  // Print one last reset to clear all symbolizer contextual state for the process.
  fprintf(out, "{{{reset:end}}}\n");
}
