// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/note.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/elfldltl/vmo.h>
#include <lib/fit/defer.h>
#include <lib/ld/remote-dynamic-linker.h>
#include <lib/ld/testing/get-test-vmo.h>
#include <lib/ld/testing/interp.h>
#include <lib/ld/testing/vdso.h>
#include <lib/zx/suspend_token.h>
#include <lib/zx/thread.h>
#include <lib/zxdump/elf-search.h>

#include <array>
#include <concepts>
#include <cstdint>
#include <filesystem>
#include <initializer_list>
#include <ranges>
#include <string>
#include <tuple>
#include <vector>

#include <gmock/gmock.h>

#include "dump-tests.h"
#include "test-file.h"

namespace zxdump::testing {
namespace {

using ByteVector = TestProcessForElfSearch::ByteVector;
using ElfId = TestProcessForElfSearch::ElfId;
using ElfIdList = TestProcessForElfSearch::ElfIdList;
using ElfIdAtBase = TestProcessForElfSearch::ElfIdAtBase;
using ElfIdAtBaseList = TestProcessForElfSearch::ElfIdAtBaseList;

using ::testing::AllOf;
using ::testing::Contains;
using ::testing::Field;
using ::testing::IsEmpty;
using ::testing::IsSupersetOf;
using ::testing::Not;
using ::testing::UnorderedElementsAreArray;

ElfId MakeElfId(std::initializer_list<uint8_t> id, std::string_view soname) {
  ByteVector bytes;
  for (uint8_t byte : id) {
    bytes.push_back(static_cast<std::byte>(byte));
  }
  return ElfId{
      .build_id = std::move(bytes),
      .soname = std::string(soname),
  };
}

// The generated .inc file has `MakeElfId({0x...,...}, "soname"),` lines.
const std::array kElfSearchIds = {
#include "test-child-elf-search.inc"
};

// gtest container matchers don't like ranges/views.
template <std::ranges::input_range Range>
std::vector<std::ranges::range_value_t<Range>> RangeVector(Range&& range) {
  return {range.begin(), range.end()};
}

template <std::ranges::input_range T>
  requires std::derived_from<std::ranges::range_value_t<T>, ElfId>
auto IdsWithoutSoname(T&& ids) {
  constexpr auto clear_soname = [](std::derived_from<ElfId> auto id) {
    id.soname = {};
    return id;
  };
  auto without = std::views::transform(std::forward<T>(ids), clear_soname);
  return RangeVector(without);
}

const auto kElfSearchIdsWithoutSoname = IdsWithoutSoname(kElfSearchIds);

using ForeignElf = elfldltl::Elf32<elfldltl::ElfData::k2Lsb>;
constexpr elfldltl::ElfMachine kForeignMachine = elfldltl::ElfMachine::kArm;
constexpr uint32_t kForeignPageSize = 0x1000;

TEST(ZxdumpTests, ElfSearchLive) {
  TestProcessForElfSearch process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());

  TaskHolder holder;
  auto insert_result = holder.Insert(process.handle());
  ASSERT_TRUE(insert_result.is_ok()) << insert_result.error_value();

  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));
  ASSERT_NO_FATAL_FAILURE(process.CheckDumpElfSearchIds());
}

TEST(ZxdumpTests, ElfSearchDump) {
  TestFile file;
  FdWriter writer(file.RewoundFd());

  TestProcessForElfSearch process;
  ASSERT_NO_FATAL_FAILURE(process.StartChild());

  ASSERT_NO_FATAL_FAILURE(process.Dump(writer));

  TaskHolder holder;
  auto read_result = holder.Insert(file.RewoundFd());
  ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();

  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));
  ASSERT_NO_FATAL_FAILURE(process.CheckDumpElfSearchIds());

  ASSERT_NO_FATAL_FAILURE(process.CheckNotes(file.RewoundFd().get()));
  ASSERT_NO_FATAL_FAILURE(process.CheckNotesElfSearchIds());
}

// With remote dynamic linking, the process is only really needed for its VMAR.
// There's no need to actually start the process, give it a stack, etc.  The
// process is always killed at the end of the test, never waited for.
class TestProcessForRemoteElfSearch : public TestProcessForElfSearch {
 public:
  void StartChild() = delete;

