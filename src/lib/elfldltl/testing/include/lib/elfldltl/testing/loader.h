// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_LOADER_H_
#define SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_LOADER_H_

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/fd.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/mmap-loader.h>
#include <lib/elfldltl/phdr.h>

#include <optional>
#include <string_view>
#include <vector>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "lib/elfldltl/testing/diagnostics.h"

#ifdef __Fuchsia__
#include <lib/elfldltl/segment-with-vmo.h>
#include <lib/elfldltl/vmar-loader.h>
#include <lib/elfldltl/vmo.h>
#include <lib/zx/vmar.h>
#endif

#include "get-test-data.h"

namespace elfldltl::testing {

// Each struct *LoaderTraits defines a few hooks so different kinds of loading
// can be tested with common code.

#ifdef __Fuchsia__
struct LocalVmarLoaderTraits {
  static constexpr std::string_view kName = "LocalVmarLoaderTraits";

  template <class Elf, template <class> class Container>
  using Info = LoadInfo<Elf, Container>;

  // The Loader object is move-constructible and move-assignable.
  // It might be default-constructible for not.
  using Loader = elfldltl::LocalVmarLoader;

  // This returns a Loader usable for in-process testing.
  static Loader MakeLoader(const zx::vmar& vmar = *zx::vmar::root_self()) {
    return elfldltl::LocalVmarLoader{vmar};
  }

  // This is a T(std::string_view) function that takes a string name of a test
  // module and returns some move-only type T representing the file's contents.
  // It will return a default-constructed T after having failed gtest
  // assertions if there's an error, so check HasFatalFailure().
  static constexpr auto TestLibProvider = GetTestLibVmo;

  // This returns some object that provides the File API, given the kind of
  // object that TestLibProvider returns.  The diagnostics object is expected
  // to be something that causes gtest failure if its error-reporting methods
  // are called.
  template <class Diagnostics>
  static auto MakeFile(zx::unowned_vmo vmo, Diagnostics& diagnostics) {
    return elfldltl::UnownedVmoFile(vmo->borrow(), diagnostics);
  }

  // This takes the return value of TestLibProvider and returns the argument to
  // pass to Loader::Load.
  static zx::unowned_vmo LoadFileArgument(const zx::vmo& vmo) { return vmo.borrow(); }

  // This indicates that the Loader::memory() method is available.
  static constexpr bool kHasMemory = true;

  // This can modify the LoadInfo segments before loading.
  template <class Diagnostics, class LoadInfo, class File, class Loader>
  static constexpr bool Normalize(Diagnostics& diag, LoadInfo& info, File& file, Loader& loader) {
    return true;
  }
};

struct RemoteVmarLoaderTraits : public LocalVmarLoaderTraits {
  static constexpr std::string_view kName = "RemoteVmarLoaderTraits";

  using Loader = elfldltl::RemoteVmarLoader;

  static Loader MakeLoader(const zx::vmar& vmar = *zx::vmar::root_self()) { return Loader{vmar}; }

  // No Loader::memory() method is available.
  static constexpr bool kHasMemory = false;
};

struct AlignedRemoteVmarLoaderTraits : public RemoteVmarLoaderTraits {
  static constexpr std::string_view kName = "AlignedRemoteVmarLoaderTraits";

  template <class Elf, template <class> class Container>
  using Info = LoadInfo<Elf, Container, PhdrLoadPolicy::kBasic, SegmentWithVmo::NoCopy>;

  using Loader = elfldltl::AlignedRemoteVmarLoader;

  static Loader MakeLoader(const zx::vmar& vmar = *zx::vmar::root_self()) { return Loader{vmar}; }

  template <class Diagnostics, class LoadInfo, class File>
  static constexpr bool Normalize(Diagnostics& diag, LoadInfo& info, const File& file,
                                  const Loader& loader) {
    return SegmentWithVmo::AlignSegments(diag, info, file.borrow(), loader.page_size());
  }
};
#endif  // __Fuchsia__

struct MmapLoaderTraits {
  static constexpr std::string_view kName = "MmapLoaderTraits";

  template <class Elf, template <class> class Container>
  using Info = LoadInfo<Elf, Container>;

