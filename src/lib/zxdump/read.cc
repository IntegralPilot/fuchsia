// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/note.h>
#include <lib/fit/defer.h>
#include <lib/zxdump/task.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <zircon/assert.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/resource.h>

#include <charconv>
#include <forward_list>
#include <string_view>
#include <variant>

#include <rapidjson/document.h>

#ifdef __Fuchsia__
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <zircon/status.h>
#endif

#include "core.h"
#include "dump-file.h"
#include "job-archive.h"
#include "live-memory-cache.h"
#include "rights.h"

namespace zxdump {

using namespace internal;

namespace {

// The result of parsing an archive member header.  The name view may point
// into the original header buffer, so this must live no longer than that.
struct MemberHeader {
  std::string_view name;
  time_t date;
  size_t size;
};

std::string_view TrimSpaces(std::string_view string) {
  auto pos = string.find_last_not_of(' ');
  if (pos == std::string_view::npos) {
    return {};
  }
  return string.substr(0, pos + 1);
}

template <typename T>
bool ParseHeaderInteger(std::string_view field, T& value) {
  field = TrimSpaces(field);
  if (field.empty()) {
    // Some special members can have wholly blank integer fields and that's OK.
    value = 0;
    return true;
  }
  const char* first = field.data();
  const char* last = first + field.size();
  auto result = std::from_chars(first, last, value);
  return result.ptr == last && result.ec != std::errc::result_out_of_range;
}

// Parse the basic archive header.  The name may need additional decoding.
fit::result<Error, MemberHeader> ParseArchiveHeader(ByteView header) {
  if (header.size() < sizeof(ar_hdr)) {
    return fit::error(Error{"truncated archive", ZX_ERR_OUT_OF_RANGE});
  }
  static_assert(alignof(ar_hdr) == 1);
  auto ar = reinterpret_cast<const ar_hdr*>(header.data());
  if (!ar->valid()) {
    return CorruptedDump();
  }
  MemberHeader member{TrimSpaces({ar->ar_name, sizeof(ar->ar_name)}), 0, 0};
  if (!ParseHeaderInteger({ar->ar_date, sizeof(ar->ar_date)}, member.date) ||
      !ParseHeaderInteger({ar->ar_size, sizeof(ar->ar_size)}, member.size)) {
    return CorruptedDump();
  }
  return fit::ok(member);
}

// Update member.name if it's an encoded reference to the long name table.
bool HandleLongName(std::string_view name_table, MemberHeader& member) {
  if (member.name.substr(0, ar_hdr::kLongNamePrefix.size()) == ar_hdr::kLongNamePrefix) {
    size_t name_offset = std::string_view::npos;
    if (!ParseHeaderInteger(member.name.substr(ar_hdr::kLongNamePrefix.size()), name_offset)) {
      return false;
    }
    member.name = name_table.substr(name_offset);
    size_t end = member.name.find(ar_hdr::kNameTableTerminator);
    if (end == 0 || end == std::string_view::npos) {
      return false;
    }
    member.name = member.name.substr(0, end);
  }
  return true;
}

// If name starts with match, then parse it as a note and store it in the map.
// The successful return value is false if the name didn't match or true if it
// was a valid note that wasn't already in the map.
template <typename Key>
fit::result<Error, std::optional<Key>> JobNoteName(std::string_view match, std::string_view name) {
  if (name.size() > match.size() && name[match.size()] == '.' && name.starts_with(match)) {
    name.remove_prefix(match.size() + 1);
    if (name.empty()) {
      return CorruptedDump();
    }
    Key key = 0;
    if (ParseHeaderInteger(name, key)) {
      return fit::ok(key);
    }
  }
  return fit::ok(std::nullopt);
}

// Add a note to an info_ or properties_ map.  Duplicates are not allowed.
template <typename Key>
fit::result<Error> AddNote(std::map<Key, ByteView>& map, Key key, ByteView data) {
  auto [it, unique] = map.insert({key, data});
  if (!unique) {
    return fit::error(Error{
        "duplicate note name in dump",
        ZX_ERR_IO_DATA_INTEGRITY,
    });
  }
  return fit::ok();
}

// rapidjson's built-in features require NUL-terminated strings.
// Modeled on rapidjson::StringBuffer from <rapidjson/stringbuffer.h>.
class StringViewStream {
 public:
  using Ch = char;

  explicit StringViewStream(std::string_view data) : data_(data) {}

  Ch Peek() const { return data_.empty() ? '\0' : data_.front(); }

  Ch Take() {
    Ch c = data_.front();
    data_.remove_prefix(1);
    return c;
  }

  size_t Tell() const { return data_.size(); }

  Ch* PutBegin() {
    RAPIDJSON_ASSERT(false);
    return 0;
  }
  void Put(Ch) { RAPIDJSON_ASSERT(false); }
  void Flush() { RAPIDJSON_ASSERT(false); }
  size_t PutEnd(Ch*) {
    RAPIDJSON_ASSERT(false);
    return 0;
  }

 private:
  std::string_view data_;
};

template <class TaskType>
struct LiveTaskHandle {
  using type = LiveHandle;
};

template <class TaskType>
using LiveTask = typename LiveTaskHandle<TaskType>::type;

#ifdef __Fuchsia__
template <>
struct LiveTaskHandle<Job> {
  using type = zx::job;
};

template <>
struct LiveTaskHandle<Process> {
  using type = zx::process;
};

template <>
struct LiveTaskHandle<Thread> {
  using type = zx::thread;
};
#endif

using InsertChild = std::variant<std::monostate, Job, Process, Thread, Resource>;

}  // namespace

// This is the real guts of the zxdump::TaskHolder class.
class TaskHolder::JobTree {
 public:
  Job& root_job() const { return root_job_; }

  Resource& info_resource() { return info_resource_; }

  size_t memory_cache_limit() const { return live_memory_cache_.cache_limit(); }

  void set_memory_cache_limit(size_t limit) { live_memory_cache_.set_cache_limit(limit); }

  LiveMemoryCache& live_memory_cache() { return live_memory_cache_; }

  // Insert any number of dumps by reading a core file or an archive.
  fit::result<Error> Insert(fbl::unique_fd fd, bool read_memory) {
    if (auto result = DumpFile::Open(std::move(fd)); result.is_error()) {
      return result.take_error();
    } else {
      dumps_.push_front(std::move(result).value());
    }
    auto& file = *dumps_.front();
    auto result = Read(file, read_memory, {0, file.size()});
    if (!read_memory) {
      file.shrink_to_fit();
    }
    if (file.size() == 0) {
      dumps_.pop_front();
    }
    return result;
  }

