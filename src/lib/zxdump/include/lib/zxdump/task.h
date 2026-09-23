// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_TASK_H_
#define SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_TASK_H_

#include <lib/elfldltl/constants.h>
#include <lib/fit/result.h>
#include <zircon/errors.h>
#include <zircon/syscalls/debug.h>

#include <cstddef>
#include <cstring>
#include <functional>
#include <map>
#include <memory>
#include <utility>
#include <vector>

#include <fbl/unique_fd.h>

#ifdef __Fuchsia__
#include <lib/zx/handle.h>
#endif

#include "buffer.h"
#include "types.h"

namespace zxdump {

constexpr size_t kReadMemoryStringLimit = 1024;

// This is the type of the optional argument to Process::read_memory, below.
enum class ReadMemorySize {
  // Return exactly the amount requested, never more or less--except that an
  // emtpy buffer may be returned for memory elided from the dump.
  kExact,

  // Return at least the amount requested, and possibly more if convenient.
  // (Still return an empty buffer for elided memory.)
  kMore,

  // Return any amount conveniently available.  An empty buffer still means the
  // requested memory was elided entirely.  But for any nonempty buffer less
  // than what was requested, the caller should make an additional read_memory
  // call at a higher address to get the remainder piecemeal; a later call will
  // fail outright if that higher address is not valid in the process.
  //
  // This mode is recommended for large reads that don't need to be contiguous
  // in a single buffer, or that will be copied elsewhere anyway.  It allows
  // Process::read_memory to minimize data copying.
  kLess,
};

// On Fuchsia, live task handles can be used via the lib/zx API.  On other
// systems, the API parts for live tasks are still available but they use a
// stub handle type that is always invalid.
#ifdef __Fuchsia__
using LiveHandle = zx::handle;
#else
// The stub type is move-only and contextually convertible to bool just like
// the real one.  It supports only a few basic methods, which do nothing and
// always report an invalid handle.
struct LiveHandle {
  LiveHandle() = default;
  LiveHandle(const LiveHandle&) = delete;
  LiveHandle(LiveHandle&&) = default;
  LiveHandle& operator=(const LiveHandle&) = delete;
  LiveHandle& operator=(LiveHandle&&) = default;

  void reset() {}

  bool is_valid() const { return false; }

  explicit operator bool() const { return is_valid(); }

  zx_status_t get_info(uint32_t topic, void* buffer, size_t buffer_size, size_t* actual_count,
                       size_t* avail_count) const {
    return ZX_ERR_BAD_HANDLE;
  }

  zx_status_t get_property(uint32_t property, void* value, size_t size) const {
    return ZX_ERR_BAD_HANDLE;
  }

