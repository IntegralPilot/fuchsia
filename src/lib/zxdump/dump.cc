// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/zxdump/dump.h"

#include <lib/zxdump/dump.h>
#include <lib/zxdump/elf-search.h>
#include <zircon/assert.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>

#include <algorithm>
#include <array>
#include <cassert>
#include <charconv>
#include <cinttypes>
#include <cstdlib>
#include <limits>
#include <map>
#include <optional>
#include <span>
#include <tuple>
#include <vector>

#include <rapidjson/document.h>
#include <rapidjson/stringbuffer.h>
#include <rapidjson/writer.h>

#ifdef __Fuchsia__
#include <lib/zx/process.h>
#endif

#include "core.h"
#include "job-archive.h"
#include "rights.h"

namespace zxdump {

namespace {

// This collects a bunch of note data, header and payload ByteView items.
// There's one of these for each thread, and one for the process.  The actual
// data the items point to is stored in Collector::notes_ and
// ThreadCollector::notes_.
class NoteData {
 public:
  using Vector = std::vector<ByteView>;

  NoteData() = default;
  NoteData(NoteData&&) = default;
  NoteData& operator=(NoteData&&) = default;

  size_t size() const { return data_.size(); }

  size_t size_bytes() const { return size_bytes_; }

  void push_back(ByteView data) {
    if (!data.empty()) {
      data_.push_back(data);
      size_bytes_ += data.size();
    }
  }

  Vector take() && { return std::move(data_); }

 private:
  Vector data_;
  size_t size_bytes_ = 0;
};

// This represents one note header, with name and padding but no desc.
template <size_t NameSize>
class NoteHeader {
 public:
  NoteHeader() = delete;
  constexpr NoteHeader(const NoteHeader&) = default;
  constexpr NoteHeader& operator=(const NoteHeader&) = default;

  constexpr NoteHeader(std::string_view name, uint32_t descsz, uint32_t type)
      : nhdr_{.namesz = NameSize + 1, .descsz = descsz, .type = type} {
    assert(name.size() == NameSize);
    name.copy(name_, NameSize);
  }

  ByteView bytes() const {
    return {
        reinterpret_cast<const std::byte*>(this),
        sizeof(*this),
    };
  }

  constexpr void set_size(uint32_t descsz) { nhdr_.descsz = descsz; }

 private:
  static constexpr size_t kAlignedSize_ = NoteAlign(NameSize + 1);
  static_assert(kAlignedSize_ < std::numeric_limits<uint32_t>::max());

  Elf::Nhdr nhdr_;
  char name_[kAlignedSize_] = {};
};

constexpr std::byte kZeroBytes[NoteAlign() - 1] = {};

// This returns a ByteView of as many zero bytes are needed for alignment
// padding after the given ELF note payload data.
constexpr ByteView PadForElfNote(ByteView data) {
  return {kZeroBytes, NoteAlign(data.size()) - data.size()};
}

class FlexNoteHeader {
 public:
  FlexNoteHeader() = default;

  FlexNoteHeader(const FlexNoteHeader&) = delete;

  FlexNoteHeader(FlexNoteHeader&& other) { std::swap(data_, other.data_); }

  FlexNoteHeader& operator=(FlexNoteHeader&& other) {
    std::swap(data_, other.data_);
    return *this;
  }

  ByteView bytes() const {
    if (!data_) {
      return {};
    }
    return {
        reinterpret_cast<const std::byte*>(data_),
        sizeof(data_->nhdr_) + NoteAlign(data_->nhdr_.namesz),
    };
  }

  void InitAccumulate(std::string_view name) {
    ZX_DEBUG_ASSERT(!data_);         // Not already called.
    ZX_DEBUG_ASSERT(!name.empty());  // Name must be nonempty.
    const size_t namesz = name.size() + 1;
    auto buf = new std::byte[sizeof(data_->nhdr_) + NoteAlign(namesz)]();
    static_assert(alignof(Data) <= alignof(std::max_align_t));
    data_ = reinterpret_cast<Data*>(buf);
    data_->nhdr_.namesz = static_cast<uint32_t>(namesz);
    name.copy(data_->name_, name.size());
  }

  void set_size(uint32_t descsz) { data_->nhdr_.descsz = descsz; }

  ~FlexNoteHeader() {
    if (data_) {
      delete[] reinterpret_cast<std::byte*>(data_);
    }
  }

 private:
  struct Data {
    Elf::Nhdr nhdr_;
    char name_[];
  };

  Data* data_ = nullptr;
};

// This represents one archive member header.
//
// The name field in the traditional header is only 16 characters.  So the
// modern protocol is to use a name of "/%u" to encode an offset into the
// name table, which is a special member at the beginning of the archive,
// itself named "//".
class ArchiveMemberHeader {
 public:
  ArchiveMemberHeader() {
    // Initialize the header.  All fields are left-justified and padded with
    // spaces.  There are no separators between fields.
    memset(&header_, ' ', sizeof(header_));
    static_assert(ar_hdr::kMagic.size() == sizeof(header_.ar_fmag));
    ar_hdr::kMagic.copy(header_.ar_fmag, sizeof(header_.ar_fmag));
  }

  // The name is copied directly into the header, truncated if necessary.
  // The size must be filled in later, and the date may be.
  explicit ArchiveMemberHeader(std::string_view name) : ArchiveMemberHeader() {
    name.copy(header_.ar_name, sizeof(header_.ar_name));
    Init();
  }

  // The name is stored here to go into the name table later.  The name
  // table offset and size must be filled in later, and the date may be.
  // This constructor
  void InitAccumulate(std::string name) {
    // Each name in the table is terminated by a slash and newline.
    name_ = std::move(name);
    name_ += "/\n";
    Init();
  }

  // This sets up the state for the special name table member.
  void InitNameTable(size_t size) {
    Check();
    header_.ar_name[0] = '/';
    header_.ar_name[1] = '/';
    set_size(size);
  }

  void set_name_offset(size_t name_offset) {
    Check();
    ZX_DEBUG_ASSERT(header_.ar_name[0] == ' ');
    header_.ar_name[0] = '/';
    auto [ptr, ec] = std::to_chars(&header_.ar_name[1], std::end(header_.ar_name), name_offset);
    ZX_ASSERT_MSG(ec == std::errc{}, "archive member name offset %zu too large for header",
                  name_offset);
  }

  void set_size(size_t size) {
    Check();
    auto [ptr, ec] = std::to_chars(header_.ar_size, std::end(header_.ar_size), size);
    ZX_ASSERT_MSG(ec == std::errc{}, "archive member size %zu too large for header", size);
  }

  void set_date(time_t mtime) {
    Check();
    auto [ptr, ec] = std::to_chars(header_.ar_date, std::end(header_.ar_date), mtime);
    ZX_ASSERT_MSG(ec == std::errc{}, "archive member timestamp %zu too large for header", mtime);
  }

  ByteView bytes() const {
    Check();
    return {reinterpret_cast<const std::byte*>(&header_), sizeof(header_)};
  }

  ByteView name_bytes() const {
    Check();
    ZX_DEBUG_ASSERT(!name_.empty());
    return {reinterpret_cast<const std::byte*>(name_.data()), name_.size()};
  }

 private:
  void Check() const {
    [[maybe_unused]] std::string_view magic{
        header_.ar_fmag,
        sizeof(header_.ar_fmag),
    };
    ZX_DEBUG_ASSERT(magic == ar_hdr::kMagic);
  }

