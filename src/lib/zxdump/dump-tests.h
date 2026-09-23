// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ZXDUMP_DUMP_TESTS_H_
#define SRC_LIB_ZXDUMP_DUMP_TESTS_H_

#include <lib/fdio/spawn.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/vmar.h>
#include <lib/zxdump/dump.h>
#include <lib/zxdump/fd-writer.h>
#include <lib/zxdump/task.h>
#include <lib/zxdump/zstd-writer.h>
#include <unistd.h>
#include <zircon/status.h>

#include <array>
#include <cstdio>
#include <functional>
#include <string_view>
#include <vector>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace zxdump::testing {

constexpr time_t kNoDate = 0;           // Value for no date recorded.
constexpr time_t kTestDate = 74697240;  // Long, long ago.

// A simple test program starts up and waits.
class TestProcess {
 public:
  static constexpr std::string_view kDsoSoname = "libzxdump-test-child-dso.so";

  zx::unowned_process borrow() const { return zx::unowned_process{process_}; }

  LiveHandle handle() const {
    zx::process dup;
    EXPECT_EQ(ZX_OK, process_.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup));
    return dup;
  }

  ~TestProcess() {
    if (process_) {
      EXPECT_EQ(ZX_OK, process_.kill());
    }
    if (job_ && kill_job_) {
      EXPECT_EQ(ZX_OK, job_.kill());
    }
  }

  TestProcess& SpawnAction(const fdio_spawn_action_t& action) {
    spawn_actions_.push_back(action);
    return *this;
  }

  void StartChild(std::vector<const char*> args = {}) {
    args.insert(args.begin(), kChildProgram);
    args.push_back(nullptr);
    ASSERT_FALSE(*std::prev(args.end()));
    char err_msg[FDIO_SPAWN_ERR_MSG_MAX_LENGTH] = "";
    ASSERT_EQ(fdio_spawn_etc(job().get(), FDIO_SPAWN_CLONE_ALL, kChildProgram, args.data(), environ,
                             spawn_actions_.size(), spawn_actions_.data(),
                             process_.reset_and_get_address(), err_msg),
              ZX_OK)
        << err_msg;
  }

  zx::vmar CreateProcess(std::string_view name) {
    zx::vmar vmar;
    zx_status_t status = zx::process::create(job(), name.data(), static_cast<uint32_t>(name.size()),
                                             0, &process_, &vmar);
    EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
    return vmar;
  }

  const zx::process& process() const { return process_; }

  void AddToTaskHolder(TaskHolder& holder, Process*& holder_process) {
    zx::process proc;
    ASSERT_EQ(process_.duplicate(ZX_RIGHT_SAME_RIGHTS, &proc), ZX_OK);
    auto result = holder.Insert(std::move(proc));
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    zxdump::Object& object = *result;
    ASSERT_EQ(object.type(), ZX_OBJ_TYPE_PROCESS);
    holder_process = &static_cast<zxdump::Process&>(object);
  }

  zx_koid_t koid() const { return GetKoid(process_); }

  // Explicitly choose the job to use.
  void set_job(zx::job&& job, bool kill_job = false) {
    job_ = std::move(job);
    kill_job_ = kill_job;
  }
  void set_job(const zx::job& job) { ASSERT_EQ(ZX_OK, job.duplicate(ZX_RIGHT_SAME_RIGHTS, &job_)); }

  // This returns the job StartChild will launch the test process in.
  // If set_job hasn't been called, it just uses the default job.
  const zx::job& job() {
    if (!job_) {
      return *default_job_;
    }
    return job_;
  }

  zx_koid_t job_koid() const { return GetKoid(job_); }

  // Create a new empty job and set_job() to that.
  void HermeticJob(const zx::job& parent = *zx::job::default_job()) {
    ASSERT_FALSE(job_);
    ASSERT_EQ(ZX_OK, zx::job::create(parent, 0, &job_));
    kill_job_ = true;
  }

  // This is a standard SegmentCallback that can be used.
  static fit::result<zxdump::Error, zxdump::SegmentDisposition> PruneAllMemory(
      zxdump::SegmentDisposition segment, const zx_info_maps_t& maps, const zx_info_vmo_t& vmo) {
    segment.filesz = 0;
    return fit::ok(segment);
  }

  static fit::result<zxdump::Error, zxdump::SegmentDisposition> DumpAllMemory(
      zxdump::SegmentDisposition segment, const zx_info_maps_t& maps, const zx_info_vmo_t& vmo) {
    return fit::ok(segment);
  }

 private:
  static constexpr const char* kChildProgram = "/pkg/bin/zxdump-test-child";

  template <typename Handle>
  static zx_koid_t GetKoid(const Handle& object) {
    zx_info_handle_basic_t info = {.koid = ZX_KOID_INVALID};
    EXPECT_EQ(ZX_OK, object.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    return info.koid;
  }

  zx::unowned_job default_job_ = zx::job::default_job();
  std::vector<fdio_spawn_action_t> spawn_actions_;
  zx::process process_;
  zx::job job_;
  bool kill_job_ = false;
};

