// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "phys/boot-zbi.h"

#include <inttypes.h>
#include <lib/fit/defer.h>
#include <lib/memalloc/range.h>
#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/item.h>

#include <ktl/byte.h>
#include <ktl/optional.h>
#include <ktl/string_view.h>
#include <ktl/utility.h>
#include <phys/allocation.h>
#include <phys/main.h>
#include <phys/stdio.h>
#include <phys/zbi.h>
#include <pretty/cpp/sizes.h>

#include <ktl/enforce.h>

namespace {

// zbitl::PrintViewCopyError only uses the read_offset if read_error is not
// nullopt, and only uses the write_offset if write_error is not nullopt.  So
// they must be set even though the error_type confers no information.

constexpr fit::error<BootZbi::Error> ReadError(ktl::string_view error, uint32_t offset) {
  return fit::error{BootZbi::Error{
      .zbi_error = error,
      .read_offset = offset,
      .read_error = BootZbi::Error::ReadError{},
  }};
}

constexpr fit::error<BootZbi::Error> WriteError(ktl::string_view error, uint32_t offset) {
  return fit::error{BootZbi::Error{
      .zbi_error = error,
      .write_offset = offset,
      .write_error = BootZbi::Error::WriteError{},
  }};
}

constexpr fit::error<BootZbi::Error> InputError(BootZbi::InputZbi::Error error) {
  return ReadError(error.zbi_error, error.item_offset);
}

constexpr fit::error<BootZbi::Error> EmptyZbi(fit::result<BootZbi::InputZbi::Error> result) {
  if (result.is_error()) {
    return InputError(result.error_value());
  }
  return fit::error{BootZbi::Error{"empty ZBI"}};
}

constexpr fit::error<BootZbi::Error> OutputError(BootZbi::Zbi::Error error) {
  return WriteError(error.zbi_error, error.item_offset);
}

constexpr fit::error<BootZbi::Error> OutputError(
    BootZbi::InputZbi::CopyError<BootZbi::Bytes> error) {
  return fit::error{BootZbi::Error{
      .zbi_error = error.zbi_error,
      .read_offset = error.read_offset,
      .read_error = BootZbi::Error::ReadError{},
      .write_offset = error.write_offset,
      .write_error = BootZbi::Error::WriteError{},
  }};
}

const ZbiKernelImage* GetZirconKernel(const ktl::byte* kernel_payload) {
  return reinterpret_cast<const ZbiKernelImage*>(
      // The payload is the kernel item contents, i.e. the zbi_kernel_t header
      // followed by the rest of the load image.  But the actual kernel load
      // image for purposes of address arithmetic is defined as being the whole
      // ZBI container, i.e. the whole ZbiKernelImage enchilada that has the
      // ZBI file (container) zbi_header_t followed by the kernel item's
      // zbi_header_t followed by that payload.  In a proper bootable ZBI, the
      // kernel item must be first and so kernel_ could always just be set to
      // zbi_.storage().data().  However, Init() permits synthetic
      // ZBI_TYPE_DISCARD items at the start to be left by previous boot shim
      // code and hence the kernel item might not be the first item in the
      // container here. So, instead calculate the offset back from this
      // payload in memory to where the beginning of the whole container would
      // be: thus the ZbiKernelImage pointer here finds the zbi_kernel_t
      // payload in the right place in memory, and the kernel item zbi_header_t
      // before it.  Nothing in the kernel boot protocol actually cares about
      // looking at the container zbi_header_t (or the kernel item
      // zbi_header_t, for that matter), they are just accounted for in the
      // address arithmetic to simplify the normal way a boot loader does the
      // loading.  The later uses of kernel_ in Load() and elsewhere likewise
      // don't care about those headers, only about the zbi_kernel_t portion
      // and the aligned physical memory address that corresponds to the
      // ZbiKernelImage pointer.  Unlike the formal ZBI boot protocol, the
      // Load() code handles the case where this address is not properly
      // aligned for the kernel handoff; but in the likely event that this
      // initial address is actually aligned, Load() may be able to avoid
      // additional memory allocation and copying.
      kernel_payload - (2 * sizeof(zbi_header_t)));
}

}  // namespace

