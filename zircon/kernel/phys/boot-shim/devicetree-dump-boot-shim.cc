// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/devicetree/devicetree.h>

#include <fbl/alloc_checker.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/main.h>
#include <phys/stdio.h>
#include <phys/symbolize.h>

namespace {

constexpr const char* kShimName = "devicetree-dump-shim";

constexpr size_t Base64EncodeLen(size_t len) { return ((len + 2) / 3) * 4; }

size_t Base64Encode(const uint8_t* src, size_t len, char* dst) {
  const char alphabet[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  char* p = dst;
  for (size_t i = 0; i < len; i += 3) {
    uint32_t val =
        (src[i] << 16) | ((i + 1 < len ? src[i + 1] : 0) << 8) | (i + 2 < len ? src[i + 2] : 0);
    *p++ = alphabet[(val >> 18) & 0x3F];
    *p++ = alphabet[(val >> 12) & 0x3F];
    *p++ = (i + 1 < len) ? alphabet[(val >> 6) & 0x3F] : '=';
    *p++ = (i + 2 < len) ? alphabet[val & 0x3F] : '=';
  }
  *p = '\0';
  return static_cast<size_t>(p - dst);
}

}  // namespace

void PhysMain(void* flat_devicetree_blob, arch::EarlyTicks ticks) {
  InitStdout();
  ApplyRelocations();

  AddressSpace aspace;
  InitMemory(flat_devicetree_blob, {}, &aspace);
  MainSymbolize symbolize(kShimName);

  // At this point UART should be available, and we should just encode.
  devicetree::ByteView fdt_blob(static_cast<const uint8_t*>(flat_devicetree_blob),
                                std::numeric_limits<uintptr_t>::max());
  devicetree::Devicetree fdt(fdt_blob);

  size_t base64_size = Base64EncodeLen(fdt.size_bytes()) + 1;
  fbl::AllocChecker checker;
  Allocation base64_buffer = Allocation::New(checker, memalloc::Type::kPhysScratch, base64_size);
  if (!checker.check()) {
    printf("Failed to allocate buffer(size=%#zx) for the base64 encoding of the devicetree.",
           base64_size);
  } else {
    size_t encode_len =
        Base64Encode(reinterpret_cast<const uint8_t*>(fdt.fdt().data()), fdt.size_bytes(),
                     reinterpret_cast<char*>(base64_buffer.data().data()));

    printf("Devicetree Base64 Dump Begin encoded_bytes=%zu\n", encode_len);
    printf("%s\n", reinterpret_cast<const char*>(base64_buffer.data().data()));
    printf("\nDevicetree Base64 Dump End\n");
  }
  abort();
}