  zx_status_t get_child(uint64_t koid, zx_rights_t rights, LiveHandle* result) const {
    return ZX_ERR_BAD_HANDLE;
  }
};
#endif

// This is an opaque type used internally.
namespace internal {
class DumpFile;
}  // namespace internal

// Forward declarations for below.
class Object;
class Resource;
class Task;
class Job;
class Process;
class Thread;

// This is an opaque type used internally.
namespace internal {
class DumpFile;
}  // namespace internal

// This is the API for reading in dumps, both `ET_CORE` files and job archives.
//
// The zxdump::TaskHolder object is a container that holds the data from any
// number of dump files.  It provides access to the data as a Zircon job tree.
// The Job, Process, and Thread objects represent the jobs and processes found
// in dump files.  Each object provides calls analogous to the Zircon get_info,
// get_property, read_memory, and read_state calls, as well as get_child for
// nagivating a task tree.
//
// Dumps are inserted into the container by providing the file descriptor.  The
// type of file will be determined automatically from its contents.  Dump files
// can be ELF core dump (`ET_CORE`) files, or `ar` archive files.  An archive
// file can be a job archive or just a plain archive of other dump files.  Job
// archives can be mere "stub archives", or full hierarchical job archives, or
// flattened job archives.
//
// All the jobs and processes found in the dumps inserted then self-assemble
// into a job tree.  If the same task (same KOID) appears a second time either
// in two dump files or in two members of a job archive, insertion fails but
// may have added some of the tasks from the dump anyway.
//
// If every process and every job but one is a child of another job found in
// the dump so they all form a single job tree, then `root_job` returns the
// root of that tree.  If not, then `root_job` returns a fake "root job" with a
// KOID of 0 and no information or properties available except for the children
// and process lists.  These show all the jobs that don't have parent jobs that
// were dumped, i.e. the roots of job trees; and all the processes that aren't
// part of any dumped job at all.  Hence populating the container with a single
// ELF core dump will yield a fake root job whose sole child is that process.
//
// Methods that can fail use a result type with zxdump::Error.  When the
// `status_` field is ZX_ERR_IO, that means the failure was in a POSIXish
// filesystem access function and `errno` is set to indicate the exact error.
// Otherwise the error codes have mostly the same meaning they would have for
// the real Zircon calls, with some amendments:
//
//  * ZX_ERR_NOT_SUPPORTED just means the dump didn't include the requested
//    type of data.  It doesn't indicate whether the kernel didn't support it,
//    or the dump-writer intentionally chose not to dump it, or the dump was
//    just truncated, etc.
//
//  * Process::read_memory fails with ZX_ERR_NOT_FOUND if the dump indicated
//    the memory mapping existed but the dump did not include that memory.
//    ZX_ERR_OUT_OF_RANGE means the memory is absent because the dump was
//    truncated though this memory was intended to be included in the dump.
//    ZX_ERR_NO_MEMORY has the kernel's meaning that there was no memory
//    mapped at that address in the process.  ZX_ERR_NOT_SUPPORTED means that
//    the dump was inserted with the `read_memory=false` flag.
//
class TaskHolder {
 public:
  // Default-constructible, move-only.
  TaskHolder();
  TaskHolder(TaskHolder&&) = default;
  TaskHolder& operator=(TaskHolder&&) = default;
  ~TaskHolder();

  // Read the dump file from the file descriptor and insert its tasks.  If
  // `read_memory` is false, state will be trimmed after reading in all the
  // notes so less memory is used and the file descriptor is never kept open;
  // but read_memory calls will always fail with ZX_ERR_NOT_SUPPORTED.
  fit::result<Error> Insert(fbl::unique_fd fd, bool read_memory = true);

  // Insert a live task (job or process) or resource.  Live threads cannot be
  // inserted alone, only their containing process.
  fit::result<Error, std::reference_wrapper<Object>> Insert(LiveHandle obj);

  // Insert system data (returned by system_get_*, below) taken from the
  // currently running system.
  fit::result<Error> InsertSystem();

  // Yields the current root job.  If all tasks in the eye of the TaskHolder
  // form a unified tree, this returns the actual root job in that tree.
  // Otherwise, this is the fake "root job" that reads as KOID 0 with no data
  // available except the Job::children and Job::processes lists holding each
  // orphaned task not claimed by any parent job.  It's always safe to hold
  // onto this reference for the life of the TaskHolder.  If more tasks are
  // added, this will start returning a different reference.  An old reference
  // to the fake root job will read as having no children and no processes if
  // all the tasks self-assembled into a single tree after more dumps were
  // inserted, and later start reporting new orphan tasks inserted after that.
  Job& root_job() const;

  // Yields the info resource.  If a live handle to the info resource was
  // passed to Insert, this can access its data.  If dumps inserted include the
  // "privileged kernel" information, that is attached to this fake info
  // resource though it doesn't have information like a KOID itself.  If
  // multiple dumps supply kernel information, conflicting data inserted later
  // will be ignored.  If both live and dump data are available, the dump data
  // will obscure the live data.
  Resource& info_resource() const;