BootZbi::Size BootZbi::SuggestedAllocation(uint32_t zbi_size_bytes) {
  return {.size = zbi_size_bytes, .alignment = kArchZbiKernelAlignment};
}

BootZbi::Size BootZbi::GetKernelAllocationSize(BootZbi::Zbi::iterator kernel_item) {
  ZX_ASSERT(kernel_item->header->type == ZBI_TYPE_STORAGE_KERNEL);
  uint64_t kernel_without_reserve = zbitl::UncompressedLength(*kernel_item->header);
  auto zircon_kernel = GetZirconKernel(kernel_item->payload.data());
  return SuggestedAllocation(static_cast<uint32_t>(kernel_without_reserve +
                                                   zircon_kernel->data_kernel.reserve_memory_size));
}

void BootZbi::InitKernelFromItem() { SetKernel(GetZirconKernel(kernel_item_->payload.data())); }

void BootZbi::InitKernel(Allocation kernel) {
  kernel_buffer_ = ktl::move(kernel);
  zbi_ = InputZbi{kernel_buffer_.data()};
  kernel_item_ = zbi_.begin();
  zbi_.ignore_error();
  InitKernelFromItem();
}

void BootZbi::InitData(Allocation data) {
  data_buffer_ = ktl::move(data);
  data_ = Zbi{data_buffer_.data()};
}

fit::result<BootZbi::Error> BootZbi::Init(InputZbi arg_zbi) {
  InitZbi(arg_zbi);
  auto it = zbi_.begin();
  if (it == zbi_.end()) {
    return EmptyZbi(zbi_.take_error());
  }

  while (it != zbi_.end()) {
    auto [header, payload] = *it;

    switch (header->type) {
      case kArchZbiKernelType: {
        kernel_item_ = it;
        // Valid kernel_item implies no iteration error.
        zbi_.ignore_error();
        InitKernelFromItem();
        return fit::ok();
      }

      case ZBI_TYPE_DISCARD:
        // A boot shim might leave a dummy item at the start.  Allow it.
        ++it;
        continue;
    }
    // Any other item should not be the first item seen.
    break;
  }

  if (auto result = zbi_.take_error(); result.is_error()) {
    return InputError(result.error_value());
  }

  return ReadError(
      "ZBI does not start with valid kernel item",
      it == zbi_.end() ? static_cast<uint32_t>(sizeof(zbi_header_t)) : it.item_offset());
}

fit::result<BootZbi::Error> BootZbi::Init(InputZbi arg_zbi, InputZbi::iterator kernel_item) {
  InitZbi(arg_zbi);

  kernel_item_ = zbi_.begin();
  while (true) {
    if (kernel_item_ == zbi_.end()) {
      return InputError(zbi_.take_error().error_value());
    }
    if (kernel_item_.item_offset() == kernel_item.item_offset()) {
      break;
    }
    ++kernel_item_;
  }
  // Valid kernel_item implies no error.
  zbi_.ignore_error();
  InitKernelFromItem();
  return fit::ok();
}

bool BootZbi::KernelCanLoadInPlace() const {
  // The kernel (container header) must be aligned as per the ZBI protocol.
  if (KernelLoadAddress() % kArchZbiKernelAlignment != 0) {
    return false;
  }

  // If we have relocated the kernel, then it will already be in-place.
  if (kernel_buffer_) {
    ZX_DEBUG_ASSERT(kernel_buffer_.size_bytes() >= KernelMemorySize());
    return true;
  }

  // The incoming ZBI must supply enough reusable headroom for the kernel.
  uint32_t in_place_start =
      kernel_item_.item_offset() - static_cast<uint32_t>(sizeof(zbi_header_t));
  uint32_t in_place_space = static_cast<uint32_t>(zbi_.storage().size()) - in_place_start;
  return in_place_space >= KernelMemorySize();
}