  void Init() {
    Check();

    // The mode field is encoded in octal, but we always emit a constant value
    // anyway.  Other integer fields are encoded in decimal.
    kZero_.copy(header_.ar_date, sizeof(header_.ar_date));
    kZero_.copy(header_.ar_uid, sizeof(header_.ar_uid));
    kZero_.copy(header_.ar_gid, sizeof(header_.ar_gid));
    kMode_.copy(header_.ar_mode, sizeof(header_.ar_mode));
  }

  static constexpr std::string_view kZero_{"0"};
  static constexpr std::string_view kMode_{"400"};  // octal

  std::string name_;
  ar_hdr header_;
};

constexpr const std::byte kArchiveMemberPadByte = std::byte{'\n'};
constexpr ByteView kArchiveMemberPad{&kArchiveMemberPadByte, 1};

// This returns any necessary padding after the given member contents.
constexpr ByteView PadForArchive(ByteView data) {
  if (data.size() % 2 != 0) {
    return kArchiveMemberPad;
  }
  return {};
}

// Each note format has an object in notes_ of a NoteBase type.
// The Type is the n_type field for the ELF note header.
// The Class represents a set of notes handled the same way.
// It provides the ELF note name, as well as other details used below.
template <typename Class, uint32_t Type>
class NoteBase {
 public:
  NoteBase() = default;
  NoteBase(NoteBase&&) noexcept = default;
  NoteBase& operator=(NoteBase&&) noexcept = default;

  void AddToNoteData(NoteData& notes) const {
    if (!data_.empty()) {
      notes.push_back(header_.bytes());
      notes.push_back(data_);
      notes.push_back(Class::Pad(data_));
    }
  }

  bool empty() const { return data_.empty(); }

  const auto& header() const { return header_; }
  auto& header() { return header_; }

  size_t size_bytes() const {
    if (empty()) {
      return 0;
    }
    return header_.bytes().size() + data_.size() + Class::Pad(data_).size();
  }

  void clear() { data_ = {}; }

 protected:
  // The subclass Collect method calls this when it might have data.
  void Emplace(ByteView data) {
    if (!data.empty()) {
      ZX_ASSERT(data.size() <= std::numeric_limits<uint32_t>::max());
      header_.set_size(static_cast<uint32_t>(data.size()));
      data_ = data;
    }
  }

 private:
  using HeaderType = decltype(Class::MakeHeader(Type));
  static_assert(std::is_move_constructible_v<HeaderType>);
  static_assert(std::is_move_assignable_v<HeaderType>);

  // This holds the storage for the note header that NoteData points into.
  HeaderType header_ = Class::MakeHeader(Type);

  // This points into the subclass storage for the note contents.
  // It's empty until the subclass calls Emplace.
  ByteView data_;
};

// Each class derived from NoteBase uses a core.h kFooNoteName constant in:
// ```
//   static constexpr auto MakeHeader = kMakeNote<kFooNoteName>;
//   static constexpr auto Pad = PadForElfNote;
// ```
template <const std::string_view& Name>
constexpr auto kMakeNote = [](uint32_t type) { return NoteHeader<Name.size()>(Name, 0, type); };

// Each job-specific class uses a job-archive.h kFooName constant in:
// ```
//   static constexpr auto MakeHeader = kMakeMember<kFooName>;
//   static constexpr auto Pad = PadForArchive;
// ```
template <const std::string_view& Name, bool NoType = false>
constexpr auto kMakeMember = [](uint32_t type) {
  std::string name{Name};
  if constexpr (NoType) {
    ZX_DEBUG_ASSERT(type == 0);
  } else {
    name += '.';
    name += std::to_string(type);
  }
  ArchiveMemberHeader header;
  header.InitAccumulate(std::move(name));
  return header;
};

// This is called with each note (classes derived from NoteBase) when its
// information is required.  It can be called more than once, so it does
// nothing if it's already collected the data.  Each NoteBase subclass below
// has a Collect(const Handle&)->fit::result<Error> method that should call
// NoteBase::Emplace when it has acquired data, and then won't be called again.
constexpr auto CollectNote = [](auto& handle, auto& note) -> fit::result<Error> {
  if (note.empty()) {
    return note.Collect(handle);
  }
  return fit::ok();
};

// Notes based on zx_object_get_info calls use this.
template <typename Class, zx_object_info_topic_t Topic>
class InfoNote : public NoteBase<Class, Topic> {
 public:
  fit::result<Error> Collect(Object& object) {
    Object& handle =
        std::is_same_v<typename Class::Handle, Resource> ? object.info_resource() : object;

    if constexpr (kIsSpan<T>) {
      // Cache the data for iteration.  This tells zxdump::Object to refresh
      // live data when this is used for the thread list so it will catch up
      // with racing new threads.
      auto result = handle.get_info<Topic>(Topic == ZX_INFO_PROCESS_THREADS);
      if (result.is_error()) {
        return result.take_error();
      }
      info_ = result.value();
    }

    // This gets the same data just cached, but in generic ByteView form.
    auto result = handle.get_info(Topic);
    if (result.is_error()) {
      return result.take_error();
    }

    this->Emplace(result.value());
    return fit::ok();
  }

  auto info() const { return info_; }

 private:
  using T = typename InfoTraits<Topic>::type;
  struct Unused {};
  std::conditional_t<kIsSpan<T>, T, Unused> info_;
};

// Notes based on the fixed-sized property calls use this.
template <typename Class, uint32_t Property>
class PropertyNote : public NoteBase<Class, Property> {
 public:
  fit::result<Error> Collect(typename Class::Handle& handle) {
    auto result = handle.template get_property<Property>();
    if (result.is_error()) {
      return result.take_error();
    }
    data_ = result.value();
    ByteView bytes{reinterpret_cast<std::byte*>(&data_), sizeof(data_)};
    if constexpr (Property == ZX_PROP_NAME) {
      bytes = {reinterpret_cast<std::byte*>(data_.data()), data_.size()};
    }
    this->Emplace(bytes);
    return fit::ok();
  }

 private:
  typename PropertyTraits<Property>::type data_;
};

// Notes based on the fixed-sized read_state calls use this.
template <typename Class, uint32_t Kind>
class ThreadStateNote : public NoteBase<Class, Kind> {
 public:
  fit::result<Error> Collect(Thread& thread) {
    auto result = thread.read_state(Kind);
    if (result.is_error()) {
      return result.take_error();
    }
    this->Emplace(result.value());
    return fit::ok();
  }
};

template <typename Class>
class JsonNote : public NoteBase<Class, 0> {
 public:
  // The writer object holds a reference to the note object.  When the writer
  // is destroyed, everything it wrote will be reified into the note contents.
  class JsonWriter {
   public:
    auto& writer() { return writer_; }

    ~JsonWriter() {
      note_.Emplace({
          reinterpret_cast<const std::byte*>(note_.data_.GetString()),
          note_.data_.GetSize(),
      });
    }

   private:
    friend JsonNote;
    explicit JsonWriter(JsonNote& note) : writer_(note.data_), note_(note) {}

    rapidjson::Writer<rapidjson::StringBuffer> writer_;
    JsonNote& note_;
  };

  [[nodiscard]] auto MakeJsonWriter() { return JsonWriter{*this}; }

  [[nodiscard]] bool Set(const rapidjson::Value& value) {
    return value.Accept(MakeJsonWriter().writer());
  }

  // CollectNoteData will call this, but it has nothing to do.
  static constexpr auto Collect = [](const auto& handle) -> fit::result<Error> {
    return fit::ok();
  };