  // These can't fail, but return empty/zero if no corresponding data is in the
  // dump.  If multiple dumps supply system-wide information, only the first
  // dump's data will be used.  There is no checking that the system-wide data
  // in dumps is valid; malformed data may be treated like no data at all but
  // still may prevent well-formed data in other dumps from being used.
  uint32_t system_get_dcache_line_size() const;
  uint32_t system_get_num_cpus() const;
  uint64_t system_get_page_size() const;
  uint64_t system_get_physmem() const;
  std::string_view system_get_version_string() const;

  // Get and set the limit (in bytes) of unreferenced Process::read_memory
  // pages from live processes that may be cached for reuse.
  size_t memory_cache_limit() const;
  void set_memory_cache_limit(size_t limit);

 private:
  friend Object;
  friend Task;
  friend Job;
  friend Process;
  friend Thread;
  friend Resource;
  class JobTree;
  class LiveMemoryCache;

  std::unique_ptr<JobTree> tree_;
};

// This is the superclass of all (virtual) kernel object types.
// All the methods here correspond to the generic zx::object methods.
class Object {
 public:
  // None of the Object types can be constructed in any way.
  // They are only used via references returned by the owning TaskHolder.
  Object(const Object&) = delete;

  // The subclass constructors need to be public so that they can be called by
  // the std::optional and std::map emplace methods, but they are intended as
  // private so none of these objects can exist outside the TaskHolder.  Since
  // the JobTree& is a required argument in the subclass constructors and the
  // class is private, outside code cannot construct new objects.
  explicit Object(TaskHolder::JobTree& tree, LiveHandle live = {})
      : tree_{tree}, live_(std::move(live)) {}

  struct WaitItem {
    std::reference_wrapper<Object> handle;
    zx_signals_t waitfor = 0;
    zx_signals_t pending = 0;
  };

  using WaitItemVector = std::vector<WaitItem>;

  // Every task has a KOID.  This is just shorthand for extracting it from
  // ZX_INFO_HANDLE_BASIC.  The fake root job returns zero (ZX_KOID_INVALID).
  zx_koid_t koid() const;

  // This is a shorthand for extracting the type from ZX_INFO_HANDLE_BASIC.
  //  * If it returns ZX_OBJ_TYPE_JOB, `static_cast<Job&>(*this)` is safe.
  //  * If it returns ZX_OBJ_TYPE_PROCESS, `static_cast<Process&>(*this)` is.
  //  * If it returns ZX_OBJ_TYPE_THREAD, `static_cast<Thread&>(*this)` is.
  //  * If it returns ZX_OBJ_TYPE_RESOURCE, `static_cast<Resource&>(*this)` is.

  // The only objects on which get_info<ZX_INFO_HANDLE_BASIC> can fail are the
  // fake root job and the fake root resource; type() on those returns zero
  // (ZX_OBJ_TYPE_NONE).
  zx_obj_type_t type() const;

  // This returns the timestamp of the dump, which may be zero.
  time_t date() const { return date_; }

  // This is provided for parity with zx::object::get_child, but just using
  // Process::threads, Job::children, or Job::processes is much more convenient
  // for iterating through the lists reported by get_info.  However, on live
  // tasks either get_child or find to reach specific known KOIDs can be much
  // more efficient than accessing the one of the child-list methods, which
  // will acquire all the child handles implicitly, not just the matching one.
  //
  // **Notes for live objects:** Unlike zx::object::get_child, this does not
  // take a zx_rights_t parameter even when referring to a live object.
  // Instead, it always requests the full suite of rights that are necessary to
  // do get_info, get_property, read_memory, read_state, etc.--everything the
  // dumper usually needs.  There is only ever a single Object for each KOID,
  // so returning live object handle with lesser rights once would mean later
  // calls couldn't use greater rights on the same KOID later.
  fit::result<Error, std::reference_wrapper<Object>> get_child(zx_koid_t koid);

  // Find a task by KOID: this task or a descendent task.
  fit::result<Error, std::reference_wrapper<Object>> find(zx_koid_t koid);

