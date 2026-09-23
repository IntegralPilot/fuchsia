// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_IPC_RECORDS_H_
#define SRC_DEVELOPER_DEBUG_IPC_RECORDS_H_

#include <stdint.h>

#include <algorithm>
#include <compare>
#include <iterator>
#include <optional>
#include <string>
#include <vector>

#include "src/developer/debug/ipc/automation_instruction.h"
#include "src/developer/debug/shared/address_range.h"
#include "src/developer/debug/shared/register_id.h"
#include "src/developer/debug/shared/register_value.h"
#include "src/developer/debug/shared/serialization.h"

namespace debug_ipc {

#pragma pack(push, 8)

enum class ExceptionType : uint32_t {
  // No current exception, used as placeholder or to indicate not set.
  kNone = 0,

  // Zircon defines this as a sort of catch-all exception.
  kGeneral,

  // The usual band of execution traps.
  kPageFault,
  kUndefinedInstruction,
  kUnalignedAccess,

  // Indicates the process was killed due to misusing a syscall, e.g. passing a bad handle.
  kPolicyError,

  // Synthetic exeptions used by zircon to communicated with the debugger. The debug agent generally
  // shouldn't pass these on, but we should recognize them at least.
  kThreadStarting,
  kThreadExiting,
  kProcessStarting,

  // Hardware breakpoints are issues by the CPU via debug registers.
  kHardwareBreakpoint,

  // HW exceptions triggered on memory read/write.
  kWatchpoint,

  // Single-step completion issued by the CPU.
  kSingleStep,

  // Software breakpoint. This will be issued when a breakpoint is hit and when the debugged program
  // manually issues a breakpoint instruction.
  kSoftwareBreakpoint,

  // Indicates this exception is not a real CPU exception but was generated internally for the
  // purposes of sending a stop notification. The frontend uses this value when the thread didn't
  // actually do anything, but the should be updated as if it hit an exception.
  kSynthetic,

  // For exception codes the debugger doesn't recognize.
  kUnknown,

  kLast  // Not an actual exception type, for range checking.
};
const char* ExceptionTypeToString(ExceptionType);
bool IsDebug(ExceptionType);

// Exception handling strategy.
enum class ExceptionStrategy : uint32_t {
  // No current exception, used as placeholder or to indicate not set.
  kNone = 0,

  // Indicates that the debugger only gets the first chance to handle the
  // exception, before the thread and process-level handlers.
  kFirstChance,

  // Indicates that the debugger also gets a second first chance to handle
  //  the exception after process-level handler.
  kSecondChance,

  kLast,  // Not an actual exception strategy, for range checking.
};

const char* ExceptionStrategyToString(ExceptionStrategy);

std::optional<ExceptionStrategy> ToExceptionStrategy(uint32_t raw_value);

std::optional<uint32_t> ToRawValue(ExceptionStrategy strategy);

// A process+thread koid pair for referring to a thread. While a thread koid is globally unique and
// doesn't technically need a process koid to scope it, most code deals with a process/thread
// hierarchy so maintaining both is more convenient.
struct ProcessThreadId {
  uint64_t process = 0;
  uint64_t thread = 0;

  bool operator==(const ProcessThreadId& other) const {
    return process == other.process && thread == other.thread;
  }
  bool operator!=(const ProcessThreadId& other) const { return !operator==(other); }

  // For ordered containers.
  bool operator<(const ProcessThreadId& other) const {
    return std::tie(process, thread) < std::tie(other.process, other.thread);
  }

  void Serialize(Serializer& ser, uint32_t ver) { ser | process | thread; }
};

struct ExceptionRecord {
  ExceptionRecord() { memset(&arch, 0, sizeof(Arch)); }

  // Race conditions or other errors can conspire to mean the exception records are not valid. In
  // order to differentiate this case from "0" addresses, this flag indicates validity of the "arch"
  // union.
  bool valid = false;

  union Arch {
    // Exception record for x64.
    struct {
      uint64_t vector;
      uint64_t err_code;
      uint64_t cr2;
    } x64;

    // Exception record for ARM64.
    struct {
      uint32_t esr;
      uint64_t far;
    } arm64;

    // Exception record for RISC-V 64.
    struct {
      uint64_t cause;
      uint64_t tval;
    } riscv64;
  } arch;

  ExceptionStrategy strategy = ExceptionStrategy::kNone;

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | valid;
    ser.SerializeBytes(&arch, sizeof(Arch));
    ser | strategy;
  }
};

