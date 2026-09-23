// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_LLVM_PROFDATA_INCLUDE_LIB_LLVM_PROFDATA_LLVM_PROFDATA_H_
#define SRC_LIB_LLVM_PROFDATA_INCLUDE_LIB_LLVM_PROFDATA_LLVM_PROFDATA_H_

#include <cstddef>
#include <span>
#include <string_view>

class LlvmProfdata {
 public:
  struct LiveData {
    std::span<std::byte> counters, bitmap, uniform_counters;
  };

  // Bounds within the raw profile data containing all active live data sections.
  struct LiveDataBounds {
    size_t offset = 0;
    size_t size = 0;
  };

  // This is the minimum alignment required for LiveData::counters::data().
  static size_t LiveDataCountersAlignment() {
    return UsingSingleByteCounters() ? 1 : sizeof(uint64_t);
  }

  // The object can be default-constructed and copied into, but cannot be used
  // in its default-constructed state.
  LlvmProfdata() = default;
  LlvmProfdata(const LlvmProfdata&) = default;
  LlvmProfdata& operator=(const LlvmProfdata&) = default;

  // This initializes the object based on the current module's own
  // instrumentation data.  This must be called before other methods below.
  void Init(std::span<const std::byte> build_id);

  // This returns the size of the data blob to be published.
  // The return value is zero if there is no data to publish.
  size_t size_bytes() const { return size_bytes_; }

  // These return the offset and size within the blob of the aligned uint64_t[]
  // counters array.
  size_t counters_offset() const { return counters_offset_; }
  size_t counters_size_bytes() const { return counters_size_bytes_; }

  // This returns true if single byte counters are used.
  // It is only public to be used by tests.
  static bool UsingSingleByteCounters();

  // These return the offset and size within the blob of the char[] bitmap
  // bytes array.
  size_t bitmap_offset() const { return bitmap_offset_; }
  size_t bitmap_size_bytes() const { return bitmap_size_bytes_; }

  // These return the offset and size within the blob of the uint64_t[] uniform
  // counters array.
  size_t uniform_counters_offset() const { return uniform_counters_offset_; }
  size_t uniform_counters_size_bytes() const { return uniform_counters_size_bytes_; }

  // If the data appears to be valid llvm-profdata format with a build ID, then
  // return the subspan that is just the build ID bytes themselves.  Otherwise
  // return an empty span.  This does only minimal format validation that is
  // sufficient to find the build ID safely, and does not guarantee that the
  // other sizes in the header are valid.
  static std::span<const std::byte> BuildIdFromRawProfile(std::span<const std::byte> data);

  // Return true if data appears to be a valid llvm-profdata dump whose build
  // ID matches the one passed to Init.
  bool Match(std::span<const std::byte> data);

  // This must be passed a span of at least size_bytes() whose pointer must be
  // aligned to kAlign bytes.  Write the fixed metadata into the buffer, but
  // leave the live data areas in the buffer untouched.  Returns the subspans
  // covering the live data.
  LiveData WriteFixedData(std::span<std::byte> data) { return DoFixedData(data, false); }

  // Verify the contents after Match(data) is true, causing assertion failures
  // if the data was corrupted.  After this, the data is verified to match what
  // WriteFixedData would have written.  Returns the subspans covering the
  // live data, just as WriteFixedData would have done.
  LiveData VerifyMatch(std::span<std::byte> data) { return DoFixedData(data, true); }

  // Return the bounds within the raw profile data that contain all active
  // live data sections.
  LiveDataBounds bounds() const;

  // Return the LiveData containing the individual subspans (e.g. counters,
  // bitmap) pointing into a memory region that maps the live data bounds.
  // The size of the `bounds` span must be at least `bounds().size` bytes.
  LiveData LiveDataForBounds(std::span<std::byte> bounds_data) const;

  // Copy out the current live data values from their link-time locations where
  // they have accumulated since startup.  The data.counters buffer must be at
  // least counters_size_bytes() and must be aligned as uint64, and the
  // data.bitmap must be at least bitmap_size_bytes(), usually the return value
  // of WriteFixedData or VerifyMatch.
  void CopyLiveData(LiveData data);

  // This is like CopyLiveData, but instead of overwriting the buffer, it
  // merges the data with the existing live data values in the buffer.
  void MergeLiveData(LiveData data);

  // This merges the from values into the to values.
  static void MergeLiveData(LiveData to, LiveData from);

  // After CopyLiveData or MergeLiveData has prepared the buffers, start using
  // them for live data updates.  This can be called again later to switch to
  // different buffers.
  static void UseLiveData(LiveData data);

  // This resets the runtime after UseLiveData so that the original link-time
  // counter locations will be updated hereafter.  It's only used in tests.
  static void UseLinkTimeLiveData();

  // The data blob must be aligned to 8 bytes in memory.
  static constexpr size_t kAlign = 8;

  // This is the name associated with the data in the fuchsia.debugdata FIDL
  // protocol.
  static constexpr std::string_view kDataSinkName = "llvm-profile";
  static constexpr std::string_view kFileSuffix = ".profraw";

  // This is a human-readable title used in log messages about the dump.
  static constexpr std::string_view kAnnounce = "LLVM Profile";

 private:
  LiveData DoFixedData(std::span<std::byte> data, bool match);

  std::span<const std::byte> build_id_;
  size_t size_bytes_ = 0;
  size_t counters_offset_ = 0;
  size_t counters_size_bytes_ = 0;
  size_t bitmap_size_bytes_ = 0;
  size_t bitmap_offset_ = 0;
  size_t uniform_counters_offset_ = 0;
  size_t uniform_counters_size_bytes_ = 0;
};

#endif  // SRC_LIB_LLVM_PROFDATA_INCLUDE_LIB_LLVM_PROFDATA_LLVM_PROFDATA_H_