bool BootZbi::FixedKernelOverlapsData(uint64_t kernel_load_address) const {
  uint64_t start1 = kernel_load_address;
  uint64_t start2 = reinterpret_cast<uintptr_t>(data_.storage().data());
  uint64_t end1 = start1 + KernelMemorySize();
  uint64_t end2 = start2 + data_.storage().size();
  return start1 <= start2 ? start2 < end1 : start1 < end2;
}

fit::result<BootZbi::Error> BootZbi::Load(uint32_t extra_data_capacity,
                                          ktl::optional<uintptr_t> kernel_load_address,
                                          ktl::optional<uintptr_t> data_load_address) {
  ZX_ASSERT(data_.storage().empty());

  auto input_address = reinterpret_cast<uintptr_t>(zbi_.storage().data());
  auto input_capacity = zbi_alloc_->size_bytes();

  auto it = kernel_item_;
  ++it;
  zbi_.ignore_error();

  // Init() has identified the kernel item in the input ZBI.  We now have an
  // image in memory and know what its pieces are:
  //
  //  zbi_.storage().data() -> offset 0: zbi_header_t (ZBI_TYPE_CONTAINER)
  //                          ~~~
  //         kernel_item_.item_offset(): zbi_header_t (ZBI_TYPE_KERNEL_*)
  //                  .payload_offset(): zbi_kernel_t
  //                                     ...kernel load image...
  //                   it.item_offset(): zbi_header_t       (first data item)
  //                  .payload_offset(): data payload...    (first data item)
  //                                     ...                (first data item)
  //                                     zbi_header_t       (second data item)
  //                                     data payload...    (second data item)
  //                                     ...                (second data item)
  //                                          .
  //                                          .
  //                                          .
  //                                     zbi_header_t       (last data item)
  //                                     data payload ...   (last data item)
  //  input_address + zbi_.size_bytes(): <end of input ZBI as loaded>
  //                          ~~~
  //  input_address +    input_capacity: <end of known-available extra memory>
  //
  // To meet the ZBI boot protocol, we're transforming that into two separate
  // contiguous blocks of memory, each of which can be located anywhere.
  //
  // Legend: KLA = KernelLoadAddress(), KH = KernelHeader()
  //         KLS = KernelLoadSize(), KMS = KernelMemorySize()
  //         DLA = DataLoadAddress(), DLS = DataLoadSize()
  //         EDC = extra_data_capacity argument to Load()
  //
  // Kernel memory image, aligned to kArchZbiKernelAlignment:
  //
  //                     KLA +  0: zbi_header_t (ZBI_TYPE_CONTAINER, ignored)
  //                     KLA + 32: zbi_header_t (ZBI_TYPE_KERNEL_*, ignored)
  //                KH = KLA + 64: zbi_kernel_t
  //                               ...start of kernel load image proper...
  //                               .
  //              KLA + KH->entry: kernel entry point instruction
  //                               .
  //                               ...more kernel load image...
  //                               .
  //                    KLA + KLS: ...zbi_kernel_t.reserve_memory_size bytes...
  //                    KLA + KMS: <end of kernel memory image>
  //
  // Data ZBI image, aligned to kArchZbiDataAlignment:
  //
  //                     DLA +  0: zbi_header_t       (ZBI_TYPE_CONTAINER)
  //                     DLA + 32: zbi_header_t       (first data item)
  //                     DLA + 64: data payload...    (first data item)
  //                               ...                (first data item)
  //                               zbi_header_t       (second data item)
  //                               data payload...    (second data item)
  //                               ...                (second data item)
  //                                    .
  //                                    .
  //                                    .
  //                               zbi_header_t       (last original data item)
  //                               data payload ...   (last original data item)
  //                    DLA + DLS: <zbi_header_t>     (caller fills in later)
  //                               <data payload...>  (caller fills in later)
  //                                    .
  //                                    .
  //                                    .
  //                               <zbi_header_t>     (last shim-added item)
  //                               <data payload...>  (last shim-added item)
  //              DLA + DLS + EDC: <end of DataZbi().storage() capacity>
  //
  // In a proper bootable ZBI in its original state, the kernel item must be
  // the first item so kernel_item_.item_offset() is 32 (sizeof(zbi_header_t)).
  // However, this code supports uses in boot shims that had other fish to fry
  // first and so Init() allowed any number of ZBI_TYPE_DISCARD items at the
  // start before kernel_item_.  In that case kernel_ now points 32 bytes back
  // into the ~~~ discard area.  When there are no discard items, then kernel_
  // now points directly at the start of the container; if this is already
  // aligned to kArchZbiKernelAlignment, then it may be possible to use
  // it where it is.
  //
  // In the kernel memory image, the ZBI headers and the zbi_kernel_t payload
  // header are only there for the convenience of the boot loader (or shim,
  // i.e. this code).  The loaded kernel code doesn't actually care about any
  // what's in that memory.  It's just part of the address arithmetic for the
  // kernel to calculate its own aligned load address (KLA) from the runtime PC
  // value at its entry point (KLA + KH->entry).  However, for the data ZBI,
  // the container header must be well-formed and give the correct length since
  // it is the only means to communicate the size of the data ZBI, and all data
  // item headers must be well-formed items understood (or at least tolerated)
  // by the system being booted.  (Items added by boot loaders do not
  // necessarily having strictly valid ZBI item headers beyond the basic length
  // and type fields, so permissive checking mode is used.)
  //
  // In the general case, we can just allocate two new separate blocks of
  // memory and copy the kernel and data images into them respectively.  But
  // when possible we can optimize the total memory use by reusing some or all
  // of the space where the input ZBI (and any excess capacity known to follow
  // it) sits, and/or optimize CPU time by leaving either the kernel load image
  // or the data items' image where it is rather than copying it.  This code
  // calculates what options are available based on the sizes and alignments
  // required and then chooses the option that copies the fewest bytes.  To
  // increase the cases where copying can be avoided, this can yield a data ZBI
  // that actually has a ZBI_TYPE_DISCARD item inserted before the first real
  // data item from the input ZBI just to make alignment arithmetic line up.

  uintptr_t data_address = 0, aligned_data_address = 0;
  uint32_t data_load_size = sizeof(zbi_header_t);
  if (it != zbi_.end()) {
    data_address = input_address + it.item_offset() - sizeof(zbi_header_t);
    aligned_data_address = data_address & -kArchZbiDataAlignment;
    data_load_size =
        static_cast<uint32_t>(zbi_.size_bytes() - it.item_offset()) + sizeof(zbi_header_t);
  }

  // There must be a container header for the data ZBI even if it's empty.
  const uint32_t data_required_size = data_load_size + extra_data_capacity;
  // The incoming space can be reused for the data ZBI if either the tail is
  // already exactly aligned to leave space for a header with correct
  // alignment, or there's enough space to insert a ZBI_TYPE_DISCARD item after
  // an aligned header.
  if (data_address != 0 && data_address % kArchZbiDataAlignment == 0) {
    // It so happens it's perfectly aligned to use the whole thing in place.
    // The lower pages used for the kernel image will just be skipped over.
    data_.storage() = {
        reinterpret_cast<ktl::byte*>(data_address),
        input_capacity - (data_address - input_address),
    };
  } else if (aligned_data_address > input_address &&
             data_address - aligned_data_address >= (2 * sizeof(zbi_header_t))) {
    // Aligning down leaves enough space to insert a ZBI header to consume
    // the remaining space with a ZBI_TYPE_DISCARD item so the actual
    // contents can be left in place.
    data_.storage() = {
        reinterpret_cast<ktl::byte*>(aligned_data_address),
        input_capacity - (aligned_data_address - input_address),
    };
  }

  if (kernel_load_address && FixedKernelOverlapsData(*kernel_load_address)) {
    // There's a fixed kernel load address, so the data ZBI cannot be allowed
    // to reuse the memory where it will go.  This memory will already have
    // been reserved from the allocator, but the incoming data might be there.
    data_.storage() = {};
  }

  // Pick the largest one to keep in place.
  if (KernelCanLoadInPlace() && KernelLoadSize() > data_load_size) {
    data_.storage() = {};
  }

  // Try to extend the allocation in place to fit the extra bytes, if needed.
  if (!data_.storage().empty() &&
      // Note that
      //  * `data_address` points to the start of the item following the kernel ZBI in the input
      //  ZBI.
      //  * `aligned_data_address` includes `extra` items prepended to the data ZBI, to avoid
      //  copying
      //     it around to meet alignment requirements (e.g. ZBI_TYPE_DISCARD).
      // This means that `data_.storage()` include these extra bytes, meaning that it has increased
      // the amount of bytes available for this purpose. But these extra bytes are not useful for
      // the `extra_capacity` because these must be allocated at the tail.
      //
      // We can obtain the amount of bytes available at the tail by compensating for the aligned
      // start, if it happen to be aligned from the start, then this term becomes 0.
      (aligned_data_address + data_.storage().size_bytes()) - data_address < data_required_size) {
    fbl::AllocChecker ac;

    size_t new_size =
        fbl::round_up((data_address + data_required_size) - input_address, Allocation::PageSize());
    // Extend the allocation to fit this space.
    zbi_alloc_.Extend(ac, new_size);

    if (!ac.check()) {
      data_.storage() = {};
    } else {
      data_.storage() = {
          data_.storage().data(),
          zbi_alloc_.size_bytes() + input_address - aligned_data_address,
      };
    }
  }

  // If we are relocating the data zbi, and the destination data overlaps with the kernel current
  // location, we need to relocate the kernel image, to avoid clubbering the kernel data, by copying
  // the data zbi over it.
  bool relocated_data_overlaps_with_kernel =
      data_load_address.has_value() &&
      *data_load_address >= KernelLoadAddress() + KernelLoadSize() - DataLoadSize();

  if (!KernelCanLoadInPlace() || !data_.storage().empty() || relocated_data_overlaps_with_kernel) {
    // Allocate space for the kernel image and copy it in.
    fbl::AllocChecker ac;
    kernel_buffer_ =
        Allocation::New(ac, memalloc::Type::kKernel, static_cast<size_t>(KernelMemorySize()),
                        kArchZbiKernelAlignment);
    if (!ac.check()) {
      return WriteError("cannot allocate memory for kernel image",
                        static_cast<uint32_t>(KernelMemorySize()));
    }
    memcpy(kernel_buffer_.get(), KernelImage(), KernelLoadSize());
    kernel_ = reinterpret_cast<ZbiKernelImage*>(kernel_buffer_.get());
  }

  if (data_.storage().empty()) {
    // Allocate new space for the data ZBI and copy it over.
    fbl::AllocChecker ac;
    data_buffer_ =
        Allocation::New(ac, memalloc::Type::kDataZbi, data_required_size, kArchZbiDataAlignment);
    if (!ac.check()) {
      return WriteError("cannot allocate memory for data ZBI", data_required_size);
    }
    data_.storage() = data_buffer_.data();
    if (auto result = data_.clear(); result.is_error()) {
      return OutputError(result.error_value());
    }
    if (auto result = data_.Extend(it, zbi_.end()); result.is_error()) {
      return OutputError(result.error_value());
    }
  } else if (data_address % kArchZbiDataAlignment == 0) {
    // The data ZBI is perfect where it is.  Just overwrite where the end
    // of the kernel item was copied from with the new container header.
    auto hdr = reinterpret_cast<zbi_header_t*>(data_.storage().data());
    hdr[0] = zbitl::ContainerHeader(static_cast<uint32_t>(data_load_size - sizeof(zbi_header_t)));
  } else {
    // There's an aligned spot before the data ZBI's first item where we can
    // insert both a new container header and an item header to sop up the
    // remaining space before the first item without copying any data.
    auto hdr = reinterpret_cast<zbi_header_t*>(data_.storage().data());
    auto discard_size = data_address - aligned_data_address - sizeof(hdr[1]);
    auto data_size = data_load_size + sizeof(hdr[1]) + discard_size;
    ZX_ASSERT(aligned_data_address > input_address);
    ZX_ASSERT(data_address > aligned_data_address);
    ZX_ASSERT(data_address - aligned_data_address >= sizeof(hdr[1]));
    ZX_ASSERT(discard_size < data_size);
    hdr[0] = zbitl::ContainerHeader(static_cast<uint32_t>(data_size - sizeof(zbi_header_t)));
    hdr[1] = zbitl::SanitizeHeader({
        .type = ZBI_TYPE_DISCARD,
        .length = static_cast<uint32_t>(discard_size),
    });
  }

  ZX_ASSERT(KernelCanLoadInPlace());
  ZX_ASSERT(data_.storage().size() >= data_required_size);
  ZX_ASSERT(data_.storage().size() - data_.size_bytes() >= extra_data_capacity);
  return fit::ok();
}