  void Create() {
    ASSERT_TRUE(vmar_ = CreateProcess());

    // TODO(https://fxbug.dev/425988091): get_info<ZX_INFO_PROCESS_VMOS>
    // currently fails with ZX_ERR_BAD_STATE when the process hasn't been
    // started yet, even though it could work fine like ZX_INFO_PROCESS_MAPS
    // already does.  Work around it by "starting" the process, but with its
    // (only) thread suspended so it will never run with its bogus register
    // values.  This should all be removed when the kernel bug is fixed.
    zx::thread thread;
    zx_status_t status = zx::thread::create(process(), kChildName, strlen(kChildName), 0, &thread);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
    status = thread.suspend(&thread_suspended_);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
    status = borrow()->start(thread, 0, 0, {}, 0);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

  const zx::vmar& vmar() { return vmar_; }

  void DumpRemote(ElfIdAtBaseList expected_elf, bool check_notes = true);

  ~TestProcessForRemoteElfSearch() {
    zx::unowned_process process = borrow();
    if (*process) {
      zx_status_t status = process->kill();
      EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
    }
  }

 private:
  zx::vmar vmar_;
  zx::suspend_token thread_suspended_;  // Destroyed  after process->kill().
};

template <class Elf = elfldltl::Elf<>, elfldltl::ElfMachine Machine = elfldltl::ElfMachine::kNative>
class RemoteLinker {
 public:
  using Linker = ld::RemoteDynamicLinker<Elf, ld::RemoteLoadZygote::kNo, Machine>;
  using Decoded = Linker::Module::Decoded;
  using DecodedPtr = Decoded::Ptr;
  using ElfIdAtBase = TestProcessForRemoteElfSearch::ElfIdAtBase;
  using ElfIdAtBaseList = TestProcessForRemoteElfSearch::ElfIdAtBaseList;

  static constexpr bool kLocal =
      std::is_same_v<Elf, elfldltl::Elf<>> && Machine == elfldltl::ElfMachine::kNative;

  void Init(std::string_view name)
    requires(kLocal && std::is_same_v<Elf, elfldltl::Elf<>> &&
             Machine == elfldltl::ElfMachine::kNative)
  {
    Init(name, ld::testing::GetExecutableVmo(name), zx_system_get_page_size(), GetVdso());
  }

  void Init(std::string_view name, zx::vmo executable_vmo, Linker::size_type page_size,
            DecodedPtr vdso) {
    ASSERT_TRUE(executable_vmo);

    std::optional<std::string> libprefix = ld::testing::ConfigFromInterp(
        ld::testing::FindInterp<elfldltl::UnownedVmoFile>(executable_vmo.borrow()));
    lib_path_ = ld::testing::GetExecutableLibPath(name, libprefix);

    elfldltl::testing::ExpectOkDiagnostics diag;
    linker_.set_abi_stub(
        Linker::AbiStub::Create(diag, elfldltl::testing::GetTestLibVmo(Linker::AbiStub::kFilename),
                                zx_system_get_page_size()));
    ASSERT_TRUE(linker_.abi_stub());

    auto decode = [page_size](zx::vmo vmo) -> DecodedPtr {
      elfldltl::testing::ExpectOkDiagnostics diag;
      return Decoded::Create(diag, std::move(vmo), page_size);
    };

    auto get_dep = [this, decode](const Linker::Module::Soname& soname) -> Linker::GetDepResult {
      std::filesystem::path path = lib_path_ / soname.str();
      if (auto decoded = decode(elfldltl::testing::GetTestLibVmo(path.string()))) {
        return decoded;
      }
      return std::nullopt;
    };

    auto executable = decode(std::move(executable_vmo));
    ASSERT_TRUE(executable);
    ASSERT_TRUE(executable->HasModule());
    typename Linker::InitModuleList init_list;
    init_list.push_back(Linker::Executable(executable));
    if (vdso) {
      init_list.push_back(Linker::Implicit(std::move(vdso)));
    }
    auto result = linker_.Init(diag, std::move(init_list), get_dep, Machine);
    ASSERT_TRUE(result);
    init_modules_ = *result;
  }