 private:
  rapidjson::StringBuffer data_;
};

// These are called via std::apply on ProcessNotes and ThreadNotes tuples.

constexpr auto CollectNoteData =
    // For each note that hasn't already been fetched, try to fetch it now.
    [](auto& handle, auto&... note) -> fit::result<Error, size_t> {
  // This value is always replaced (or ignored), but the type is not
  // default-constructible.
  fit::result<Error> result = fit::ok();

  size_t total = 0;
  auto collect = [&handle, &result, &total](auto& note) -> bool {
    result = CollectNote(handle, note);
    if (result.is_ok()) {
      ZX_DEBUG_ASSERT(note.size_bytes() % 2 == 0);
      total += note.size_bytes();
      return true;
    }
    switch (result.error_value().status_) {
      case ZX_ERR_NOT_SUPPORTED:
      case ZX_ERR_BAD_STATE:
        // These just mean the data is not available because it never
        // existed or the thread is dead.
        return true;

      default:
        return false;
    }
  };

  if (!(collect(note) && ...)) {
    // The fold expression bailed out early after setting result.
    return result.take_error();
  }

  return fit::ok(total);
};

constexpr auto DumpNoteData =
    // Return a vector pointing at all the nonempty note data.
    [](const auto&... note) -> NoteData::Vector {
  NoteData data;
  (note.AddToNoteData(data), ...);
  return std::move(data).take();
};

template <size_t N, typename T>
void CollectJson(rapidjson::Document& dom, const char (&name)[N], T value) {
  rapidjson::Value json_value{value};
  dom.AddMember(name, json_value, dom.GetAllocator());
}

template <size_t N>
void CollectJson(rapidjson::Document& dom, const char (&name)[N], std::string_view value) {
  rapidjson::Value json_value{
      value.data(),
      static_cast<rapidjson::SizeType>(value.size()),
  };
  dom.AddMember(name, json_value, dom.GetAllocator());
}

fit::result<Error, rapidjson::Document> CollectSystemJson(const TaskHolder& holder) {
  std::string_view version = holder.system_get_version_string();
  if (version.empty()) {
    return fit::error{Error{"no system data available", ZX_ERR_NOT_SUPPORTED}};
  }

  rapidjson::Document dom;
  dom.SetObject();
  {
    rapidjson::Value value(version.data(), static_cast<rapidjson::SizeType>(version.size()));
    dom.AddMember("version_string", value, dom.GetAllocator());
  }
  {
    rapidjson::Value value{holder.system_get_dcache_line_size()};
    dom.AddMember("dcache_line_size", value, dom.GetAllocator());
  }
  {
    rapidjson::Value value{holder.system_get_num_cpus()};
    dom.AddMember("num_cpus", value, dom.GetAllocator());
  }
  {
    rapidjson::Value value{holder.system_get_page_size()};
    dom.AddMember("page_size", value, dom.GetAllocator());
  }
  {
    rapidjson::Value value{holder.system_get_physmem()};
    dom.AddMember("physmem", value, dom.GetAllocator());
  }

  return fit::ok(std::move(dom));
}

constexpr auto CollectSystemNote = [](const TaskHolder& holder, auto& note) -> fit::result<Error> {
  auto result = CollectSystemJson(holder);
  if (result.is_error()) {
    return result.take_error();
  }
  bool ok = note.Set(std::move(result).value());
  ZX_ASSERT(ok);
  return fit::ok();
};

template <typename Class, typename... Notes>
using KernelNotes = std::tuple<  // The whole tuple of all note types:
    Notes...,                    // First the process or job notes.
    // Now the kernel note types.
    InfoNote<Class, ZX_INFO_HANDLE_BASIC>,  // Identifies info resource KOID.
    InfoNote<Class, ZX_INFO_CPU_STATS>,     //
    InfoNote<Class, ZX_INFO_KMEM_STATS>,    //
    InfoNote<Class, ZX_INFO_GUEST_STATS>>;

constexpr auto CollectKernelNoteData = [](auto&& handle, auto& notes) -> fit::result<Error> {
  auto collect = [&](auto&... note) -> fit::result<Error, size_t> {
    return CollectNoteData(handle, note...);
  };
  if (auto result = std::apply(collect, notes); result.is_error()) {
    return result.take_error();
  }
  return fit::ok();
};

template <typename Class>
class Remark : public NoteBase<Class, 0> {
 public:
  using Base = NoteBase<Class, 0>;

  Remark() = default;

  Remark(Remark&& other) { *this = std::move(other); }

  Remark& operator=(Remark&& other) {
    Base::operator=(static_cast<Base&&>(other));
    data_ = std::move(other.data_);
    this->Emplace(data_);
    return *this;
  }

  Remark(std::string_view name, ByteView data) : data_(data.begin(), data.end()) {
    ZX_ASSERT(!name.empty());
    ZX_ASSERT(!data.empty());
    std::string remark_name{kRemarkNotePrefix};
    remark_name += name;
    this->header().InitAccumulate(std::move(remark_name));
    this->Emplace(data_);
  }

 private:
  std::vector<std::byte> data_;
};

struct JobRemarkClass {
  static ArchiveMemberHeader MakeHeader(uint32_t type) { return {}; }
  static constexpr auto Pad = PadForArchive;
};

struct ProcessRemarkClass {
  static FlexNoteHeader MakeHeader(uint32_t type) { return {}; }
  static constexpr auto Pad = PadForElfNote;
};

template <class RemarkClass>
class CollectorBase {
 public:
  static_assert(std::is_move_constructible_v<Remark<RemarkClass>>);
  static_assert(std::is_move_assignable_v<Remark<RemarkClass>>);

  void AddRemarks(std::string_view name, ByteView data) { remarks_.emplace_back(name, data); }

  // Returns a vector of views into the storage held in this->remarks_.
  NoteData::Vector GetRemarks() const {
    NoteData data;
    for (const auto& note : remarks_) {
      note.AddToNoteData(data);
    }
    ZX_DEBUG_ASSERT(data.size_bytes() == remarks_size_bytes());
    return std::move(data).take();
  }

  template <typename T>
  bool OnRemarkHeaders(T&& call) {
    for (auto& note : remarks_) {
      if (!call(note.header())) {
        return false;
      }
    }
    return true;
  }

  size_t remarks_size_bytes() const {
    size_t total = 0;
    for (const auto& remark : remarks_) {
      total += remark.size_bytes();
    }
    return total;
  }

 private:
  std::vector<Remark<RemarkClass>> remarks_;
};

}  // namespace

// The public class is just a container for a std::unique_ptr to this private
// class, so no implementation details of the object need to be visible in the
// public header.
class ProcessDump::Collector : public CollectorBase<ProcessRemarkClass> {
 public:
  // Only constructed by Emplace.
  Collector() = delete;

  // Only Emplace and clear call this.  The process is mandatory and all other
  // members are safely default-initialized.  The suspend token handle is held
  // solely to be held, i.e. to be released on destruction of the collector.
  // It's expected to stay std::nullopt until SuspendAndCollectThreads is
  // called, and then not to be std::nullopt any more when CollectThreads
  // runs, indicating that the process is suspended.  We use std::optional
  // rather than just an invalid handle so that the asserts can distinguish
  // the "not suspended" state in our logic from the non-Fuchsia case's fake
  // handles that are all always invalid.  (When dumping a postmortem process,
  // the logic goes through all the paces of suspension but everything reports
  // as already ready already by dint of being dead and there are no real
  // suspend tokens.)
  Collector(Process& process, std::optional<LiveHandle> suspended)
      : process_(process), process_suspended_(std::move(suspended)) {}

  Process& process() const { return process_; }

  // Reset to initial state, except that if the process is already suspended,
  // it stays that way.
  void clear() { *this = Collector{process_, std::move(process_suspended_)}; }