void BootZbi::Log() {
  LogAddresses();
  LogBoot(KernelEntryAddress());
}

[[noreturn]] void BootZbi::Boot(ktl::optional<void*> argument) {
  ZX_ASSERT_MSG(KernelCanLoadInPlace(), "Has Load() been called?");
  ZbiBoot(static_cast<uintptr_t>(KernelEntryAddress()), argument.value_or(data_.storage().data()));
}

#define ADDR "0x%016" PRIx64

void BootZbi::LogAddresses() const {
  debugf("%s:    Kernel @ [" ADDR ", " ADDR ")  %s\n", ProgramName(), KernelLoadAddress(),
         KernelLoadAddress() + KernelLoadSize(), pretty::FormattedBytes(KernelLoadSize()).c_str());
  debugf("%s:       BSS @ [" ADDR ", " ADDR ")  %s\n", ProgramName(),
         KernelLoadAddress() + KernelLoadSize(), KernelLoadAddress() + KernelMemorySize(),
         pretty::FormattedBytes(static_cast<size_t>(KernelHeader()->reserve_memory_size)).c_str());
  debugf("%s:       ZBI @ [" ADDR ", " ADDR ")  %s\n", ProgramName(), DataLoadAddress(),
         DataLoadAddress() + DataLoadSize(),
         pretty::FormattedBytes(static_cast<size_t>(DataLoadSize())).c_str());
}

void BootZbi::LogBoot(uint64_t entry) {
  debugf("%s:     Entry @  " ADDR "  Booting...\n", ProgramName(), entry);
}

void BootZbi::InitZbi(BootZbi::InputZbi input_zbi) {
  zbi_ = ktl::move(input_zbi);

  ZX_ASSERT(reinterpret_cast<uintptr_t>(input_zbi.storage().data()) % Allocation::PageSize() == 0);
  // Take ownership of the allocation containing the zbi.
  zbi_alloc_ = Allocation::Adopt(memalloc::Range{
      // ZBI MUST be page aligned.
      .addr = reinterpret_cast<uint64_t>(input_zbi.storage().data()),
      .size = input_zbi.storage().size_bytes(),
      // Force it to be `kDataZbi`
      .type = memalloc::Type::kDataZbi});
}
