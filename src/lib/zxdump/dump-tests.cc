// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dump-tests.h"

#include <lib/fit/defer.h>
#include <lib/zxdump/dump.h>
#include <lib/zxdump/fd-writer.h>
#include <lib/zxdump/task.h>
#include <lib/zxdump/zstd-writer.h>
#include <unistd.h>
#include <zircon/status.h>

#include <array>
#include <cinttypes>
#include <cstdio>
#include <string_view>
#include <type_traits>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>

#include "rights.h"
#include "test-data-holder.h"
#include "test-file.h"
#include "test-tool-process.h"

// The dump format is complex enough that direct testing of output data would
// be tantamount to reimplementing the reader, and golden binary files aren't
// easy to match up with fresh data from a live system where all the KOID and
// statistics values will be different every time.  So the main method used to
// test the dumper is via end-to-end tests that dump into a file via the dumper
// API, read the dump back using the reader API, and then compare the data from
// the dump to the data from the original live tasks.

namespace zxdump::testing {

using namespace std::literals;

using ::testing::Contains;
using ::testing::FieldsAre;
using ::testing::UnorderedElementsAreArray;

void TestProcessForPropertiesAndInfo::StartChild() {
  SpawnAction({
      .action = FDIO_SPAWN_ACTION_SET_NAME,
      .name = {kChildName},
  });
  ASSERT_NO_FATAL_FAILURE(TestProcess::StartChild());
}

template <typename Writer>
void TestProcessForPropertiesAndInfo::Dump(Writer& writer, PrecollectFunction precollect,
                                           SegmentCallback prune) {
  const bool dump_memory = prune != nullptr;
  if (!prune) {
    prune = PruneAllMemory;
  }

  zxdump::TaskHolder holder;
  auto insert_result = holder.Insert(handle());
  ASSERT_TRUE(insert_result.is_ok()) << insert_result.error_value();
  ASSERT_EQ(insert_result->get().type(), ZX_OBJ_TYPE_PROCESS);
  zxdump::ProcessDump dump(static_cast<zxdump::Process&>(insert_result->get()));

  ASSERT_NO_FATAL_FAILURE(precollect(holder, dump));

  auto collect_result = dump.CollectProcess(std::move(prune));
  ASSERT_TRUE(collect_result.is_ok()) << collect_result.error_value();

  auto dump_result = dump.DumpHeaders(writer.AccumulateFragmentsCallback());
  ASSERT_TRUE(dump_result.is_ok()) << dump_result.error_value();

  auto write_result = writer.WriteFragments();
  ASSERT_TRUE(write_result.is_ok()) << write_result.error_value();
  const size_t bytes_written = write_result.value();

  auto memory_result = dump.DumpMemory(writer.WriteCallback());
  ASSERT_TRUE(memory_result.is_ok()) << memory_result.error_value();
  const size_t total_with_memory = memory_result.value();

  if (dump_memory) {
    // Dumping the memory should have added a bunch to the dump.
    EXPECT_LT(bytes_written, total_with_memory);
  } else {
    // We pruned all memory, so DumpMemory should not have added any output.
    EXPECT_EQ(bytes_written, total_with_memory);
  }
}

template void TestProcessForPropertiesAndInfo::Dump(FdWriter&, PrecollectFunction, SegmentCallback);
template void TestProcessForPropertiesAndInfo::Dump(ZstdWriter&, PrecollectFunction,
                                                    SegmentCallback);

void TestProcessForPropertiesAndInfo::CheckDump(zxdump::TaskHolder& holder, bool threads_dumped) {
  auto find_result = holder.root_job().find(koid());
  ASSERT_TRUE(find_result.is_ok()) << find_result.error_value();

  ASSERT_EQ(find_result->get().type(), ZX_OBJ_TYPE_PROCESS);
  zxdump::Process& read_process = static_cast<zxdump::Process&>(find_result->get());

  {
    auto name_result = read_process.get_property<ZX_PROP_NAME>();
    ASSERT_TRUE(name_result.is_ok()) << name_result.error_value();
    std::string_view name(name_result->data(), name_result->size());
    name = name.substr(0, name.find_first_of('\0'));
    EXPECT_EQ(name, std::string_view(kChildName));
  }

  {
    auto threads_result = read_process.get_info<ZX_INFO_PROCESS_THREADS>();
    ASSERT_TRUE(threads_result.is_ok()) << threads_result.error_value();
    EXPECT_EQ(threads_result->size(), size_t{1});
  }

  // Even though ZX_INFO_PROCESS_THREADS is present, threads() only
  // returns anything if the threads were actually dumped.
  {
    auto threads_result = read_process.threads();
    ASSERT_TRUE(threads_result.is_ok()) << threads_result.error_value();
    if (threads_dumped) {
      EXPECT_EQ(threads_result->get().size(), size_t{1});
    } else {
      EXPECT_EQ(threads_result->get().size(), size_t{0});
    }
  }

  {
    auto info_result = read_process.get_info<ZX_INFO_HANDLE_BASIC>();
    ASSERT_TRUE(info_result.is_ok()) << info_result.error_value();
    EXPECT_EQ(info_result->type, ZX_OBJ_TYPE_PROCESS);
    EXPECT_EQ(info_result->koid, koid());
  }
}

void TestProcessForSystemInfo::StartChild() {
  SpawnAction({
      .action = FDIO_SPAWN_ACTION_SET_NAME,
      .name = {kChildName},
  });
  ASSERT_NO_FATAL_FAILURE(TestProcess::StartChild());

  auto result = live_holder_.InsertSystem();
  EXPECT_TRUE(result.is_ok()) << result.error_value();
}

void TestProcessForSystemInfo::Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump) {
  auto result = dump.CollectSystem(live_holder_);
  ASSERT_TRUE(result.is_ok()) << result.error_value();
}