using PrecollectFunction =
    fit::function<void(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump)>;

class TestProcessForPropertiesAndInfo : public TestProcess {
 public:
  // Start a child for basic property & info dump testing.
  void StartChild();

  static void NoPrecollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump) {}

  // Do the basic dump using the dumper API.
  template <typename Writer>
  void Dump(Writer& writer, PrecollectFunction precollect = NoPrecollect,
            SegmentCallback prune = nullptr);

  // Verify a dump file for that child was inserted and looks right.
  void CheckDump(zxdump::TaskHolder& holder, bool threads_dumped = true);

 private:
  static constexpr const char* kChildName = "zxdump-property-test-child";
};

// The template and its instantiations are defined in dump-tests.cc.
extern template void TestProcessForPropertiesAndInfo::Dump(FdWriter&, PrecollectFunction,
                                                           SegmentCallback);
extern template void TestProcessForPropertiesAndInfo::Dump(ZstdWriter&, PrecollectFunction,
                                                           SegmentCallback);

class TestProcessForSystemInfo : public TestProcessForPropertiesAndInfo {
 public:
  // Start a child for system information dump testing.
  void StartChild();

  // Do the basic dump using the dumper API.
  template <typename Writer>
  void Dump(Writer& writer) {
    TestProcessForPropertiesAndInfo::Dump(
        writer, std::bind_front(&TestProcessForSystemInfo::Precollect, this));
  }

  // Verify a dump file for that child was inserted and looks right.
  void CheckDump(zxdump::TaskHolder& holder);

 private:
  static constexpr const char* kChildName = "zxdump-system-test-child";

  void Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump);

  zxdump::TaskHolder live_holder_;
};

class TestProcessForKernelInfo : public TestProcessForPropertiesAndInfo {
 public:
  // Start a child for privileged kernel information dump testing.
  void StartChild();

  // Do the basic dump using the dumper API.
  template <typename Writer>
  void Dump(Writer& writer) {
    TestProcessForPropertiesAndInfo::Dump(
        writer, std::bind_front(&TestProcessForKernelInfo::Precollect, this));
  }

  // Verify a dump file for that child was inserted and looks right.
  void CheckDump(zxdump::TaskHolder& holder);

  const LiveHandle& info_resource() const { return info_resource_; }

 private:
  static constexpr const char* kChildName = "zxdump-kernel-test-child";

  void Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump);

  LiveHandle info_resource_;
};

class TestProcessForRemarks : public TestProcessForPropertiesAndInfo {
 public:
  static constexpr std::string_view kTestRemarksNameNoSuffix = "teststuff";
  static constexpr std::string_view kTextRemarksName = "teststuff.txt";
  static constexpr std::string_view kDefaultRemarksName = "remarks.txt";
  static constexpr std::string_view kTextRemarksData = "insert remark here";
  static constexpr std::string_view kBinaryRemarksName = "teststuff.bin";
  static constexpr std::array kBinaryTestData = {
      std::byte{1},
      std::byte{2},
      std::byte{3},
  };
  static constexpr ByteView kBinaryRemarksData{
      kBinaryTestData.data(),
      kBinaryTestData.size(),
  };
  static constexpr std::string_view kJsonRemarksName = "teststuff.json";
  static constexpr std::string_view kDefaultJsonRemarksName = "remarks.json";
  static constexpr std::string_view kRawJsonRemarksData = R"""({ "foo": [ 1, 3 ] })""";
  static constexpr std::string_view kNormalizedJsonRemarksData = R"""({"foo":[1,3]})""";