  // This implements Job::children(), Job::processes(), and Process::threads().
  // If the map is empty in a live task, fill it.
  template <zx_object_info_topic_t KoidListTopic, auto KoidMapMember, class Parent>
  auto GetChildren(Parent& parent)
      -> fit::result<Error, decltype(std::ref(parent.*KoidMapMember))> {
    using Child = typename std::decay_t<decltype(parent.*KoidMapMember)>::mapped_type;

    if ((parent.*KoidMapMember).empty() && parent.live()) {
      // The first time called on a live task (or on repeated calls iff the
      // first time there were no children), populate the whole list.
      // Start by fetching a fresh list of child KOIDs.
      auto koids = parent.template get_info<KoidListTopic>(true);
      if (koids.is_error()) {
        return koids.take_error();
      }

      for (zx_koid_t koid : *koids) {
        if (koid == ZX_KOID_INVALID) {
          continue;
        }

        auto live_child = GetLiveChild(parent, koid, kGetChildRights<Child>);
        if (live_child.is_error()) {
          if (live_child.error_value().status_ == ZX_ERR_NOT_FOUND) {
            // It's not an error if the child died since we collected the list.
            continue;
          }
          return live_child.take_error();
        }

        auto result = PlaceLiveChild<KoidMapMember>(parent, *std::move(live_child));
        if (result.is_error()) {
          return result.take_error();
        }
      }
    }
    return fit::ok(std::ref(parent.*KoidMapMember));
  }

  template <class Parent, auto KoidMapMember>
  using ChildType =
      typename std::decay_t<decltype(std::declval<Parent>().*KoidMapMember)>::mapped_type;

  template <class Parent>
  using GetChildRef =
      std::reference_wrapper<std::conditional_t<std::is_same_v<Parent, Process>, Thread, Task>>;

  template <class Parent>
  using GetChildResult = fit::result<Error, GetChildRef<Parent>>;

  template <auto... KoidMapMember, class Parent>
  GetChildResult<Parent> GetChild(Parent& parent, zx_koid_t koid) {
    GetChildResult<Parent> result = kChildNotFound;

    auto find = [&](auto children) -> bool {
      auto found = (parent.*children).find(koid);
      if (found == (parent.*children).end()) {
        return false;
      }
      result = fit::ok(std::ref(found->second));
      return true;
    };

    if (!(find(KoidMapMember) || ...) && parent.live()) {
      result = PlaceLiveChild<KoidMapMember...>(parent, koid);
    }

    return result;
  }

  fit::result<Error, std::reference_wrapper<Task>> FindChild(Job& job, zx_koid_t koid) {
    auto parent = parent_map_.find(koid);
    if (parent != parent_map_.end() && &parent->second.get() != &superroot_) {
      // This is a known KOID.  Go directly to its parent.
      return job.get_child(koid);
    }

    // The superroot might have live children.  Otherwise, if the job is from
    // a dump, it's not going to have any new children to find.
    if (&job != &superroot_ && !job.live()) {
      return kChildNotFound;
    }

    // First check this job itself for a direct child.
    // Ignore the error of just not finding it, but not other errors.
    auto result = job.get_child(koid);
    if (result.is_ok() || result.error_value().status_ != ZX_ERR_NOT_FOUND) {
      return result;
    }

    auto check_children = [koid, &result](auto list) -> bool {
      if (list.is_error()) {
        result = list.take_error();
        return true;
      }
      for (auto& [child_koid, child] : list->get()) {
        ZX_DEBUG_ASSERT(child_koid != koid);
        result = child.get_child(koid);
        if (result.is_ok() || result.error_value().status_ != ZX_ERR_NOT_FOUND) {
          return true;
        }
      }
      return false;
    };

    // Check all the processes first, in case this is a thread KOID.
    // Finally, recurse on all child jobs.

    check_children(job.processes()) || check_children(job.children());

    return result;
  }

  // A live parent is looking for a direct child by KOID.
  // Fetch the live child handle and then place it in the parent.
  template <auto... KoidMapMember, class Parent>
  GetChildResult<Parent> PlaceLiveChild(Parent& parent, zx_koid_t koid) {
    constexpr zx_rights_t kRights = (kGetChildRights<ChildType<Parent, KoidMapMember>> | ...);

    auto live_child = GetLiveChild(parent, koid, kRights);
    if (live_child.is_error()) {
      return live_child.take_error();
    }

    return PlaceLiveChild<KoidMapMember...>(parent, *std::move(live_child));
  }

  // A parent has a new live child to consider.  Make sure the handle is kosher
  // for one of the parent's lists, and place the new child.
  template <auto... KoidMapMember, class Parent>
  GetChildResult<Parent> PlaceLiveChild(Parent& parent, LiveHandle live,
                                        std::optional<zx_info_handle_basic_t> info = std::nullopt) {
    if (!info) {
      info.emplace();
      zx_status_t status =
          live.get_info(ZX_INFO_HANDLE_BASIC, &*info, sizeof(*info), nullptr, nullptr);
      if (status != ZX_OK) {
        return fit::error(Error{"invalid live task", status});
      }
    }

    GetChildResult<Parent> result = kWrongType;

    auto place = [&](auto& children) -> bool {
      using Child = typename std::decay_t<decltype(children)>::mapped_type;

      if (info->type != kObjType<Child>) {
        return false;
      }

      auto [it, unique] = children.try_emplace(info->koid, *this);
      Child& child = it->second;

      if (child.live() && !unique) {
        result = fit::error{Error{
            "same live task inserted twice",
            ZX_ERR_ALREADY_EXISTS,
        }};
        return false;
      }

      // Record the time of the first sample from this task.
      // If the task was already inserted from a dump, keep its old stamp.
      if (unique || child.date_ == 0) {
        child.date_ = time(nullptr);
      }

      // Store the basic info we already collected.  If the task was already
      // inserted from a dump, the new live info will override the past info.
      // Other topics with old info from the dump remain, however.
      auto buffer = GetBuffer(sizeof(*info));
      memcpy(buffer, &*info, sizeof(*info));
      ByteView data{buffer, sizeof(*info)};
      child.info_.insert_or_assign(ZX_INFO_HANDLE_BASIC, data);

      child.live_ = std::move(live);
      ReifyTask(child, parent);

      result = fit::ok(std::ref(child));
      return true;
    };

    (place(parent.*KoidMapMember) || ...);

    return result;
  }

  template <class Parent>
  static fit::result<Error, LiveHandle> GetLiveChild(Parent& parent, zx_koid_t koid,
                                                     zx_rights_t rights) {
    // Use a momentary move to get the object into the right type.  This
    // has the same in effect as using zx::unowned_* types but doesn't
    // require the non-Fuchsia LiveHandle stub to have unowned support.
    LiveTask<Parent> live_parent(std::move(parent.live()));
    auto restore_live =
        fit::defer([&parent, &live_parent]() { parent.live() = std::move(live_parent); });
    LiveHandle live_child;
    zx_status_t status = live_parent.get_child(koid, rights, &live_child);
    if (status != ZX_OK) {
      return fit::error(Error{"zx_object_get_child", status});
    }
    return fit::ok(std::move(live_child));
  }