void TestProcessForSystemInfo::CheckDump(zxdump::TaskHolder& holder) {
  EXPECT_EQ(holder.system_get_dcache_line_size(), zx_system_get_dcache_line_size());
  EXPECT_EQ(holder.system_get_num_cpus(), zx_system_get_num_cpus());
  EXPECT_EQ(holder.system_get_page_size(), zx_system_get_page_size());
  EXPECT_EQ(holder.system_get_physmem(), zx_system_get_physmem());

  std::string_view version = zx_system_get_version_string();
  EXPECT_EQ(holder.system_get_version_string(), version);
}

void TestProcessForKernelInfo::StartChild() {
  SpawnAction({
      .action = FDIO_SPAWN_ACTION_SET_NAME,
      .name = {kChildName},
  });
  ASSERT_NO_FATAL_FAILURE(TestProcess::StartChild());

  // Fetch the info resource, since we'll need it to dump.
  auto info_result = zxdump::GetInfoResource();
  EXPECT_TRUE(info_result.is_ok()) << info_result.error_value();
  info_resource_ = *std::move(info_result);
}

void TestProcessForKernelInfo::Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump) {
  zxdump::LiveHandle info_resource_copy;
  EXPECT_EQ(ZX_OK, info_resource().duplicate(ZX_RIGHT_SAME_RIGHTS, &info_resource_copy));
  auto insert_result = holder.Insert(std::move(info_resource_copy));
  EXPECT_TRUE(insert_result.is_ok()) << insert_result.error_value();

  auto result = dump.CollectKernel();
  EXPECT_TRUE(result.is_ok()) << result.error_value();
}

void TestProcessForKernelInfo::CheckDump(zxdump::TaskHolder& holder) {
  using KernelData = TestDataHolder<   //
      InfoTraits<ZX_INFO_CPU_STATS>,   //
      InfoTraits<ZX_INFO_KMEM_STATS>,  //
      InfoTraits<ZX_INFO_GUEST_STATS>>;

  zxdump::Resource& info = holder.info_resource();
  EXPECT_NE(info.koid(), ZX_KOID_INVALID);
  EXPECT_EQ(info.type(), ZX_OBJ_TYPE_RESOURCE);

  KernelData dump_data, live_data;

  {
    // Use a fresh holder to populate the live data.  It can consume the info
    // resource handle we used in Precollect, since we've already dumped and
    // don't need it any more.
    zxdump::TaskHolder live_holder;
    auto live_info = live_holder.Insert(std::move(info_resource_));
    ASSERT_TRUE(live_info.is_ok()) << live_info.error_value();
    ASSERT_NO_FATAL_FAILURE(live_data.Fill(*live_info));
  }

  // Fetch all the data from the dump.
  ASSERT_NO_FATAL_FAILURE(dump_data.Fill(info));

  // Check that the dump data makes sense as data collected before the live
  // data just collected (after the dump was made).
  dump_data.Check(live_data);
}

void TestProcessForRemarks::StartChild() {
  SpawnAction({
      .action = FDIO_SPAWN_ACTION_SET_NAME,
      .name = {kChildName},
  });
  ASSERT_NO_FATAL_FAILURE(TestProcess::StartChild());
}

