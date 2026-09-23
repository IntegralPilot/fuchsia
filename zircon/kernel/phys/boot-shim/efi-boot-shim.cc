// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/efi-boot-shim.h>
#include <lib/zbitl/error-stdio.h>

#include <ktl/string_view.h>
#include <ktl/utility.h>
#include <phys/efi/efi-boot-zbi.h>
#include <phys/efi/file.h>
#include <phys/efi/main.h>
#include <phys/symbolize.h>

#include <ktl/enforce.h>

namespace {

constexpr ktl::string_view kDefaultZbiFilename = DEFAULT_ZBI_FILENAME;

using Shim = boot_shim::EfiBootShim<>;

}  // namespace

int main(int argc, char** argv) {
  MainSymbolize symbolize(argc == 0 ? "efi-boot-shim" : argv[0]);

  Shim shim(symbolize.name());

  ktl::string_view filename;
  if (argc == 0) {
    filename = kDefaultZbiFilename;
  } else if (argc == 1) {
    printf("Usage: %s PATH.zbi\n", argv[0]);
    return 1;
  } else {
    filename = argv[1];
  }

  printf("%s: Loading ZBI from file \"%.*s\"...\n", shim.shim_name(),
         static_cast<int>(filename.size()), filename.data());

  EfiFilePtr file;
  if (auto result = EfiOpenFile(filename); result.is_ok()) {
    file = ktl::move(result).value();
  } else {
    shim.Check("Cannot open ZBI file", result);
    return 1;
  }

  if (shim.Check("Failed to collect UEFI data", shim.Init(gEfiSystemTable))) {
    EfiBootZbi boot(ktl::move(file));
    auto error =
        shim.LoadAndBoot(gEfiSystemTable->BootServices, gEfiImageHandle, boot.LoadFunction(),
                         boot.LastChanceFunction(), boot.BootFunction());
    printf("%s: Failed to load ZBI: ", shim.shim_name());
    zbitl::PrintViewError(error);
    printf("\n");
    boot.DataZbi().ignore_error();
  }

  return 1;
}
