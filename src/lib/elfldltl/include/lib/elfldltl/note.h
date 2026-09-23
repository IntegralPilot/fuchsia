// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_NOTE_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_NOTE_H_

#include <lib/fit/result.h>

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <optional>
#include <span>
#include <string_view>
#include <tuple>
#include <type_traits>

#include "layout.h"
#include "memory.h"
#include "phdr.h"

namespace elfldltl {

// The usual way to use the note parser is via elfldltl::Elf<...>::NoteSegment,
// which is an alias for the NoteSegment class defined below.

// This represents one decoded ELF note.  It's created ephemerally to yield
// views on the name and desc (payload), along with the type value.
struct ElfNote {
  using Bytes = std::span<const std::byte>;

  constexpr ElfNote() = default;

  constexpr ElfNote(const ElfNote&) = default;

  template <typename Header>
  ElfNote(const Header& nhdr, Bytes note)
      : name(std::string_view{reinterpret_cast<const char*>(note.data()), note.size()}.substr(
            nhdr.name_offset(), nhdr.namesz)),
        desc(note.subspan(nhdr.desc_offset(), nhdr.descsz)),
        type(nhdr.type) {}

  constexpr ElfNote& operator=(const ElfNote& other) = default;

  // Match against an expected name.
  template <size_t N>
  constexpr bool Is(const char (&that_name)[N]) const {
    return name == std::string_view{that_name, N};
  }

  // Match against an expected name and type.
  template <typename T, size_t N>
  constexpr bool Is(const char (&that_name)[N], const T& that_type) const {
    static_assert(sizeof(T) <= sizeof(uint32_t));
    return type == static_cast<uint32_t>(that_type) && Is(that_name);
  }

  // Match a GNU build ID note.
  constexpr bool IsBuildId() const { return Is("GNU", ElfNoteType::kGnuBuildId); }

  // Call `out(char)` to emit each desc byte in hex.
  template <typename Out>
  constexpr void HexDump(Out&& out) const {
    constexpr auto& kDigits = "0123456789abcdef";
    for (auto byte : desc) {
      auto x = static_cast<uint8_t>(byte);
      out(kDigits[x >> 4]);
      out(kDigits[x & 0xf]);
    }
  }

  // Send the hex string of desc to the stdio stream.
  void HexDump(FILE* f) const {
    HexDump([f](char c) { putc(c, f); });
  }

  // Return the number of characters HexDump() will write.
  constexpr size_t HexSize() const { return desc.size() * 2; }

  // Fill a fixed-sized buffer with as many hex characters as will fit.
  template <size_t N>
  constexpr std::string_view HexString(char (&buffer)[N]) const {
    size_t i = 0;
    HexDump([&](char c) {
      if (i < N) {
        buffer[i++] = c;
      }
    });
    return {buffer, i};
  }

  std::string_view name;
  Bytes desc;
  uint32_t type = 0;
};

// This is a forward-iterable container view of notes in a note segment,
// constructible from the raw bytes.
template <ElfData Data = ElfData::kNative>
class ElfNoteSegment {
 public:
  using Bytes = ElfNote::Bytes;

  using Nhdr = LayoutBase<Data>::Nhdr;

  class iterator {
   public:
    constexpr iterator() = default;
    constexpr iterator(const iterator&) = default;

    constexpr bool operator==(const iterator& other) const {
      return other.notes_.data() == notes_.data() && other.notes_.size() == notes_.size();
    }

    constexpr bool operator!=(const iterator& other) const { return !(*this == other); }

    ElfNote operator*() const {
      assert(Check(notes_));
      return {Header(notes_), notes_};
    }

    iterator& operator++() {  // prefix
      assert(Check(notes_));
      notes_ = notes_.subspan(Header(notes_).size_bytes());
      if (!Check(notes_)) {
        // Ignore any odd bytes at the end of the segment and move to end()
        // state if there isn't space for another note.
        notes_ = notes_.subspan(notes_.size());
      }
      assert(notes_.empty() || Check(notes_));
      return *this;
    }

    iterator operator++(int) {  // postfix
      iterator result = *this;
      ++*this;
      return result;
    }

   private:
    friend ElfNoteSegment;

    // notes_ holds the remainder to be iterated over.  It's always empty (the
    // end() state), or else contains at least one whole valid note.