struct ComponentInfo {
  std::string moniker;
  std::string url;

  void Serialize(Serializer& ser, uint32_t ver) { ser | moniker | url; }
};

enum class TaskType : uint32_t {
  kUnknown = 0,
  kProcess = 1,
  kJob = 2,
};

// Note: see "ps" source:
// https://fuchsia.googlesource.com/fuchsia/+/HEAD/src/sys/bin/psutils/ps.c
struct ProcessTreeRecord {
  TaskType type = TaskType::kJob;
  uint64_t koid = 0;
  std::string name;

  // The following fields are only valid on kJob and will be skipped if type is kProcess.

  // The component information if the process is running in a component. There could be many
  // components for a single process. An empty vector means there was no component associated with
  // the process. Order of components is not guaranteed.
  std::vector<ComponentInfo> components;

  std::vector<ProcessTreeRecord> children;

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | type | koid | name;
    if (type == TaskType::kJob) {
      ser | components | children;
    }
  }
};

struct StackFrame {
  // The different unwinding methods available in the unwinder. See //src/lib/unwinder for details.
  enum class Trust {
    kScan,
    kSCS,
    kSigReturn,
    kFP,
    kPLT,
    kArmEhAbi,
    kCFI,
    kContext,
    kUnknown,
  };

  // Whether or not the PC value for this frame is set from the "return address" register, or if it
  // was set directly by unwinding instructions.
  enum class AddressType {
    // This frame was recovered by the return address register.
    kReturn,
    // This frame was recovered by an explicit instruction to set PC to a specific value, or was
    // recovered from context.
    kExact,
    kUnknown,
  };

  StackFrame() = default;
  StackFrame(uint64_t ip, uint64_t sp, uint64_t cfa = 0, Trust trust = Trust::kContext,
             AddressType pc_is_return_address = AddressType::kExact,
             std::vector<debug::RegisterValue> r = {})
      : ip(ip),
        sp(sp),
        cfa(cfa),
        trust(trust),
        pc_is_return_address(pc_is_return_address),
        regs(std::move(r)) {}

  // Comparisons (primarily for tests).
  bool operator==(const StackFrame& other) const {
    return ip == other.ip && sp == other.sp && cfa == other.cfa && regs == other.regs;
  }
  bool operator!=(const StackFrame& other) const { return !operator==(other); }

  // Instruction pointer.
  uint64_t ip = 0;

  // Stack pointer.
  uint64_t sp = 0;

  // Canonical frame address. This is the stack pointer of the previous
  // frame at the time of the call. 0 if unknown.
  uint64_t cfa = 0;

  // The Trust that restored this stack frame according to the unwinder.
  Trust trust = Trust::kUnknown;

  // Indication from the unwinder tha the PC was recovered from a return address register (i.e. LR
  // on ARM, rax on x64, or ra on riscv). When this is false, it means that the unwinding
  // instructions set PC directly, and it was not recovered from a return address register.
  //
  // This is very important for accurate symbolization of PCs that are on the beginning or ending
  // addresses for a particular function. For the case where PC was restored as a return address
  // (the "normal" case), we want to subtract 1 from the return address, which is normally pointing
  // to the address _after_ the callsite, when we want to symbolize the function where the call came
  // from. On the other hand when the unwinding instructions set PC explicitly, we should not apply
  // this subtraction during symbolization.
  AddressType pc_is_return_address = AddressType::kUnknown;

  // Known general registers for this stack frame. See IsGeneralRegister() for
  // which registers are counted as "general".
  //
  // Every frame should contain the register for the IP and SP for the current
  // architecture (duplicating the above two fields).
  std::vector<debug::RegisterValue> regs;

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | ip | sp | cfa | regs;

    if (ver >= 70) {
      ser | trust;
    }

    if (ver >= 71) {
      ser | pc_is_return_address;
    }
  }

  static const char* TrustToString(Trust trust);
};

struct ThreadRecord {
  enum class State : uint32_t {
    kNew = 0,  // The thread is newly created and running.
    kRunning,
    kSuspended,
    kBlocked,
    kDying,
    kDead,
    kCoreDump,

    kLast  // Not an actual thread state, for range checking.
  };
  static const char* StateToString(State);

  enum class BlockedReason : uint32_t {
    kNotBlocked = 0,  // Used when State isn't kBlocked.

