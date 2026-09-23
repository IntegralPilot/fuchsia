// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <phys/address-space.h>
#include <phys/elf-image.h>

// This method is in a separate file so that users of the phys:elf-image
// static_library() need not link in phys:address-space if they don't use it.

fit::result<AddressSpace::MapError> ElfImage::MapInto(MapSegmentFunction map_segment) const {
  fit::result<AddressSpace::MapError> result = fit::ok();
  load_info().VisitSegments([&](const auto& segment) {
    const arch::AccessPermissions access_perms = {
        .readable = segment.readable(),
        .writable = segment.writable(),
        .executable = segment.executable(),
    };
    result = map_segment(segment.vaddr() + load_bias(), segment.offset(),  //
                         segment.filesz(), segment.memsz(), access_perms);
    return result.is_ok();
  });
  return result;
}

fit::result<AddressSpace::MapError> ElfImage::MapInto(AddressSpace& aspace) const {
  auto map = [this, &aspace](uintptr_t vaddr, size_type offset,  //
                             size_type filesz, size_type memsz,
                             arch::AccessPermissions permissions) {
    // This requires that the full memsz is already available in the image()
    // with the filesz..memsz bytes already zeroed as Load() will have done.
    ZX_DEBUG_ASSERT(offset + memsz <= memory_image().size_bytes());
    return aspace.Map(vaddr, memsz, physical_load_address() + offset,
                      AddressSpace::NormalMapSettings(permissions));
  };
  return MapInto(map);
}