  // This can be called at most once and must be called first if at all.  If
  // this is not called, then threads may be allowed to run while the dump
  // takes place, yielding an inconsistent memory image; and CollectProcess
  // will report only about memory and process-wide state, nothing about
  // threads.  Afterwards the process remains suspended until the Collector is
  // destroyed.
  fit::result<Error> SuspendAndCollectThreads() {
    ZX_ASSERT(!process_suspended_);
    ZX_DEBUG_ASSERT(notes_size_bytes_ == 0);
    auto result = process_.get().suspend();
    if (result.is_ok()) {
      process_suspended_ = std::move(result).value();
    } else if (result.error_value().status_ == ZX_ERR_NOT_SUPPORTED) {
      // Suspending yourself isn't supported, but that's OK.
      ZX_DEBUG_ASSERT(ProcessIsSelf());
    } else {
      return result.take_error();
    }
    return CollectThreads();
  }

  fit::result<Error> CollectSystem(const TaskHolder& holder) {
    return CollectSystemNote(holder, std::get<SystemNote>(notes_));
  }

  fit::result<Error> CollectKernel();

  // This collects information about memory and other process-wide state.  The
  // return value gives the total size of the ET_CORE file to be written.
  // Collection is cut short without error if the ET_CORE file would already
  // exceed the size limit without even including the memory.
  fit::result<Error, size_t> CollectProcess(SegmentCallback prune, size_t limit) {
    // Collect the process-wide note data.
    auto collect = [this](auto&... note) -> fit::result<Error, size_t> {
      return CollectNoteData(process_, note...);
    };
    if (auto result = std::apply(collect, notes_); result.is_error()) {
      return result.take_error();
    } else {
      notes_size_bytes_ += result.value();
    }

    // Clear out from any previous use.
    phdrs_.clear();

    // The first phdr is the main note segment.
    const Elf::Phdr note_phdr = {
        .type = elfldltl::ElfPhdrType::kNote,
        .flags = Elf::Phdr::kRead,
        .filesz = notes_size_bytes_ + remarks_size_bytes(),
        .align = NoteAlign(),
    };
    phdrs_.push_back(note_phdr);

    // Find the memory segments and build IDs.  This fills the phdrs_ table.
    if (auto result = FindMemory(std::move(prune)); result.is_error()) {
      return result.take_error();
    }

    // Now figure everything else out to write out a full ET_CORE file.
    return fit::ok(Layout());
  }

  // Accumulate header and note data to be written out, by calling
  // `dump(offset, ByreView{...})` repeatedly.
  // The views point to storage in this->notes and ThreadCollector::notes_.
  fit::result<Error, size_t> DumpHeaders(DumpCallback dump, size_t limit) {
    // Layout has already been done.
    ZX_ASSERT(ehdr_.type == elfldltl::ElfType::kCore);

    size_t offset = 0;
    auto append = [&](ByteView data) -> bool {
      if (offset >= limit || limit - offset < data.size()) {
        return false;
      }
      bool bail = dump(offset, data);
      offset += data.size();
      return bail;
    };

    // Generate the ELF headers.
    if (append({reinterpret_cast<std::byte*>(&ehdr_), sizeof(ehdr_)})) {
      return fit::ok(offset);
    }
    if (ehdr_.shnum > 0) {
      ZX_DEBUG_ASSERT(ehdr_.shnum() == 1);
      ZX_DEBUG_ASSERT(ehdr_.shoff == offset);
      if (append({reinterpret_cast<std::byte*>(&shdr_), sizeof(shdr_)})) {
        return fit::ok(offset);
      }
    }
    if (append({reinterpret_cast<std::byte*>(phdrs_.data()), phdrs_.size() * sizeof(phdrs_[0])})) {
      return fit::ok(offset);
    }

    // Returns true early if any append call returns true.
    auto append_notes = [&](auto&& notes) -> bool {
      return std::any_of(notes.begin(), notes.end(), append);
    };

    // Generate the process-wide note data.
    if (append_notes(notes())) {
      return fit::ok(offset);
    }

    // Generate the note data for each thread.
    for (const auto& [koid, thread] : threads_) {
      if (append_notes(thread.notes())) {
        return fit::ok(offset);
      }
    }

    ZX_DEBUG_ASSERT(offset % NoteAlign() == 0);
    ZX_DEBUG_ASSERT(offset == headers_size_bytes() + notes_size_bytes());

    append_notes(GetRemarks());

    ZX_DEBUG_ASSERT(offset % NoteAlign() == 0);
    ZX_DEBUG_ASSERT(offset == headers_size_bytes() + notes_size_bytes() + remarks_size_bytes());
    return fit::ok(offset);
  }

  // Dump the memory data by calling `dump(size_t offset, ByteView data)` with
  // the data meant for the given offset into the ET_CORE file.  The data is in
  // storage only available during the callback.  The `dump` function returns
  // some fit::result<error_type> type.  DumpMemory returns an "error" result
  // for errors reading the memory in.  The "success" result holds the results
  // from the callback.  If `dump` returns an error result, that is returned
  // immediately.  If it returns success, additional callbacks will be made
  // until all the data has been dumped, and the final `dump` callback's return
  // value will be the "success" return value.
  fit::result<Error, size_t> DumpMemory(DumpCallback dump, size_t limit) {
    size_t offset = headers_size_bytes() + notes_size_bytes() + remarks_size_bytes();
    for (const auto& segment : phdrs_) {
      if (segment.type == elfldltl::ElfPhdrType::kLoad) {
        uintptr_t vaddr = segment.vaddr;
        if (segment.offset >= limit) {
          break;
        }
        const size_t size = std::min(segment.filesz(), limit - segment.offset);
        if (size == 0) {
          continue;
        }
        size_t left = size;
        offset = segment.offset;
        do {
          // This yields some nonempty subset of the requested range, and
          // possibly more than requested.
          auto read =
              process_.get().read_memory<std::byte, ByteView>(vaddr, size, ReadMemorySize::kLess);
          if (read.is_error()) {
            return read.take_error();
          }

          // Note the buffer is still owned by read.value().
          ByteView chunk = read.value()->subspan(0, std::min(size, read.value()->size()));
          // TODO(mcgrathr): subset dump must be detected in layout phase
          ZX_DEBUG_ASSERT(!chunk.empty());

          // Send it to the callback to write it out.
          if (dump(offset, chunk)) {
            return fit::ok(offset);
          }

          vaddr += chunk.size();
          offset += chunk.size();
          left -= chunk.size();
        } while (left > 0);
        ZX_DEBUG_ASSERT_MSG(offset == segment.offset + size, "%#zx != %#" PRIx64 " + %#zx", offset,
                            segment.offset(), size);
      }
    }
    return fit::ok(offset);
  }

  void set_date(time_t date) { std::get<DateNote>(notes_).Set(date); }

  // This can be used from a Collect `prune_segment` callback function.
  using NoteResult = fit::result<Error, std::optional<SegmentDisposition::Note>>;
  NoteResult FindBuildIdNote(const zx_info_maps_t& segment) {
    auto on_phdrs = [this, &segment](const auto& phdrs) -> NoteResult {
      if (!phdrs->empty()) {
        auto id = DetectElfIdentity(process_, segment, *phdrs);
        if (id.is_error()) {
          return id.take_error();
        }
        if (id->build_id.size > 0) {
          return fit::ok(id->build_id);
        }
      }
      return fit::ok(std::nullopt);
    };
    auto elf = DetectElf(process_, segment);
    if (elf.is_error()) {
      return elf.take_error();
    }
    return std::visit(on_phdrs, *elf);
  }

 private:
  struct ProcessInfoClass {
    using Handle = Process;
    static constexpr auto MakeHeader = kMakeNote<kProcessInfoNoteName>;
    static constexpr auto Pad = PadForElfNote;
  };

  template <zx_object_info_topic_t Topic>
  using ProcessInfo = InfoNote<ProcessInfoClass, Topic>;