void TestProcessForRemarks::Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump) {
  {
    auto result = dump.Remarks(kDefaultRemarksName, kTextRemarksData);
    EXPECT_TRUE(result.is_ok()) << result.error_value();
  }
  {
    auto result = dump.Remarks(kTextRemarksName, kTextRemarksData);
    EXPECT_TRUE(result.is_ok()) << result.error_value();
  }
  {
    auto result = dump.Remarks(kBinaryRemarksName, kBinaryRemarksData);
    EXPECT_TRUE(result.is_ok()) << result.error_value();
  }
  {
    auto result = dump.Remarks(kDefaultJsonRemarksName, kNormalizedJsonRemarksData);
    EXPECT_TRUE(result.is_ok()) << result.error_value();
  }
  {
    auto result = dump.Remarks(kJsonRemarksName, kNormalizedJsonRemarksData);
    EXPECT_TRUE(result.is_ok()) << result.error_value();
  }
}

void TestProcessForRemarks::CheckDump(zxdump::TaskHolder& holder) {
  auto find_result = holder.root_job().find(koid());
  ASSERT_TRUE(find_result.is_ok()) << find_result.error_value();

  ASSERT_EQ(find_result->get().type(), ZX_OBJ_TYPE_PROCESS);
  zxdump::Process& read_process = static_cast<zxdump::Process&>(find_result->get());

  const auto& remarks = read_process.remarks();
  EXPECT_EQ(remarks.size(), 5u);
  size_t n = 0;
  for (const auto& [name, remark] : remarks) {
    switch (n++) {
      case 0:
        EXPECT_EQ(name, kDefaultRemarksName);
        EXPECT_EQ(AsString(remark), kTextRemarksData);
        break;
      case 1:
        EXPECT_EQ(name, kTextRemarksName);
        EXPECT_EQ(AsString(remark), kTextRemarksData);
        break;
      case 2:
        EXPECT_EQ(name, kBinaryRemarksName);
        EXPECT_THAT(remark, ::testing::ElementsAreArray(kBinaryRemarksData));
        break;
      case 3:
        EXPECT_EQ(name, kDefaultJsonRemarksName);
        EXPECT_EQ(AsString(remark), kNormalizedJsonRemarksData);
        break;
      case 4:
        EXPECT_EQ(name, kJsonRemarksName);
        EXPECT_EQ(AsString(remark), kNormalizedJsonRemarksData);
        break;
      default:
        FAIL() << "too many remarks";
        break;
    }
  }
}

std::string IntsString(std::span<const int> ints) {
  std::string str;
  for (int i : ints) {
    if (!str.empty()) {
      str += ',';
    }
    str += std::to_string(i);
  }
  return str;
}

void TestProcessForMemory::StartChild() {
  SpawnAction({
      .action = FDIO_SPAWN_ACTION_SET_NAME,
      .name = {kChildName},
  });

  fbl::unique_fd read_pipe;
  {
    int pipe_fd[2];
    ASSERT_EQ(0, pipe(pipe_fd)) << strerror(errno);
    read_pipe.reset(pipe_fd[STDIN_FILENO]);
    SpawnAction({
        .action = FDIO_SPAWN_ACTION_TRANSFER_FD,
        .fd = {.local_fd = pipe_fd[STDOUT_FILENO], .target_fd = STDOUT_FILENO},
    });
  }

  const std::array<int, 2> memory_sizes = {
      2 * static_cast<int>(zx_system_get_page_size()), /* allocated pages */
      static_cast<int>(zx_system_get_page_size()) /* reserved pages */,
  };
  ASSERT_NO_FATAL_FAILURE(TestProcess::StartChild({
      "-m",
      kMemoryText.data(),
      "-M",
      IntsString(std::span(kMemoryInts)).c_str(),
      "-w",
      kMemoryText.data(),
      "-p",
      IntsString(std::span(memory_sizes)).c_str(),
  }));

  // The test-child wrote the pointers where the -m text and -M int array
  // appear in its memory.  Reading these immediately synchronizes with the
  // child having started up and progressed far enough to have this memory in
  // place before the process gets dumped.
  FILE* pipef = fdopen(read_pipe.get(), "r");
  ASSERT_TRUE(pipef) << "fdopen: " << read_pipe.get() << strerror(errno);
  auto close_pipef = fit::defer([pipef]() { fclose(pipef); });
  std::ignore = read_pipe.release();

  ASSERT_EQ(4, fscanf(pipef, "%" SCNx64 "\n%" SCNx64 "\n%" SCNx64 "\n%" SCNx64, &text_ptr_,
                      &ints_ptr_, &wtext_ptr_, &pages_ptr_));
}