  void Load(zx::unowned_vmar vmar) {
    elfldltl::testing::ExpectOkDiagnostics diag;
    ASSERT_TRUE(linker_.Allocate(diag, vmar->borrow()));
    ASSERT_TRUE(linker_.Relocate(diag));
    ASSERT_TRUE(linker_.Load(diag));
    linker_.Commit();
  }

  static ElfIdAtBase ModuleElfIdAtBase(const Linker::Module& module) {
    const auto& build_id = module.module().build_id;
    return {
        {
            .build_id{build_id.begin(), build_id.end()},
            .soname{module.soname().str()},
        },
        module.load_info().vaddr_start() + module.load_bias(),
    };
  }

  ElfIdAtBaseList GetElfIdAtBaseList() const {
    auto range = std::views::transform(modules(), ModuleElfIdAtBase);
    return ElfIdAtBaseList{range.begin(), range.end()};
  }

  const std::filesystem::path lib_path() const { return lib_path_; }
  const Linker::Module::List& modules() const { return linker_.modules(); }
  const Linker::InitResult& init_modules() const { return init_modules_; }

  static Linker::InitModule Executable(std::string_view name) { return Linker::Executable(); }

  static DecodedPtr GetVdso()
    requires(kLocal)
  {
    static const DecodedPtr vdso = []() -> DecodedPtr {
      zx::vmo vdso_vmo;
      zx_status_t status = ld::testing::GetVdsoVmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vdso_vmo);
      EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
      elfldltl::testing::ExpectOkDiagnostics diag;
      return Decoded::Create(diag, std::move(vdso_vmo), zx_system_get_page_size());
    }();
    return vdso;
  }

 private:
  std::filesystem::path lib_path_;
  Linker linker_;
  Linker::InitResult init_modules_;
};

TEST(ZxdumpTests, ElfSearchLiveRemote) {
  RemoteLinker<> linker;
  ASSERT_NO_FATAL_FAILURE(linker.Init("symbol-filter"));

  // Load up the process with those modules.
  TestProcessForRemoteElfSearch process;
  ASSERT_NO_FATAL_FAILURE(process.Create());
  ASSERT_TRUE(process.borrow()->is_valid());
  ASSERT_NO_FATAL_FAILURE(linker.Load(process.vmar().borrow()));

  // Now the runtime module locations and ID can be collected into ElfIdAtBase
  // from the source of truth: the dynamic linker that put them there.
  const ElfIdAtBaseList expected_elf = linker.GetElfIdAtBaseList();
  EXPECT_GT(expected_elf.size(), 1u);
  EXPECT_EQ(expected_elf.size(), linker.modules().size());

  // Collect the ElfId data from the zxdump API under test.
  TaskHolder holder;
  auto insert_result = holder.Insert(process.handle());
  ASSERT_TRUE(insert_result.is_ok()) << insert_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));

  // That should match the source of truth exactly with no additional modules.
  EXPECT_THAT(process.found_elf(), UnorderedElementsAreArray(expected_elf));
}