  struct ProcessPropertyClass {
    using Handle = Process;
    static constexpr auto MakeHeader = kMakeNote<kProcessPropertyNoteName>;
    static constexpr auto Pad = PadForElfNote;
  };

  template <uint32_t Prop>
  using ProcessProperty = PropertyNote<ProcessPropertyClass, Prop>;

  struct ThreadInfoClass {
    using Handle = Thread;
    static constexpr auto MakeHeader = kMakeNote<kThreadInfoNoteName>;
    static constexpr auto Pad = PadForElfNote;
  };

  template <zx_object_info_topic_t Topic>
  using ThreadInfo = InfoNote<ThreadInfoClass, Topic>;

  struct ThreadPropertyClass {
    using Handle = Thread;
    static constexpr auto MakeHeader = kMakeNote<kThreadPropertyNoteName>;
    static constexpr auto Pad = PadForElfNote;
  };

  template <uint32_t Prop>
  using ThreadProperty = PropertyNote<ThreadPropertyClass, Prop>;

  // Classes using zx_thread_read_state use this.
  struct ThreadStateClass {
    using Handle = Thread;
    static constexpr auto MakeHeader = kMakeNote<kThreadStateNoteName>;
    static constexpr auto Pad = PadForElfNote;
  };

  template <zx_thread_state_topic_t Topic>
  using ThreadState = ThreadStateNote<ThreadStateClass, Topic>;

  struct SystemClass {
    static constexpr auto MakeHeader = kMakeNote<kSystemNoteName>;
    static constexpr auto Pad = PadForElfNote;
  };

  using SystemNote = JsonNote<SystemClass>;

  struct DateClass {
    static constexpr auto MakeHeader = kMakeNote<kDateNoteName>;
    static constexpr auto Pad = PadForElfNote;
  };

  class DateNote : public NoteBase<DateClass, 0> {
   public:
    fit::result<Error> Collect(Process& process) { return fit::ok(); }

    void Set(time_t date) {
      date_ = date;
      Emplace({reinterpret_cast<const std::byte*>(&date_), sizeof(date_)});
    }

   private:
    time_t date_ = 0;
  };

  struct KernelInfoClass {
    using Handle = Resource;
    static constexpr auto MakeHeader = kMakeNote<kKernelInfoNoteName>;
    static constexpr auto Pad = PadForElfNote;
  };

  template <typename... Notes>
  using WithKernelNotes = KernelNotes<KernelInfoClass, Notes...>;

  using ThreadNotes = std::tuple<
      // This lists all the notes that can be extracted from a thread.
      ThreadInfo<ZX_INFO_HANDLE_BASIC>, ThreadProperty<ZX_PROP_NAME>,
      // Ordering of the notes after the first two is not specified and can
      // change.  Nothing separates the notes for one thread from the notes for
      // the next thread, but consumers recognize the zx_info_handle_basic_t
      // note as the key for a new thread's notes.  Whatever subset of these
      // notes that is available for the given thread is present in the dump.
      ThreadInfo<ZX_INFO_THREAD>,                   //
      ThreadInfo<ZX_INFO_THREAD_EXCEPTION_REPORT>,  //
      ThreadInfo<ZX_INFO_THREAD_STATS>,             //
      ThreadInfo<ZX_INFO_TASK_RUNTIME>,             //
      ThreadState<ZX_THREAD_STATE_GENERAL_REGS>,    //
      ThreadState<ZX_THREAD_STATE_FP_REGS>,         //
      ThreadState<ZX_THREAD_STATE_VECTOR_REGS>,     //
      ThreadState<ZX_THREAD_STATE_DEBUG_REGS>,      //
      ThreadState<ZX_THREAD_STATE_SINGLE_STEP>>;

  using ProcessNotes = WithKernelNotes<
      // This lists all the notes for process-wide state.
      ProcessInfo<ZX_INFO_HANDLE_BASIC>, ProcessProperty<ZX_PROP_NAME>,
      // Ordering of the notes after the first two is not specified and can
      // change.
      DateNote,                                            // Self-elides.
      SystemNote,                                          // Optional.
      ProcessInfo<ZX_INFO_PROCESS>,                        //
      ProcessInfo<ZX_INFO_PROCESS_THREADS>,                //
      ProcessInfo<ZX_INFO_TASK_STATS>,                     //
      ProcessInfo<ZX_INFO_TASK_RUNTIME>,                   //
      ProcessInfo<ZX_INFO_PROCESS_MAPS>,                   //
      ProcessInfo<ZX_INFO_PROCESS_VMOS>,                   //
      ProcessInfo<ZX_INFO_PROCESS_HANDLE_STATS>,           //
      ProcessInfo<ZX_INFO_HANDLE_TABLE>,                   //
      ProcessProperty<ZX_PROP_PROCESS_DEBUG_ADDR>,         //
      ProcessProperty<ZX_PROP_PROCESS_BREAK_ON_LOAD>,      //
      ProcessProperty<ZX_PROP_PROCESS_VDSO_BASE_ADDRESS>,  //
      ProcessProperty<ZX_PROP_PROCESS_HW_TRACE_CONTEXT_ID>>;

  class ThreadCollector {
   public:
    ThreadCollector() = delete;
    ThreadCollector(ThreadCollector&&) = default;

    explicit ThreadCollector(zx_koid_t koid) : koid_(koid) {}

    // Acquire the thread handle if possible.
    fit::result<Error> Acquire(Process& process) {
      if (!handle_) {
        auto result = process.get_child(koid_);
        if (result.is_error()) {
          // It's not an error if the thread has simply died already so the
          // KOID is no longer valid.
          if (result.error_value().status_ != ZX_ERR_NOT_FOUND) {
            return result.take_error();
          }
          handle_ = nullptr;
        } else {
          zxdump::Object& child = result.value();
          handle_ = static_cast<zxdump::Thread*>(&child);
        }
      }
      return fit::ok();
    }

    // Return the item to wait for this thread if it needs to be waited for.
    [[nodiscard]] std::optional<Object::WaitItem> wait() const {
      if (handle_ && *handle_) {
        return Object::WaitItem{.handle = **handle_, .waitfor = kWaitFor_};
      }
      return std::nullopt;
    }

    // This can be called after the wait() item has been used in wait_many.
    // If it still needs to be waited for, it returns success but zero size.
    // The next call to wait() will show whether collection is finished.
    fit::result<Error, size_t> Collect(zx_signals_t pending) {
      ZX_DEBUG_ASSERT(handle_);
      ZX_DEBUG_ASSERT(*handle_);

      if (pending & kWaitFor_) {
        // Now that this thread is quiescent, collect its data.
        // Reset *handle_ so wait() will say no next time.
        // It's only needed for the collection being done right now.
        Thread& thread = **handle_;
        handle_ = nullptr;
        auto collect = [&thread](auto&... note) { return CollectNoteData(thread, note...); };
        return std::apply(collect, notes_);
      }

      // Still need to wait for this one.
      return fit::ok(0);
    }

    // Returns a vector of views into the storage held in this->notes_.
    NoteData::Vector notes() const {
      ZX_DEBUG_ASSERT(handle_);
      ZX_DEBUG_ASSERT(!*handle_);
      return std::apply(DumpNoteData, notes_);
    }

   private:
    static constexpr zx_signals_t kWaitFor_ =  // Suspension or death is fine.
        ZX_THREAD_SUSPENDED | ZX_THREAD_TERMINATED;

    zx_koid_t koid_;

    // This is std::nullopt before the thread has been acquired.  Once the
    // thread has been acquired, this holds its thread handle until it's been
    // collected.  Once it's been collected, this holds nullptr.
    std::optional<Thread*> handle_;

