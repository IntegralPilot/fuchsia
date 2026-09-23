// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_BUFFER_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_BUFFER_H_

#if defined(__Fuchsia__)
#include <lib/zx/vmo.h>
#endif

#include <lib/magma/magma_common_defs.h>
#include <lib/magma/platform/platform_handle.h>
#include <lib/magma/platform/platform_object.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/macros.h>
#include <lib/magma/util/status.h>

#include <memory>

namespace magma {

// In general only the const functions in this class must be implemented in a threadsafe way
class PlatformBuffer : public PlatformObject {
 public:
  class MappingAddressRange {
   public:
    virtual ~MappingAddressRange() {}

    virtual uint64_t Length() = 0;
    virtual uint64_t Base() = 0;

    static std::unique_ptr<MappingAddressRange> CreateDefault() { return Create(nullptr); }
    static std::unique_ptr<MappingAddressRange> Create(
        std::unique_ptr<magma::PlatformHandle> handle);

   protected:
    MappingAddressRange() {}
    MappingAddressRange(MappingAddressRange&) = delete;
    void operator=(MappingAddressRange&) = delete;
  };

  class Mapping {
   public:
    virtual ~Mapping() {}
    virtual void* address() = 0;
  };

  enum Flags {
    kMapRead = 1,
    kMapWrite = 2,
  };

  static std::unique_ptr<PlatformBuffer> Create(uint64_t size, const char* name);

  // Import takes ownership of the handle.
  static std::unique_ptr<PlatformBuffer> Import(uint32_t handle);

#if defined(__Fuchsia__)
  static std::unique_ptr<PlatformBuffer> Import(zx::vmo handle);
#endif

  virtual ~PlatformBuffer() {}

  // Returns the size of the buffer.
  virtual uint64_t size() const = 0;

  virtual bool IsMapped() const = 0;

  // Creates a duplicate handle whose lifetime can be tracked with HasChildren.
  virtual bool CreateChild(uint32_t* handle_out) = 0;

  // Returns true if one or more child buffers exist.
  // Note: when last child is released, transition from true to false may have some delay.
  virtual bool HasChildren() const = 0;

  // Ensures the specified pages are backed by real memory.
  // note: the implementation of this function is required to be threadsafe.
  virtual bool CommitPages(uint64_t start_page_index, uint64_t page_count) const = 0;

  // Tries to ensure the pages are not backed by memory, and resets their contents to 0. May fail if
  // the VMO is pinned.
  virtual magma::Status DecommitPages(uint64_t start_page_index, uint64_t page_count) const = 0;

  virtual bool GetBufferInfo(magma_buffer_info_t* buffer_info_out) const = 0;

  // If |alignment| isn't 0, it must be a power of 2 and page-aligned. It's
  // invalid to map the same buffer twice with different alignments.
  virtual bool MapCpu(void** addr_out, uintptr_t alignment = 0) = 0;
  virtual bool UnmapCpu() = 0;

  virtual bool MapAtCpuAddr(uint64_t addr, uint64_t offset, uint64_t length) = 0;

  // |flags| is a set of elements of Flags, above.
  virtual bool MapCpuWithFlags(uint64_t offset, uint64_t length, uint64_t flags,
                               std::unique_ptr<Mapping>* mapping_out) = 0;

  virtual bool CleanCache(uint64_t offset, uint64_t size, bool invalidate) = 0;

  virtual bool SetCachePolicy(magma_cache_policy_t cache_policy) = 0;

  virtual magma_status_t GetCachePolicy(magma_cache_policy_t* cache_policy_out) = 0;

  // Returns true if MapCpu should be able to return true. The buffer must be
  // readable and writable.
  virtual magma_status_t GetIsMappable(magma_bool_t* is_mappable_out) = 0;

  virtual magma::Status SetMappingAddressRange(
      std::unique_ptr<MappingAddressRange> address_range) = 0;

  // Read from an offset in the VMO into a buffer.
  virtual bool Read(void* buffer, uint64_t offset, uint64_t length) = 0;

  // Write from a buffer into an offset in the VMO.
  virtual bool Write(const void* buffer, uint64_t offset, uint64_t length) = 0;

  virtual bool SetName(const char* name) = 0;
  virtual std::string GetName() const = 0;

  static bool IdFromHandle(uint32_t handle, uint64_t* id_out);

  // Deprecated
  static uint64_t MinimumMappableAddress();

  // Deprecated; returns the length of the region where memory can be mapped.
  static uint64_t MappableAddressRegionLength();
};

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_PLATFORM_PLATFORM_BUFFER_H_
