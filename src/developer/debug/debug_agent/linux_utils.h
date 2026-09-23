// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_UTILS_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_UTILS_H_

#include <elf.h>
#include <sys/ptrace.h>
#include <sys/user.h>
#include <zircon/types.h>

#include <filesystem>
#include <map>
#include <optional>
#include <vector>

#include <fbl/unique_fd.h>

#include "src/developer/debug/debug_agent/thread_handle.h"
#include "src/developer/debug/shared/address_range.h"

namespace debug_agent {
namespace linux {

// Wrapper around pidfd_open.
fbl::unique_fd OpenPidfd(int pid);

std::optional<user_regs_struct> ReadGeneralRegisters(int pid);
zx_status_t WriteGeneralRegisters(int pid, const user_regs_struct& regs);

// Represents one line in the Linux /proc/.../maps file.
struct MapEntry {
  debug::AddressRange range;

  bool read = false;
  bool write = false;
  bool execute = false;
  bool shared = false;

  uint64_t offset = 0;
  std::string device;
  int inode = 0;
  std::string path;
};

// Parse/read the given Linux maps file.
std::vector<MapEntry> ParseMaps(const std::string& contents);
std::vector<MapEntry> GetMaps(int pid);

// Entries from /proc/.../stat we care about. This is not all of them and more can be added here if
// needed.
struct ProcStat {
  int pid = 0;
  std::string name;
  char state = 0;
};

// Parse/read the given Linux status file (this is "status" with key/value pairs and not the "stat"
// file that's just numbers). Returns an empty may on failure (the process doesn't exist or
// something).
std::map<std::string, std::string> ParseStatus(const std::string& contents);
std::map<std::string, std::string> GetStatus(int pid);

// Returns the executable file name as defined in /proc/<pid>/exe. Empty on failure.
std::string GetExe(int pid);

// Returns the command line for the given process. Will be empty on failure.
std::vector<std::string> GetCommandLine(int pid);

// Returns the load address of the ld.so for the given process ID. Returns 0 on failure.
uint64_t GetLdSoLoadAddress(int pid);

// Returns the load address and path for the ld.so associated with the given process ID. Returns
// nullopt on failure.
struct LdSoInfo {
  uint64_t load_address = 0;
  std::string path;
};
std::optional<LdSoInfo> GetLdSoInfo(int pid);

// Returns the ld.so debug address in memory for the given process. Returns 0 if not found.
uint64_t GetLdSoDebugAddress(int pid);

// Returns the address at which to set a breakpoint to observe load events for the given process.
// Returns 0 if not found.
uint64_t GetLdSoBreakpointAddress(int pid);

// Reads the contents of /proc/<pid>/auxv and returns the key/value store. The keys are AT_*
// constants from <linux/auxvec.h>. Returns the empty map on failure.
std::map<uint64_t, uint64_t> ReadAuxv(int pid);

// Decodes the value of a /proc/status "State" line. Returns nullopt if parsing failed.
std::optional<ThreadHandle::State> ThreadStateFromLinuxState(const std::string& linux_state);

// Returns a vector of all numeric subdirectories in the given path. The path is specifyable to
// support enumerating the "task" subdirectory of a process as well as the toplevel processes.
std::vector<int> GetDirectoryPids(const std::filesystem::path& path = "/proc");

// Returns the string for the given signal name. If include_number is set, this also includes the
// numeric signal number.
std::string SignalToString(int signal, bool include_number);

// Given a local path to the dynamic loader on disk, returns the address (relative to the beginning
// of the module in memory) of the debug address or load breakpoint address.
//
// The Fuchsia loader does not expose these symbols which is why they're in a Linux-specific file.
uint64_t GetLdSoDebugRelativeAddress(const std::string& ld_so_path);
uint64_t GetLdSoBreakpointRelativeAddress(const std::string& ld_so_path);

}  // namespace linux
}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_UTILS_H_