    ThreadNotes notes_;
  };

  // Returns a vector of views into the storage held in this->notes_.
  NoteData::Vector notes() const { return std::apply(DumpNoteData, notes_); }

  // Some of the process-wide state is needed in Suspend anyway, so pre-collect
  // it directly in the notes.
  auto& process_threads() { return std::get<ProcessInfo<ZX_INFO_PROCESS_THREADS>>(notes_); }

  auto& process_maps() { return std::get<ProcessInfo<ZX_INFO_PROCESS_MAPS>>(notes_); }

  auto& process_vmos() { return std::get<ProcessInfo<ZX_INFO_PROCESS_VMOS>>(notes_); }

  // Acquire all the threads.  Then collect all their data as soon as they are
  // done suspending, waiting as necessary.
  fit::result<Error> CollectThreads() {
    ZX_DEBUG_ASSERT(process_suspended_ || ProcessIsSelf());
    threads_.clear();
    while (true) {
      // We need fresh data each time through to see if there are new threads.
      // Since the process is suspended, no new threads will run in user mode.
      // But threads already running might not have finished suspension yet,
      // and while not suspended they may create and/or start new threads that
      // will "start suspended" but their suspension is asynchronous too.
      // Hence, don't use CollectNote here, because it caches old data.
      //
      // Also a third party with the process handle could be creating and
      // starting new suspended threads too.  We don't really care about that
      // or about the racing new threads, because they won't have any user
      // state that's interesting to dump yet.  So if we overlook those threads
      // the dump will just appear to be from before they existed.
      if (auto get = process_threads().Collect(process_); get.is_error()) {
        return get;
      }

      std::vector<zxdump::Object::WaitItem> wait_for;
      std::vector<ThreadCollector*> wait_for_threads;

      // Look for new threads or unfinished threads.
      for (zx_koid_t koid : process_threads().info()) {
        auto& thread = threads_.try_emplace(threads_.end(), koid, koid)->second;

        // Make sure we have the thread handle if possible.
        // If this is not a new thread, this is a no-op.
        if (auto result = thread.Acquire(process_); result.is_error()) {
          return result.take_error();
        }

        if (auto wait = thread.wait()) {
          // This thread hasn't been collected yet.  Wait for it to finish
          // suspension (or die).
          wait_for.push_back(*wait);
          wait_for_threads.push_back(&thread);
        }
      }

      // If there are no unfinished threads, collection is all done.
      if (wait_for.empty()) {
        return fit::ok();
      }

      // Wait for a thread to finish its suspension (or death).
      if (auto result = Object::wait_many(wait_for); result.is_error()) {
        return result.take_error();
      }

      ZX_DEBUG_ASSERT(wait_for.size() == wait_for_threads.size());
      for (size_t i = 0; i < wait_for.size(); ++i) {
        auto result = wait_for_threads[i]->Collect(wait_for[i].pending);
        if (result.is_error()) {
          return result.take_error();
        }
        notes_size_bytes_ += result.value();
      }

      // Even if all known threads are quiescent now, another iteration is
      // needed to be sure that no new threads were created by these threads
      // before they went quiescent.
    }
  }

  // Populate phdrs_.  The p_offset fields are filled in later by Layout.
  fit::result<Error> FindMemory(SegmentCallback prune_segment) {
    // Make sure we have the relevant information to scan.
    if (auto result = CollectNote(process_, process_maps()); result.is_error()) {
      if (result.error_value().status_ == ZX_ERR_NOT_SUPPORTED) {
        // This just means there is no information in the dump.
        return fit::ok();
      }
      return result;
    }
    if (auto result = CollectNote(process_, process_vmos()); result.is_error()) {
      if (result.error_value().status_ == ZX_ERR_NOT_SUPPORTED) {
        // This just means there is no information in the dump.
        return fit::ok();
      }
      return result;
    }

    // The mappings give KOID and some info but the VMO info is also needed.
    // So make a quick cross-reference table to find one from the other.
    std::map<zx_koid_t, const zx_info_vmo_t&> vmos;
    for (const zx_info_vmo_t& info : process_vmos().info()) {
      vmos.emplace(info.koid, info);
    }

    constexpr auto elf_flags = [](zx_vm_option_t mmu_flags) -> Elf::Word {
      return (((mmu_flags & ZX_VM_PERM_READ) ? Elf::Phdr::kRead : 0) |
              ((mmu_flags & ZX_VM_PERM_WRITE) ? Elf::Phdr::kWrite : 0) |
              ((mmu_flags & ZX_VM_PERM_EXECUTE) ? Elf::Phdr::kExecute : 0));
    };

    // Go through each mapping.  They are in ascending address order.
    uintptr_t address_limit = 0;
    for (const zx_info_maps_t& info : process_maps().info()) {
      if (info.type == ZX_INFO_MAPS_TYPE_MAPPING) {
        ZX_ASSERT(info.base >= address_limit);
        address_limit = info.base + info.size;
        ZX_ASSERT(info.base < address_limit);

        // Add a PT_LOAD segment for the mapping no matter what.
        // It will be present with p_filesz==0 if the memory is elided.
        {
          const Elf::Phdr new_phdr = {
              .type = elfldltl::ElfPhdrType::kLoad,
              .flags = elf_flags(info.u.mapping.mmu_flags),
              .vaddr = info.base,
              .filesz = info.size,
              .memsz = info.size,
              .align = process_.get().dump_page_size(),
          };
          phdrs_.push_back(new_phdr);
        }
        Elf::Phdr& segment = phdrs_.back();

        // The default-constructed state elides the whole segment.
        SegmentDisposition dump;

        // If the KOID isn't in the ZX_INFO_PROCESS_VMOS list, then it could be
        // a non-VMO mapping (iob or clock) or it could be an anomaly due to a
        // sample that's not synchronized with ZX_INFO_PROCESS_MAPS (e.g. if
        // the process wasn't stopped, or mapping changes were made from
        // outside the process).  In those cases, just elide the segment since
        // there's no way to know if it's possible or safe to get its contents.
        if (auto it = vmos.find(info.u.mapping.vmo_koid); it != vmos.end()) {
          const auto& [koid, vmo] = *it;
          ZX_DEBUG_ASSERT(koid == vmo.koid);
          ZX_DEBUG_ASSERT(vmo.koid == info.u.mapping.vmo_koid);

          // Default choice: dump the whole thing.  But never dump device
          // memory, which could cause side effects on memory-mapped devices
          // just from reading the physical address.
          if (ZX_INFO_VMO_TYPE(vmo.flags) != ZX_INFO_VMO_TYPE_PHYSICAL) {
            dump.filesz = segment.filesz;
          }

          // If this mapping covers past the end of the VMO, trim the excess
          // away.  Note that we have to handle overflows because the
          // vmo_offset could be greater than the VMO size.
          uint64_t vmo_size_after_offset;
          if (sub_overflow(vmo.size_bytes, info.u.mapping.vmo_offset, &vmo_size_after_offset)) {
            dump.filesz = 0;
          } else {
            dump.filesz = std::min(vmo_size_after_offset, dump.filesz);
          }

          // Let the callback decide about this segment.
          if (auto result = prune_segment(dump, info, vmo); result.is_error()) {
            return result.take_error();
          } else {
            dump = result.value();
          }
        }

        ZX_ASSERT(dump.filesz <= info.size);
        segment.filesz = dump.filesz;

        // The callback can choose to emit an ELF note whose contents are
        // embedded within this PT_LOAD segment.  This becomes a PT_NOTE
        // segment directly in the ET_CORE file, pointing at the dumped memory
        // expected to be in ELF note format.
        if (dump.note) {
          // The note's vaddr and size are a subset of the segment's.  Add an
          // extra PT_NOTE segment pointing to the chosen memory area.  The
          // callback sets p_vaddr and p_filesz but we sanitize the rest.
          ZX_ASSERT(dump.note->vaddr >= segment.vaddr);
          ZX_ASSERT(dump.note->vaddr - segment.vaddr <= segment.filesz);
          ZX_ASSERT(dump.note->size <= segment.filesz - (dump.note->vaddr - segment.vaddr));
          const Elf::Phdr note_phdr = {
              .type = elfldltl::ElfPhdrType::kNote,
              .flags = Elf::Phdr::kRead,
              .vaddr = dump.note->vaddr,
              .filesz = dump.note->size,
              .memsz = dump.note->size,
              .align = NoteAlign(),
          };
          phdrs_.push_back(note_phdr);
        }
      }
    }

    return fit::ok();
  }

