// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_BOOT_OPTIONS_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_BOOT_OPTIONS_H_

#include <ktl/byte.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/string_view.h>

#include "early-boot.h"

struct BootOptions;

// Before this BootOptions::Get() returns nullptr.  It's called just before the
// BootOptions will be properly filled.
void InstallBootOptions(const BootOptions*);

// Sets the given boot-options with the specifications encoded in the given ZBI, as well in an
// additional legacy command-line when relevant.
//
// This function does not explicitly modify global state. It is the
// responsibility of the caller to use InstallBootOptions() and call
// `SetUartConsole(boot_options.serial)`.
//
// |legacy_cmdline| override cmdline items contained in the ZBI.
//
// Given a uart specification provided by multiple sources such as lgeacy uart driver,
// ZBI UART driver item, ZBI cmdline item and legacy command-line the priority for determining
// which to use is the following:
//
// (1) Legacy cmdline (e.g. boot loader command-line)
// (2) ZBI cmdline item.
// (3) ZBI UART driver item.
// (4) Legacy UART driver (e.g. ACPI or devicetree).
//
// It is expected that |boot_opts.serial| is set to legacy UART driver(if any) in order to preserve
// this priority.
//
void SetBootOptions(BootOptions& boot_opts, ktl::optional<EarlyBootZbi> zbi,
                    ktl::string_view legacy_cmdline = {});

// Just like |SetBootOptions| but leaves the entropy from the command-line if
// available, which will restore the entropy bits. This allows to reuse BootOptions infratructure
// for parsing specific arguments.
void SetBootOptionsWithoutEntropy(BootOptions& boot_opts, ktl::optional<EarlyBootZbi> zbi,
                                  ktl::string_view legacy_cmdline = {});

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_BOOT_OPTIONS_H_