// Load both 64-bit and 32-bit modules into the process.
void LoadBiarch(TestProcessForRemoteElfSearch& process, ElfIdAtBaseList& expected_elf) {
  auto load = [&expected_elf](auto& linker, zx::unowned_vmar vmar) {
    ASSERT_NO_FATAL_FAILURE(linker.Load(vmar->borrow()));
    std::ranges::move(linker.GetElfIdAtBaseList(), std::back_inserter(expected_elf));
  };

  {
    // The kernel reserves the lowest part of the address space, so the root
    // VMAR doesn't start at zero.  The VMAR for the 32-bit address space will
    // not be quite 4GiB in size, so adjust to make sure it ends at exactly
    // 4GiB.  In fact, no 32-bit userland ever expects to have a segment in the
    // very last page, where the page-rounded vaddr+memsz wraps around to 0.
    // So make the VMAR one page smaller to ensure nothing gets placed all the
    // way up there.
    const size_t kAddressLimit = (size_t{1} << 32) - zx_system_get_page_size();
    zx_info_vmar_t root_vmar_info;
    zx_status_t status = process.vmar().get_info(ZX_INFO_VMAR, &root_vmar_info,
                                                 sizeof(root_vmar_info), nullptr, nullptr);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
    ASSERT_LT(root_vmar_info.base, kAddressLimit);

    constexpr zx_vm_option_t kVmarOptions =
        // Require the specific offset of 0 and allow exact placement within.
        ZX_VM_SPECIFIC | ZX_VM_CAN_MAP_SPECIFIC |
        // Allow all kinds of mappings.
        ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_EXECUTE;
    const size_t vmar_size = kAddressLimit - root_vmar_info.base;
    zx::vmar vmar;
    uintptr_t vmar_addr;
    status = process.vmar().allocate(kVmarOptions, 0, vmar_size, &vmar, &vmar_addr);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
    EXPECT_EQ(vmar_addr, root_vmar_info.base);

    RemoteLinker<ForeignElf, kForeignMachine> linker;
    constexpr std::string_view kForeignName = "symbol-filter-elf32";
    const std::filesystem::path path =
        ld::testing::GetExecutableLibPath(kForeignName) / kForeignName;
    ASSERT_NO_FATAL_FAILURE(linker.Init(
        kForeignName, elfldltl::testing::GetTestLibVmo(path.string()), kForeignPageSize, nullptr));
    ASSERT_NO_FATAL_FAILURE(load(linker, vmar.borrow()));
  }

  {
    RemoteLinker<> linker;
    ASSERT_NO_FATAL_FAILURE(linker.Init("symbol-filter"));
    ASSERT_NO_FATAL_FAILURE(load(linker, process.vmar().borrow()));
  }
}

TEST(ZxdumpTests, ElfSearchLiveRemoteBiarch) {
  TestProcessForRemoteElfSearch process;
  ASSERT_NO_FATAL_FAILURE(process.Create());
  ASSERT_TRUE(process.borrow()->is_valid());

  ElfIdAtBaseList expected_elf;
  ASSERT_NO_FATAL_FAILURE(LoadBiarch(process, expected_elf));

  // Collect the ElfId data from the zxdump API under test.
  TaskHolder holder;
  auto insert_result = holder.Insert(process.handle());
  ASSERT_TRUE(insert_result.is_ok()) << insert_result.error_value();
  ASSERT_NO_FATAL_FAILURE(process.CheckDump(holder));

  // That should match the source of truth.
  EXPECT_THAT(process.found_elf(), UnorderedElementsAreArray(expected_elf));
}

TEST(ZxdumpTests, ElfSearchDumpRemote) {
  TestProcessForRemoteElfSearch process;
  ASSERT_NO_FATAL_FAILURE(process.Create());
  ASSERT_TRUE(process.borrow()->is_valid());

  // Populate the process.
  RemoteLinker<> linker;
  ASSERT_NO_FATAL_FAILURE(linker.Init("symbol-filter"));
  ASSERT_NO_FATAL_FAILURE(linker.Load(process.vmar().borrow()));

  // Make the dump and check it against the data from the RemoteLinker.
  ASSERT_NO_FATAL_FAILURE(process.DumpRemote(linker.GetElfIdAtBaseList()));
}

TEST(ZxdumpTests, ElfSearchDumpRemoteBiarch) {
  TestProcessForRemoteElfSearch process;
  ASSERT_NO_FATAL_FAILURE(process.Create());
  ASSERT_TRUE(process.borrow()->is_valid());

  // Populate the process.
  ElfIdAtBaseList expected_elf;
  ASSERT_NO_FATAL_FAILURE(LoadBiarch(process, expected_elf));

  // TODO(https://fxbug.dev/395606674): FindBuildIdNote API needs revamp to not
  // double-report modules with first file page mapped twice in notes though
  // main ElfSearch API is OK.  Fix & remove the flag argument from DumpRemote.
  ASSERT_NO_FATAL_FAILURE(process.DumpRemote(std::move(expected_elf), false));
}

}  // namespace