  // Populate the header fields and reify phdrs_ with p_offset values.
  // This chooses where everything will go in the ET_CORE file.
  size_t Layout() {
    // Fill in the file header boilerplate.
    ehdr_.magic = Elf::Ehdr::kMagic;
    ehdr_.elfclass = elfldltl::ElfClass::k64;
    ehdr_.elfdata = elfldltl::ElfData::k2Lsb;
    ehdr_.ident_version = elfldltl::ElfVersion::kCurrent;
    ehdr_.type = elfldltl::ElfType::kCore;
    ehdr_.machine = elfldltl::ElfMachine::kNative;
    ehdr_.version = elfldltl::ElfVersion::kCurrent;
    size_t offset = ehdr_.phoff = ehdr_.ehsize = sizeof(ehdr_);
    ehdr_.phentsize = sizeof(phdrs_[0]);
    offset += phdrs_.size() * sizeof(phdrs_[0]);
    if (phdrs_.size() < Elf::Ehdr::kPnXnum) {
      ehdr_.phnum = static_cast<uint16_t>(phdrs_.size());
    } else {
      shdr_.info = static_cast<uint32_t>(phdrs_.size());
      ehdr_.phnum = Elf::Ehdr::kPnXnum;
      ehdr_.shnum = 1;
      ehdr_.shentsize = sizeof(shdr_);
      ehdr_.shoff = offset;
      offset += sizeof(shdr_);
    }
    ZX_DEBUG_ASSERT(offset == headers_size_bytes());

    // Now assign offsets to all the segments.
    auto place = [&offset](Elf::Phdr& phdr) {
      if (phdr.filesz == 0) {
        phdr.offset = 0;
      } else {
        offset = (offset + phdr.align - 1) & -size_t{phdr.align};
        phdr.offset = offset;
        offset += phdr.filesz;
      }
    };

    // First is the initial note segment.
    ZX_DEBUG_ASSERT(!phdrs_.empty());
    ZX_DEBUG_ASSERT(phdrs_[0].type == elfldltl::ElfPhdrType::kNote);
    place(phdrs_[0]);

    // Now place the remaining segments, if any.
    for (auto& phdr : std::span(phdrs_).subspan(1)) {
      switch (phdr.type) {
        case elfldltl::ElfPhdrType::kLoad:
          place(phdr);
          break;

        case elfldltl::ElfPhdrType::kNote: {
          // This is an ELF note segment.
          // It lies within the preceding PT_LOAD segment.
          const auto& load = (&phdr)[-1];
          ZX_DEBUG_ASSERT(load.type == elfldltl::ElfPhdrType::kLoad);
          ZX_DEBUG_ASSERT(phdr.vaddr >= load.vaddr);
          phdr.offset = phdr.vaddr - load.vaddr + load.offset;
          break;
        }

        default:
          ZX_ASSERT_MSG(false, "generated p_type %#x ???", static_cast<unsigned int>(phdr.type()));
      }
    }

    ZX_DEBUG_ASSERT(offset % NoteAlign() == 0);
    return offset;
  }

  size_t headers_size_bytes() const {
    return sizeof(ehdr_) + (sizeof(phdrs_[0]) * phdrs_.size()) +
           (ehdr_.phnum == Elf::Ehdr::kPnXnum ? sizeof(shdr_) : 0);
  }

  size_t notes_size_bytes() const { return notes_size_bytes_; }

  bool ProcessIsSelf() {
#ifdef __Fuchsia__
    zx_info_handle_basic_t info;
    zx_status_t status =
        zx::process::self()->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
    ZX_ASSERT_MSG(status == ZX_OK, "ZX_INFO_HANDLE_BASIC on self: %d", status);
    return info.koid == process_.get().koid();
#endif
    return false;
  }

  std::reference_wrapper<Process> process_;
  std::optional<zxdump::LiveHandle> process_suspended_;
  ProcessNotes notes_;

  std::map<zx_koid_t, ThreadCollector> threads_;

  std::vector<Elf::Phdr> phdrs_;
  Elf::Ehdr ehdr_ = {};
  Elf::Shdr shdr_ = {};  // Only used for the PN_XNUM case.

  // This collects the totals for process-wide and thread notes.
  size_t notes_size_bytes_ = 0;
};

ProcessDump::ProcessDump(ProcessDump&&) noexcept = default;
ProcessDump& ProcessDump::operator=(ProcessDump&&) noexcept = default;
ProcessDump::~ProcessDump() = default;

Process& ProcessDump::process() const { return collector_->process(); }

void ProcessDump::clear() { collector_->clear(); }

fit::result<Error> ProcessDump::SuspendAndCollectThreads() {
  return collector_->SuspendAndCollectThreads();
}

fit::result<Error, size_t> ProcessDump::CollectProcess(SegmentCallback prune, size_t limit) {
  return collector_->CollectProcess(std::move(prune), limit);
}

fit::result<Error> ProcessDump::CollectKernel() { return collector_->CollectKernel(); }

fit::result<Error> ProcessDump::CollectSystem(const TaskHolder& holder) {
  return collector_->CollectSystem(holder);
}

fit::result<Error, std::optional<SegmentDisposition::Note>> ProcessDump::FindBuildIdNote(
    const zx_info_maps_t& segment) {
  return collector_->FindBuildIdNote(segment);
}

fit::result<Error, size_t> ProcessDump::DumpHeadersImpl(DumpCallback dump, size_t limit) {
  return collector_->DumpHeaders(std::move(dump), limit);
}

fit::result<Error, size_t> ProcessDump::DumpMemoryImpl(DumpCallback callback, size_t limit) {
  return collector_->DumpMemory(std::move(callback), limit);
}

void ProcessDump::set_date(time_t date) { collector_->set_date(date); }

fit::result<Error> ProcessDump::Remarks(std::string_view name, ByteView data) {
  collector_->AddRemarks(name, data);
  return fit::ok();
}

// A single Collector cannot be used for a different process later.  It can be
// clear()'d to reset all state other than the process handle and the process
// being suspended.
ProcessDump::ProcessDump(Process& process) noexcept : collector_{new Collector{process, {}}} {}

class JobDump::Collector : public CollectorBase<JobRemarkClass> {
 public:
  Collector() = delete;

  // Only Emplace and clear call this.  The job is mandatory and all other
  // members are safely default-initialized.
  explicit Collector(Job& job) : job_(job) {}

  Job& job() const { return job_; }

  // Reset to initial state.
  void clear() { *this = Collector{job_}; }