    kException,
    kSleeping,
    kFutex,
    kPort,
    kChannel,
    kWaitOne,
    kWaitMany,
    kInterrupt,
    kPager,

    kLast  // Not an actual blocked reason, for range checking.
  };
  static const char* BlockedReasonToString(BlockedReason);

  // Indicates how much of the stack was attempted to be retrieved in this
  // call. This doesn't indicate how many stack frames were actually retrieved.
  // For example, there could be no stack frames because they weren't
  // requested, or there could be no stack frames due to an error.
  enum class StackAmount : uint32_t {
    // A backtrace was not attempted. This will always be the case if the
    // thread is neither suspended nor blocked in an exception.
    kNone = 0,

    // The frames vector contains a minimal stack only (if available) which
    // is defined as the top two frames. This is used when the stack frames
    // have not been specifically requested since retrieving the full stack
    // can be slow. The frames can still be less than 2 if there was an error
    // or if there is only one stack frame.
    kMinimal,

    // The frames are the full stack trace (up to some maximum).
    kFull,

    kLast  // Not an actual state, for range checking.
  };

  ProcessThreadId id;
  std::string name;
  State state = State::kNew;
  // Only valid when state is kBlocked.
  BlockedReason blocked_reason = BlockedReason::kNotBlocked;
  StackAmount stack_amount = StackAmount::kNone;

  // The frames of the top of the stack when the thread is in suspended or blocked in an exception
  // (if possible). See stack_amnount for how to interpret this.
  //
  // This could still be empty in the "kMinimal" or "kFull" cases if retrieval failed, which can
  // happen in some valid race conditions if the thread was killed out from under the debug agent.
  std::vector<StackFrame> frames;

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | id | name | state | blocked_reason | stack_amount | frames;
  }
};

struct AddressRegion {
  std::string name;
  uint64_t base = 0;
  uint64_t size = 0;
  uint64_t depth = 0;
  uint64_t vmo_koid = 0;  // Fuchsia only.
  uint64_t vmo_offset = 0;
  uint64_t committed_bytes = 0;

  // MMU flags.
  bool read = false;
  bool write = false;
  bool execute = false;
  bool shared = false;  // Linux only.

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | name | base | size | depth | vmo_koid | vmo_offset | committed_bytes | read | write |
        execute | shared;
  }
};

struct ProcessRecord {
  uint64_t process_koid = 0;
  std::string process_name;

  // The component information if the process is running in a component. There could be many
  // components for a single process. An empty vector means there was no component associated with
  // the process. Order of components is not guaranteed.
  std::vector<ComponentInfo> components;

  std::vector<ThreadRecord> threads;

  // The shared address space if this is either a prototype process (it was created with
  // zx_process_create(ZX_PROCESS_SHARED)) or if this is a shared process (it was created with
  // zx_process_create_shared()). Empty if there is no shared address space.
  std::optional<AddressRegion> shared_address_space = std::nullopt;

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | process_koid | process_name | threads | components;
    if (ver > 69) {
      ser | shared_address_space;
    }
  }
};

struct MemoryBlock {
  // Begin address of this memory.
  uint64_t address = 0;

  // When true, indicates this is valid memory, with the data containing the
  // memory. False means that this range is not mapped in the process and the
  // data will be empty.
  bool valid = false;

  // Length of this range. When valid == true, this will be the same as
  // data.size(). When valid == false, this will be whatever the length of
  // the invalid region is, and data will be empty.
  uint32_t size = 0;

  // The actual memory. Filled in only if valid == true.
  std::vector<uint8_t> data;

  void Serialize(Serializer& ser, uint32_t ver) { ser | address | valid | size | data; }
};

struct ProcessBreakpointSettings {
  // The process is required to be nonzero. A zero thread ID indicates this is a process-wide
  // breakpoint. Otherwise, this is the thread to break.
  ProcessThreadId id;

  // Address to break at.
  uint64_t address = 0;

  // Range is used for watchpoints.
  debug::AddressRange address_range;

  void Serialize(Serializer& ser, uint32_t ver) { ser | id | address | address_range; }
};

// What threads to stop when the breakpoint is hit. These are ordered such that the integer values
// increase for larger scopes.
enum class Stop : uint32_t {
  kNone = 0,  // Don't stop anything but accumulate hit counts.
  kThread,    // Stop only the thread that hit the breakpoint.
  kProcess,   // Stop all threads of the process that hit the breakpoint.
  kAll        // Stop all threads of all processes attached to the debugger.
};