    explicit constexpr iterator(Bytes notes) : notes_(notes) {
      assert(notes_.empty() || Check(notes_));
    }

    Bytes notes_;
  };

  using const_iterator = iterator;

  constexpr ElfNoteSegment() = default;

  constexpr ElfNoteSegment(const ElfNoteSegment&) = default;

  explicit constexpr ElfNoteSegment(Bytes notes)
      : notes_(notes.size() < sizeof(Nhdr) ? Bytes{} : notes) {}

  constexpr ElfNoteSegment& operator=(const ElfNoteSegment&) = default;

  constexpr ElfNoteSegment& operator=(Bytes notes) noexcept {
    *this = ElfNoteSegment(notes);
    return *this;
  }

  iterator begin() const { return Check(notes_) ? iterator{notes_} : end(); }

  iterator end() const { return iterator{notes_.subspan(notes_.size())}; }

 private:
  // This is safe only if the size has been checked.
  static Nhdr Header(Bytes data) {
    assert(data.size_bytes() >= sizeof(Nhdr));
    // Use memcpy just in case the data is not properly aligned in memory.
    Nhdr nhdr;
    memcpy(&nhdr, data.data(), sizeof(nhdr));
    return nhdr;
  }

  // Returns true if the data starts with a valid note.
  static bool Check(Bytes data) {
    return data.size() >= sizeof(Nhdr) && Check(Header(data), data.size());
  }

  // Returns true if the header is valid for the given size.
  static constexpr bool Check(const Nhdr& hdr, size_t size) {
    // Take pains to avoid integer overflow.
    const size_t name_pad = Nhdr::Align(hdr.namesz) - hdr.namesz;
    const size_t desc_pad = Nhdr::Align(hdr.descsz) - hdr.descsz;
    return size - hdr.name_offset() >= hdr.namesz &&
           size - hdr.name_offset() - hdr.namesz >= name_pad &&
           size - hdr.desc_offset() >= hdr.descsz &&
           size - hdr.desc_offset() - hdr.descsz >= desc_pad;
  }

  Bytes notes_;
};

// This is a shared base class for elfldltl::PhdrFileNoteObserver and
// elfldltl::PhdrMemoryNoteObserver, see below.

template <ElfData Data, typename... Callback>
class PhdrNoteObserverBase {
 public:
  static_assert((std::is_invocable_r_v<fit::result<fit::failed, bool>, Callback, ElfNote> && ...));

  using NoteSegment = ElfNoteSegment<Data>;

  PhdrNoteObserverBase() = delete;

  constexpr PhdrNoteObserverBase(const PhdrNoteObserverBase&) = default;
  constexpr PhdrNoteObserverBase(PhdrNoteObserverBase&&) = default;

  constexpr PhdrNoteObserverBase& operator=(const PhdrNoteObserverBase&) = default;
  constexpr PhdrNoteObserverBase& operator=(PhdrNoteObserverBase&&) = default;

  template <class Diag>
  bool Finish(Diag& diag) {
    return true;
  }

 protected:
  explicit constexpr PhdrNoteObserverBase(Callback... callback)
      : callback_{std::forward<Callback>(callback)...} {}

  constexpr bool AllCallbacksDone() const {
    constexpr auto all_callbacks_done = [](const auto&... callback) -> bool {
      return (!callback && ...);
    };
    return std::apply(all_callbacks_done, callback_);
  }

  constexpr bool DoCallbacks(ElfNote::Bytes bytes) {
    for (const ElfNote& note : NoteSegment{bytes}) {
      auto all_callbacks_ok = [&note](auto&&... callback) -> bool {
        auto do_callback = [&note](auto& callback) -> bool {
          if (callback) {
            fit::result<fit::failed, bool> result = (*callback)(note);
            if (result.is_error()) [[unlikely]] {
              return false;
            }
            // This callback was happy.  Does it want more calls?
            if (!result.value()) {
              callback.reset();
            }
          }
          return true;
        };
        return (do_callback(callback) && ...);
      };
      if (!std::apply(all_callbacks_ok, callback_)) [[unlikely]] {
        return false;
      }
    }
    return true;
  }