  fit::result<Error, std::reference_wrapper<Object>> Insert(LiveHandle live) {
    zx_info_handle_basic_t info;
    if (zx_status_t status =
            live.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
        status != ZX_OK) {
      return fit::error(Error{"invalid live handle", status});
    }

    switch (info.type) {
      case ZX_OBJ_TYPE_RESOURCE:
        return InsertLiveResource(std::move(live), info);

      case ZX_OBJ_TYPE_JOB:
      case ZX_OBJ_TYPE_PROCESS:
        return InsertLiveTask(std::move(live), info);

      default:
        return kWrongType;
    }
  }

  fit::result<Error, std::reference_wrapper<Resource>> InsertLiveResource(
      LiveHandle live, const zx_info_handle_basic_t& info) {
    zx_info_resource_t resource;
    if (zx_status_t status =
            live.get_info(ZX_INFO_RESOURCE, &resource, sizeof(resource), nullptr, nullptr);
        status != ZX_OK) {
      return fit::error(Error{"invalid resource handle", status});
    }

    if (resource.kind != ZX_RSRC_KIND_SYSTEM && resource.base != ZX_RSRC_SYSTEM_INFO_BASE) {
      return fit::error{Error{
          "only info resource supported",
          ZX_ERR_NOT_SUPPORTED,
      }};
    }

    if (info_resource_.live()) {
      // There's already a live info resource attached.
      return fit::error{Error{
          "live info resource handle already inserted",
          ZX_ERR_ALREADY_EXISTS,
      }};
    }

    // Clear out any old data from dumps and inject the basic data we have now.
    info_resource_.info_.clear();
    info_resource_.properties_.clear();

    auto inject = [this](zx_object_info_topic_t topic, auto& info) {
      auto buffer = GetBuffer(sizeof(info));
      memcpy(buffer, &info, sizeof(info));
      info_resource_.info_.emplace(topic, ByteView{buffer, sizeof(info)});
    };

    inject(ZX_INFO_HANDLE_BASIC, info);
    inject(ZX_INFO_RESOURCE, resource);

    info_resource_.live_ = std::move(live);

    return fit::ok(std::ref(info_resource_));
  }

  fit::result<Error, std::reference_wrapper<Task>> InsertLiveTask(
      LiveHandle live, const zx_info_handle_basic_t& info) {
    auto place = EnterChild(info.koid);
    auto& [is_new, parent] = place;

    if (!is_new && parent.type() != ZX_OBJ_TYPE_JOB) {
      // Some existing process expects this to be a thread KOID.  But we
      // already know it's not a thread handle, so something is fishy.
      return fit::error{Error{
          "live handle matches dangling thread KOID; corrupted dump?",
          ZX_ERR_IO_DATA_INTEGRITY,
      }};
    }

    Job& job = static_cast<Job&>(parent);
    return PlaceLiveChild<&Job::children_, &Job::processes_>(job, std::move(live), info);
  }

  std::pair<bool, Task&> EnterChild(zx_koid_t koid) {
    // If this is a known KOID, find its expectant parent.
    // Otherwise, it will be adopted by the superroot.
    auto [it, unique] = parent_map_.try_emplace(koid, superroot_);
    auto& [parent_koid, parent] = *it;
    ZX_DEBUG_ASSERT(parent_koid == koid);
    return {unique, parent};
  }

  template <auto KoidMapMember, class Child>
  fit::result<Error> PlaceDump(Child&& child) {
    zx_koid_t koid = child.koid();

    auto place = EnterChild(koid);
    auto& [new_child, parent_task] = place;

    if (!new_child && parent_task.type() != ZX_INFO_JOB) {
      return fit::error{Error{
          "dumped task KOID matches dangling thread KOID; corrupted dump?",
          ZX_ERR_IO_DATA_INTEGRITY,
      }};
    }
    Job& parent_job = static_cast<Job&>(parent_task);

    // Place the new Child into the parent's (children or processes) list.
    // This constructs a fresh Child in place because only that constructor is
    // public.  Then we can move the pending data into the placed object via
    // the private move-assignment operator.
    auto [it, unique] = (parent_job.*KoidMapMember).try_emplace(koid, *this);
    Child& placed_child = it->second;
    if (!unique) {
      return kDuplicate<Child>;
    }
    placed_child = std::forward<Child>(child);

    // Reify the task so parent_map_ lists all its children.
    ReifyTask(placed_child, parent_job);

    return fit::ok();
  }

  void AssertIsSuperroot(Object& object) { ZX_DEBUG_ASSERT(&object == &superroot_); }

  // Unlike generic get_info, the view is always fully aligned for casting.
  fit::result<Error, ByteView> GetSuperrootInfo(zx_object_info_topic_t topic) {
    auto children_koid_list = [this](auto children, auto& info_children) {
      if (!info_children) {
        // No value cached.
        zx_koid_t* p = new zx_koid_t[(superroot_.*children).size()];
        info_children.reset(p);
        for (const auto& [koid, job] : superroot_.*children) {
          *p++ = koid;
        }
      }
      return fit::ok(ByteView{
          reinterpret_cast<const std::byte*>(info_children.get()),
          (superroot_.*children).size(),
      });
    };

    switch (topic) {
      case ZX_INFO_JOB_CHILDREN:
        return children_koid_list(&Job::children_, superroot_info_children_);

      case ZX_INFO_JOB_PROCESSES:
        return children_koid_list(&Job::processes_, superroot_info_processes_);
        ;

      default:
        return fit::error(Error{"fake root job info", ZX_ERR_NOT_SUPPORTED});
    }
  }

  // Allocate a buffer saved for the life of this holder.
  std::byte* GetBuffer(size_t size) {
    std::byte* buffer = new std::byte[size];
    buffers_.emplace_front(buffer);
    return buffer;
  }

  void TakeBuffer(std::unique_ptr<std::byte[]> owned_buffer) {
    buffers_.push_front(std::move(owned_buffer));
  }

  template <typename T>
  T GetSystemData(const char* key) const;

  uint64_t system_get_page_size() const;

#ifdef __Fuchsia__
  fit::result<Error> InsertSystem() {
    std::string_view version = zx_system_get_version_string();
    auto version_string = rapidjson::StringRef(version.data(), version.size());
    system_.SetObject()
        .AddMember("version_string", version_string, system_.GetAllocator())
        .AddMember("dcache_line_size", zx_system_get_dcache_line_size(), system_.GetAllocator())
        .AddMember("num_cpus", zx_system_get_num_cpus(), system_.GetAllocator())
        .AddMember("page_size", zx_system_get_page_size(), system_.GetAllocator())
        .AddMember("physmem", zx_system_get_physmem(), system_.GetAllocator());
    return fit::ok();
  }
#endif