  // This gets the full info block for this topic, whatever its size.  Note the
  // data is not necessarily aligned in memory, so it can't be safely accessed
  // with reinterpret_cast.
  //
  // **Notes for live objects:** When called on a live object, this ordinarily
  // gets each topic just once and then returns the cached value; the optional
  // refresh_live flag says to ignore any cached data and always get fresh data
  // now (which will be cached for later calls without the flag).  Note that a
  // failed call with refresh_live will *not* remove old cached data, so it
  // might be retrieved by calling again without the flag even if something
  // prevents getting new info.
  fit::result<Error, ByteView> get_info(zx_object_info_topic_t topic, bool refresh_live = false,
                                        size_t record_size = 0);

  // Get statically-typed info for a topic chosen at a compile time.  Some
  // types return a single `zx_info_*_t` object.  Others return a span of const
  // type that points into storage permanently cached for the lifetime of the
  // containing TaskHolder.  See <lib/zxdump/types.h> for topic->type mappings.
  template <zx_object_info_topic_t Topic>
  fit::result<Error, InfoTraitsType<Topic>> get_info(bool refresh_live = false) {
    using Info = typename InfoTraits<Topic>::type;
    if constexpr (kIsSpan<Info>) {
      using Element = typename RemoveSpan<Info>::type;
      auto result = get_info_aligned(Topic, sizeof(Element), alignof(Element), refresh_live);
      if (result.is_error()) {
        return result.take_error();
      }
      return fit::ok(Info{
          reinterpret_cast<Element*>(result.value().data()),
          result.value().size() / sizeof(Element),
      });
    } else {
      auto result = get_info(Topic, refresh_live, sizeof(Info));
      if (result.is_error()) {
        return result.take_error();
      }
      ByteView bytes = result.value();
      Info data;
      if (bytes.size() < sizeof(data)) {
        return fit::error(Error{"truncated info note", ZX_ERR_NOT_SUPPORTED});
      }
      memcpy(&data, bytes.data(), sizeof(data));
      return fit::ok(data);
    }
  }

  // This gets the property, whatever its size.  Note the data is not
  // necessarily aligned in memory, so it can't be safely accessed with
  // reinterpret_cast.
  fit::result<Error, ByteView> get_property(uint32_t property);

  // Get a statically-typed property chosen at compile time.
  // See <lib/zxdump/types.h> for property->type mappings.
  template <uint32_t Property>
  fit::result<Error, PropertyTraitsType<Property>> get_property() {
    auto result = get_property(Property);
    if (result.is_error()) {
      return result.take_error();
    }
    ByteView bytes = result.value();
    PropertyTraitsType<Property> data;
    if (bytes.size() < sizeof(data)) {
      return fit::error(Error{"truncated property note", ZX_ERR_NOT_SUPPORTED});
    }
    memcpy(&data, bytes.data(), sizeof(data));
    return fit::ok(data);
  }

  static fit::result<Error> wait_many(WaitItemVector& wait_items);

  bool is_live() const { return live_.is_valid(); }

  // As a convenience, TaskHolder::info_resource() is proxied by every Object.
  Resource& info_resource();

  // A job or process can have dump remarks, stored as a vector of {name, data}
  // pairs.
  const auto& remarks() const { return remarks_; }

 protected:
  // The class is abstract.  Only the subclasses can be created and destroyed.
  Object() = delete;

  // Move construction and assignment are only used during initial creation.
  Object(Object&&) noexcept = default;
  Object& operator=(Object&&) noexcept = default;

  ~Object();

  LiveHandle& live() { return live_; }

  TaskHolder::JobTree& tree() { return tree_; }

  bool empty() const { return info_.empty(); }

  std::byte* GetBuffer(size_t size);
  void TakeBuffer(std::unique_ptr<std::byte[]> buffer);

 private:
  friend TaskHolder::JobTree;