void TestProcessForMemory::CheckDump(zxdump::TaskHolder& holder, bool memory_elided) {
  auto find_result = holder.root_job().find(koid());
  ASSERT_TRUE(find_result.is_ok()) << find_result.error_value();

  ASSERT_EQ(find_result->get().type(), ZX_OBJ_TYPE_PROCESS);
  zxdump::Process& read_process = static_cast<zxdump::Process&>(find_result->get());

  {
    auto name_result = read_process.get_property<ZX_PROP_NAME>();
    ASSERT_TRUE(name_result.is_ok()) << name_result.error_value();
    std::string_view name(name_result->data(), name_result->size());
    name = name.substr(0, name.find_first_of('\0'));
    EXPECT_EQ(name, std::string_view(kChildName));
  }

  // Basic test.
  {
    auto memory_result = read_process.read_memory<char>(text_ptr_, kMemoryText.size());
    ASSERT_TRUE(memory_result.is_ok())
        << memory_result.error_value() << " reading 0x" << std::hex << text_ptr_;
    if (memory_elided) {
      EXPECT_TRUE(memory_result->empty()) << " read " << memory_result->size_bytes();
    } else {
      std::string_view text{(memory_result->data()), memory_result->size()};
      ASSERT_EQ(text.size(), kMemoryText.size())
          << " reading 0x" << std::hex << text_ptr_ << " copied at "
          << static_cast<const void*>(text.data());
      EXPECT_EQ(text, kMemoryText)  //
          << " reading 0x" << std::hex << text_ptr_ << " copied at "
          << static_cast<const void*>(text.data());
    }
  }

  // Test with a non-byte-sized type.
  {
    auto memory_result = read_process.read_memory<int>(ints_ptr_, kMemoryInts.size());
    ASSERT_TRUE(memory_result.is_ok())
        << memory_result.error_value() << " reading 0x" << std::hex << ints_ptr_;
    std::span ints = **memory_result;
    if (memory_elided) {
      EXPECT_TRUE(ints.empty()) << " read " << ints.size_bytes();
    } else {
      static_assert(std::is_same_v<const int, decltype(ints)::element_type>);
      ASSERT_EQ(ints.size(), kMemoryInts.size());
      for (size_t i = 0; i < kMemoryInts.size(); ++i) {
        EXPECT_EQ(ints[i], kMemoryInts[i]);
      }
    }
  }

  // Readahead test.
  {
    // Only ask to read half the string's actual size, so there will definitely
    // be more than that available in the dump.
    auto memory_result =
        read_process.read_memory<char>(text_ptr_, kMemoryText.size() / 2, ReadMemorySize::kMore);
    ASSERT_TRUE(memory_result.is_ok()) << memory_result.error_value();
    if (memory_elided) {
      EXPECT_TRUE(memory_result->empty()) << " read " << memory_result->size_bytes();
    } else {
      std::string_view text{(memory_result->data()), memory_result->size()};
      // Even if the whole string ended on a page boundary, that much (which we
      // know is more than the minimum requested) will be available.
      ASSERT_GE(text.size(), kMemoryText.size());
      EXPECT_TRUE(text.starts_with(kMemoryText));
    }
  }

  // Test a read crossing a page boundary.
  auto test_memory_pages = [memory_elided](uint64_t ptr, size_t sample_size,
                                           std::span<const uint8_t> contents) {
    if (memory_elided) {
      EXPECT_TRUE(contents.empty()) << " read " << contents.size_bytes();
    } else {
      ASSERT_GE(contents.size(), sample_size);
      for (size_t i = 0; i < sample_size; ++i) {
        const unsigned int actual = contents[i];
        const unsigned int expected = static_cast<uint8_t>(ptr + i);
        EXPECT_EQ(actual, expected) << i << " of " << sample_size << " at " << std::hex << ptr + i;
      }
    }
  };

  {
    constexpr size_t kSampleSize = 20;
    ASSERT_TRUE(pages_ptr_ % zx_system_get_page_size() == 0) << std::hex << pages_ptr_;
    const uint64_t ptr = pages_ptr_ + zx_system_get_page_size() - (kSampleSize / 2);
    auto memory_result = read_process.read_memory<uint8_t>(ptr, kSampleSize);
    ASSERT_TRUE(memory_result.is_ok()) << memory_result.error_value();
    ASSERT_NO_FATAL_FAILURE(test_memory_pages(ptr, kSampleSize, **memory_result));
    if (!memory_elided) {
      EXPECT_EQ(memory_result->size(), kSampleSize);
    }
  }

  // Test that reading the non-allocated page returns either an error or zero bytes.
  {
    constexpr size_t kSampleSize = 20;
    ASSERT_TRUE(pages_ptr_ % zx_system_get_page_size() == 0) << std::hex << pages_ptr_;
    const uint64_t ptr = pages_ptr_ + 2 * zx_system_get_page_size();
    auto memory_result = read_process.read_memory<uint8_t>(ptr, kSampleSize);
    if (read_process.is_live()) {
      EXPECT_TRUE(memory_result.is_error()) << "Read " << memory_result->size_bytes() << " bytes";
    } else {
      ASSERT_TRUE(memory_result.is_ok()) << memory_result.error_value();
      EXPECT_EQ(memory_result->size(), 0u);
    }
  }

  // Test a read that can return less than requested.
  {
    constexpr size_t kSampleSize = 20;
    const uint64_t ptr = pages_ptr_ + zx_system_get_page_size() - (kSampleSize / 2);
    auto memory_result = read_process.read_memory<uint8_t>(ptr, kSampleSize, ReadMemorySize::kLess);
    ASSERT_TRUE(memory_result.is_ok()) << memory_result.error_value();
    const size_t sample_size = std::min(kSampleSize, memory_result->size());
    ASSERT_NO_FATAL_FAILURE(test_memory_pages(ptr, sample_size, **memory_result));
    if (!memory_elided) {
      if (read_process.is_live()) {
        // A live read should have been truncated to keep it in the one page.
        EXPECT_EQ(sample_size, kSampleSize / 2);
      } else {
        // Reading a dump always has all the data if it wasn't elided: if it's
        // an mmap'd file, it's all on hand; if it's another kind of dump, the
        // data is being copied anyway so there's no benefit to returning less.
        EXPECT_EQ(sample_size, kSampleSize);
      }
    }
  }
}

