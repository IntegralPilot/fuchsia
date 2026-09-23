// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_TEST_DEBUGDATA_PROPAGATION_DEBUGDATA_INFO_H_
#define ZIRCON_KERNEL_PHYS_TEST_DEBUGDATA_PROPAGATION_DEBUGDATA_INFO_H_

#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/item.h>

#include <array>
#include <string_view>

struct Debugdata {
  std::string_view sink;
  std::string_view payload;
  std::string_view log;
  std::string_view vmo_name;

  constexpr uint32_t size() const {
    return static_cast<uint32_t>(sink.size() + payload.size() + log.size() + vmo_name.size());
  }

  constexpr uint32_t aligned_size() const { return zbitl::AlignedPayloadLength(size()); }
};

inline constexpr auto kDebugdataItems = std::to_array<Debugdata>({
    {
        .sink = "debug-data-sink-1",
        .payload = "Hello World!\n",
        .vmo_name = "kernel/some_file.debugdata",
    },
    {
        .sink = "debug-data-sink-2",
        .payload = "Good Bye World!\n",
        .log = "Hello\nThis\nis a multiline\nlog\nBye\nnow!\n",
        .vmo_name = "kernel/some_file_2.debugdata",
    },
});

#endif  // ZIRCON_KERNEL_PHYS_TEST_DEBUGDATA_PROPAGATION_DEBUGDATA_INFO_H_