void TestProcessForRemoteElfSearch::DumpRemote(ElfIdAtBaseList expected_elf, bool check_notes) {
  // Dump the process populated by remote dynamic linking.
  TestFile file;
  {
    FdWriter writer(file.RewoundFd());
    ASSERT_NO_FATAL_FAILURE(Dump(writer));
  }

  // Check the contents of the dump, collecting found_elf().
  {
    TaskHolder holder;
    auto read_result = holder.Insert(file.RewoundFd());
    ASSERT_TRUE(read_result.is_ok()) << read_result.error_value();
    ASSERT_NO_FATAL_FAILURE(CheckDump(holder));
  }

  // That should match the source of truth exactly with no additional modules.
  EXPECT_THAT(found_elf(), UnorderedElementsAreArray(expected_elf));

  if (!check_notes) {
    if (!::testing::Test::HasFailure()) {
      GTEST_SUCCEED() << "TODO(https://fxbug.dev/395606674): "
                      << "skipping NT_GNU_BUILD_ID core notes check on biarch";
    }
    return;
  }

  // Now check NT_GNU_BUILD_ID notes directly, resetting found_elf().
  ASSERT_NO_FATAL_FAILURE(CheckNotes(file.RewoundFd().get()));

  // That should also match the source of truth exactly, but without having
  // detected the SONAMEs from the notes alone.
  ElfIdAtBaseList expected_notes_elf = IdsWithoutSoname(expected_elf);
  EXPECT_THAT(found_elf(), UnorderedElementsAreArray(expected_notes_elf));
}

// This is called before dumping starts to make callbacks.
void TestProcessForElfSearch::Precollect(zxdump::TaskHolder& holder, zxdump::ProcessDump& dump) {
  dump_ = &dump;
}

// This is the callback made from DumpMemory for each segment.
// It needs the dumper pointer saved in Precollect.
fit::result<zxdump::Error, zxdump::SegmentDisposition>
TestProcessForElfSearch::DumpAllMemoryWithBuildIds(zxdump::SegmentDisposition segment,
                                                   const zx_info_maps_t& maps,
                                                   const zx_info_vmo_t& vmo) {
  if (segment.filesz > 0 && zxdump::IsLikelyElfMapping(maps)) {
    auto result = dump_->FindBuildIdNote(maps);
    if (result.is_error()) {
      return result.take_error();
    }
    segment.note = result.value();
  }
  return fit::ok(segment);
}

void TestProcessForElfSearch::StartChild() {
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

  ASSERT_NO_FATAL_FAILURE(TestProcess::StartChild({"-d", "-D"}));

  // The test-child wrote the pointers dladdr returned as module base addresses
  // for the main and DSO symbols.  Reading these immediately synchronizes with
  // the child having started up and progressed far enough to have finished all
  // its loading before the process gets dumped.
  FILE* pipef = fdopen(read_pipe.get(), "r");
  ASSERT_TRUE(pipef) << "fdopen: " << read_pipe.get() << strerror(errno);
  auto close_pipef = fit::defer([pipef]() { fclose(pipef); });
  std::ignore = read_pipe.release();

  ASSERT_EQ(2, fscanf(pipef, "%" SCNx64 "\n%" SCNx64, &main_ptr_, &dso_ptr_));
}

zx::vmar TestProcessForElfSearch::CreateProcess() { return TestProcess::CreateProcess(kChildName); }

void TestProcessForElfSearch::CheckDump(zxdump::TaskHolder& holder) {
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

  found_elf_.clear();
  auto record_elf = [this, &read_process](const zx_info_maps_t& segment) {
    ASSERT_EQ(segment.type, ZX_INFO_MAPS_TYPE_MAPPING);
    auto detect = zxdump::DetectElf(read_process, segment);
    ASSERT_TRUE(detect.is_ok()) << detect.error_value();

    EXPECT_EQ(std::visit(
                  [](const auto& phdrs) {
                    EXPECT_THAT(*phdrs, Not(IsEmpty()));
                    return phdrs->empty();
                  },
                  *detect),
              detect->empty());
    auto identity = zxdump::DetectElfIdentity(read_process, segment, *detect);
    ASSERT_TRUE(identity.is_ok()) << identity.error_value();

    ASSERT_GT(identity->build_id.size, zxdump::ElfIdentity::kBuildIdOffset);
    auto id_bytes = read_process.read_memory<std::byte, zxdump::ByteView>(
        identity->build_id.vaddr + zxdump::ElfIdentity::kBuildIdOffset,
        identity->build_id.size - zxdump::ElfIdentity::kBuildIdOffset);
    ASSERT_TRUE(id_bytes.is_ok()) << id_bytes.error_value();
    ElfId elf_id = {
        .build_id{id_bytes.value()->begin(), id_bytes.value()->end()},
    };

    if (identity->soname.size != 0) {
      auto read = read_process.read_memory<char, std::string_view>(  //
          identity->soname.vaddr, identity->soname.size);
      ASSERT_TRUE(read.is_ok()) << read.error_value();
      elf_id.soname = **read;
    }

    found_elf_.emplace_back(std::move(elf_id), segment.base);
  };

  auto read_maps = read_process.get_info<ZX_INFO_PROCESS_MAPS>();
  ASSERT_TRUE(read_maps.is_ok()) << read_maps.error_value();
  std::span maps = *read_maps;
  while (!maps.empty()) {
    auto result = zxdump::ElfSearch(read_process, maps);
    ASSERT_TRUE(result.is_ok());
    if (result->empty()) {
      // Nothing more found.
      break;
    }

    ASSERT_NO_FATAL_FAILURE(record_elf(result->front()));

    // The next iteration will scan the remainder after that image's segments.
    std::span remainder =  // Get the remainder including the image...
        maps.subspan(result->data() - maps.data())
            .subspan(result->size());  // ... and skip past the image.
    maps = remainder;
  }
}

