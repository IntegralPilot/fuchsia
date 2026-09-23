// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>
#include <lib/ld/testing/test-elf-object.h>

#include <string_view>

#include "load-tests.h"

namespace ld::testing {
namespace {

using namespace elfldltl::literals;
using namespace std::literals;

std::vector<std::string> SplitLogLines(std::string_view log) {
  std::vector<std::string> log_lines;
  while (!log.empty()) {
    size_t n = log.find('\n');
    EXPECT_NE(n, std::string_view::npos) << "last log line unterminated: " << log;
    if (n == std::string_view::npos) {
      break;
    }
    log_lines.emplace_back(log.substr(0, n));
    log.remove_prefix(n + 1);
  }
  return log_lines;
}

std::string TestModuleMarkup(const TestElfObject& module, size_t idx, std::string_view name) {
  return "{{{module:"s + std::to_string(idx) + ':' + std::string(name) +
         ":elf:" + std::string(module.build_id_hex) + "}}}";
}

// TODO(https://fxbug.dev/374053770): An apparent incremental build bug makes
// this appear to be flaky. Re-enable this when the build bug is fixed.
TYPED_TEST(LdLoadTests, DISABLED_SymbolizerMarkup) {
  if constexpr (!TestFixture::kCanCollectLog) {
    GTEST_SKIP() << "test requires log capture";
  }

  const std::string executable_name_string =
      "indirect-deps"s + std::string(TestFixture::kTestExecutableSuffix);
  const elfldltl::Soname<> executable_name{executable_name_string};

  const TestElfLoadSet* load_set = TestElfLoadSet::Get(executable_name);
  ASSERT_TRUE(load_set) << executable_name;
  TestElfLoadSet::SonameMap soname_map = load_set->MakeSonameMap();

  constexpr int64_t kReturnValue = 17;

  ASSERT_NO_FATAL_FAILURE(this->Init({}, {"LD_DEBUG=1"}));

  ASSERT_NO_FATAL_FAILURE(this->Needed({
      "libindirect-deps-a.so",
      "libindirect-deps-b.so",
      "libindirect-deps-c.so",
  }));

  ASSERT_NO_FATAL_FAILURE(this->Load("indirect-deps"));

  EXPECT_EQ(this->Run(), kReturnValue);

  // Collect the log and slice it into lines.
  const std::vector<std::string> log_lines = SplitLogLines(this->CollectLog());

  // There should be at least one module line and one mmap line per module.
  EXPECT_GE(log_lines.size(), 2u * 5u);

  auto next_line = [left = std::span{log_lines}](bool skip_mmap = false) mutable  //
      -> std::optional<std::string_view> {
    std::string_view line;
    do {
      if (left.empty()) {
        return std::nullopt;
      }
      line = left.front();
      EXPECT_FALSE(line.empty());
      left = left.subspan(1);
      // If skip_mmap is true, we just saw a module line for a module whose
      // segment details we don't know, so just expect to see some number of
      // mmap lines for it before the next module we're looking for.
    } while (skip_mmap && line.starts_with("{{{mmap:"sv));
    return line;
  };

  auto expect_mmap = [page_size = static_cast<size_t>(sysconf(_SC_PAGE_SIZE)), &next_line](
                         const TestElfObject& module, size_t idx) {
    for (const auto& phdr : module.load_segments) {
      const uint64_t vaddr = phdr.vaddr & -page_size;
      const uint64_t memsz = ((phdr.vaddr + phdr.memsz + page_size - 1) & -page_size) - vaddr;
      std::string flags;
      if (phdr.flags & elfldltl::Elf<>::Phdr::kRead) {
        flags += 'r';
      }
      if (phdr.flags & elfldltl::Elf<>::Phdr::kWrite) {
        flags += 'w';
      }
      if (phdr.flags & elfldltl::Elf<>::Phdr::kExecute) {
        flags += 'x';
      }
      uint64_t mmap_memsz, mmap_file_vaddr;
      size_t mmap_modid;
      char mmap_flags[5];
      std::optional<std::string_view> line = next_line();
      ASSERT_TRUE(line) << "missing mmap line for " << module.soname << " after "
                        << &phdr - module.load_segments.data() << " phdrs";
      ASSERT_EQ(sscanf(std::string(*line).c_str(),
                       "{{{mmap:%*x:%" PRIx64 ":load:%zi:%4[^:]:%" PRIx64 " }}}", &mmap_memsz,
                       &mmap_modid, mmap_flags, &mmap_file_vaddr),
                4)
          << *line << " for " << module.soname;
      EXPECT_EQ(mmap_modid, idx) << *line << " for " << module.soname;
      EXPECT_EQ(mmap_file_vaddr, vaddr) << *line << " for " << module.soname;
      EXPECT_EQ(mmap_memsz, memsz) << *line << " for " << module.soname;
      EXPECT_EQ(std::string(mmap_flags), flags) << *line << " for " << module.soname;
    }
  };

  auto expect_module = [load_set, &soname_map, &expect_mmap, &next_line](
                           elfldltl::Soname<> name, size_t idx, bool skip_mmap = false) {
    std::string_view modname = idx == 0 ? "<application>"sv : name.str();

    auto it = soname_map.end();
    if (idx > 0) {
      it = soname_map.find(name);
      ASSERT_NE(it, soname_map.end()) << name;
    }
    const TestElfObject& module =  // Use the SONAME lookup unless idx is 0.
        idx > 0 ? it->second : load_set->main_module();
    std::string markup = TestModuleMarkup(module, idx, modname);

    std::optional<std::string_view> line = next_line(skip_mmap);
    EXPECT_THAT(line, ::testing::Optional(::testing::StrEq(markup)))
        << "Expected " << name << " (" << modname << ") [" << module.build_id_hex << "] at ID "
        << idx << " but got " << line.value_or("<EOF>"sv);
    expect_mmap(module, idx);
  };

  expect_module(executable_name, 0);

  size_t idx = 1;
  bool skip_mmap_before_deps = false;
  if constexpr (TestFixture::kTestExecutableNeedsVdso) {
    // The vDSO will be the first dependency.  We don't know its build ID,
    // so just check the name.
    std::optional<std::string_view> line = next_line();
    EXPECT_THAT(line, ::testing::Optional(::testing::StartsWith(
                          "{{{module:1:"s +
                          std::string{TestFixture::kTestExecutableNeedsVdso->str()} + ":elf:"s)))
        << line.value_or("<EOF>"sv);
    ++idx;
    // If we expected the vDSO module line, we expect mmap lines
    // for it too, though we don't know what they'll say.
    skip_mmap_before_deps = true;
  }

  expect_module("libindirect-deps-a.so"_soname, idx++, skip_mmap_before_deps);

  expect_module("libindirect-deps-b.so"_soname, idx++);

  expect_module("libindirect-deps-c.so"_soname, idx++);

  // None of these modules links against ld.so itself, and on non-Fuchsia none
  // links against the vDSO.  But those appear at the end of the list anyway.
  auto find_ld_module = soname_map.find(ld::abi::Abi<>::kSoname);
  ASSERT_NE(find_ld_module, soname_map.end());
  const TestElfObject& ld_module = find_ld_module->second;
  std::optional<std::string_view> module_line = next_line();
  ASSERT_THAT(module_line, ::testing::Optional(
                               ::testing::StartsWith("{{{module:"s + std::to_string(idx) + ':')));
  if (!TestFixture::kTestExecutableNeedsVdso &&
      !module_line->starts_with("{{{module:"s + std::to_string(idx) + ':' +
                                std::string{ld::abi::Abi<>::kSoname.str()})) {
    // This must be the vDSO.  The ld.so module will be after it.  We don't
    // know what the vDSO's mmap lines should look like, so just skip them all.
    ++idx;
    module_line = next_line(true);
  }

  // Finally the ld.so module will appear.
  EXPECT_EQ(*module_line, TestModuleMarkup(ld_module, idx, ld::abi::Abi<>::kSoname.str()));
  expect_mmap(ld_module, idx);

  // There should be no more log lines after that.
  std::optional<std::string_view> tail = next_line();
  EXPECT_EQ(tail, std::nullopt) << *tail;
}

TYPED_TEST(LdLoadTests, Backtrace) {
  if constexpr (std::is_same_v<ld::testing::LdStartupInProcessTests, TestFixture>) {
    GTEST_SKIP() << "test not supported in-process";
  }

  constexpr int64_t kReturnValue = 17;

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Load("backtrace"));

  EXPECT_EQ(this->Run(), kReturnValue);

  this->ExpectLog("");
}

}  // namespace
}  // namespace ld::testing
