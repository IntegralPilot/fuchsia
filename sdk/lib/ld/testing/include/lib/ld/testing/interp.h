// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TESTING_INTERP_H_
#define LIB_LD_TESTING_INTERP_H_

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/file.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/testing/diagnostics.h>

#include <filesystem>
#include <span>
#include <string>

#include <gtest/gtest.h>

namespace ld::testing {

using elfldltl::testing::ExpectOkDiagnostics;

// This does a minimal Ehdr decoding and Phdr scan on any ELF format file just
// enough to read out a PT_INTERP string if present.  A empty std::string is
// returned if there was none, and gtest failures are logged for other issues.
//
// The required template parameter is the elfldltl::FileApi type and the
// deducible second parameter is the argument type for the File constructor
// that takes a Diagnostics objects as its second argument.
template <template <class Diagnostics> class File, typename FileArg>
  requires elfldltl::FileWithDiagnosticsApi<  //
      File<ExpectOkDiagnostics>, ExpectOkDiagnostics, FileArg>
inline std::string FindInterp(FileArg&& file_arg) {
  std::string result;
  ExpectOkDiagnostics diag;
  File file{std::forward<FileArg>(file_arg), diag};
  auto scan_phdrs = [&diag, &file, &result](const auto& ehdr, const auto& phdrs) -> bool {
    for (const auto& phdr : phdrs) {
      if (phdr.type == elfldltl::ElfPhdrType::kInterp) {
        size_t len = phdr.filesz;
        EXPECT_GT(len, 0u) << "empty PT_INTERP";
        if (len > 0) {
          auto read_chars = file.template ReadArrayFromFile<char>(
              phdr.offset,
              elfldltl::ContainerArrayFromFile<
                  elfldltl::StdContainer<std::vector>::Container<char>>(diag, "impossible"),
              len - 1);
          if (read_chars) {
            std::span<const char> chars = *read_chars;
            result = std::string_view{chars.data(), chars.size()};
            return true;
          }
        }
        return false;
      }
    }
    return true;
  };
  auto phdr_allocator = [&diag]<typename T>(size_t count) {
    return elfldltl::ContainerArrayFromFile<elfldltl::StdContainer<std::vector>::Container<T>>(
        diag, "impossible")(count);
  };
  EXPECT_TRUE(elfldltl::WithLoadHeadersFromFile(diag, file, phdr_allocator, scan_phdrs,
                                                std::nullopt, std::nullopt));
  return result;
}

// This extracts the prefix (before '/') from a PT_INTERP string and optionally
// registers gtest failures if it doesn't match the expected value.
std::optional<std::string> ConfigFromInterp(  //
    const std::filesystem::path& interp,
    std::optional<std::string_view> expected_config = std::nullopt);

}  // namespace ld::testing

#endif  // LIB_LD_TESTING_INTERP_H_
