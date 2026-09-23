// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// clang-format off
// Included early because of conflicts.
#include "src/lib/elflib/elflib.h"
// clang-format on

#include "src/developer/debug/debug_agent/linux_utils.h"

#include <ctype.h>
#include <fcntl.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <unistd.h>
#include <zircon/errors.h>

#include <optional>
#include <sstream>

#include <fbl/unique_fd.h>
#include <linux/auxvec.h>
#include <linux/limits.h>

#include "src/developer/debug/debug_agent/elf_utils.h"
#include "src/developer/debug/debug_agent/process_handle.h"
#include "src/lib/files/eintr_wrapper.h"
#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace debug_agent {
namespace linux {

namespace {

// Syscall ID of the pidfd_open() function. From "man pidfd_open".
constexpr long kPidFDOpenSyscallID = 434;

// Reads "/proc/<pid>/<name>" and returns it as a single string. Returns the empty string on error.
std::string ReadProcFile(int pid, const char* name) {
  std::string file_name = fxl::StringPrintf("/proc/%d/%s", pid, name);

  std::string result;
  files::ReadFileToString(file_name, &result);
  return result;
}

// Returns if this path is composed only of numeric digits.
bool IsNumericPath(const std::filesystem::path& path) {
  for (char c : path.string()) {
    if (!isdigit(c))
      return false;
  }
  return true;
}

// Converts a /proc/* path to a PID if it references one. Otherwise returns nullopt.
std::optional<int> PathToPid(const std::filesystem::path& path) {
  auto last_part = path.filename();
  if (!IsNumericPath(last_part))
    return std::nullopt;

  return atoi(last_part.c_str());
}

}  // namespace

fbl::unique_fd OpenPidfd(int pid) { return fbl::unique_fd(syscall(kPidFDOpenSyscallID, pid, 0)); }

std::optional<user_regs_struct> ReadGeneralRegisters(int pid) {
  user_regs_struct regs;

  iovec vec;
  vec.iov_base = &regs;
  vec.iov_len = sizeof(regs);
  if (ptrace(PTRACE_GETREGSET, pid, NT_PRSTATUS, &vec) == -1) {
    return std::nullopt;
  }
  return regs;
}

zx_status_t WriteGeneralRegisters(int pid, const user_regs_struct& regs) {
  iovec vec;
  vec.iov_base = const_cast<void*>(static_cast<const void*>(&regs));
  vec.iov_len = sizeof(regs);
  if (ptrace(PTRACE_SETREGSET, pid, NT_PRSTATUS, &vec) == -1)
    return ZX_ERR_INTERNAL;
  return ZX_OK;
}

std::vector<MapEntry> ParseMaps(const std::string& contents) {
  std::vector<MapEntry> result;

  std::istringstream in(contents);
  while (in.good()) {
    MapEntry entry;

    // <address>-<address>
    uint64_t begin = 0, end = 0;
    char hyphen = 0;
    in >> std::hex >> begin >> hyphen >> std::hex >> end;
    if (!in.good() || end < begin)
      return result;
    entry.range = debug::AddressRange(begin, end);

    // Flags.
    char r_bit, w_bit, x_bit, s_bit;
    in >> r_bit >> w_bit >> x_bit >> s_bit;
    if (!in.good())
      return result;
    entry.read = r_bit == 'r';
    entry.write = w_bit == 'w';
    entry.execute = x_bit == 'x';
    entry.shared = s_bit == 's';

    // Offset, device, inode.
    in >> std::hex >> entry.offset >> entry.device >> std::dec >> entry.inode;

    // Everything else to the end is the path (could be empty).
    while (in.good() && in.peek() == ' ')
      in.get();  // Skip whitespace before.
    std::getline(in, entry.path);

    result.push_back(std::move(entry));
  }
  return result;
}

std::vector<MapEntry> GetMaps(int pid) { return ParseMaps(ReadProcFile(pid, "maps")); }

std::map<std::string, std::string> ParseStatus(const std::string& contents) {
  std::istringstream in(contents);

  std::map<std::string, std::string> result;
  do {
    std::string line;
    std::getline(in, line);

    // Split on the first colon.
    std::string key, value;
    for (size_t i = 0; i < line.size(); i++) {
      if (line[i] == ':') {
        key = line.substr(0, i);

        // Skip spaces.
        i++;
        while (i < line.size() && (line[i] == ' ' || line[i] == '\t'))
          i++;

        // Everything after that is the value.
        value = line.substr(i);
        break;
      }
    }
    if (key.empty() && value.empty())
      break;

    result[key] = std::move(value);
  } while (in.good());

  return result;
}

std::map<std::string, std::string> GetStatus(int pid) {
  return ParseStatus(ReadProcFile(pid, "status"));
}

std::string GetExe(int pid) {
  std::string link_path = fxl::StringPrintf("/proc/%d/exe", pid);

  char dest_path[PATH_MAX];  // Not null terminated!
  ssize_t result = readlink(link_path.c_str(), dest_path, PATH_MAX);
  if (result > 0)
    return std::string(dest_path, result);

  // Some links aren't set, so fall back on the name from the proc status.
  return GetStatus(pid)["Name"];
}

std::vector<std::string> GetCommandLine(int pid) {
  // /proc/<pid>/cmdline is a null-separated view of the command line.
  return fxl::SplitStringCopy(ReadProcFile(pid, "cmdline"), std::string_view("\0", 1),
                              fxl::kKeepWhitespace, fxl::kSplitWantNonEmpty);
}

uint64_t GetLdSoLoadAddress(int pid) { return ReadAuxv(pid)[AT_BASE]; }

std::optional<LdSoInfo> GetLdSoInfo(int pid) {
  uint64_t load_addr = GetLdSoLoadAddress(pid);
  if (!load_addr)
    return std::nullopt;

  for (const MapEntry& entry : GetMaps(pid)) {
    if (entry.range.begin() == load_addr)
      return LdSoInfo{.load_address = load_addr, .path = entry.path};
  }
  return std::nullopt;
}

uint64_t GetLdSoDebugAddress(int pid) {
  auto info = GetLdSoInfo(pid);
  if (!info)
    return 0;

  if (uint64_t relative_addr = GetLdSoDebugRelativeAddress(info->path))
    return info->load_address + relative_addr;
  return 0;
}

uint64_t GetLdSoBreakpointAddress(int pid) {
  auto info = GetLdSoInfo(pid);
  if (!info)
    return 0;

  if (uint64_t relative_addr = GetLdSoBreakpointRelativeAddress(info->path))
    return info->load_address + relative_addr;
  return 0;
}

std::map<uint64_t, uint64_t> ReadAuxv(int pid) {
  std::map<uint64_t, uint64_t> result;

  std::string contents = ReadProcFile(pid, "auxv");
  size_t key_count = contents.size() / sizeof(uint64_t);
  const uint64_t* raw = reinterpret_cast<const uint64_t*>(contents.data());

  for (size_t i = 0; i < key_count; i++)
    result[raw[i * 2]] = raw[i * 2 + 1];

  return result;
}

std::optional<ThreadHandle::State> ThreadStateFromLinuxState(const std::string& linux_state) {
  using State = debug_ipc::ThreadRecord::State;
  using BlockedReason = debug_ipc::ThreadRecord::BlockedReason;

  if (linux_state.empty())
    return std::nullopt;

  // The first character gives the state.
  // TODO(brettw) We should probably extend our thread state options to better cover the Linux ones.
  switch (linux_state[0]) {
    case 'S':  // Sleeping
      return ThreadHandle::State(BlockedReason::kSleeping);
    case 'D':  // Waiting in uninterruptible disk sleep
      return ThreadHandle::State(BlockedReason::kSleeping);
    case 'T':  // Stopped (on a signal) or (before Linux 2.6.33) trace stopped
      return ThreadHandle::State(BlockedReason::kException);
    case 't':  // Tracing stop (Linux 2.6.33 onward)
      return ThreadHandle::State(State::kSuspended);
    case 'K':  // Wakekill (Linux 2.6.33 to 3.13 only)
    case 'W':  // Waking (Linux 2.6.33 to 3.13 only)
      return ThreadHandle::State(State::kRunning);
    case 'P':  // Parked (Linux 3.9 to 3.13 only)
      return ThreadHandle::State(BlockedReason::kSleeping);
    case 'X':  // Dead (from Linux 2.6.0 onward)
    case 'x':  // Dead (Linux 2.6.33 to 3.13 only)
    case 'Z':  // Zombie
      return ThreadHandle::State(State::kDead);
    case 'R':  // Running
      return ThreadHandle::State(State::kRunning);
    default:
      return std::nullopt;
  }
}

std::vector<int> GetDirectoryPids(const std::filesystem::path& path) {
  std::vector<int> result;

  std::error_code ec;
  for (const auto& child : std::filesystem::directory_iterator(path, ec)) {
    if (std::filesystem::is_directory(child.path(), ec)) {
      if (auto parsed = PathToPid(child.path()))
        result.push_back(*parsed);
    }
  }
  return result;
}

std::string SignalToString(int signal, bool include_number) {
  // clang-format off
  const char* name = nullptr;
  switch (signal) {
    case SIGABRT:   name = "SIGABRT";          break;
    case SIGALRM:   name = "SIGALRM";          break;
    case SIGBUS:    name = "SIGBUS";           break;
    case SIGCHLD:   name = "SIGCHLD";          break;
    case SIGCONT:   name = "SIGCONT";          break;
    case SIGFPE:    name = "SIGFPE";           break;
    case SIGHUP:    name = "SIGHUP";           break;
    case SIGILL:    name = "SIGILL";           break;
    case SIGINT:    name = "SIGINT";           break;
    case SIGKILL:   name = "SIGKILL";          break;
    case SIGPIPE:   name = "SIGPIPE";          break;
    case SIGPOLL:   name = "SIGPOLL";          break;
    case SIGPROF:   name = "SIGPROF";          break;
    case SIGPWR:    name = "SIGPWR";           break;
    case SIGQUIT:   name = "SIGQUIT";          break;
    case SIGSEGV:   name = "SIGSEGV";          break;
    case SIGSTKFLT: name = "SIGSTKFLT";        break;
    case SIGSTOP:   name = "SIGSTOP";          break;
    case SIGSYS:    name = "SIGSYS";           break;
    case SIGTERM:   name = "SIGTERM";          break;
    case SIGTRAP:   name = "SIGTRAP";          break;
    case SIGTSTP:   name = "SIGTSTP";          break;
    case SIGTTIN:   name = "SIGTTIN";          break;
    case SIGTTOU:   name = "SIGTTOU";          break;
    case SIGURG:    name = "SIGURG";           break;
    case SIGUSR1:   name = "SIGUSR1";          break;
    case SIGUSR2:   name = "SIGUSR2";          break;
    case SIGVTALRM: name = "SIGVTALRM";        break;
    case SIGWINCH:  name = "SIGWINCH";         break;
    case SIGXCPU:   name = "SIGXCPU";          break;
    case SIGXFSZ:   name = "SIGXFSZ";          break;
    default:        name = "<Unknown signal>"; break;
  }
  // clang-format on

  if (include_number) {
    std::ostringstream out;
    out << name << " (" << signal << ")";
    return out.str();
  }
  return name;
}

uint64_t GetLdSoDebugRelativeAddress(const std::string& ld_so_path) {
  auto elf = elflib::ElfLib::Create(ld_so_path);
  if (!elf)
    return 0;

  // LLDB and GDB have a list of different symbols to look for for different dynamic loaders.
  // We're only currently concerned with the one loader so omit that for now.
  if (auto sym = elf->GetDynamicSymbol("_r_debug"))
    return sym->st_value;

  return 0;
}

uint64_t GetLdSoBreakpointRelativeAddress(const std::string& ld_so_path) {
  auto elf = elflib::ElfLib::Create(ld_so_path);
  if (!elf)
    return 0;

  // Different loaders use different symbol names. This is the list that everybody uses:
  static const char* kDebugStateNames[] = {
      "_dl_debug_state", "__dl_rtld_db_dlactivity", "_r_debug_state",
      "r_debug_state",   "rtld_db_dlactivity",      "_rtld_debug_state",
  };

  for (const char* name : kDebugStateNames) {
    if (auto sym = elf->GetDynamicSymbol(name))
      return sym->st_value;
  }

  return 0;
}

}  // namespace linux
}  // namespace debug_agent