  // Start a child for dump remarks testing.
  void StartChild();

  // Do the basic dump using the dumper API.
  template <typename Writer>
  void Dump(Writer& writer) {
    TestProcessForPropertiesAndInfo::Dump(writer, Precollect);
  }

  // Verify a dump file for that child was inserted and looks right.
  void CheckDump(zxdump::TaskHolder& holder);

  static std::string_view AsString(ByteView bytes) {
    return {reinterpret_cast<const char*>(bytes.data()), bytes.size()};
  }

 private:
  static constexpr const char* kChildName = "zxdump-remarks-test-child";

  static void Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump);
};

class TestProcessForMemory : public TestProcessForPropertiesAndInfo {
 public:
  // This is the test data made present in the child's memory by StartChild.
  static constexpr std::string_view kMemoryText = "in the course of human events";
  static constexpr std::wstring_view kMemoryWideText = L"in the course of human events";
  static constexpr std::array kMemoryInts = {17, 23, 42, 55, 66};

  // Start a child for basic memory dump testing.
  void StartChild();

  // Do the full-memory dump using the dumper API.
  template <typename Writer>
  void Dump(Writer& writer, SegmentCallback prune = DumpAllMemory) {
    TestProcessForPropertiesAndInfo::Dump(writer, NoPrecollect, std::move(prune));
  }

  // Verify a dump file for that child was inserted and looks right.
  void CheckDump(zxdump::TaskHolder& holder, bool memory_elided = false);

  // These give the addresses where the test data is found in the child.

  uint64_t text_ptr() const { return text_ptr_; }

  uint64_t ints_ptr() const { return ints_ptr_; }

  uint64_t wtext_ptr() const { return wtext_ptr_; }

  uint64_t pages_ptr() const { return pages_ptr_; }

 private:
  static constexpr const char* kChildName = "zxdump-memory-test-child";

  uint64_t text_ptr_ = 0;
  uint64_t ints_ptr_ = 0;
  uint64_t wtext_ptr_ = 0;
  uint64_t pages_ptr_ = 0;
};

class TestProcessForThreads : public TestProcessForPropertiesAndInfo {
 public:
  static constexpr size_t kThreadCount = 5;

  // Start a child for thread dump testing.
  void StartChild();

  // Do the basic dump using the dumper API.
  template <typename Writer>
  void Dump(Writer& writer) {
    TestProcessForPropertiesAndInfo::Dump(writer, Precollect);
  }

  // Verify a dump file for that child was inserted and looks right.
  void CheckDump(zxdump::TaskHolder& holder);

  std::span<const zx_koid_t, kThreadCount> thread_koids() const { return thread_koids_; }

 private:
  static constexpr const char* kChildName = "zxdump-thread-test-child";

  static void Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump);

  std::array<zx_koid_t, kThreadCount> thread_koids_ = {};
};

class TestProcessForThreadState : public TestProcessForPropertiesAndInfo {
 public:
  static constexpr size_t kThreadCount = 5;

  // Start a child for thread state dump testing.
  void StartChild();

  // Do the basic dump using the dumper API.
  template <typename Writer>
  void Dump(Writer& writer) {
    TestProcessForPropertiesAndInfo::Dump(writer, Precollect);
  }

  // Verify a dump file for that child was inserted and looks right.
  void CheckDump(zxdump::TaskHolder& holder);

  std::span<const zx_koid_t, kThreadCount> thread_koids() const { return thread_koids_; }

 private:
  static constexpr const char* kChildName = "zxdump-thread-state-test-child";