void TestProcessForThreads::StartChild() {
  SpawnAction({
      .action = FDIO_SPAWN_ACTION_SET_NAME,
      .name = {kChildName},
  });

  fbl::unique_fd read_pipe;
  {
    int pipe_fd[2];
    ASSERT_EQ(0, pipe(pipe_fd)) << strerror(errno);
    read_pipe.reset(pipe_fd[STDIN_FILENO]);
    SpawnAction({
        .action = FDIO_SPAWN_ACTION_TRANSFER_FD,
        .fd = {.local_fd = pipe_fd[STDOUT_FILENO], .target_fd = STDOUT_FILENO},
    });
  }

  ASSERT_NO_FATAL_FAILURE(TestProcess::StartChild({
      "-t",
      std::to_string(kThreadCount - 1).c_str(),
  }));

  // The test-child wrote the KOID for each thread.  Reading these immediately
  //  synchronizes with the child having started up and progressed far enough
  //  to have all the threads launched up place before the process gets dumped.
  FILE* pipef = fdopen(read_pipe.get(), "r");
  ASSERT_TRUE(pipef) << "fdopen: " << read_pipe.get() << strerror(errno);
  auto close_pipef = fit::defer([pipef]() { fclose(pipef); });
  std::ignore = read_pipe.release();

  for (zx_koid_t& koid : thread_koids_) {
    // scanf needs readahead and the child will hang after writing so don't
    // match the trailing \n explicitly; once it terminates each line it will
    // be implicitly skipped before the next as the leading space matches all
    // whitespace.  But the final \n will be just seen in the readahead and not
    // cause scanf to try to read any more from the pipe, which won't have any.
    ASSERT_EQ(1, fscanf(pipef, " %" SCNu64, &koid));
  }
}

void TestProcessForThreads::Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump) {
  auto result = dump.SuspendAndCollectThreads();
  EXPECT_TRUE(result.is_ok()) << result.error_value();
}

void TestProcessForThreads::CheckDump(zxdump::TaskHolder& holder) {
  auto find_result = holder.root_job().find(koid());
  ASSERT_TRUE(find_result.is_ok()) << find_result.error_value();

  ASSERT_EQ(find_result->get().type(), ZX_OBJ_TYPE_PROCESS);
  zxdump::Process& read_process = static_cast<zxdump::Process&>(find_result->get());

  auto list_result = read_process.get_info<ZX_INFO_PROCESS_THREADS>();
  ASSERT_TRUE(list_result.is_ok()) << list_result.error_value();
  EXPECT_THAT(*list_result, UnorderedElementsAreArray(thread_koids()));

  // Test get_child.
  std::map<zx_koid_t, zxdump::Object*> objects;
  std::map<zx_koid_t, zxdump::Thread*> threads;
  for (zx_koid_t koid : thread_koids()) {
    auto child_result = read_process.get_child(koid);
    ASSERT_TRUE(child_result.is_ok())
        << read_process.koid() << ".get_child(" << koid << ") -> " << child_result.error_value();
    zxdump::Object& child = *child_result;
    objects.emplace(koid, &child);
    ASSERT_EQ(child.type(), ZX_OBJ_TYPE_THREAD);
    zxdump::Thread& thread = static_cast<zxdump::Thread&>(child);
    threads.emplace(koid, &thread);
    EXPECT_EQ(thread.koid(), koid);
  }
  ASSERT_EQ(objects.size(), kThreadCount);
  ASSERT_EQ(threads.size(), kThreadCount);

  // Test find.
  for (auto [koid, object] : objects) {
    auto find_result = read_process.find(koid);
    ASSERT_TRUE(find_result.is_ok()) << find_result.error_value();
    zxdump::Object& child = *find_result;
    EXPECT_EQ(object, &child);
    EXPECT_EQ(child.type(), ZX_OBJ_TYPE_THREAD);
    EXPECT_EQ(child.koid(), koid);
  }

  // Test threads().
  auto threads_result = read_process.threads();
  ASSERT_TRUE(threads_result.is_ok()) << threads_result.error_value();
  EXPECT_EQ(threads_result->get().size(), threads.size());
  for (auto& [koid, thread] : threads_result->get()) {
    EXPECT_THAT(threads, Contains(FieldsAre(koid, &thread)));
  }
}