void TestProcessForElfSearch::CheckDumpElfSearchIds() const {
  EXPECT_THAT(found_ids(), IsSupersetOf(kElfSearchIds));

  // Only the main executable should have no SONAME.
  EXPECT_THAT(found_elf(), Contains(Field("soname", &ElfId::soname, IsEmpty())).Times(1));

  // That module's base should be what dladdr said in the child.
  EXPECT_THAT(found_elf(), Contains(AllOf(Field("soname", &ElfId::soname, IsEmpty()),
                                          Field("load address", &ElfIdAtBase::base, main_ptr()))));

  // Similar pair for the DSO module.
  EXPECT_THAT(found_elf(), Contains(Field(&ElfId::soname, kDsoSoname)).Times(1));
  EXPECT_THAT(found_elf(), Contains(AllOf(Field("soname", &ElfId::soname, kDsoSoname),
                                          Field("load address", &ElfIdAtBase::base, dso_ptr()))));
}

void TestProcessForElfSearch::CheckNotes(int fd) {
  auto must_read = [fd](auto&& span, off_t pos) {
    std::span data(span);
    ssize_t nread = pread(fd, data.data(), data.size_bytes(), pos);
    ASSERT_GE(nread, 0) << strerror(errno);
    ASSERT_EQ(data.size_bytes(), static_cast<size_t>(nread));
  };

  zxdump::Elf::Ehdr ehdr;
  ASSERT_NO_FATAL_FAILURE(must_read(std::span(&ehdr, 1), 0));

  std::vector<zxdump::Elf::Phdr> phdrs(ehdr.phnum);
  ASSERT_NO_FATAL_FAILURE(must_read(phdrs, ehdr.phoff));

  found_elf_.clear();
  for (const zxdump::Elf::Phdr& phdr : phdrs) {
    if (phdr.type == elfldltl::ElfPhdrType::kNote) {
      ByteVector bytes(phdr.filesz, {});
      ASSERT_NO_FATAL_FAILURE(must_read(bytes, phdr.offset));
      elfldltl::ElfNoteSegment<elfldltl::ElfData::k2Lsb> notes(bytes);
      for (const auto& note : notes) {
        if (note.IsBuildId()) {
          found_elf_.push_back({
              {.build_id{note.desc.begin(), note.desc.end()}},
              (&phdr)[-1].vaddr,
          });
        }
      }
    }
  }
}

void TestProcessForElfSearch::CheckNotesElfSearchIds() const {
  // First check that the right build IDs were found at all.
  EXPECT_THAT(found_ids(), IsSupersetOf(kElfSearchIdsWithoutSoname));

  // Now check that each was found at its expected load address.
  const auto expected_elf = RangeVector(std::views::transform(  //
      kElfSearchIds, [this](const ElfId& id) -> ElfIdAtBase {
        return {
            {.build_id = id.build_id},
            id.soname.empty() ? main_ptr_ : dso_ptr_,
        };
      }));
  EXPECT_THAT(found_elf(), IsSupersetOf(expected_elf));
}

}  // namespace zxdump::testing