  static constexpr uint64_t kRegisterValue = 0x1234d00df00d8765;

  static void Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump);

  std::array<zx_koid_t, kThreadCount> thread_koids_ = {};
};

class TestProcessForElfSearch : public zxdump::testing::TestProcessForPropertiesAndInfo {
 public:
  using ByteVector = std::vector<std::byte>;

  struct ElfId {
    constexpr auto operator<=>(const ElfId&) const = default;

    ByteVector build_id;
    std::string soname;
  };

  using ElfIdList = std::vector<ElfId>;

  struct ElfIdAtBase : public ElfId {
    constexpr ElfIdAtBase(ElfId id, uint64_t id_base) : ElfId{std::move(id)}, base{id_base} {}

    constexpr auto operator<=>(const ElfIdAtBase&) const = default;

    uint64_t base;
  };

  using ElfIdAtBaseList = std::vector<ElfIdAtBase>;

  static constexpr const char* kChildName = "zxdump-elf-search-test-child";

  // Start a child for ELF search testing.
  void StartChild();

  // Create a process for remote dynamic linking, used instead of StartChild().
  zx::vmar CreateProcess();

  // Do the full-memory dump using the dumper API.
  template <typename Writer>
  void Dump(Writer& writer) {
    zxdump::testing::TestProcessForPropertiesAndInfo::Dump(
        writer, std::bind_front(&TestProcessForElfSearch::Precollect, this),
        std::bind_front(&TestProcessForElfSearch::DumpAllMemoryWithBuildIds, this));
  }

  // Verify a dump file for that child was inserted and looks right.
  // Then found_elf() will return the modules found.
  void CheckDump(zxdump::TaskHolder& holder);

  // Verify the build ID PT_NOTEs in the ET_CORE file directly.
  // Then found_elf() will return the modules found.
  void CheckNotes(int fd);

  // Check for CheckDump / CheckNotes finding canonical expected modules.
  void CheckDumpElfSearchIds() const;
  void CheckNotesElfSearchIds() const;

  // These give the addresses where the modules were reported in the child.

  uint64_t main_ptr() const { return main_ptr_; }

  uint64_t dso_ptr() const { return dso_ptr_; }

  const ElfIdAtBaseList& found_elf() const { return found_elf_; }

  ElfIdList found_ids() const {
    // gmock container matchers don't like ranges/views so construct a
    // temporary vector instead of just a std::views::transform range adapter.
    return ElfIdList{
        found_elf_.begin(),
        found_elf_.end(),
    };
  }

 private:
  void Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump);

  fit::result<zxdump::Error, zxdump::SegmentDisposition> DumpAllMemoryWithBuildIds(
      zxdump::SegmentDisposition segment, const zx_info_maps_t& maps, const zx_info_vmo_t& vmo);

  zxdump::ProcessDump* dump_ = nullptr;
  uint64_t dso_ptr_ = 0;
  uint64_t main_ptr_ = 0;
  ElfIdAtBaseList found_elf_;
};

inline std::ostream& operator<<(std::ostream& os, std::span<const std::byte> bytes) {
  for (std::byte byte : bytes) {
    char buf[3];
    snprintf(buf, sizeof(buf), "%02x", static_cast<unsigned int>(byte));
    os << buf;
  }
  return os;
}

inline std::ostream& operator<<(std::ostream& os, const TestProcessForElfSearch::ElfId& id) {
  if (id.soname.empty()) {
    return os << "\n    { build ID: " << id.build_id << ", no SONAME }";
  }
  return os << "\n    { build ID: " << id.build_id << ", SONAME: \"" << id.soname << "\" }";
}

inline std::ostream& operator<<(std::ostream& os, const TestProcessForElfSearch::ElfIdAtBase& id) {
  return os << static_cast<const TestProcessForElfSearch::ElfId&>(id) <<  //
         "\n        @ " << std::showbase << std::hex << id.base;
}

}  // namespace zxdump::testing

#endif  // SRC_LIB_ZXDUMP_DUMP_TESTS_H_