void TestProcessForThreadState::StartChild() {
  SpawnAction({
      .action = FDIO_SPAWN_ACTION_SET_NAME,
      .name = {kChildName},
  });

  fbl::unique_fd read_pipe;
  {
    int pipe_fd[2];
    ASSERT_EQ(0, pipe(pipe_fd)) << strerror(errno);
    read_pipe.reset(pipe_fd[STDIN_FILENO]);
    SpawnAction({
        .action = FDIO_SPAWN_ACTION_TRANSFER_FD,
        .fd = {.local_fd = pipe_fd[STDOUT_FILENO], .target_fd = STDOUT_FILENO},
    });
  }

  ASSERT_NO_FATAL_FAILURE(TestProcess::StartChild({
      "-t",
      std::to_string(kThreadCount - 1).c_str(),
      "-C",
      std::to_string(kRegisterValue).c_str(),
  }));

  // The test-child wrote the KOID for each thread.  Reading these immediately
  // synchronizes with the child having started up and progressed far enough to
  // have all the threads launched and crashed before the process gets dumped.
  FILE* pipef = fdopen(read_pipe.get(), "r");
  ASSERT_TRUE(pipef) << "fdopen: " << read_pipe.get() << strerror(errno);
  auto close_pipef = fit::defer([pipef]() { fclose(pipef); });
  std::ignore = read_pipe.release();

  for (zx_koid_t& koid : thread_koids_) {
    // scanf needs readahead and the child will hang after writing so don't
    // match the trailing \n explicitly; once it terminates each line it will
    // be implicitly skipped before the next as the leading space matches all
    // whitespace.  But the final \n will be just seen in the readahead and not
    // cause scanf to try to read any more from the pipe, which won't have any.
    ASSERT_EQ(1, fscanf(pipef, " %" SCNu64, &koid));
  }
}

void TestProcessForThreadState::Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump) {
  auto result = dump.SuspendAndCollectThreads();
  EXPECT_TRUE(result.is_ok()) << result.error_value();
}

void TestProcessForThreadState::CheckDump(zxdump::TaskHolder& holder) {
  auto find_result = holder.root_job().find(koid());
  ASSERT_TRUE(find_result.is_ok()) << find_result.error_value();

  ASSERT_EQ(find_result->get().type(), ZX_OBJ_TYPE_PROCESS);
  zxdump::Process& read_process = static_cast<zxdump::Process&>(find_result->get());

  auto list_result = read_process.get_info<ZX_INFO_PROCESS_THREADS>();
  ASSERT_TRUE(list_result.is_ok()) << list_result.error_value();
  EXPECT_THAT(*list_result, UnorderedElementsAreArray(thread_koids()));

  // This has an overload for each machine's general-registers type.
  // Checking the dump reads the right one for the dump's machine.
  struct GetCrashRegister {
    constexpr uint64_t operator()(const zx_arm64_thread_state_general_regs_t& regs) const {
      return regs.r[0];
    }

    constexpr uint64_t operator()(const zx_riscv64_thread_state_general_regs_t& regs) const {
      return regs.a0;
    }

    constexpr uint64_t operator()(const zx_x86_64_thread_state_general_regs_t& regs) const {
      return regs.rax;
    }
  };

  // This takes the result of zxdump::Thread::read_state<RegsType>.
  auto check_crash_register = [](auto result) {
    ASSERT_TRUE(result.is_ok()) << result.error_value();
    EXPECT_EQ(GetCrashRegister{}(*result), kRegisterValue);
  };

  auto threads_result = read_process.threads();
  ASSERT_TRUE(threads_result.is_ok()) << threads_result.error_value();
  for (auto& [koid, thread] : threads_result->get()) {
    // The first KOID printed is the main thread, which doesn't crash.
    // So skip that one.
    if (koid != thread_koids().front()) {
      switch (read_process.dump_machine()) {
        case elfldltl::ElfMachine::kAarch64:
          check_crash_register(thread.read_state<zx_arm64_thread_state_general_regs_t>());
          break;
        case elfldltl::ElfMachine::kRiscv:
          check_crash_register(thread.read_state<zx_riscv64_thread_state_general_regs_t>());
          break;
        case elfldltl::ElfMachine::kX86_64:
          check_crash_register(thread.read_state<zx_x86_64_thread_state_general_regs_t>());
          break;
        default:
          FAIL() << "unsupported machine " << static_cast<uint32_t>(read_process.dump_machine());
      }
    }
  }
}