// NOTE: read-only could be added in the future as arm64 supports them. They're not added today as
//       x64 does not support them and presenting a common platform is cleaner for now.
enum class BreakpointType : uint32_t {
  kSoftware = 0,  // Software code execution.
  kHardware,      // Hardware code execution.
  kReadWrite,     // Hardware read/write.
  kWrite,         // Hardware write.

  kLast,  // Not a real type, end marker.
};
const char* BreakpointTypeToString(BreakpointType);

// Read, ReadWrite and Write are considered watchpoint types.
bool IsWatchpointType(BreakpointType);

constexpr uint32_t kDebugAgentInternalBreakpointId = static_cast<uint32_t>(-1);

struct BreakpointSettings {
  // The ID if this breakpoint. This is assigned by the client. This is different than the ID in
  // the console frontend which can be across mutliple processes or may match several addresses in
  // a single process.
  //
  // The ID kDebugAgentInternalBreakpointId is reserved for internal use by the backend.
  uint32_t id = 0;

  BreakpointType type = BreakpointType::kSoftware;

  // Name used to recognize a breakpoint. Useful for debugging purposes. Optional.
  std::string name;

  // When set, the breakpoint will automatically be removed as soon as it is
  // hit.
  bool one_shot = false;

  // What should stop when the breakpoint is hit.
  Stop stop = Stop::kAll;

  // Processes to which this breakpoint applies.
  //
  // If any process specifies a nonzero thread_koid, it must be the only process (a breakpoint can
  // apply either to all threads in a set of processes, or exactly one thread globally).
  std::vector<ProcessBreakpointSettings> locations;

  // Handles the automatic collection of memory if it's requested.
  bool has_automation = false;

  std::vector<debug_ipc::AutomationInstruction> instructions;

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | id | type | name | one_shot | stop | locations | has_automation | instructions;
  }
};

struct BreakpointStats {
  uint32_t id = 0;
  uint32_t hit_count = 0;

  // On a "breakpoint hit" message from the debug agent, if this flag is set,
  // the agent has deleted the breakpoint because it was a one-shot breakpoint.
  // Whenever a client gets a breakpoint hit with this flag set, it should
  // clear the local state associated with the breakpoint.
  bool should_delete = false;

  void Serialize(Serializer& ser, uint32_t ver) { ser | id | hit_count | should_delete; }
};

// Information on one loaded module.
struct Module {
  std::string name;            // The main executable binary will normally have an empty name.
  uint64_t base = 0;           // Load address of this file.
  uint64_t debug_address = 0;  // Link map address for this module.
  std::string build_id;

  void Serialize(Serializer& ser, uint32_t ver) { ser | name | base | debug_address | build_id; }
};

struct AttachConfig {
  enum class Priority {
    // Neither claim the DEBUGGER exception channel, nor send modules. This can be used to
    // create objects for new processes in the backend, but not do anything with them
    // until the client requests it later.
    kMinimal = 0,

    // Claim the target's DEBUGGER exception channel, but do not send modules until the
    // client requests them via |ModulesRequest|.
    kWeak = 1,

    // Pause the process and notify modules immediately. Also claim the target's DEBUGGER
    // exception channel.
    kStrong = 2,
  } priority = Priority::kStrong;

  // The target of the attach. On Fuchsia, we can attach directly to both jobs and processes. On
  // linux, we can only attach to processes. This doesn't necessarily define specifically what
  // we're attaching to, but is more of a "what was the intent of the filter that created this
  // attach configuration". That is to say, a config with a |kJob| target may be part of an
  // AttachRequest with a process koid.
  TaskType target = TaskType::kProcess;

  void Serialize(Serializer& ser, uint32_t ver) {
    if (ver < 75) {
      // The |weak| boolean is deprecated in v75 in favor of |priority|.

      // Set up the old weak boolean for serialization. Note that |kMinimal| is _not_ considered
      // equivalent to |kWeak| for old backends, because |kMinimal| expects that no exception
      // channels will be used, but |kWeak| does.
      bool weak = priority == Priority::kWeak;

      ser | weak;

      // And read it back for deserialization.
      priority = weak ? Priority::kWeak : Priority::kStrong;
    }

    if (ver >= 66) {
      ser | target;
    }

    if (ver >= 75) {
      ser | priority;
    }
  }

