// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/container.h>
#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/phdr.h>
#include <lib/elfldltl/testing/fuzzer.h>
#include <zircon/assert.h>

#include <vector>

namespace {

constexpr size_t kPageSize = 0x1000;

constexpr auto segment_less = [](const auto& first, const auto& second) -> bool {
  // Each argument is of a std::variant<...> type and they may or may not
  // both be the same variant.
  return std::visit(
      [](const auto& first, const auto& second) -> bool {
        return first.vaddr() + first.memsz() <= second.vaddr();
      },
      first, second);
};

template <class Elf>
struct LoadInfoFuzzer {
  using size_type = typename Elf::size_type;
  using Phdr = typename Elf::Phdr;
  using LoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StdContainer<std::vector>::Container>;

  using FuzzerInputs = elfldltl::testing::FuzzerInput<alignof(Phdr), Phdr, uint8_t>;

  int operator()(FuzzedDataProvider& provider) const {
    FuzzerInputs inputs(provider);
    auto [phdrs, blob] = inputs.inputs();

    elfldltl::Diagnostics diag{[](auto&&...) -> bool { return true; }};
    LoadInfo info;
    elfldltl::DecodePhdrs(diag, phdrs, info.GetPhdrObserver(kPageSize));

    // Verify the segments are in ascending vaddr order.
    ZX_ASSERT(std::is_sorted(info.segments().begin(), info.segments().end(), segment_less));

    // Verify no empty segments.
    for (const auto& segment : info.segments()) {
      std::visit([](const auto& segment) { ZX_ASSERT(segment.memsz() > 0); }, segment);
    }

    // Fuzz FindSegment.
    FuzzedDataProvider blob_provider(blob.data(), blob.size());
    while (blob_provider.remaining_bytes() > 0) {
      size_type vaddr = blob_provider.ConsumeIntegral<size_type>();
      auto found = info.FindSegment(vaddr);
      if (found == info.segments().end()) {
        // Not found, verify no segment matches.
        for (const auto& segment : info.segments()) {
          std::visit(
              [vaddr](const auto& segment) {
                ZX_ASSERT(vaddr < segment.vaddr() || vaddr >= segment.vaddr() + segment.memsz());
              },
              segment);
        }
      } else {
        // Verify the found segment does match.
        std::visit(
            [vaddr](const auto& segment) {
              ZX_ASSERT(segment.vaddr() <= vaddr);
              ZX_ASSERT(vaddr - segment.vaddr() < segment.memsz());
            },
            *found);
      }
    }

    return 0;
  }
};

using Fuzzer = elfldltl::testing::ElfFuzzer<LoadInfoFuzzer>;

}  // namespace

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  FuzzedDataProvider provider(data, size);
  return Fuzzer{}(provider);
}
