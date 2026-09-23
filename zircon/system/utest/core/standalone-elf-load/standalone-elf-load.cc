// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/dynamic.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/mapped-vmo-file.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/note.h>
#include <lib/elfldltl/symbol.h>
#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/elfldltl/vmar-loader.h>
#include <lib/symbolizer-markup/writer.h>
#include <lib/zx/vmar.h>

#include <optional>
#include <string_view>

#include <zxtest/zxtest.h>

#include "test-module.h"

namespace {

using namespace std::string_view_literals;

constexpr std::array kTestModules = {TEST_MODULE_LIST};
static_assert(kTestModules.size() > 3);

constexpr elfldltl::SymbolName kTestRoData{"kTestRoData"};

auto LogSink() { return zxtest::Runner::GetInstance()->mutable_reporter()->mutable_log_sink(); }

void Log(std::string_view str) {
  LogSink()->Write("%.*s", static_cast<int>(str.size()), str.data());
}

bool TestOneModule(std::string_view test_module_name, unsigned int id) {
  // Fetch the test module via fuchsia.ldsvc (userboot in the standalone case).
  zx::vmo vmo = elfldltl::testing::GetTestLibVmo(test_module_name);
  if (!vmo) {
    return false;
  }

  // Just map the whole file in, as it simplifies fetching the build ID in the
  // first phdr scan.
  elfldltl::MappedVmoFile file;
  {
    auto result = file.Init(vmo.borrow());
    EXPECT_TRUE(result.is_ok()) << test_module_name << ": " << result.status_string();
  }

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  // This lambda does all the work.  It gets instantiated for the specific ELF
  // format of the file (32 vs 64, endian).
  bool ran = false;
  auto load = [test_module_name, id, &vmo, &file, &diag, &ran]<class Ehdr, class Phdr>(
                  const Ehdr& ehdr, std::span<const Phdr> phdrs) -> bool {
    using Elf = typename Ehdr::ElfLayout;
    using Word = typename Elf::Word;
    using size_type = typename Elf::size_type;
    using Dyn = typename Elf::Dyn;
    using LoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;

    // Do basic header checks.  This doesn't care what machine it's for (the
    // second argument can be a specific elfldltl::ElfMachine value to require
    // that or omitted to require this machine).
    EXPECT_TRUE(ehdr.Loadable(diag, std::nullopt)) << test_module_name << " not loadable";

    // Default construction loads into the root VMAR.  A constructor argument
    // of any other sub-VMAR could be provided as well (even e.g. one in the
    // restricted mode address space).  The test code below just assumes that
    // once it's all loaded, the addresses are accessible in this process.
    elfldltl::LocalVmarLoader loader;

    // This will collect the build ID from the file, for the symbolizer markup.
    std::span<const std::byte> build_id;
    auto build_id_observer = [&build_id](const auto& note) -> fit::result<fit::failed, bool> {
      if (!note.IsBuildId()) {
        // This is a different note, so keep looking.
        return fit::ok(true);
      }
      build_id = note.desc;
      // Tell the caller not to call again for another note.
      return fit::ok(false);
    };

    // Get all the essentials from the phdrs: load info, the build ID note, and
    // the PT_DYNAMIC phdr.
    LoadInfo load_info;
    std::optional<Phdr> dyn_phdr;
    EXPECT_TRUE(elfldltl::DecodePhdrs(
        diag, phdrs, load_info.GetPhdrObserver(static_cast<size_type>(loader.page_size())),
        elfldltl::PhdrFileNoteObserver(Elf{}, file, elfldltl::NoArrayFromFile<std::byte>{},
                                       build_id_observer),
        elfldltl::PhdrDynamicObserver<Elf>(dyn_phdr)));

    if (!loader.Load(diag, load_info, vmo.borrow())) {
      ADD_FAILURE() << "cannot load " << test_module_name;
      return false;
    }

    // Log symbolizer markup context for the test module to ease debugging.
    symbolizer_markup::Writer markup_writer{Log};
    load_info.SymbolizerContext(
        markup_writer, id, test_module_name, build_id,
        static_cast<size_type>(load_info.vaddr_start() + loader.load_bias()));

    // Read the PT_DYNAMIC, which leads to symbol information.
    std::span<const Dyn> dyn;
    {
      EXPECT_TRUE(dyn_phdr) << test_module_name << " has no PT_DYNAMIC";
      if (!dyn_phdr) {
        return false;
      }
      auto read_dyn =
          loader.memory().ReadArray<Dyn>(dyn_phdr->vaddr, dyn_phdr->filesz / sizeof(Dyn));
      EXPECT_TRUE(read_dyn) << test_module_name << " PT_DYNAMIC not read";
      if (read_dyn) {
        dyn = *read_dyn;
      }
    }

    // Decode PT_DYNAMIC just enough to get the symbols.
    elfldltl::SymbolInfo<Elf> symbol_info;
    EXPECT_TRUE(elfldltl::DecodeDynamic(diag, loader.memory(), dyn,
                                        elfldltl::DynamicSymbolInfoObserver(symbol_info)));

    // Look up the kTestRoData symbol and read out the Word (uint32_t) there.
    if (const auto* sym = kTestRoData.Lookup(symbol_info)) {
      const uintptr_t test_rodata_addr = sym->value + loader.load_bias();
      const Word* const ptr = reinterpret_cast<const Word*>(test_rodata_addr);
      EXPECT_EQ(*ptr, kTestRoDataValue);
    } else {
      ADD_FAILURE() << test_module_name << " has no " << kTestRoData;
    }

    if (ehdr.machine == elfldltl::ElfMachine::kNative) {
      // It's for the right machine, so try calling its entry point function.
      const uintptr_t entry = ehdr.entry + loader.load_bias();
      const auto test_start = reinterpret_cast<TestStartFunction*>(entry);
      EXPECT_EQ(test_start(17, 23), 17 + 23);
      LogSink()->Write("Ran code in %.*s!\n", static_cast<int>(test_module_name.size()),
                       test_module_name.data());
      ran = true;
    }

    // Don't use loader.Commit(), so the test module will be unmapped now.
    return true;
  };

  // This reads the ELF header just enough to dispatch to an instantiation of
  // the lambda for the specific ELF format found (accepting all four formats).
  auto phdr_allocator = [&diag]<typename T>(size_t count) {
    return elfldltl::ContainerArrayFromFile<elfldltl::StdContainer<std::vector>::Container<T>>(
        diag, "impossible")(count);
  };
  EXPECT_TRUE(elfldltl::WithLoadHeadersFromFile(diag, file, phdr_allocator, load, std::nullopt,
                                                std::nullopt));

  return ran;
}

TEST(StandaloneElfLoadTests, Load) {
  // This will be the module ID used in the symbolizer markup output for each
  // test module.  Start way above what the test itself will have used.
  unsigned int id = 100;

  bool any_ran = false;
  for (std::string_view test_module_name : kTestModules) {
    any_ran = TestOneModule(test_module_name, ++id) || any_ran;
  }
  EXPECT_TRUE(any_ran) << "no test module was for this machine?";
}

}  // namespace