  static bool ShouldDeferModules(const AttachConfig& config);
};

const char* AttachPriorityToString(AttachConfig::Priority priority);

// LoadInfoHandleTable -----------------------------------------------------------------------------

// VMO-specific handle information from zx_info_vmo that's not in the InfoHandle structure.
struct InfoHandleVmo {
  char name[32];  // Needs to be POD for use in union below, and 32 is the max from the kernel.
  uint64_t size_bytes;
  uint64_t parent_koid;
  uint64_t num_children;
  uint64_t num_mappings;
  uint64_t share_count;
  uint32_t flags;
  uint64_t committed_bytes;
  uint32_t cache_policy;
  uint64_t metadata_bytes;
  uint64_t committed_change_events;
};

// This structure is assumed to be entirely POD.
struct InfoHandle {
  // Provide 0-init that covers the union.
  InfoHandle() { memset(this, 0, sizeof(InfoHandle)); }

  // Standard information from zx_info_handle_extended.
  //
  // There is a special case for a VMO. It is possible to have a VMO mapped without a handle to it.
  // These will appear here but the handle_value will be 0.
  uint32_t type;
  uint32_t handle_value;
  uint32_t rights;
  uint32_t reserved;
  uint64_t koid;
  uint64_t related_koid;
  uint64_t peer_owner_koid;

  // Type-specific handle information.
  union {
    InfoHandleVmo vmo;  // Valid when type == ZX_OBJ_TYPE_VMO.
    // Other types go here.
  } ext;

  void Serialize(Serializer& ser, uint32_t ver) { ser.SerializeBytes(this, sizeof(*this)); }
};

// Filters -----------------------------------------------------------------------------------------
constexpr uint32_t kInvalidFilterId = static_cast<uint32_t>(-1);

struct FilterConfig {
  // Whether or not this is a weak filter. The backend needs to know this so when newly spawned
  // processes that match a filter hit the loader breakpoint can know whether or not to eagerly send
  // modules to the front end.
  bool weak = false;

  // Indicates a recursive filter, which should match all child components spawned in the realm of
  // this filter's matching component.
  bool recursive = false;

  // Fuchsia only. Indicate that the client is only interested in attaching to the closest job
  // matching the filter. This prevents attaching to processes directly.
  bool job_only = false;

  // Indicates that we should _never_ claim an exception channel for matching processes. This is
  // useful for cases that want to have access to things like processes and threads which can be
  // suspended and inspected without requiring available exception channels and possibly receiving
  // spurious exceptions that they are not interested in. This is typically only used internally and
  // is not exposed via the command line or FIDL interfaces.
  bool never_attach = false;

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | weak | recursive | job_only;

    if (ver >= 74) {
      ser | never_attach;
    }
  }

  static AttachConfig ToAttachConfig(const FilterConfig& filter_config);
};

struct Filter {
  enum class Type : uint32_t {
    kUnset,
    kProcessNameSubstr,
    kProcessName,
    kComponentName,
    kComponentUrl,
    kComponentMoniker,
    kComponentMonikerSuffix,
    kComponentMonikerPrefix,

    kLast,
  } type = Type::kUnset;
  static const char* TypeToString(Type);

  std::string pattern;
  uint64_t job_koid = 0;  // must be 0 when type is kComponent*.

  // The originator of this filter. This is encoded in the high byte of a 32 bit integer on the
  // wire, but logically contained separately in the |Identifier| class below.
  enum class Originator : uint8_t {
    kUnknown = 0,
    // The DebugAgent originated this filter. This will only be the case for a temporary filter sent
    // via the NotifyFilterCreated. Clients will persist the filter if they choose with their own
    // identifier and originator value.
    kAgent = 1 << 5,
    // Originated by zxdb.
    kZxdb = 1 << 6,
    // Originated by DebugAgentServer, the provider of the "DebugAgent" FIDL API. This must be the
    // highest bit to stay backwards compatible with the previous |kFilterIdBase| used in the server
    // implementation.
    kFidlServer = 1 << 7,
    // Newly allocated filter originators can go here. Any unallocated values are reserved.
    kInvalid = 0xff,
  };

  struct IdentifierComponents {
    Originator originator;
    uint32_t value;
  };

  // This class is introduced in version 73 to clarify and simplify Filter creation and
  // identification between all entities in the system. Filters can be created by any of the
  // entities specified in |Originator|. This is ABI compatible with the previous uint32_t id value.
  class Identifier {
   public:
    Identifier() = default;

