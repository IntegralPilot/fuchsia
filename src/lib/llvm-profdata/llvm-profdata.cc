// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/llvm-profdata/llvm-profdata.h>
#include <zircon/assert.h>

#include <functional>
#include <span>

#ifndef HAVE_LLVM_PROFDATA
#error "build system regression"
#endif

// This is defined for a test build.
#ifdef HAVE_LLVM_PROFDATA_OVERRIDE
#undef HAVE_LLVM_PROFDATA
#define HAVE_LLVM_PROFDATA HAVE_LLVM_PROFDATA_OVERRIDE
#endif

#if !HAVE_LLVM_PROFDATA

// If not compiled with instrumentation at all, then all the link-time
// references in the real implementation below won't work.  So provide stubs.

void LlvmProfdata::Init(std::span<const std::byte> build_id) {}

LlvmProfdata::LiveData LlvmProfdata::DoFixedData(std::span<std::byte> data, bool match) {
  return {};
}

LlvmProfdata::LiveDataBounds LlvmProfdata::bounds() const { return {}; }

LlvmProfdata::LiveData LlvmProfdata::LiveDataForBounds(std::span<std::byte> bounds_data) const {
  return {};
}

void LlvmProfdata::CopyLiveData(LiveData data) {}

void LlvmProfdata::MergeLiveData(LiveData data) {}

void LlvmProfdata::UseLiveData(LiveData data) {}

#else  // HAVE_LLVM_PROFDATA

#include <algorithm>
#include <array>
#include <atomic>
#include <cstdint>
#include <cstring>

#include <profile/InstrProfData.inc>