  fit::result<Error, ByteView> get_info_aligned(zx_object_info_topic_t topic, size_t record_size,
                                                size_t align, bool refresh_live);
  fit::result<Error, ByteView> GetSuperrootInfo(zx_object_info_topic_t topic);

  // A plain reference would make the type not movable.
  std::reference_wrapper<TaskHolder::JobTree> tree_;
  std::map<zx_object_info_topic_t, ByteView> info_;
  std::map<uint32_t, ByteView> properties_;
  std::vector<std::pair<std::string_view, ByteView>> remarks_;
  time_t date_ = 0;
  LiveHandle live_;
};

// As with zx::task, this is the superclass of Job, Process, and Thread.
class Task : public Object {
 public:
  using Object::Object;

  // This returns a suspend token.
  fit::result<Error, LiveHandle> suspend();

 protected:
  Task(Task&&) noexcept = default;
  Task& operator=(Task&&) noexcept = default;

  ~Task();
};

// A Thread is a Task and also has register state.
class Thread : public Task {
 public:
  using Task::Task;

  ~Thread();

  Process& process() const { return *process_; }

  fit::result<Error, ByteView> read_state(zx_thread_state_topic_t topic);

  // Use read_state<zx_thread_state_*_t>() with the machine-specific type for a
  // particular kind of state, which implies the topic.
  template <class StateType>
  fit::result<Error, StateType> read_state() {
    StateType state;
    using Traits = ThreadStateTraits<StateType>;
    auto result = read_state(Traits::kTopic, Traits::kMachine, sizeof(state));
    if (result.is_error()) {
      return result.take_error();
    }
    memcpy(&state, result->data(), sizeof(state));
    return fit::ok(state);
  }

 private:
  friend Process;
  friend TaskHolder::JobTree;

  Thread(Thread&&) noexcept = default;
  Thread& operator=(Thread&&) noexcept = default;

  fit::result<Error, ByteView> read_state(zx_thread_state_topic_t topic,
                                          elfldltl::ElfMachine machine, size_t size);

  std::map<zx_thread_state_topic_t, ByteView> state_;
  Process* process_ = nullptr;
};

// A Process is a Task and also has threads and memory.
class Process : public Task {
 public:
  using ThreadMap = std::map<zx_koid_t, Thread>;

  using Task::Task;

  ~Process();

  // Return the size of memory pages in this process, always a power of two.
  // For a live process, this returns zx_system_get_page_size().  In a dump,
  // it's determined from the PT_LOAD segments found (even if the dump elides
  // the actual memory).  If the dump had no memory segments and no system
  // information to indicate the page size post mortem, this is 1 to maintain
  // the power-of-two invariant.
  uint64_t dump_page_size() const { return dump_page_size_; }

  // Return the machine architecture of this process.
  // In a dump, it's determined by the ELF file header.
  elfldltl::ElfMachine dump_machine() const { return dump_machine_; }

  // This is the same as what you'd get from get_info<ZX_INFO_PROCESS_THREADS>
  // and then get_child on each KOID, but pre-cached.  Note the returned map is
  // not const so the Thread references can be non-const, but the caller must
  // not modify the map itself.
  fit::result<Error, std::reference_wrapper<ThreadMap>> threads();

  // Find a task by KOID: this process or one of its threads.
  fit::result<Error, std::reference_wrapper<Task>> find(zx_koid_t koid);

  // zxdump::Object::get_child actually just dispatches to this method.
  fit::result<Error, std::reference_wrapper<Thread>> get_child(zx_koid_t koid);