 private:
  std::tuple<std::optional<Callback>...> callback_;
};

// elfldltl::PhdrFileNoteObserver(file_api_object, callback...) can be passed
// to elfldltl::DecodePhdrs to call each callback, as if a function with the
// type `fit::result<fit::failed, bool>(ElfNote)`, on each note in the file.
// It returns false the first time there in an error return from a callback, or
// earlier if there is a problem reading notes from the file.  The callback's
// bool success value says whether this callback needs to be called again for
// future notes.  When no callback wants to see more notes, the observer will
// stop decoding them but still return true so the calling DecodePhdrs pass
// continues to call other observers.
//
// This will read both allocated and non-allocated notes, but always read them
// from the file rather than from memory (for allocated notes, it should be the
// same data as if the file were loaded into memory and then the notes read out
// of memory, unless the note contents are writable or RELRO data).
template <ElfData Data, class File, class Allocator, typename... Callback>
class PhdrFileNoteObserver
    : public PhdrObserver<PhdrFileNoteObserver<Data, File, Allocator, Callback...>,
                          ElfPhdrType::kNote>,
      public PhdrNoteObserverBase<Data, Callback...> {
 public:
  using Base = PhdrNoteObserverBase<Data, Callback...>;

  PhdrFileNoteObserver() = delete;

  // Copyable and/or movable if Allocator and Callback are.
  constexpr PhdrFileNoteObserver(const PhdrFileNoteObserver&) = default;
  constexpr PhdrFileNoteObserver(PhdrFileNoteObserver&&) noexcept = default;

  template <ElfApi Elf>
  explicit constexpr PhdrFileNoteObserver(Elf&& elf, File& file, Allocator allocator,
                                          Callback... callback)
      : Base{std::forward<Callback>(callback)...}, file_(&file), allocator_(std::move(allocator)) {
    static_assert(std::decay_t<Elf>::kData == Data);
    static_assert(std::is_copy_constructible_v<PhdrFileNoteObserver> ==
                  (std::is_copy_constructible_v<Allocator> &&
                   (std::is_copy_constructible_v<Callback> && ...)));
    static_assert(std::is_move_constructible_v<PhdrFileNoteObserver> ==
                  (std::is_move_constructible_v<Allocator> &&
                   (std::is_move_constructible_v<Callback> && ...)));
    static_assert(
        std::is_copy_assignable_v<PhdrFileNoteObserver> ==
        (std::is_copy_assignable_v<Allocator> && (std::is_copy_assignable_v<Callback> && ...)));
    static_assert(
        std::is_move_assignable_v<PhdrFileNoteObserver> ==
        (std::is_move_assignable_v<Allocator> && (std::is_move_assignable_v<Callback> && ...)));
  }

  // Copy-assignable and/or move-assignable if Allocator and Callback are.
  constexpr PhdrFileNoteObserver& operator=(const PhdrFileNoteObserver&) = default;
  constexpr PhdrFileNoteObserver& operator=(PhdrFileNoteObserver&&) noexcept = default;

  template <class Diag, typename Phdr>
  constexpr bool Observe(Diag& diag, PhdrTypeMatch<ElfPhdrType::kNote> type, const Phdr& phdr) {
    if (phdr.filesz == 0) [[unlikely]] {
      return true;
    }

    // Short-circuit if all no more per-note callbacks are necessary.  We're
    // still getting Observe calls because we return true to indicate no error.
    if (this->AllCallbacksDone()) {
      return true;
    }

    auto bytes = file_->template ReadArrayFromFile<std::byte>(phdr.offset, allocator_, phdr.filesz);
    if (!bytes) [[unlikely]] {
      return diag.FormatError("failed to read note segment from file");
    }

    return this->DoCallbacks(*bytes);
  }

 private:
  File* file_;
  Allocator allocator_;
};

// Deduction guide.  When used without template parameters, the first
// constructor argument is an empty Elf<...> object to identify the format; the
// second is always an lvalue reference (see memory.h); the third is forwarded
// as the allocator object used by File::ReadArrayFromFile<std::byte>; while
// the later arguments (some objects invocable as if functions with type
// `fit::result<fit::failed, bool>(ElfNote)`) are moved or copied so they can
// safely be temporaries.  Use std::ref or std::cref to make the
// PhdrFileNoteObserver object hold a callback by reference instead.
template <ElfApi Elf, class File, typename Allocator, typename... Callback>
PhdrFileNoteObserver(Elf&&, File&, Allocator&&, Callback&&...)
    -> PhdrFileNoteObserver<std::decay_t<Elf>::kData, File, std::decay_t<Allocator>,
                            std::decay_t<Callback>...>;

// elfldltl::PhdrMemoryNoteObserver works like elfldltl::PhdrFileNoteObserver,
// but reads PT_NOTE contents from an ELF file's in-memory image via the Memory
// API (using p_vaddr and p_memsz) in place of reading from the ELF file itself
// via the File API (using p_offset and p_filesz).
template <ElfData Data, class Memory, typename... Callback>
class PhdrMemoryNoteObserver
    : public PhdrObserver<PhdrMemoryNoteObserver<Data, Memory, Callback...>, ElfPhdrType::kNote>,
      public PhdrNoteObserverBase<Data, Callback...> {
 public:
  using Base = PhdrNoteObserverBase<Data, Callback...>;

  PhdrMemoryNoteObserver() = delete;

  // Copyable and/or movable if Callback is.
  constexpr PhdrMemoryNoteObserver(const PhdrMemoryNoteObserver&) = default;
  constexpr PhdrMemoryNoteObserver(PhdrMemoryNoteObserver&&) noexcept = default;

  template <ElfApi Elf>
  explicit constexpr PhdrMemoryNoteObserver(Elf&& elf, Memory& memory, Callback... callback)
      : Base{std::forward<Callback>(callback)...}, memory_(&memory) {
    static_assert(std::decay_t<Elf>::kData == Data);
    static_assert(std::is_copy_constructible_v<PhdrMemoryNoteObserver> ==
                  (std::is_copy_constructible_v<Callback> && ...));
    static_assert(std::is_move_constructible_v<PhdrMemoryNoteObserver> ==
                  (std::is_move_constructible_v<Callback> && ...));
    static_assert(std::is_copy_assignable_v<PhdrMemoryNoteObserver> ==
                  (std::is_copy_assignable_v<Callback> && ...));
    static_assert(std::is_move_assignable_v<PhdrMemoryNoteObserver> ==
                  (std::is_move_assignable_v<Callback> && ...));
  }

  // Copy-assignable and/or move-assignable if Callback is.
  constexpr PhdrMemoryNoteObserver& operator=(const PhdrMemoryNoteObserver&) = default;
  constexpr PhdrMemoryNoteObserver& operator=(PhdrMemoryNoteObserver&&) noexcept = default;

  template <class Diag, typename Phdr>
  constexpr bool Observe(Diag& diag, PhdrTypeMatch<ElfPhdrType::kNote> type, const Phdr& phdr) {
    if (phdr.memsz == 0) [[unlikely]] {
      return true;
    }

    // Short-circuit if all no more per-note callbacks are necessary.  We're
    // still getting Observe calls because we return true to indicate no error.
    if (this->AllCallbacksDone()) {
      return true;
    }

    auto bytes = memory_->template ReadArray<std::byte>(phdr.vaddr, phdr.memsz);
    if (!bytes) [[unlikely]] {
      return diag.FormatError("failed to read note segment from memory image");
    }

    return this->DoCallbacks(*bytes);
  }

  template <class Diag>
  bool Finish(Diag& diag) {
    return true;
  }

 private:
  Memory* memory_;
  std::tuple<std::optional<Callback>...> callback_;
};

// Deduction guide, as for PhdrFileNoteObserver but with no allocator argument.
template <ElfApi Elf, class Memory, typename... Callback>
PhdrMemoryNoteObserver(Elf&&, Memory&, Callback&&...)
    -> PhdrMemoryNoteObserver<std::decay_t<Elf>::kData, Memory, std::decay_t<Callback>...>;

// This returns a fit::result<fit::failed.bool >(ElfNote) callback object that
// can be passed to PhdrFileNoteObserver or PhdrMemoryNoteObserver.  That
// callback updates build_id to the file's (first) build ID note.  If the
// optional second argument is true, that callback returns fit::ok(false) after
// it's found the build ID, so that PhdrFileNoteObserver would continue to call
// additional callbacks on this and other notes rather than finish immediately.
constexpr auto ObserveBuildIdNote(std::optional<ElfNote>& build_id, bool keep_going = true) {
  return [keep_going, &build_id](const ElfNote& note) -> fit::result<fit::failed, bool> {
    if (!build_id) {
      if (!note.IsBuildId()) {
        return fit::ok(true);
      }
      build_id = note;
    }
    if (!keep_going) {
      return fit::failed();
    }
    return fit::ok(false);
  };
}

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_NOTE_H_