namespace {

TEST(ZxdumpTests, ProcessDumpBasic) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcess process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());

  zxdump::TaskHolder dump_holder;
  zx::process process_dup;
  zx_status_t status = process.process().duplicate(ZX_RIGHT_SAME_RIGHTS, &process_dup);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  auto insert_result = dump_holder.Insert(std::move(process_dup));
  ASSERT_TRUE(insert_result.is_ok()) << insert_result.error_value();
  zxdump::Object& inserted_object = *insert_result;
  EXPECT_EQ(inserted_object.type(), ZX_OBJ_TYPE_PROCESS);

  zxdump::ProcessDump dump(static_cast<zxdump::Process&>(inserted_object));

  auto collect_result = dump.CollectProcess(TestProcess::PruneAllMemory);
  ASSERT_TRUE(collect_result.is_ok()) << collect_result.error_value();

  auto dump_result = dump.DumpHeaders(writer.AccumulateFragmentsCallback());
  ASSERT_TRUE(dump_result.is_ok()) << dump_result.error_value();

  auto write_result = writer.WriteFragments();
  ASSERT_TRUE(write_result.is_ok()) << write_result.error_value();
  const size_t bytes_written = write_result.value();

  auto memory_result = dump.DumpMemory(writer.WriteCallback());
  ASSERT_TRUE(memory_result.is_ok()) << memory_result.error_value();
  const size_t total_with_memory = memory_result.value();

  // We pruned all memory, so DumpMemory should not have added any output.
  EXPECT_EQ(bytes_written, total_with_memory);

  // Now read the file back in.
  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();

  // The dump has no jobs, so there should be a placeholder "super-root".
  EXPECT_EQ(ZX_KOID_INVALID, holder.root_job().koid());

  auto processes = holder.root_job().processes();
  ASSERT_TRUE(processes.is_ok()) << processes.error_value();

  // The fake job should have exactly one process.
  EXPECT_EQ(processes->get().size(), 1u);
  for (auto& [read_koid, read_process] : processes->get()) {
    EXPECT_NE(read_koid, ZX_KOID_INVALID);

    // Get the basic info from the real live process handle.
    zx_info_handle_basic_t basic;
    ASSERT_EQ(ZX_OK, process.borrow()->get_info(ZX_INFO_HANDLE_BASIC, &basic, sizeof(basic),
                                                nullptr, nullptr));
    EXPECT_EQ(read_koid, basic.koid);
    EXPECT_EQ(ZX_OBJ_TYPE_PROCESS, basic.type);

    // Get the same info from the dump and verify they match up.  Note that the
    // zx_info_handle_basic_t::rights in the dump is not usually particularly
    // meaningful about the dumped process, because it's just whatever rights
    // the dumper's own process handle had.  But in this case it does exactly
    // match the handle we just checked, since that's what we used to dump.
    auto read_basic = read_process.get_info<ZX_INFO_HANDLE_BASIC>();
    ASSERT_TRUE(read_basic.is_ok()) << read_basic.error_value();
    EXPECT_EQ(basic.koid, read_basic->koid);
    EXPECT_EQ(basic.rights, read_basic->rights);
    EXPECT_EQ(basic.type, read_basic->type);
    EXPECT_EQ(basic.related_koid, read_basic->related_koid);
  }
}

TEST(ZxdumpTests, ProcessDumpPropertiesAndInfo) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForPropertiesAndInfo process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder, false));
}

TEST(ZxdumpTests, ProcessDumpToZstdFile) {
  constexpr std::string_view kName = "zstd-process-dump-test";

  // We'll verify the data written to the file by decompressing it with the
  // zstd tool and reading in the resulting uncompressed file.
  zxdump::testing::TestToolProcess zstd;
  ASSERT_NO_FATAL_FAILURE(zstd.Init());

  // Set up the writer to send the compressed data to a temporary file.
  zxdump::testing::TestToolProcess::File& zstd_file =
      zstd.MakeFile(kName, zxdump::testing::TestToolProcess::File::kZstdSuffix);
  zxdump::ZstdWriter writer(zstd_file.CreateInput());

  TestProcessForPropertiesAndInfo process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  // Complete the compressed stream.
  auto finish = writer.Finish();
  ASSERT_TRUE(finish.is_ok()) << finish.error_value();

  // Decompress the file using the tool.
  zxdump::testing::TestToolProcess::File& plain_file = zstd.MakeFile(kName);
  std::vector<std::string> args({
      "-d"s,
      "-q"s,
      zstd_file.name(),
      "-o"s,
      plain_file.name(),
  });
  ASSERT_NO_FATAL_FAILURE(zstd.Start("zstd"s, args));
  ASSERT_NO_FATAL_FAILURE(zstd.CollectStdout());
  ASSERT_NO_FATAL_FAILURE(zstd.CollectStderr());
  int exit_status;
  ASSERT_NO_FATAL_FAILURE(zstd.Finish(exit_status));
  EXPECT_EQ(exit_status, EXIT_SUCCESS);

  // The zstd tool would complain about a malformed file.
  EXPECT_EQ(zstd.collected_stderr(), "");
  EXPECT_EQ(zstd.collected_stdout(), "");

  // Now read in the uncompressed file and check its contents.
  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(plain_file.OpenOutput());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder, false));
}