  // Read process memory at the given vaddr.  This tries to read at least count
  // contiguous elements of type T, and succeeds if that was valid memory of
  // type T in the process.  The optional size_mode argument can permit it to
  // return a buffer with more or less data than the exact size requested; see
  // the enum definition above for details.  In all modes it can return an
  // empty buffer if the memory was elided from the dump.  The success return
  // value is a move-only type; see <zxdump/buffer.h> for full details.
  template <typename T = std::byte, class View = std::span<const T>>
  fit::result<Error, Buffer<T, View>> read_memory(
      uint64_t vaddr, size_t count, ReadMemorySize size_mode = ReadMemorySize::kExact) {
    static_assert(!std::is_reference_v<T>,
                  "cannot instantiate zxdump::Process::read_memory with a reference type");
    static_assert(
        std::is_same_v<const T*, decltype(std::data(View{}))>,
        "must instantiate zxdump::Process::read_memory with corresponding T and View types");
    static_assert(alignof(T) <= __STDCPP_DEFAULT_NEW_ALIGNMENT__,
                  "alignment too large; must read as bytes and copy data");
    if (vaddr % alignof(T) != 0) {
      return fit::error{Error{
          .op_ = "vaddr misaligned for type",
          .status_ = ZX_ERR_INVALID_ARGS,
      }};
    }

    Buffer<> buffer;
    if (auto result = ReadMemoryImpl(vaddr, count * sizeof(T), size_mode, alignof(T));
        result.is_ok()) {
      buffer = *std::move(result);
    } else {
      return result.take_error();
    }

    return fit::ok(std::move(buffer).Reinterpret<T, View>());
  }

  // This just reads a single datum of type T from dumped process memory at
  // vaddr, and simply returns it by value.
  template <typename T>
  fit::result<Error, T> read_memory(uint64_t vaddr) {
    using namespace std::literals;
    auto result = read_memory<T>(vaddr, 1);
    if (result.is_error()) {
      return result.take_error();
    }
    if (result->empty()) {
      return fit::error{Error{
          .op_ = "memory elided from dump"sv,
          .status_ = ZX_ERR_NO_MEMORY,
      }};
    }
    return fit::ok(result->front());
  }

  // Convenience function to read a C-style NUL-terminated string, while
  // reading no more than limit chars total.  No string is returned if no NUL
  // terminator is found, but the std::string returned does not include the NUL
  // terminator in its size() value (though of course its c_str() value is
  // NUL-terminated).  If the memory is accessible but no NUL terminator is
  // found before the size limit is reached, the error result will use
  // ZX_ERR_OUT_OF_RANGE.  If the necessary memory was simply elided from the
  // dump, the error result will use ZX_ERR_NOT_SUPPORTED.
  fit::result<Error, std::string> read_memory_string(uint64_t vaddr,
                                                     size_t limit = kReadMemoryStringLimit) {
    return read_memory_basic_string<char>(vaddr, limit);
  }

  // The same thing is provided in u8string, u16string, u32string, and wstring
  // variants.

  fit::result<Error, std::u8string> read_memory_u8string(uint64_t vaddr,
                                                         size_t limit = kReadMemoryStringLimit /
                                                                        sizeof(char8_t)) {
    return read_memory_basic_string<char8_t>(vaddr, limit);
  }

  fit::result<Error, std::u16string> read_memory_u16string(uint64_t vaddr,
                                                           size_t limit = kReadMemoryStringLimit /
                                                                          sizeof(char16_t)) {
    return read_memory_basic_string<char16_t>(vaddr, limit);
  }

  fit::result<Error, std::u32string> read_memory_u32string(uint64_t vaddr,
                                                           size_t limit = kReadMemoryStringLimit /
                                                                          sizeof(char32_t)) {
    return read_memory_basic_string<char32_t>(vaddr, limit);
  }

  fit::result<Error, std::wstring> read_memory_wstring(uint64_t vaddr,
                                                       size_t limit = kReadMemoryStringLimit /
                                                                      sizeof(wchar_t)) {
    return read_memory_basic_string<wchar_t>(vaddr, limit);
  }

 private:
  friend TaskHolder;
  class LiveMemory;

  Process(Process&&) noexcept = default;
  Process& operator=(Process&&) noexcept = default;

  struct Segment {
    uint64_t offset, filesz, memsz;
  };