  using Loader = elfldltl::MmapLoader<>;

  static Loader MakeLoader() { return Loader{}; }

  static constexpr auto TestLibProvider = GetTestLib;

  template <class Diagnostics>
  static auto MakeFile(int fd, Diagnostics& diagnostics) {
    return elfldltl::FdFile(fd, diagnostics);
  }

  static int LoadFileArgument(const fbl::unique_fd& fd) { return fd.get(); }

  // This indicates that the Loader::memory() method is available.
  static constexpr bool kHasMemory = true;

  template <class Diagnostics, class LoadInfo, class File>
  static constexpr bool Normalize(Diagnostics& diag, LoadInfo& info, File& file, Loader& loader) {
    return true;
  }
};

using LoaderTypes = ::testing::Types<
#ifdef __Fuchsia__
    LocalVmarLoaderTraits, RemoteVmarLoaderTraits, AlignedRemoteVmarLoaderTraits,
#endif
    MmapLoaderTraits>;

// This can be used as the base class for a test fixture, either via:
//   TYPED_TEST_SUITE(DerivedTests, LoaderTypes);
//   TYPED_TEST(DerivedTest, ...) {
// or just as a single class given a specific LoaderTraits parameter.
// The default template parameter uses POSIX-compatible mechanism that
// works on both Fuchsia and POSIX-like systems.
template <class Traits = MmapLoaderTraits, class Elf = Elf<>>
class LoadTests : public ::testing::Test {
 public:
  using Loader = Traits::Loader;
  using LoadInfo = Traits::template Info<Elf, StdContainer<std::vector>::Container>;
  using Addr = Elf::Addr;
  using Phdr = Elf::Phdr;
  using TestLib = decltype(Traits::TestLibProvider({}));

  struct LoadResult {
    Loader loader;
    std::vector<Phdr> phdrs;
    Addr phoff;
    Addr entry;
    LoadInfo info;
    std::optional<typename Elf::size_type> stack_size;
  };

  static constexpr LoadInfo::Region kNoRelro{};

  template <typename... LoaderArgs>
  void Load(const TestLib& test_lib, std::optional<LoadResult>& result,
            LoaderArgs&&... loader_args) {
    result = std::nullopt;

    // Bail out if the test_lib argument expression had a failure.
    ASSERT_FALSE(this->HasFailure());

    auto diag = ExpectOkDiagnostics();
    auto file = Traits::MakeFile(Traits::LoadFileArgument(test_lib), diag);

    auto headers = elfldltl::LoadHeadersFromFile<Elf>(
        diag, file,
        elfldltl::ContainerArrayFromFile<elfldltl::StdContainer<std::vector>::Container<Phdr>>(
            diag, "impossible"));
    ASSERT_TRUE(headers);
    auto& [ehdr, phdrs_result] = *headers;

    ASSERT_FALSE(phdrs_result.empty());
    result = LoadResult{
        .loader = Traits::MakeLoader(std::forward<LoaderArgs>(loader_args)...),
        .phdrs = std::move(phdrs_result),
        .phoff = ehdr.phoff,
        .entry = ehdr.entry,
    };

    std::span<const Phdr> phdrs = result->phdrs;
    ASSERT_TRUE(elfldltl::DecodePhdrs(diag, phdrs,
                                      result->info.GetPhdrObserver(result->loader.page_size()),
                                      PhdrStackObserver<Elf>{result->stack_size}));

    ASSERT_TRUE(Traits::Normalize(diag, result->info, file, result->loader));

    ASSERT_TRUE(result->loader.Load(diag, result->info, Traits::LoadFileArgument(test_lib)));
  }

  template <typename... LoaderArgs>
  void Load(std::string_view name, std::optional<LoadResult>& result, LoaderArgs&&... loader_args) {
    ASSERT_NO_FATAL_FAILURE(
        Load(Traits::TestLibProvider(name), result, std::forward<LoaderArgs>(loader_args)...));
  }
};

}  // namespace elfldltl::testing

#endif  // SRC_LIB_ELFLDLTL_TESTING_INCLUDE_LIB_ELFLDLTL_TESTING_LOADER_H_