  // This collects information about job-wide state.
  fit::result<Error, size_t> CollectJob() {
    // Collect the job-wide note data.
    auto collect = [this](auto&... note) -> fit::result<Error, size_t> {
      return CollectNoteData(job_, note...);
    };
    auto result = std::apply(collect, notes_);
    if (result.is_error()) {
      return result.take_error();
    }
    ZX_DEBUG_ASSERT(result.value() % 2 == 0);

    // Each note added its name to the name table inside CollectNoteData.
    auto count_job_note_names = [](const auto&... note) -> size_t {
      return (note.header().name_bytes().size() + ...);
    };
    size_t name_table_size = std::apply(count_job_note_names, notes_);
    OnRemarkHeaders([&name_table_size](const ArchiveMemberHeader& header) {
      name_table_size += header.name_bytes().size();
      return true;
    });
    name_table_.InitNameTable(name_table_size);

    // The name table member will be padded on the way out.
    name_table_size += name_table_size % 2;

    return fit::ok(kArchiveMagic.size() +        // Archive header +
                   name_table_.bytes().size() +  // name table member header +
                   name_table_size +             // name table contents +
                   result.value());              // note members & headers.
  }

  auto CollectChildren() { return job_.get().children(); }

  auto CollectProcesses() { return job_.get().processes(); }

  fit::result<Error> CollectSystem(const TaskHolder& holder) {
    return CollectSystemNote(holder, std::get<SystemNote>(notes_));
  }

  fit::result<Error> CollectKernel();

  fit::result<Error, size_t> DumpHeaders(DumpCallback dump, time_t mtime) {
    size_t offset = 0;
    auto append = [&](ByteView data) -> bool {
      bool bail = dump(offset, data);
      offset += data.size();
      return bail;
    };

    // Generate the archive header.
    if (append(ArchiveMagic())) {
      return fit::ok(offset);
    }
    ZX_DEBUG_ASSERT(offset % 2 == 0);

    // The name table member header has been initialized.  Write it out now.
    if (append(name_table_.bytes())) {
      return fit::ok(offset);
    }
    ZX_DEBUG_ASSERT(offset % 2 == 0);

    // Finalize each note by setting its name and date fields, and stream
    // out the contents of the name table at the same time.  Additional
    // members streamed out later can only use the truncated name field in
    // the member header.
    size_t name_table_pos = 0;
    auto finish_note_header = [&](ArchiveMemberHeader& header) -> bool {
      header.set_date(mtime);
      header.set_name_offset(name_table_pos);
      ByteView name = header.name_bytes();
      name_table_pos += name.size();
      return append(name);
    };

    if (!OnRemarkHeaders(finish_note_header)) {
      return fit::ok(offset);
    }

    auto finalize = [&](auto&... note) { return (finish_note_header(note.header()) || ...); };
    if (std::apply(finalize, notes_)) {
      return fit::ok(offset);
    }
    ZX_DEBUG_ASSERT(offset % 2 == name_table_pos % 2);

    if (name_table_pos % 2 != 0 && append(kArchiveMemberPad)) {
      return fit::ok(offset);
    }
    ZX_DEBUG_ASSERT(offset % 2 == 0);

    // Generate the job-wide note data.
    for (ByteView data : notes()) {
      if (append(data)) {
        return fit::ok(offset);
      }
    }
    ZX_DEBUG_ASSERT(offset % 2 == 0);

    // Generate the remarks note data.
    for (ByteView data : GetRemarks()) {
      if (append(data)) {
        return fit::ok(offset);
      }
    }
    ZX_DEBUG_ASSERT(offset % 2 == 0);

    return fit::ok(offset);
  }

 private:
  struct JobInfoClass {
    using Handle = Job;
    static constexpr auto MakeHeader = kMakeMember<kJobInfoName>;
    static constexpr auto Pad = PadForArchive;
  };

  template <zx_object_info_topic_t Topic>
  using JobInfo = InfoNote<JobInfoClass, Topic>;

  struct JobPropertyClass {
    using Handle = Job;
    static constexpr auto MakeHeader = kMakeMember<kJobPropertyName>;
    static constexpr auto Pad = PadForArchive;
  };

  template <uint32_t Prop>
  using JobProperty = PropertyNote<JobPropertyClass, Prop>;

  struct SystemClass {
    static constexpr auto MakeHeader = kMakeMember<kSystemNoteName, true>;
    static constexpr auto Pad = PadForArchive;
  };

  using SystemNote = JsonNote<SystemClass>;

  struct KernelInfoClass {
    using Handle = Resource;
    static constexpr auto MakeHeader = kMakeMember<kKernelInfoNoteName>;
    static constexpr auto Pad = PadForArchive;
  };

  template <typename... Notes>
  using WithKernelNotes = KernelNotes<KernelInfoClass, Notes...>;

  // These are named for use by CollectChildren and CollectProcesses.
  using Children = JobInfo<ZX_INFO_JOB_CHILDREN>;
  using Processes = JobInfo<ZX_INFO_JOB_PROCESSES>;

  using JobNotes = WithKernelNotes<
      // This lists all the notes for job-wide state.
      JobInfo<ZX_INFO_HANDLE_BASIC>, JobProperty<ZX_PROP_NAME>,
      // Ordering of the other notes is not specified and can change.
      SystemNote,                      // Optionally included in any given job.
      JobInfo<ZX_INFO_JOB>,            //
      JobInfo<ZX_INFO_JOB_CHILDREN>,   //
      JobInfo<ZX_INFO_JOB_PROCESSES>,  //
      JobInfo<ZX_INFO_TASK_RUNTIME>>;

  // Returns a vector of views into the storage held in this->notes_.
  NoteData::Vector notes() const { return std::apply(DumpNoteData, notes_); }

  std::reference_wrapper<Job> job_;
  ArchiveMemberHeader name_table_;
  JobNotes notes_;
};

ByteView JobDump::ArchiveMagic() { return std::as_bytes(std::span(kArchiveMagic)); }

JobDump::JobDump(JobDump&&) noexcept = default;
JobDump& JobDump::operator=(JobDump&&) noexcept = default;
JobDump::~JobDump() = default;

Job& JobDump::job() const { return collector_->job(); }

fit::result<Error> JobDump::CollectKernel() { return collector_->CollectKernel(); }

fit::result<Error> JobDump::CollectSystem(const TaskHolder& holder) {
  return collector_->CollectSystem(holder);
}

fit::result<Error, size_t> JobDump::CollectJob() { return collector_->CollectJob(); }

fit::result<Error, std::reference_wrapper<Job::JobMap>> JobDump::CollectChildren() {
  return collector_->CollectChildren();
}

fit::result<Error, std::reference_wrapper<Job::ProcessMap>> JobDump::CollectProcesses() {
  return collector_->CollectProcesses();
}

fit::result<Error, size_t> JobDump::DumpHeadersImpl(DumpCallback callback, time_t mtime) {
  return collector_->DumpHeaders(std::move(callback), mtime);
}

fit::result<Error, size_t> JobDump::DumpMemberHeaderImpl(DumpCallback callback, size_t offset,
                                                         std::string_view name, size_t size,
                                                         time_t mtime) {
  ArchiveMemberHeader header{name};
  header.set_size(size);
  header.set_date(mtime);
  callback(offset, header.bytes());
  return fit::ok(offset + header.bytes().size());
}

size_t JobDump::MemberHeaderSize() { return sizeof(ar_hdr); }

fit::result<Error> ProcessDump::Collector::CollectKernel() {
  return CollectKernelNoteData(process_, notes_);
}

fit::result<Error> JobDump::Collector::CollectKernel() {
  return CollectKernelNoteData(job_, notes_);
}

fit::result<Error> JobDump::Remarks(std::string_view name, ByteView data) {
  collector_->AddRemarks(name, data);
  return fit::ok();
}

// A single Collector cannot be used for a different job later.  It can be
// clear()'d to reset all state other than the job handle.
JobDump::JobDump(Job& job) noexcept : collector_{new Collector{job}} {}

}  // namespace zxdump