  fit::result<Error, Buffer<>> ReadMemoryImpl(uint64_t vaddr, size_t size, ReadMemorySize size_mode,
                                              size_t alignment);
  fit::result<Error, Buffer<>> ReadLiveMemory(uint64_t vaddr, size_t size,
                                              ReadMemorySize size_mode);

  template <typename CharT = char>
  fit::result<Error, std::basic_string<CharT>> read_memory_basic_string(
      uint64_t vaddr, size_t limit = kReadMemoryStringLimit / sizeof(CharT));

  std::map<zx_koid_t, Thread> threads_;
  std::map<uint64_t, Segment> memory_;
  uint64_t dump_page_size_ = 1;
  elfldltl::ElfMachine dump_machine_ = elfldltl::ElfMachine::kNone;
  internal::DumpFile* dump_ = nullptr;
  std::unique_ptr<LiveMemory> live_memory_;
};

// Only these instantiations are actually defined in the library.
extern template fit::result<Error, std::basic_string<char>> Process::read_memory_basic_string<char>(
    uint64_t vaddr, size_t limit);
#if __cpp_lib_char8_t
extern template fit::result<Error, std::basic_string<char8_t>>
Process::read_memory_basic_string<char8_t>(uint64_t vaddr, size_t limit);
#endif
extern template fit::result<Error, std::basic_string<char16_t>>
Process::read_memory_basic_string<char16_t>(uint64_t vaddr, size_t limit);
extern template fit::result<Error, std::basic_string<char32_t>>
Process::read_memory_basic_string<char32_t>(uint64_t vaddr, size_t limit);
extern template fit::result<Error, std::basic_string<wchar_t>>
Process::read_memory_basic_string<wchar_t>(uint64_t vaddr, size_t limit);

// A Job is a Task and also has child jobs and processes.
class Job : public Task {
 public:
  using JobMap = std::map<zx_koid_t, Job>;
  using ProcessMap = std::map<zx_koid_t, Process>;

  using Task::Task;

  ~Job();

  // This is the same as what you'd get from get_info<ZX_INFO_JOB_CHILDREN> and
  // then get_child on each KOID, but pre-cached.  Note the returned map is not
  // const so the Job references can be non-const, but the caller must not
  // modify the map itself.
  fit::result<Error, std::reference_wrapper<JobMap>> children();

  // This is the same as what you'd get from get_info<ZX_INFO_JOB_PROCESSES>
  // and then get_child on each KOID, but pre-cached.  Note the returned map is
  // not const so the Process references can be non-const, but the caller must
  // not modify the map itself.
  fit::result<Error, std::reference_wrapper<ProcessMap>> processes();

  // Find a task by KOID: this task or a descendent task.
  fit::result<Error, std::reference_wrapper<Task>> find(zx_koid_t koid);

  // zxdump::Object::get_child actually just dispatches to this method.
  fit::result<Error, std::reference_wrapper<Task>> get_child(zx_koid_t koid);

 private:
  friend TaskHolder::JobTree;

  Job(Job&&) noexcept = default;
  Job& operator=(Job&&) noexcept = default;

  fit::result<Error, ByteView> GetSuperrootInfo(zx_object_info_topic_t topic);

  JobMap children_;
  ProcessMap processes_;
};

// Resource objects just hold access to information, but are a distinct type.
class Resource : public Object {
 public:
  using Object::Object;

  ~Resource();

 private:
  friend TaskHolder::JobTree;

  Resource& operator=(Resource&&) noexcept = default;
};

// Get the live root job of the running system, e.g. for TaskHolder::Insert.
fit::result<Error, LiveHandle> GetRootJob();

// Get the live info resource handle of the running system.
fit::result<Error, LiveHandle> GetInfoResource();

}  // namespace zxdump

#endif  // SRC_LIB_ZXDUMP_INCLUDE_LIB_ZXDUMP_TASK_H_