namespace {

constexpr uint64_t kMagic = INSTR_PROF_RAW_MAGIC_64;

using IntPtrT = intptr_t;

enum ValueKind {
#define VALUE_PROF_KIND(Enumerator, Value, Descr) Enumerator = Value,
#include <profile/InstrProfData.inc>
};

struct __llvm_profile_data {
#define INSTR_PROF_DATA(Type, LLVMType, Name, Initializer) Type Name;
#include <profile/InstrProfData.inc>
};

struct alignas(INSTR_PROF_DATA_ALIGNMENT) VTableProfData {
#define INSTR_PROF_VTABLE_DATA(Type, LLVMType, Name, Initializer) Type Name;
#include <profile/InstrProfData.inc>
};

extern "C" {

// This is sometimes emitted by the compiler with a different value.
// The header is expected to use whichever value this had at link time.
// This supplies the default value when the compiler doesn't supply it.
[[gnu::weak]] extern const uint64_t INSTR_PROF_RAW_VERSION_VAR = INSTR_PROF_RAW_VERSION;

// The compiler emits phantom references to this as a way to ensure
// that the runtime is linked in.
extern const int INSTR_PROF_PROFILE_RUNTIME_VAR = 0;

// In relocating mode, the compiler adds this to the address of a profiling
// counter in .bss for the counter to actually update.  At startup, this is
// zero so the .bss counters get updated.  When data is being published, the
// live-published counters get copied from the .bss counters and then this is
// set so future updates are redirected to the published copy.
//
// This definition is weak in case the standard profile runtime is also linked
// in with its own definition.
[[gnu::weak]] extern uintptr_t INSTR_PROF_PROFILE_COUNTER_BIAS_VAR = 0;

#ifdef INSTR_PROF_PROFILE_BITMAP_BIAS_VAR
[[gnu::weak]] extern uintptr_t INSTR_PROF_PROFILE_BITMAP_BIAS_VAR = 0;
#endif

// These are outcalls made by the value-profiling instrumentation.  This
// runtime doesn't support value-profiling in any meaningful way.  But the
// entry points are provided so that instrumented code can be linked against
// this runtime instead of the standard runtime.  The stubs here are made weak
// in case both this and the standard runtime are linked in.

[[gnu::weak]] extern void INSTR_PROF_VALUE_PROF_FUNC(uint64_t TargetValue, void* Data,
                                                     uint32_t CounterIndex) {}

[[gnu::weak]] extern void INSTR_PROF_VALUE_PROF_MEMOP_FUNC(uint64_t TargetValue, void* Data,
                                                           uint32_t CounterIndex) {}

}  // extern "C"

// Here __PE_COFF__ really means EFI.  At link-time, it's Windows/x64 essentially.
// TODO(https://fxbug.dev/435771251): InstrProfData.inc uses #ifdef _WIN32 in upstream LLVM.
// Add __PE_COFF__ defined check when support for __PE_COFF__ lands.
#ifdef __PE_COFF__

// These magic section names don't have macros in InstrProfData.inc,
// though their ".blah$M" counterparts do.

// Merge read-write sections into .data.
#pragma comment(linker, "/MERGE:.lprfc=.data")
#pragma comment(linker, "/MERGE:.lprfd=.data")

// Do not merge .lprfn and .lcovmap into .rdata.
// `llvm-cov` must be able to find them after the fact.

// The ".blah$A" and ".blah$Z" placeholder sections get magically sorted with
// ".blah$M" in between them, so these symbols identify the bounds of the
// compiler-emitted data at link time.  Empty sections don't work in PE-COFF
// the way they do in ELF and Mach-O, so these need to waste space in the
// placeholder sections and then compensate in the pointer calculations.
#define PECOFF_SECTION_RO(section_prefix, name, type) \
  PECOFF_SECTION_IMPL(section_prefix, name, const type)
#define PECOFF_SECTION_RW(section_prefix, name, type) \
  PECOFF_SECTION_IMPL(section_prefix, name, type, write)
#define PECOFF_SECTION_IMPL(section_prefix, name, type, ...)                        \
  PECOFF_SECTION_IMPL_EDGE(section_prefix##$A, name##Begin, type, 1, ##__VA_ARGS__) \
  PECOFF_SECTION_IMPL_EDGE(section_prefix##$Z, name##End, type, 0, ##__VA_ARGS__)
#define PECOFF_SECTION_IMPL_EDGE(section_name, symbol, type, n, ...) \
  PECOFF_SECTION_PRAGMA(section(#section_name, read, ##__VA_ARGS__)) \
  [[gnu::section(#section_name)]] type symbol##_placeholder[1] = {}; \
  constexpr type* symbol = &symbol##_placeholder[n];
#define PECOFF_SECTION_PRAGMA(x) _Pragma(#x)

// This data is morally `const`, i.e. it's a RELRO case in the ELF world.  But
// there is no RELRO in PE-COFF (?) so it's just a writable section and the
// compiler wants the declaration's constness to match #pragma section above.
PECOFF_SECTION_RW(.lprfd, Data, __llvm_profile_data)

PECOFF_SECTION_RO(.lprfn, Names, char)

PECOFF_SECTION_RW(.lprfvt, VTableData, VTableProfData)
PECOFF_SECTION_RO(.lprfvns, VNames, char)

PECOFF_SECTION_RW(.lprfc, Counters, char)

PECOFF_SECTION_RW(.lprfb, Bitmap, char)

PECOFF_SECTION_RW(.lprfuc, UniformCounters, char)

#elif defined(__APPLE__)

extern "C" {

[[gnu::visibility("hidden")]] extern const __llvm_profile_data DataBegin[] __asm__(
    "section$start$__DATA$" INSTR_PROF_DATA_SECT_NAME);
[[gnu::visibility("hidden")]] extern const __llvm_profile_data DataEnd[] __asm__(
    "section$end$__DATA$" INSTR_PROF_DATA_SECT_NAME);

[[gnu::visibility("hidden")]] extern const char NamesBegin[] __asm__(
    "section$start$__DATA$" INSTR_PROF_NAME_SECT_NAME);
[[gnu::visibility("hidden")]] extern const char NamesEnd[] __asm__(
    "section$end$__DATA$" INSTR_PROF_NAME_SECT_NAME);

[[gnu::visibility("hidden")]] extern const VTableProfData VTableDataBegin[] __asm__(
    "section$start$__DATA$" INSTR_PROF_VTAB_SECT_NAME);
[[gnu::visibility("hidden")]] extern const VTableProfData VTableDataEnd[] __asm__(
    "section$end$__DATA$" INSTR_PROF_VTAB_SECT_NAME);

[[gnu::visibility("hidden")]] extern const char VNamesBegin[] __asm__(
    "section$start$__DATA$" INSTR_PROF_VNAME_SECT_NAME);
[[gnu::visibility("hidden")]] extern const char VNamesEnd[] __asm__(
    "section$end$__DATA$" INSTR_PROF_VNAME_SECT_NAME);

[[gnu::visibility("hidden")]] extern char CountersBegin[] __asm__(
    "section$start$__DATA$" INSTR_PROF_CNTS_SECT_NAME);
[[gnu::visibility("hidden")]] extern char CountersEnd[] __asm__(
    "section$end$__DATA$" INSTR_PROF_CNTS_SECT_NAME);

[[gnu::visibility("hidden")]] extern char BitmapBegin[] __asm__(
    "section$start$__DATA$" INSTR_PROF_BITS_SECT_NAME);
[[gnu::visibility("hidden")]] extern char BitmapEnd[] __asm__(
    "section$end$__DATA$" INSTR_PROF_BITS_SECT_NAME);

[[gnu::visibility("hidden")]] extern char UniformCountersBegin[] __asm__(
    "section$start$__DATA$" INSTR_PROF_QUOTE(INSTR_PROF_UCNTS_COMMON));
[[gnu::visibility("hidden")]] extern char UniformCountersEnd[] __asm__(
    "section$end$__DATA$" INSTR_PROF_QUOTE(INSTR_PROF_UCNTS_COMMON));

}  // extern "C"

#else  // Not __PE_COFF__ or __APPLE__.

#ifndef __ELF__
#error "unsupported object file format???"
#endif

extern "C" {

// ELF linkers implicitly provide __start_SECNAME and __stop_SECNAME symbols
// when there is a SECNAME output section.  If selective instrumentation causes
// no actual metadata sections to be emitted, or even if all instrumentation
// sections in the input are in GC'd groups, then there is no such output
// section and so these symbols aren't defined.  In the userland runtime, this
// is handled simply by using weak references to the symbols.  However, those
// references require GOT slots for PIC-friendly links even with hidden
// visibility since there is no way for a PC-relative relocation to be resolved
// to absolute zero to indicate a missing value.  So instead, we need to ensure
// that there will be a zero-length section of the expected name that induces
// the linker to resolve the __start_SECNAME and __stop_SECNAME symbols.
// Having an explicit empty section with SHF_GNU_RETAIN accomplishes that
// without adding anything to the actual memory image.  Since the start and
// stop symbols are equal, the loops across them will just do nothing.

#define PROFDATA_SECTION(type, begin, end, section, writable)        \
  [[gnu::visibility("hidden")]] extern type begin[] __asm__(         \
      INSTR_PROF_QUOTE(INSTR_PROF_SECT_START(section)));             \
  [[gnu::visibility("hidden")]] extern type end[] __asm__(           \
      INSTR_PROF_QUOTE(INSTR_PROF_SECT_STOP(section)));              \
  __asm__(".pushsection " INSTR_PROF_QUOTE(section) ",\"aR" writable \
                                                    "\",%progbits\n" \
                                                    ".popsection")

PROFDATA_SECTION(const __llvm_profile_data, DataBegin, DataEnd, INSTR_PROF_DATA_COMMON, "");

PROFDATA_SECTION(const char, NamesBegin, NamesEnd, INSTR_PROF_NAME_COMMON, "");

PROFDATA_SECTION(const VTableProfData, VTableDataBegin, VTableDataEnd, INSTR_PROF_VTAB_COMMON, "");
PROFDATA_SECTION(const char, VNamesBegin, VNamesEnd, INSTR_PROF_VNAME_COMMON, "");

PROFDATA_SECTION(char, CountersBegin, CountersEnd, INSTR_PROF_CNTS_COMMON, "w");

PROFDATA_SECTION(char, BitmapBegin, BitmapEnd, INSTR_PROF_BITS_COMMON, "w");

PROFDATA_SECTION(char, UniformCountersBegin, UniformCountersEnd, INSTR_PROF_UCNTS_COMMON, "w");

}  // extern "C"

#endif  // Not __PE_COFF__ or __APPLE__.

struct ProfRawHeader {
  size_t binary_ids_size() const { return static_cast<size_t>(BinaryIdsSize); }

#define INSTR_PROF_RAW_HEADER(Type, Name, Initializer) Type Name;
#include <profile/InstrProfData.inc>
};

constexpr size_t kAlignAfterBuildId = sizeof(uint64_t);

constexpr size_t PaddingSize(size_t chunk_size_bytes) {
  return (kAlignAfterBuildId - (chunk_size_bytes % kAlignAfterBuildId)) % kAlignAfterBuildId;
}

constexpr size_t PaddingSize(std::span<const std::byte> chunk) {
  return PaddingSize(chunk.size_bytes());
}

constexpr size_t BinaryIdsSize(std::span<const std::byte> build_id) {
  if (build_id.empty()) {
    return 0;
  }
  return sizeof(uint64_t) + build_id.size_bytes() + PaddingSize(build_id);
}

template <typename T>
[[gnu::const]] std::span<T> GetArray(T* begin, T* end) {
  auto begin_bytes = reinterpret_cast<const std::byte*>(begin);
  auto end_bytes = reinterpret_cast<const std::byte*>(end);
  size_t size_bytes = end_bytes - begin_bytes;
  return {begin, (size_bytes + sizeof(T) - 1) / sizeof(T)};
}

[[gnu::const]] auto ProfDataArray() { return GetArray(DataBegin, DataEnd); }

[[gnu::const]] auto VTableDataArray() { return GetArray(VTableDataBegin, VTableDataEnd); }

// This is the .bss data that gets updated live by instrumented code when the
// bias is set to zero.
[[gnu::const]] std::span<char> ProfCountersData() {
  return std::span<char>(&CountersBegin[0], CountersEnd - CountersBegin);
}

[[gnu::const]] std::span<char> ProfBitmapData() {
  return std::span<char>(&BitmapBegin[0], BitmapEnd - BitmapBegin);
}

[[gnu::const]] std::span<char> ProfUniformCountersData() {
  return std::span<char>(&UniformCountersBegin[0], UniformCountersEnd - UniformCountersBegin);
}

[[gnu::const]] size_t CountersSize() {
  if (LlvmProfdata::UsingSingleByteCounters())
    return sizeof(uint8_t);
  return sizeof(uint64_t);
}

[[gnu::const]] ProfRawHeader GetHeader(std::span<const std::byte> build_id) {
  // These are used by the INSTR_PROF_RAW_HEADER initializers.
  const uint64_t NumData = ProfDataArray().size();
  const uint64_t PaddingBytesBeforeCounters = 0;
  const uint64_t NumCounters = ProfCountersData().size() / CountersSize();
  const uint64_t PaddingBytesAfterCounters =
      static_cast<uint64_t>(PaddingSize(ProfCountersData().size()));
  const uint64_t NumBitmapBytes = ProfBitmapData().size();
  const uint64_t PaddingBytesAfterBitmapBytes = 0;
  const uint64_t NamesSize = NamesEnd - NamesBegin;
  const uint64_t NumVTables = VTableDataArray().size();
  const uint64_t VNamesSize = VNamesEnd - VNamesBegin;
  const uint64_t NumUniformCounters = ProfUniformCountersData().size() / sizeof(uint64_t);
  const uint64_t PaddingBytesAfterUniformCounters =
      static_cast<uint64_t>(PaddingSize(ProfUniformCountersData().size()));
  auto __llvm_profile_get_magic = []() -> uint64_t { return kMagic; };
  auto __llvm_profile_get_version = []() -> uint64_t { return INSTR_PROF_RAW_VERSION_VAR; };
  auto __llvm_write_binary_ids = [build_id](void* ignored) -> uint64_t {
    ZX_DEBUG_ASSERT(ignored == nullptr);
    return BinaryIdsSize(build_id);
  };

  ProfRawHeader Header = {
#define INSTR_PROF_RAW_HEADER(Type, Name, Initializer) .Name = Initializer,
#include <profile/InstrProfData.inc>
  };

  // The data and names sections are omitted when binary or debuginfo correlation is used.
  if (NumData == 0 && NamesSize == 0) {
    Header.CountersDelta = 0;
    Header.NamesDelta = 0;
    Header.BitmapDelta = 0;
    Header.UniformCountersDelta = 0;
  }

  return Header;
}

// Don't publish anything if no functions were actually instrumented.
[[gnu::const]] bool NoData() {
  return ProfCountersData().empty() && ProfBitmapData().empty() &&
         ProfUniformCountersData().empty();
}

template <typename T, template <typename> class Op>
void MergeData(std::span<std::byte> to, std::span<const std::byte> from) {
  ZX_ASSERT(to.size_bytes() == from.size_bytes());
  ZX_ASSERT(to.size_bytes() % sizeof(T) == 0);

  std::span to_data{reinterpret_cast<T*>(to.data()), to.size_bytes() / sizeof(T)};

  std::span from_data{reinterpret_cast<const T*>(from.data()), from.size_bytes() / sizeof(T)};

  constexpr Op<T> op;
  for (size_t i = 0; i < to_data.size(); ++i) {
    to_data[i] = op(to_data[i], from_data[i]);
  }
}

template <typename T, template <typename> class Op, typename FromT>
void MergeSelfData(std::span<std::byte> to, std::span<FromT> from, const char* what) {
  ZX_ASSERT_MSG(to.size_bytes() >= from.size_bytes(),
                "merging %zu bytes of %s with only %zu bytes left!", from.size_bytes(), what,
                to.size_bytes());
  MergeData<T, Op>(to.subspan(0, from.size_bytes()), std::as_bytes(from));
}

void MergeCounters(std::span<std::byte> to, std::span<const std::byte> from) {
  if (LlvmProfdata::UsingSingleByteCounters())
    MergeData<uint8_t, std::logical_and>(to, from);
  else
    MergeData<uint64_t, std::plus>(to, from);
}

void UseData(std::span<std::byte> self_data, uintptr_t& bias_var, const char* what,
             size_t alignment, std::span<std::byte> data) {
  ZX_ASSERT_MSG(data.size_bytes() >= self_data.size_bytes(),
                "cannot relocate %zu bytes of %s with only %zu bytes left!", self_data.size_bytes(),
                what, data.size_bytes());

  const uintptr_t old_addr = reinterpret_cast<uintptr_t>(self_data.data());
  const uintptr_t new_addr = reinterpret_cast<uintptr_t>(data.data());
  ZX_ASSERT(new_addr % alignment == 0);
  const uintptr_t new_bias = new_addr - old_addr;

  // Now that the data has been copied (or merged), start updating the new
  // copy.  These compiler barriers should ensure we've finished all the
  // copying before updating the bias that the instrumented code uses.
  std::atomic_signal_fence(std::memory_order_seq_cst);
  bias_var = new_bias;
  std::atomic_signal_fence(std::memory_order_seq_cst);
}

}  // namespace

void LlvmProfdata::Init(std::span<const std::byte> build_id) {
  build_id_ = build_id;

  if (NoData()) {
    return;
  }

  // The sequence and sizes here should match the PublishLiveData() code.

  const ProfRawHeader header = GetHeader(build_id_);

  counters_offset_ = sizeof(header) + header.binary_ids_size() +
                     (static_cast<size_t>(header.NumData) * sizeof(__llvm_profile_data)) +
                     static_cast<size_t>(header.PaddingBytesBeforeCounters);
  counters_size_bytes_ = static_cast<size_t>(header.NumCounters) * CountersSize();
  ZX_ASSERT(counters_size_bytes_ == ProfCountersData().size_bytes());

  size_t next_offset = counters_offset_ + counters_size_bytes_ +
                       static_cast<size_t>(header.PaddingBytesAfterCounters);

  bitmap_offset_ = next_offset;
  bitmap_size_bytes_ = static_cast<size_t>(header.NumBitmapBytes);
  ZX_ASSERT(bitmap_size_bytes_ == ProfBitmapData().size_bytes());
  next_offset = bitmap_offset_ + bitmap_size_bytes_ +
                static_cast<size_t>(header.PaddingBytesAfterBitmapBytes);

  uniform_counters_offset_ = next_offset;
  uniform_counters_size_bytes_ = static_cast<size_t>(header.NumUniformCounters) * sizeof(uint64_t);
  ZX_ASSERT(uniform_counters_size_bytes_ == ProfUniformCountersData().size_bytes());
  next_offset = uniform_counters_offset_ + uniform_counters_size_bytes_ +
                static_cast<size_t>(header.PaddingBytesAfterUniformCounters);

  size_bytes_ = next_offset;

  const size_t PaddingBytesAfterNames = PaddingSize(static_cast<size_t>(header.NamesSize));
  size_bytes_ += header.NamesSize + PaddingBytesAfterNames;

  const size_t VTableSectionSize = static_cast<size_t>(header.NumVTables) * sizeof(VTableProfData);
  size_bytes_ += VTableSectionSize + PaddingSize(VTableSectionSize);
  size_bytes_ +=
      static_cast<size_t>(header.VNamesSize) + PaddingSize(static_cast<size_t>(header.VNamesSize));
}

LlvmProfdata::LiveDataBounds LlvmProfdata::bounds() const {
  size_t start = size_bytes_;
  size_t end = 0;
  auto update_bounds = [&start, &end](size_t offset, size_t size) {
    if (size > 0) {
      start = std::min(start, offset);
      end = std::max(end, offset + size);
    }
  };
  update_bounds(counters_offset_, counters_size_bytes_);
  update_bounds(bitmap_offset_, bitmap_size_bytes_);
  update_bounds(uniform_counters_offset_, uniform_counters_size_bytes_);
  if (end > start) {
    return {start, end - start};
  }
  return {};
}

bool LlvmProfdata::UsingSingleByteCounters() {
  return INSTR_PROF_RAW_VERSION_VAR & VARIANT_MASK_BYTE_COVERAGE;
}

LlvmProfdata::LiveData LlvmProfdata::DoFixedData(std::span<std::byte> data, bool match) {
  if (size_bytes_ == 0) {
    return {};
  }

  // Write bytes at the start of data and then advance data to be the remaining
  // subspan where the next call will write its data.  When merging, this
  // doesn't actually write but instead asserts that the destination already
  // has identical contents.
  auto write_bytes = [&](std::span<const std::byte> bytes, const char* what) {
    ZX_ASSERT_MSG(data.size_bytes() >= bytes.size_bytes(),
                  "%s of %zu bytes with only %zu bytes left!", what, bytes.size_bytes(),
                  data.size_bytes());
    if (match) {
      ZX_ASSERT_MSG(!memcmp(data.data(), bytes.data(), bytes.size()),
                    "mismatch somewhere in %zu bytes of %s", bytes.size(), what);
    } else {
      memcpy(data.data(), bytes.data(), bytes.size());
    }
    data = data.subspan(bytes.size());
  };

  constexpr std::array<std::byte, sizeof(uint64_t)> kPaddingBytes{};
  const std::span kPadding(kPaddingBytes);
  constexpr const char* kPaddingDoc = "alignment padding";

  // These are all the chunks to be written.
  // The sequence and sizes here must match the size_bytes() code.

  const ProfRawHeader header = GetHeader(build_id_);
  write_bytes(std::as_bytes(std::span{&header, 1}), "INSTR_PROF_RAW_HEADER");

  const uint64_t build_id_size = build_id_.size_bytes();
  if (build_id_size > 0) {
    write_bytes(std::as_bytes(std::span{&build_id_size, 1}), "build ID size");
    write_bytes(std::as_bytes(build_id_), "build ID");
    write_bytes(kPadding.subspan(0, PaddingSize(build_id_)), kPaddingDoc);
  }

  write_bytes(std::as_bytes(ProfDataArray()), INSTR_PROF_DATA_SECT_NAME);
  write_bytes(kPadding.subspan(0, static_cast<size_t>(header.PaddingBytesBeforeCounters)),
              kPaddingDoc);

  // Skip over the space in the data blob for the counters.
  ZX_ASSERT(counters_size_bytes_ == ProfCountersData().size_bytes());
  ZX_ASSERT_MSG(data.size_bytes() >= counters_size_bytes_,
                "%zu bytes of counters with only %zu bytes left!", counters_size_bytes_,
                data.size_bytes());
  std::span counters_data = data.subspan(0, counters_size_bytes_);
  data = data.subspan(counters_size_bytes_);
  write_bytes(kPadding.subspan(0, static_cast<size_t>(header.PaddingBytesAfterCounters)),
              kPaddingDoc);

  std::span<std::byte> bitmap_data;
  // Skip over the space in the data blob for the bitmap bytes.
  ZX_ASSERT(bitmap_size_bytes_ == ProfBitmapData().size_bytes());
  ZX_ASSERT_MSG(data.size_bytes() >= bitmap_size_bytes_,
                "%zu bytes of bitmap with only %zu bytes left!", bitmap_size_bytes_,
                data.size_bytes());
  bitmap_data = data.subspan(0, bitmap_size_bytes_);
  data = data.subspan(bitmap_size_bytes_);
  write_bytes(kPadding.subspan(0, static_cast<size_t>(header.PaddingBytesAfterBitmapBytes)),
              kPaddingDoc);

  std::span<std::byte> uniform_counters_data;
  // Skip over the space in the data blob for the uniform counters.
  ZX_ASSERT(uniform_counters_size_bytes_ == ProfUniformCountersData().size_bytes());
  ZX_ASSERT_MSG(data.size_bytes() >= uniform_counters_size_bytes_,
                "%zu bytes of uniform counters with only %zu bytes left!",
                uniform_counters_size_bytes_, data.size_bytes());
  uniform_counters_data = data.subspan(0, uniform_counters_size_bytes_);
  data = data.subspan(uniform_counters_size_bytes_);
  write_bytes(kPadding.subspan(0, static_cast<size_t>(header.PaddingBytesAfterUniformCounters)),
              kPaddingDoc);

  auto prof_names = std::span(NamesBegin, NamesEnd - NamesBegin);
  const size_t PaddingBytesAfterNames = PaddingSize(static_cast<size_t>(header.NamesSize));
  write_bytes(std::as_bytes(prof_names), INSTR_PROF_NAME_SECT_NAME);
  write_bytes(kPadding.subspan(0, PaddingBytesAfterNames), kPaddingDoc);

  auto vtable_data = VTableDataArray();
  write_bytes(std::as_bytes(vtable_data), INSTR_PROF_VTAB_SECT_NAME);
  write_bytes(kPadding.subspan(0, PaddingSize(vtable_data.size_bytes())), kPaddingDoc);

  auto vnames = std::span(VNamesBegin, VNamesEnd - VNamesBegin);
  write_bytes(std::as_bytes(vnames), INSTR_PROF_VNAME_SECT_NAME);
  write_bytes(kPadding.subspan(0, PaddingSize(vnames.size_bytes())), kPaddingDoc);

  return {
      .counters = counters_data,
      .bitmap = bitmap_data,
      .uniform_counters = uniform_counters_data,
  };
}

LlvmProfdata::LiveData LlvmProfdata::LiveDataForBounds(std::span<std::byte> bounds_data) const {
  auto [bounds_offset, bounds_size] = bounds();
  if (bounds_size == 0) {
    return {};
  }
  ZX_ASSERT(bounds_data.size() >= bounds_size);

  auto get_span = [&](size_t offset, size_t size) -> std::span<std::byte> {
    if (size == 0) {
      return {};
    }
    ZX_ASSERT(offset >= bounds_offset);
    ZX_ASSERT(offset + size <= bounds_offset + bounds_size);
    return bounds_data.subspan(offset - bounds_offset, size);
  };

  return {
      .counters = get_span(counters_offset_, counters_size_bytes_),
      .bitmap = get_span(bitmap_offset_, bitmap_size_bytes_),
      .uniform_counters = get_span(uniform_counters_offset_, uniform_counters_size_bytes_),
  };
}

void LlvmProfdata::CopyLiveData(LiveData data) {
  auto prof_counters = ProfCountersData();
  ZX_ASSERT_MSG(data.counters.size_bytes() >= prof_counters.size_bytes(),
                "writing %zu bytes of counters with only %zu bytes left!",
                data.counters.size_bytes(), data.counters.size_bytes());
  if (!prof_counters.empty()) {
    memcpy(data.counters.data(), prof_counters.data(), prof_counters.size_bytes());
  }

  auto prof_bitmap = ProfBitmapData();
  ZX_ASSERT_MSG(data.bitmap.size_bytes() >= prof_bitmap.size_bytes(),
                "writing %zu bytes of bitmap with only %zu bytes left!", prof_bitmap.size_bytes(),
                data.bitmap.size_bytes());
  if (!prof_bitmap.empty()) {
    memcpy(data.bitmap.data(), prof_bitmap.data(), prof_bitmap.size_bytes());
  }

  auto prof_uniform_counters = ProfUniformCountersData();
  ZX_ASSERT_MSG(data.uniform_counters.size_bytes() >= prof_uniform_counters.size_bytes(),
                "writing %zu bytes of uniform counters with only %zu bytes left!",
                prof_uniform_counters.size_bytes(), data.uniform_counters.size_bytes());
  if (!prof_uniform_counters.empty()) {
    memcpy(data.uniform_counters.data(), prof_uniform_counters.data(),
           prof_uniform_counters.size_bytes());
  }
}

// Instead of copying, merge the old counters with our values by summation and
// the old bitmap by bitwise OR.
void LlvmProfdata::MergeLiveData(LiveData data) {
  auto data_counters = data.counters;
  auto prof_counters = ProfCountersData();
  ZX_ASSERT_MSG(data_counters.size_bytes() >= prof_counters.size_bytes(),
                "merging %zu bytes of counters with only %zu bytes left!",
                prof_counters.size_bytes(), data_counters.size_bytes());
  MergeCounters(data_counters, std::as_bytes(ProfCountersData()));
  MergeSelfData<char, std::bit_or>(data.bitmap, ProfBitmapData(), "bitmap");
  auto prof_uniform_counters = ProfUniformCountersData();
  std::span<const uint64_t> prof_uniform_counters_u64{
      reinterpret_cast<const uint64_t*>(prof_uniform_counters.data()),
      prof_uniform_counters.size_bytes() / sizeof(uint64_t),
  };
  MergeSelfData<uint64_t, std::plus>(data.uniform_counters, prof_uniform_counters_u64,
                                     "uniform counters");
}

void LlvmProfdata::MergeLiveData(LiveData to, LiveData from) {
  MergeCounters(to.counters, from.counters);
  MergeData<char, std::bit_or>(to.bitmap, from.bitmap);
  MergeData<uint64_t, std::plus>(to.uniform_counters, from.uniform_counters);
}

void LlvmProfdata::UseLiveData(LiveData data) {
#ifdef INSTR_PROF_PROFILE_BITMAP_BIAS_VAR
  UseData(std::as_writable_bytes(ProfBitmapData()), INSTR_PROF_PROFILE_BITMAP_BIAS_VAR, "bitmap", 1,
          data.bitmap);
#else
  ZX_ASSERT_MSG(data.bitmap.empty(), "bitmap bytes cannot be relocated");
#endif
  UseData(std::as_writable_bytes(ProfCountersData()), INSTR_PROF_PROFILE_COUNTER_BIAS_VAR,
          "counters", LiveDataCountersAlignment(), data.counters);
}

void LlvmProfdata::UseLinkTimeLiveData() {
  std::atomic_signal_fence(std::memory_order_seq_cst);
  INSTR_PROF_PROFILE_COUNTER_BIAS_VAR = 0;
  std::atomic_signal_fence(std::memory_order_seq_cst);
}

std::span<const std::byte> LlvmProfdata::BuildIdFromRawProfile(std::span<const std::byte> data) {
  ProfRawHeader header;
  if (data.size() < sizeof(header)) {
    return {};
  }
  memcpy(&header, data.data(), sizeof(header));
  data = data.subspan(sizeof(header));

  if (header.Magic != kMagic || header.Version < 7) {
    return {};
  }

  if (header.binary_ids_size() == 0 || header.binary_ids_size() > data.size()) {
    return {};
  }
  data = data.subspan(0, header.binary_ids_size());

  uint64_t build_id_size;
  if (data.size() < sizeof(build_id_size)) {
    return {};
  }
  memcpy(&build_id_size, data.data(), sizeof(build_id_size));
  data = data.subspan(sizeof(build_id_size));

  if (data.size() < build_id_size) {
    return {};
  }
  return data.subspan(0, static_cast<size_t>(build_id_size));
}

bool LlvmProfdata::Match(std::span<const std::byte> data) {
  std::span id = BuildIdFromRawProfile(data);
  return !id.empty() && id.size_bytes() == build_id_.size_bytes() &&
         !memcmp(id.data(), build_id_.data(), build_id_.size_bytes());
}

#endif  // HAVE_LLVM_PROFDATA