    // An Identifier is created with an opaque value assigned by the originator, and the
    // originator's ordinal from the above list.
    explicit Identifier(uint32_t client_value, Originator originator)
        : value_(Encode(client_value, originator)) {}

    bool operator==(const Identifier& other) const { return other.value_ == value_; }
    std::strong_ordering operator<=>(const Identifier& other) const {
      return this->value_ <=> other.value_;
    }

    IdentifierComponents Decode() const {
      IdentifierComponents components;
      components.originator = static_cast<Originator>(value_ >> 24);
      components.value = value_ & kValueMask;
      return components;
    }

    uint32_t value() const { return value_; }

    // Note: This is ABI compatible with the previous implementation, so we don't need to check the
    // versions.
    void Serialize(Serializer& ser, uint32_t ver) { ser | value_; }

   private:
    static constexpr uint32_t kValueMask = 0x00ffffff;

    static uint32_t Encode(uint32_t client_value, Originator originator) {
      return ((static_cast<uint32_t>(originator) & 0xff) << 24) | client_value;
    }

    uint32_t value_ = kInvalidFilterId;
  };

  // The cross-ipc-boundary identifier for this filter object. Logically, the identifier has two
  // components, an "Originator", from a pre-allocated list of valid entities in the system which
  // created the original filter, and an opaque 32 bit integer, assigned by the client associated
  // with this filter.
  //
  // In practice, this is a 32 bit integer with the most significant byte identifying the originator
  // of this filter. The remaining 24 bits are reserved for the clients. Clients typically never
  // install more than a few (< 10) filters, even in extended sessions. The filter will be rejected
  // with an error if a client attempts to use a value that would encroach on the most significant
  // byte. The client value is never modified.
  //
  // This has two primary purposes:
  //
  //   1. Allow feed-forward flows from the clients. IDs are assigned by clients at filter creation
  //      time.
  //     1a. The lone exception to this is the NotifyFilterCreated event from the backend, which
  //         will construct a filter with an originator value of kAgent. See the notification for
  //         more details.
  //   2. Provide uniqueness across all clients and the backend.
  //
  // In general, this is always needed to disambiguate which client(s) should take action based on a
  // filter matching resulting in either a {Component,Process}Starting notification or as the result
  // of an UpdateFilter request matching some non-zero number of processes.
  //
  // The NotifyFilterCreated event is typically sent as a result of a client installing a filter
  // with the |recursive| option. When a match for this filter is found, the backend will construct
  // a new filter with the (typically component moniker) information and send the event to all
  // clients along with the identifier of the original filter. Clients can use this, along with
  // their respectively allocated mask, to determine when they are responsible for keeping track of
  // this new filter.
  Identifier id;

  FilterConfig config;

  void Serialize(Serializer& ser, uint32_t ver) { ser | type | pattern | job_koid | id | config; }
};

struct MatchedTask {
  uint64_t koid;

  TaskType type = TaskType::kProcess;

  void Serialize(Serializer& ser, uint32_t ver) { ser | koid | type; }
};

// Reply indicating that a filter matched one or more tasks.
struct FilterMatch {
  FilterMatch() = default;
  FilterMatch(const Filter::Identifier& id, std::vector<MatchedTask> matches)
      : id(id), matches(std::move(matches)) {}

  // See Filter::Identifier.
  Filter::Identifier id;

  // All of the pids that matched this filter.
  std::vector<MatchedTask> matches;

  void Serialize(Serializer& ser, uint32_t ver) {
    ser | id;

    if (ver < 77) {
      // Old IPC users only know about the pids.
      std::vector<uint64_t> pids(matches.size());
      std::ranges::transform(matches, std::back_inserter(pids),
                             [&](const auto& task) -> uint64_t { return task.koid; });

      ser | pids;

      matches.clear();
      matches.reserve(pids.size());

      // Deserializing means putting the pids back into our matched tasks. We lose the task type
      // information.
      std::ranges::transform(pids, std::back_inserter(matches), [&](uint64_t pid) -> MatchedTask {
        return {.koid = pid, .type = TaskType::kUnknown};
      });
    } else {
      // ver >= 77.
      ser | matches;
    }
  }
};

#pragma pack(pop)

}  // namespace debug_ipc

#endif  // SRC_DEVELOPER_DEBUG_IPC_RECORDS_H_