 private:
  using ParentMap = std::map<zx_koid_t, std::reference_wrapper<Task>>;

  template <class Child>
  static constexpr auto kDuplicate =
      fit::error(Error{"duplicate job KOID", ZX_ERR_IO_DATA_INTEGRITY});

  template <>
  constexpr fit::error<Error> kDuplicate<Process> =
      fit::error(Error{"duplicate process KOID", ZX_ERR_IO_DATA_INTEGRITY});

  static constexpr auto kChildNotFound = fit::error{Error{"zx_object_get_child", ZX_ERR_NOT_FOUND}};

  static constexpr auto kWrongType =
      fit::error{Error{"live handle has wrong type", ZX_ERR_WRONG_TYPE}};

  template <class Child>
  static constexpr zx_rights_t kGetChildRights = kChildRights;

  template <>
  constexpr zx_rights_t kGetChildRights<Thread> = kThreadRights;

  template <class ObjectType>
  static constexpr zx_obj_type_t kObjType = ZX_OBJ_TYPE_NONE;

  template <>
  constexpr zx_obj_type_t kObjType<Job> = ZX_OBJ_TYPE_JOB;

  template <>
  constexpr zx_obj_type_t kObjType<Process> = ZX_OBJ_TYPE_PROCESS;

  template <>
  constexpr zx_obj_type_t kObjType<Thread> = ZX_OBJ_TYPE_THREAD;

  // This is the actual reader, implemented below.
  fit::result<Error> Read(DumpFile& file, bool read_memory, FileRange where, time_t date = 0);
  fit::result<Error> ReadElf(DumpFile& file, FileRange where, time_t date, ByteView header,
                             bool read_memory);
  fit::result<Error> ReadArchive(DumpFile& file, FileRange archive, ByteView header,
                                 bool read_memory);

  fit::result<Error> ReadSystemNote(ByteView data);
  const rapidjson::Value* GetSystemJsonData(const char* key) const;

  // Reify a freshly-inserted task.  This populates parent_map_ with all its
  // child KOIDs.  Threads have nothing to do.

  void ReifyTask(Thread& thread, Process& parent) {
    // This handles a new live thread.  For a thread in a dump (from ReadElf),
    // the parent object will be moved so this pointer will become invalid.
    // ReifyTask<Process> below will fix these up.
    thread.process_ = &parent;
  }

  void ReifyTask(Process& process, Job& parent) {
    ReifyChildren<ZX_INFO_PROCESS_THREADS, &Process::threads_>(process);

    // Now that the Process has been moved into its final place, the
    // Thread::process_ back-pointers can be set properly.
    for (auto& [koid, thread] : process.threads_) {
      thread.process_ = &process;
    }

#ifdef __Fuchsia__
    process.dump_machine_ = elfldltl::ElfMachine::kNative;

    // Fix the proper page size even before any memory is actually read.
    // The dumper relies on this to do layout alignment.  When reading
    // from a previous dump to dump again, it's always set by observed
    // PT_LOAD p_align fields or by a system note--and in a dump where
    // neither system notes nor any PT_LOAD segments are present (i.e. a
    // process with no memory at all, since they are present for all
    // mappings whether or not that memory is in the dump) nobody cares
    // what the value is since there is no memory to contemplate anyway.
    process.dump_page_size_ = zx_system_get_page_size();
#endif
  }

  void ReifyTask(Job& job, Job& parent) {
    // Reify both lists.
    ReifyChildren<ZX_INFO_JOB_CHILDREN, &Job::children_>(job);
    ReifyChildren<ZX_INFO_JOB_PROCESSES, &Job::processes_>(job);

    // That might have moved some child tasks out of the superroot if they were
    // inserted before the new job.  The new job might also be the sole child
    // of the superroot so it should become the new root.
    Reroot();
  }

  template <zx_object_info_topic_t Topic, auto KoidMapMember, class Parent>
  void ReifyChildren(Parent& parent) {
    auto children = parent.template get_info<Topic>();
    if (children.is_ok()) {
      for (zx_koid_t koid : *children) {
        // Put each child's KOID into the parent map so that it can be matched
        // up when PlaceLiveChild or PlaceDump encounters the same task.
        auto [it, unique] = parent_map_.try_emplace(koid, parent);

        // If it was already there, the child has already been seen.  If it's
        // actually already reified in a different parent, that's a bogus
        // situation but harmless enough to ignore.  More likely it was reified
        // as child of the superroot when it was seen as an orphan.  Move it
        // from the superroot's child list to the proper parent's list.
        if constexpr (Topic != ZX_INFO_PROCESS_THREADS) {
          if (!unique && &it->second.get() == &superroot_) {
            auto& parent_children = parent.*KoidMapMember;
            auto& superroot_children = superroot_.*KoidMapMember;
            auto found = superroot_children.find(koid);
            if (found != superroot_children.end()) {
              parent_children.insert(superroot_children.extract(found));
            }
          }
        }
      }
    }
  }

  // Snap the root job pointer to the sole job or back to the superroot.
  // Also clear the cached get_info lists so they'll be regenerated on demand.
  void Reroot() {
    if (superroot_.processes_.empty() && superroot_.children_.size() == 1) {
      auto& [koid, job] = *superroot_.children_.begin();
      root_job_ = std::ref(job);
    } else if (&root_job_.get() == &superroot_) {
      // Don't clear the caches if nothing is changing.
      return;
    } else {
      root_job_ = std::ref(superroot_);
    }
    superroot_info_children_.reset();
    superroot_info_processes_.reset();
  }

  fit::result<Error> ReadKernelNote(zx_object_info_topic_t topic, ByteView data) {
    info_resource_.info_.try_emplace(topic, data);
    return fit::ok();
  }

  std::forward_list<std::unique_ptr<DumpFile>> dumps_;
  std::forward_list<std::unique_ptr<std::byte[]>> buffers_;

  rapidjson::Document system_;

  // The superroot holds all the orphaned jobs and processes that haven't been
  // claimed by a parent job.
  Job superroot_{*this};

  // This records task KOID ever encountered: either a task that's already in
  // the holder; or a task whose KOID was listed on its (presumed) parent's
  // children, processes, or threads list, but is not yet in the holder.  Each
  // KOID is associated with the parent Job or Process that has this KOID on
  // one of its lists.
  ParentMap parent_map_;

  // These are the buffers for the synthetic ZX_INFO_JOB_CHILDREN and
  // ZX_INFO_JOB_PROCESSES results returned by get_info calls on the superroot.
  // They are regenerated on demand, and cleared when new tasks are inserted.
  std::unique_ptr<zx_koid_t[]> superroot_info_children_, superroot_info_processes_;

  // The root job is either the superroot or its only child.
  std::reference_wrapper<Job> root_job_{superroot_};

