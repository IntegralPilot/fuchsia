// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "phys/boot-options.h"

#include <lib/boot-options/boot-options.h>
#include <lib/boot-options/word-view.h>
#include <lib/uart/all.h>
#include <lib/uart/sync.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/view.h>

#include <explicit-memory/bytes.h>
#include <ktl/algorithm.h>
#include <ktl/optional.h>
#include <ktl/string_view.h>
#include <ktl/utility.h>

#include <ktl/enforce.h>

namespace {

const BootOptions* gBootOptions;

}  // namespace

void InstallBootOptions(const BootOptions* boot_options) { gBootOptions = boot_options; }

const BootOptions* BootOptions::Get() { return gBootOptions; }

void SetBootOptions(BootOptions& boot_opts, ktl::optional<EarlyBootZbi> zbi,
                    ktl::string_view legacy_cmdline) {
  if (zbi) {
    // Select UART configuration from a UART driver item in the ZBI.
    for (auto [header, wrapped_payload] : *zbi) {
      if (header->type == ZBI_TYPE_KERNEL_DRIVER) {
        ktl::span payload = wrapped_payload.get();
        if (ktl::optional config = uart::all::Config<>::Match(*header, payload.data())) {
          boot_opts.serial = *config;
        }
      }
    }
    zbi->ignore_error();

    // Select UART configuration from cmdline item in the ZBI.
    for (auto [header, wrapped_payload] : *zbi) {
      if (header->type == ZBI_TYPE_CMDLINE) {
        ktl::span payload = wrapped_payload.get();
        boot_opts.SetMany({reinterpret_cast<const char*>(payload.data()), payload.size()});
      }
    }
    zbi->ignore_error();
  }

  // At last the bootloader provided arguments trumps everything.
  boot_opts.SetMany(legacy_cmdline);
}

void SetBootOptionsWithoutEntropy(BootOptions& boot_opts, ktl::optional<EarlyBootZbi> zbi,
                                  ktl::string_view legacy_cmdline) {
  SetBootOptions(boot_opts, ktl::move(zbi), legacy_cmdline);
  // Restore the entropy bits.
  // We only use boot-options parsing for kernel.serial and ignore the rest.
  // But it destructively scrubs the RedactedHex input so we have to undo that.
  if (boot_opts.entropy_mixin.len > 0) {
    // BootOptions already parsed and redacted, so put it back.
    for (auto word : WordView(legacy_cmdline)) {
      constexpr ktl::string_view kPrefix = "kernel.entropy-mixin=";
      if (word.starts_with(kPrefix)) {
        word.remove_prefix(kPrefix.length());
        memcpy(const_cast<char*>(word.data()), boot_opts.entropy_mixin.hex.data(),
               ktl::min(boot_opts.entropy_mixin.len, word.size()));
        mandatory_memset(boot_opts.entropy_mixin.hex.data(), 0, boot_opts.entropy_mixin.len);
        break;
      }
    }
  }
}