TEST(ZxdumpTests, ProcessDumpToZstdPipe) {
  // We'll verify the data by piping it directly to the zstd tool to decompress
  // as a filter with pipes on both ends, reading from that pipe.
  zxdump::testing::TestToolProcess zstd;
  ASSERT_NO_FATAL_FAILURE(zstd.Init());
  std::vector<std::string> args({"-d"s});
  ASSERT_NO_FATAL_FAILURE(zstd.Start("zstd"s, args));
  ASSERT_NO_FATAL_FAILURE(zstd.CollectStderr());

  TestProcessForPropertiesAndInfo process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  {
    // Set up the writer to send the compressed data to the tool.
    zxdump::ZstdWriter writer(std::move(zstd.tool_stdin()));

    ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

    // Complete the compressed stream.
    auto finish = writer.Finish();
    ASSERT_TRUE(finish.is_ok()) << finish.error_value();

    // The write side of the pipe is closed when the writer goes out of scope,
    // so the decompressor can finish.
  }

  // Now read in the uncompressed dump stream and check its contents.
  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(std::move(zstd.tool_stdout()), false);
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder, false));

  // The reader should have consumed the all of the tool's stdout by now,
  // so it will have been unblocked to finish after its stdin hit EOF when
  // the writer's destruction closed the pipe.
  int exit_status;
  ASSERT_NO_FATAL_FAILURE(zstd.Finish(exit_status));
  EXPECT_EQ(exit_status, EXIT_SUCCESS);

  // The zstd tool would complain about a malformed stream.
  EXPECT_EQ(zstd.collected_stderr(), "");
}

TEST(ZxdumpTests, ProcessDumpSystemInfo) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForSystemInfo process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));
}

// TODO(mcgrathr): test job archives with system info, nested repeats

TEST(ZxdumpTests, ProcessDumpKernelInfo) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForKernelInfo process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));
}

// TODO(mcgrathr): test job archives with kernel info, nested repeats

TEST(ZxdumpTests, ProcessDumpNoDate) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForPropertiesAndInfo process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();

  auto find_result = holder.root_job().find(process.koid());
  ASSERT_TRUE(find_result.is_ok()) << find_result.error_value();

  ASSERT_EQ(find_result->get().type(), ZX_OBJ_TYPE_PROCESS);
  zxdump::Process& read_process = static_cast<zxdump::Process&>(find_result->get());

  // By default no date was recorded.
  EXPECT_EQ(read_process.date(), kNoDate);
}

TEST(ZxdumpTests, ProcessDumpDate) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForPropertiesAndInfo process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());

  constexpr auto precollect = [](zxdump::TaskHolder& holder, zxdump::ProcessDump& dump) {
    dump.set_date(kTestDate);
  };
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer, precollect));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();

  auto find_result = holder.root_job().find(process.koid());
  ASSERT_TRUE(find_result.is_ok()) << find_result.error_value();

  ASSERT_EQ(find_result->get().type(), ZX_OBJ_TYPE_PROCESS);
  zxdump::Process& read_process = static_cast<zxdump::Process&>(find_result->get());

  EXPECT_EQ(read_process.date(), kTestDate);
}

// TODO(mcgrathr): test job archives w/&w/o dates

TEST(ZxdumpTests, ProcessDumpRemarks) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForRemarks process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));
}

// TODO(mcgrathr): test job archives with remarks, nested repeats

TEST(ZxdumpTests, ProcessDumpMemory) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForMemory process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());

  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));
}

TEST(ZxdumpTests, ProcessDumpThreads) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForThreads process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));
}

TEST(ZxdumpTests, ProcessDumpThreadState) {
  TestFile file;
  zxdump::FdWriter writer(file.RewoundFd());

  TestProcessForThreadState process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());
  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  zxdump::TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));
}

}  // namespace
}  // namespace zxdump::testing