  // The only resource we hold is the info resource.
  Resource info_resource_{*this};

  // Shared cache of pages read from process memory (see live-memory-cache.h).
  LiveMemoryCache live_memory_cache_;
};

// JobTree is an incomplete type outside this translation unit.  Some methods
// on TaskHolder et al need to access tree_, so they are defined here.

TaskHolder::TaskHolder() { tree_ = std::make_unique<JobTree>(); }

TaskHolder::~TaskHolder() = default;

Job& TaskHolder::root_job() const { return tree_->root_job(); }

Resource& TaskHolder::info_resource() const { return tree_->info_resource(); }

Resource& Object::info_resource() { return tree().info_resource(); }

fit::result<Error> TaskHolder::Insert(fbl::unique_fd fd, bool read_memory) {
  return tree_->Insert(std::move(fd), read_memory);
}

fit::result<Error, std::reference_wrapper<Object>> TaskHolder::Insert(LiveHandle obj) {
  return tree_->Insert(std::move(obj));
}

Object::~Object() = default;

Task::~Task() = default;

Job::~Job() = default;

Process::~Process() = default;

Thread::~Thread() = default;

Resource::~Resource() = default;

fit::result<Error, std::reference_wrapper<zxdump::Job::JobMap>> Job::children() {
  return tree().GetChildren<ZX_INFO_JOB_CHILDREN, &Job::children_>(*this);
}

fit::result<Error, std::reference_wrapper<zxdump::Job::ProcessMap>> Job::processes() {
  return tree().GetChildren<ZX_INFO_JOB_PROCESSES, &Job::processes_>(*this);
}

fit::result<Error, std::reference_wrapper<zxdump::Process::ThreadMap>> Process::threads() {
  return tree().GetChildren<ZX_INFO_PROCESS_THREADS, &Process::threads_>(*this);
}

std::byte* Object::GetBuffer(size_t size) { return tree().GetBuffer(size); }

void Object::TakeBuffer(std::unique_ptr<std::byte[]> buffer) {
  tree().TakeBuffer(std::move(buffer));
}

fit::result<Error, ByteView> Object::GetSuperrootInfo(zx_object_info_topic_t topic) {
  if (this == &(tree().info_resource())) {
    return fit::error{
        Error{"no kernel information recorded", ZX_ERR_NOT_SUPPORTED},
    };
  }
  tree_.get().AssertIsSuperroot(*this);
  return tree_.get().GetSuperrootInfo(topic);
}

fit::result<Error, ByteView> Object::get_info_aligned(  //
    zx_object_info_topic_t topic, size_t record_size, size_t align, bool refresh_live) {
  ByteView bytes;
  if (auto result = get_info(topic, refresh_live, record_size); result.is_error()) {
    return result.take_error();
  } else {
    bytes = result.value();
  }

  void* ptr = const_cast<void*>(static_cast<const void*>(bytes.data()));
  size_t space = bytes.size();
  if (std::align(align, space, ptr, space)) {
    // It's already aligned.
    return fit::ok(bytes);
  }

  // Allocate a buffer with alignment slop and make the holder hold onto it.
  space = bytes.size() + align - 1;
  ptr = tree_.get().GetBuffer(space);

  // Copy the data into the buffer with the right alignment.
  void* aligned_ptr = std::align(align, bytes.size(), ptr, space);
  memcpy(aligned_ptr, bytes.data(), bytes.size());

  // Return the aligned data in the buffer now held in the holder and replace
  // the cached data with the aligned copy for the next lookup to find.
  ByteView copy{static_cast<std::byte*>(aligned_ptr), bytes.size()};
  info_[topic] = copy;
  return fit::ok(copy);
}

fit::result<Error> TaskHolder::JobTree::Read(DumpFile& real_file, bool read_memory, FileRange where,
                                             time_t date) {
  // If the file is compressed, this will iterate with the decompressed file.
  for (DumpFile* file = &real_file; where.size >= kHeaderProbeSize;
       // Read the whole uncompressed file as a stream.  Its size is unknown.
       where = FileRange::Unbounded()) {
    ByteView header;
    if (auto result = file->ReadEphemeral(where / kHeaderProbeSize); result.is_error()) {
      return result.take_error();
    } else {
      header = result.value();
    }

    if (uint32_t word; memcpy(&word, header.data(), sizeof(word)), word == Elf::Ehdr::kMagic) {
      return ReadElf(*file, where, date, header, read_memory);
    }

    std::string_view header_string{
        reinterpret_cast<const char*>(header.data()),
        header.size(),
    };
    if (header_string.starts_with(kArchiveMagic)) {
      return ReadArchive(*file, where, header, read_memory);
    }

    // If it's not a compressed file, we don't grok it.
    if (!DumpFile::IsCompressed(header)) {
      break;
    }

    // Start streaming decompression to deliver the uncompressed dump file.
    // Then iterate to read that (streaming) file.
    auto result = file->Decompress(where, header);
    if (result.is_error()) {
      return result.take_error();
    }
    file = result.value().get();
    dumps_.push_front(std::move(result).value());
  }
  return fit::error(Error{"not an ELF or archive file", ZX_ERR_NOT_FILE});
}

fit::result<Error> TaskHolder::JobTree::ReadElf(DumpFile& file, FileRange where, time_t date,
                                                ByteView header, bool read_memory) {
  Elf::Ehdr ehdr;
  if (header.size() < sizeof(ehdr)) {
    return TruncatedDump();
  }
  memcpy(&ehdr, header.data(), sizeof(ehdr));
  if (!ehdr.Valid() || ehdr.phentsize() != sizeof(Elf::Phdr) ||
      ehdr.type != elfldltl::ElfType::kCore) {
    return fit::error(Error{"ELF file is not a Zircon core dump", ZX_ERR_IO_DATA_INTEGRITY});
  }

  // Get the count of program headers.  Large counts use a special encoding
  // marked by PN_XNUM.  The 0th section header's sh_info is the real count.
  size_t phnum = ehdr.phnum;
  if (phnum == Elf::Ehdr::kPnXnum) {
    Elf::Shdr shdr;
    if (ehdr.shoff < sizeof(ehdr) || ehdr.shnum() == 0 || ehdr.shentsize() != sizeof(shdr)) {
      return fit::error(Error{
          "invalid ELF section headers for PN_XNUM",
          ZX_ERR_IO_DATA_INTEGRITY,
      });
    }
    auto result = file.ReadEphemeral(where / FileRange{ehdr.shoff, sizeof(shdr)});
    if (result.is_error()) {
      return result.take_error();
    }
    if (result.value().size() < sizeof(shdr)) {
      return TruncatedDump();
    }
    memcpy(&shdr, result.value().data(), sizeof(shdr));
    phnum = shdr.info;
  }

  // Read the program headers.
  ByteView phdrs_bytes;
  if (ehdr.phoff > where.size || where.size / sizeof(Elf::Phdr) < phnum) {
    return TruncatedDump();
  } else {
    const size_t phdrs_size_bytes = phnum * sizeof(Elf::Phdr);
    auto result = file.ReadEphemeral(where / FileRange{ehdr.phoff, phdrs_size_bytes});
    if (result.is_error()) {
      return result.take_error();
    } else {
      phdrs_bytes = result.value();
    }
    if (phdrs_bytes.size() < phdrs_size_bytes) {
      // If it doesn't have all the phdrs, it won't have anything after them.
      return TruncatedDump();
    }
  }

  // Parse the program headers.  Note they occupy the ephemeral buffer
  // throughout the parsing loop, so it cannot use ReadEphemeral at all.

  // Process-wide notes will accumulate in the Process.
  Process process(*this);
  if (read_memory) {
    // The back-pointer is needed by Process::ReadMemoryImpl.
    process.dump_ = &file;
  }

  // Per-thread notes will accumulate in the thread until a new thread's first
  // note is seen.
  std::optional<Thread> thread;

  auto reify_thread = [this, &process, &thread]() {
    if (thread) {
      zx_koid_t koid = thread->koid();
      // Construct a new empty thread since only that constructor is public.
      // Then we can use the private move-assignment operator on it.
      auto it = process.threads_.emplace_hint(process.threads_.end(), koid, *this);
      auto& [thread_koid, placed_thread] = *it;
      ZX_DEBUG_ASSERT(thread_koid == koid);
      // Ignore duplicates here since they do no real harm.
      if (placed_thread.empty()) {
        placed_thread = *std::move(thread);
      }
    }
  };

  // Parse a note segment.  Truncated notes do not cause an error.
  auto parse_notes = [&](FileRange notes) -> fit::result<Error> {
    // Cap the segment size to what's available in the file.
    notes.size = std::min(notes.size, where.size - notes.offset);

    // Read the whole segment and keep it forever.
    Elf::NoteSegment segment;
    if (auto result = file.ReadPermanent(where / notes); result.is_error()) {
      return result.take_error();
    } else {
      segment = result.value();
    }

    // Iterate through the notes.
    for (auto [name, desc, type] : segment) {
      // All valid note names end with a NUL terminator.
      if (name.empty() || name.back() != '\0') {
        // Ignore bogus notes.  Could make them an error?
        continue;
      }
      name.remove_suffix(1);

      // Check for a system note.
      if (name == kSystemNoteName) {
        auto result = ReadSystemNote(desc);
        if (result.is_error()) {
          return result.take_error();
        }
        continue;
      }

      // Check for a kernel note.
      if (name == std::string_view{kKernelInfoNoteName}) {
        auto result = ReadKernelNote(type, desc);
        if (result.is_error()) {
          return result.take_error();
        }
        continue;
      }

      // Check for a dump date note.
      if (name == kDateNoteName) {
        if (desc.size() < sizeof(process.date_)) {
          return CorruptedDump();
        }
        memcpy(&process.date_, desc.data(), sizeof(process.date_));
        continue;
      }

      // Check for dump remarks.
      if (name.starts_with(kRemarkNotePrefix)) {
        process.remarks_.emplace_back(name.substr(kRemarkNotePrefix.size()), desc);
        continue;
      }

      // Check for a process info note.
      if (name == kProcessInfoNoteName) {
        if (type == ZX_INFO_HANDLE_BASIC) {
          zx_info_handle_basic_t info;
          if (desc.size() < sizeof(info)) {
            return CorruptedDump();
          }
          memcpy(&info, desc.data(), sizeof(info));

          // Validate the type because it's used for static_cast validation.
          if (info.type != ZX_OBJ_TYPE_PROCESS) {
            return CorruptedDump();
          }
        }
        auto result = AddNote(process.info_, type, desc);
        if (result.is_error()) {
          return result.take_error();
        }
        continue;
      }

      // Not a process info note.  Check for a process property note.
      if (name == kProcessPropertyNoteName) {
        auto result = AddNote(process.properties_, type, desc);
        if (result.is_error()) {
          return result.take_error();
        }
        continue;
      }

      // Not any kind of process note.  Check for a thread info note.
      if (name == kThreadInfoNoteName) {
        if (type == ZX_INFO_HANDLE_BASIC) {
          // This marks the first note of a new thread.  Reify the last one.
          reify_thread();

          zx_info_handle_basic_t info;
          if (desc.size() < sizeof(info)) {
            return CorruptedDump();
          }
          memcpy(&info, desc.data(), sizeof(info));

          // Validate the type because it's used for static_cast validation.
          if (info.type != ZX_OBJ_TYPE_THREAD) {
            return CorruptedDump();
          }

          // Start recording a new thread.  This is the only place that
          // constructs new Thread objects, so every extant Thread has the
          // basic info.  But we don't validate that the KOID is not zero or a
          // duplicate.  Such bogons don't really do harm.  They will be
          // visible in the threads() list or to get_child calls using their
          // bogus KOIDs, even if they are never in the ZX_INFO_PROCESS_THREADS
          // list.  That behavior is inconsistent with a real live process but
          // it's consistent with the way the dump was actually written.
          thread.emplace(*this);
        } else if (!thread) {
          return fit::error(Error{
              "first thread info note is not ZX_INFO_HANDLE_BASIC",
              ZX_ERR_IO_DATA_INTEGRITY,
          });
        }

        auto result = AddNote(thread->info_, type, desc);
        if (result.is_error()) {
          return result.take_error();
        }
        continue;
      }

      // Not a thread info note.  Check for a thread property note.
      if (name == kThreadPropertyNoteName) {
        if (!thread) {
          return fit::error(Error{
              "thread property note before thread ZX_INFO_HANDLE_BASIC note",
              ZX_ERR_IO_DATA_INTEGRITY,
          });
        }

        auto result = AddNote(thread->properties_, type, desc);
        if (result.is_error()) {
          return result.take_error();
        }
        continue;
      }

      // Not a thread property note.  Check for a thread state note.
      if (name == kThreadStateNoteName) {
        if (!thread) {
          return fit::error(Error{
              "thread state note before thread ZX_INFO_HANDLE_BASIC note",
              ZX_ERR_IO_DATA_INTEGRITY,
          });
        }

        auto result = AddNote(thread->state_, type, desc);
        if (result.is_error()) {
          return result.take_error();
        }
        continue;
      }

      // Ignore unrecognized notes.  Could make them an error?
    }

    return fit::ok();
  };

  // Validate a memory segment and add it to the memory map.
  auto add_segment = [where, &process](uint64_t vaddr,
                                       Process::Segment segment)  //
      -> fit::result<Error> {
    if (vaddr % process.dump_page_size() != 0) {
      return fit::error(Error{
          "ELF core file PT_LOAD segment p_vaddr not page-aligned",
          ZX_ERR_IO_DATA_INTEGRITY,
      });
    }
    if (segment.offset % process.dump_page_size() != 0) {
      return fit::error(Error{
          "ELF core file PT_LOAD segment p_offset not page-aligned",
          ZX_ERR_IO_DATA_INTEGRITY,
      });
    }
    ZX_DEBUG_ASSERT(segment.memsz > 0);
    if (!process.memory_.empty()) {
      const auto& [last_vaddr, last_segment] = *process.memory_.crbegin();
      ZX_DEBUG_ASSERT(last_segment.memsz > 0);
      if (vaddr <= last_vaddr) {
        return fit::error(Error{
            "ELF core file PT_LOAD segments not in ascending address order",
            ZX_ERR_IO_DATA_INTEGRITY,
        });
      }
      if (vaddr < last_vaddr + last_segment.memsz) {
        return fit::error(Error{
            "ELF core file PT_LOAD segments overlap",
            ZX_ERR_IO_DATA_INTEGRITY,
        });
      }
    }

    // Adjust the offset to place it within the archive, if any.
    if (where.size < segment.offset) {
      return fit::error(Error{
          "ELF core file PT_LOAD p_offset past end of file",
          ZX_ERR_IO_DATA_INTEGRITY,
      });
    }
    if (where.size - segment.offset < segment.filesz) {
      return fit::error(Error{
          "ELF core file PT_LOAD p_filesz past end of file",
          ZX_ERR_IO_DATA_INTEGRITY,
      });
    }
    segment.offset += where.offset;

    process.memory_.emplace_hint(process.memory_.end(), vaddr, segment);
    return fit::ok();
  };

  while (!phdrs_bytes.empty()) {
    Elf::Phdr phdr;
    if (phdrs_bytes.size() < sizeof(phdr)) {
      return TruncatedDump();
    }
    memcpy(&phdr, phdrs_bytes.data(), sizeof(phdr));
    phdrs_bytes = phdrs_bytes.subspan(sizeof(phdr));
    if (phdr.type == elfldltl::ElfPhdrType::kNote && phdr.memsz() == 0 && phdr.filesz > 0) {
      // A non-allocated note segment should hold core notes.
      auto result = parse_notes({phdr.offset, phdr.filesz});
      if (result.is_error()) {
        return result.take_error();
      }
    } else if (phdr.type == elfldltl::ElfPhdrType::kLoad && phdr.memsz > 0) {
      uint64_t page_size = std::max<uint64_t>(process.dump_page_size_, phdr.align);
      if (!std::has_single_bit(process.dump_page_size_)) {
        return fit::error(Error{
            "ELF core file PT_LOAD p_align not a power of two",
            ZX_ERR_IO_DATA_INTEGRITY,
        });
      }
      process.dump_page_size_ = page_size;
      if (read_memory) {
        const Process::Segment segment{phdr.offset, phdr.filesz, phdr.memsz};
        auto result = add_segment(phdr.vaddr, segment);
        if (result.is_error()) {
          return result.take_error();
        }
      }
    }
  }

  if (process.koid() == 0) {  // There was no ZX_INFO_HANDLE_BASIC note.
    return CorruptedDump();
  }

  // In case there was system info in this or another process or job dump but
  // no PT_LOADs, use that to set the page size.
  if (process.dump_page_size_ == 1) {
    if (uint64_t page_size = system_get_page_size()) {
      if (!std::has_single_bit(page_size)) {
        return fit::error(Error{
            "system page size not a power of two",
            ZX_ERR_IO_DATA_INTEGRITY,
        });
      }
      process.dump_page_size_ = std::max(process.dump_page_size_, page_size);
    }
  }

  if (process.dump_machine_ == elfldltl::ElfMachine::kNone) {
    process.dump_machine_ = ehdr.machine;
  }

  // Looks like a valid dump.  Finish out the last pending thread.
  reify_thread();
  return PlaceDump<&Job::processes_>(std::move(process));
}

fit::result<Error> TaskHolder::JobTree::ReadArchive(DumpFile& file, FileRange archive,
                                                    ByteView header, bool read_memory) {
  // The first member's header comes immediately after kArchiveMagic.
  archive %= kArchiveMagic.size();
  header = header.subspan(kArchiveMagic.size());

  if (archive.empty()) {
    return fit::ok();
  }

  // This holds the current member's details.
  MemberHeader member{};
  FileRange contents{};

  // This parses the header into member and contents, and consumes them from
  // archive.
  auto parse = [&archive, &member, &contents](ByteView header)  //
      -> fit::result<Error, bool> {
    if (auto result = ParseArchiveHeader(header); result.is_error()) {
      return result.take_error();
    } else {
      member = result.value();
    }
    archive %= sizeof(ar_hdr);
    if (member.size > archive.size) {
      return TruncatedDump();
    }
    contents = archive / member.size;
    archive %= member.size + (member.size & 1);
    return fit::ok(true);
  };

  // This reads and parses the next header, consuming the member from archive.
  auto next = [&](bool probe = false) -> fit::result<Error, bool> {
    ByteView header;
    if (auto result = file.ReadProbe(archive / sizeof(ar_hdr)); result.is_error()) {
      return result.take_error();
    } else {
      header = result.value();
    }
    if (probe && header.empty()) {
      return fit::ok(false);
    }
    if (header.size() < sizeof(ar_hdr)) {
      return TruncatedDump();
    }
    return parse(header);
  };

  // Parse the first member header.
  if (auto result = parse(header); result.is_error()) {
    return result.take_error();
  }

  if (member.name == ar_hdr::kSymbolTableName) {
    // An archive symbol table was created by `ar`.  `gcore` won't add one.
    // Ignore it and read the next member.
    if (archive.empty()) {
      return fit::ok();
    }
    if (auto result = next(); result.is_error()) {
      return result.take_error();
    }
  }

  std::string_view name_table;
  if (member.name == ar_hdr::kNameTableName) {
    // The special first member (or second member, if there was a symbol table)
    // is the long name table.
    if (auto result = file.ReadPermanent(contents); result.is_error()) {
      return result.take_error();
    } else {
      name_table = {
          reinterpret_cast<const char*>(result.value().data()),
          result.value().size(),
      };
    }
    if (archive.empty()) {
      return fit::ok();
    }
    if (auto result = next(); result.is_error()) {
      return result.take_error();
    }
  }

  // Any note members will collect in this Job.
  Job job{*this};

  // Process one normal member.  It might be a note or an embedded dump file.
  auto handle_member = [&]() -> fit::result<Error> {
    // Check for an info note.
    if (auto info = JobNoteName<zx_object_info_topic_t>(kJobInfoName, member.name);
        info.is_error()) {
      return info.take_error();
    } else if (info.value()) {
      const zx_object_info_topic_t topic = *info.value();
      ByteView bytes;
      if (auto result = file.ReadPermanent(contents); result.is_error()) {
        return result.take_error();
      } else {
        bytes = result.value();
      }
      if (topic == ZX_INFO_HANDLE_BASIC) {
        zx_info_handle_basic_t basic_info;
        if (bytes.size() < sizeof(basic_info)) {
          return CorruptedDump();
        }
        memcpy(&basic_info, bytes.data(), sizeof(basic_info));

        // Validate the type because it's used for static_cast validation.
        if (basic_info.type != ZX_OBJ_TYPE_JOB) {
          return CorruptedDump();
        }
      }
      if (job.date_ == 0) {
        job.date_ = member.date;
      }
      return AddNote(job.info_, topic, bytes);
    }

    // Not an info note.  Check for a property note.
    if (auto property = JobNoteName<uint32_t>(kJobPropertyName, member.name); property.is_error()) {
      return property.take_error();
    } else if (property.value()) {
      auto result = file.ReadPermanent(contents);
      if (result.is_error()) {
        return result.take_error();
      }
      return AddNote(job.properties_, *property.value(), result.value());
    }

    // Check for a system note.
    if (member.name == kSystemNoteName) {
      auto result = file.ReadEphemeral(contents);
      if (result.is_error()) {
        return result.take_error();
      }
      return ReadSystemNote(result.value());
    }

    // Check for a kernel note.
    if (auto kernel = JobNoteName<uint32_t>(kKernelInfoNoteName, member.name); kernel.is_error()) {
      return kernel.take_error();
    } else if (kernel.value()) {
      auto result = file.ReadPermanent(contents);
      if (result.is_error()) {
        return result.take_error();
      }
      return ReadKernelNote(*kernel.value(), result.value());
    }

    // Check for dump remarks.
    if (member.name.starts_with(kRemarkNotePrefix)) {
      auto result = file.ReadPermanent(contents);
      if (result.is_error()) {
        return result.take_error();
      }
      job.remarks_.emplace_back(member.name.substr(kRemarkNotePrefix.size()), result.value());
      return fit::ok();
    }

    // This member file is not a job note.  It's an embedded dump file.
    return Read(file, read_memory, contents, member.date);
  };

  // Iterate through the normal members.
  while (true) {
    // Specially-encoded member names are actually indices into the name table.
    if (!HandleLongName(name_table, member)) {
      return CorruptedDump();
    }

    if (auto result = handle_member(); result.is_error()) {
      return result.take_error();
    }

    if (archive.empty()) {
      break;
    }

    auto result = next(true);
    if (result.is_error()) {
      return result.take_error();
    }
    if (!result.value()) {
      break;
    }
  }

  // End of the archive.  Reify the job.
  if (job.koid() != ZX_KOID_INVALID) {
    // Looks like a valid job.
    return PlaceDump<&Job::children_>(std::move(job));
  }

  if (job.info_.empty() && job.properties_.empty()) {
    // This was just a plain archive, not actually a job archive at all.
    // If there were any dump remarks, attach them to the superroot.
    std::move(job.remarks_.begin(), job.remarks_.end(),
              std::back_inserter(root_job_.get().remarks_));
    return fit::ok();
  }

  // This job archive had some notes but no ZX_INFO_HANDLE_BASIC note.
  return CorruptedDump();
}

fit::result<Error> TaskHolder::JobTree::ReadSystemNote(ByteView data) {
  // If it's already been collected, then ignore new data.
  if (system_.IsObject()) {
    return fit::ok();
  }

  std::string_view sv{reinterpret_cast<const char*>(data.data()), data.size()};
  StringViewStream stream{sv};
  system_.ParseStream(stream);

  return fit::ok();
}

const rapidjson::Value* TaskHolder::JobTree::GetSystemJsonData(const char* key) const {
  if (system_.IsObject()) {
    auto it = system_.FindMember(key);
    if (it != system_.MemberEnd()) {
      return &it->value;
    }
  }
  return nullptr;
}

template <>
std::string_view TaskHolder::JobTree::GetSystemData<std::string_view>(const char* key) const {
  const rapidjson::Value* value = GetSystemJsonData(key);
  if (!value || !value->IsString()) {
    return {};
  }
  return {value->GetString(), value->GetStringLength()};
}

template <>
uint32_t TaskHolder::JobTree::GetSystemData<uint32_t>(const char* key) const {
  const rapidjson::Value* value = GetSystemJsonData(key);
  return !value              ? 0
         : value->IsUint()   ? value->GetUint()
         : value->IsNumber() ? static_cast<uint32_t>(value->GetDouble())
                             : 0;
}

template <>
uint64_t TaskHolder::JobTree::GetSystemData<uint64_t>(const char* key) const {
  const rapidjson::Value* value = GetSystemJsonData(key);
  return !value              ? 0
         : value->IsUint64() ? value->GetUint64()
         : value->IsNumber() ? static_cast<uint64_t>(value->GetDouble())
                             : 0;
}

std::string_view TaskHolder::system_get_version_string() const {
  return tree_->GetSystemData<std::string_view>("version_string");
}

uint32_t TaskHolder::system_get_dcache_line_size() const {
  return tree_->GetSystemData<uint32_t>("dcache_line_size");
}

uint32_t TaskHolder::system_get_num_cpus() const {
  return tree_->GetSystemData<uint32_t>("num_cpus");
}

uint64_t TaskHolder::JobTree::system_get_page_size() const {
  return GetSystemData<uint64_t>("page_size");
}

uint64_t TaskHolder::system_get_page_size() const { return tree_->system_get_page_size(); }

uint64_t TaskHolder::system_get_physmem() const {
  return tree_->GetSystemData<uint64_t>("physmem");
}

#ifdef __Fuchsia__
fit::result<Error> TaskHolder::InsertSystem() { return tree_->InsertSystem(); }
#endif

fit::result<Error, Buffer<>> Process::ReadLiveMemory(uint64_t vaddr, size_t size,
                                                     ReadMemorySize size_mode) {
  if (!live_memory_) {
    live_memory_ = std::make_unique<LiveMemory>(tree().live_memory_cache());
  }
  return live_memory_->ReadLiveMemory(vaddr, size, size_mode, live(), tree().live_memory_cache());
}

fit::result<Error, std::reference_wrapper<Task>> Job::get_child(zx_koid_t koid) {
  return tree().GetChild<&Job::children_, &Job::processes_>(*this, koid);
}

fit::result<Error, std::reference_wrapper<Thread>> Process::get_child(zx_koid_t koid) {
  return tree().GetChild<&Process::threads_>(*this, koid);
}

fit::result<Error, std::reference_wrapper<Task>> Job::find(zx_koid_t match) {
  if (koid() == match) {
    return fit::ok(std::ref(*this));
  }

  return tree().FindChild(*this, match);
}

}  // namespace zxdump
