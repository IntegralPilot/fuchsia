// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_STUB_RUNTIME_SERVICES_H_
#define ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_STUB_RUNTIME_SERVICES_H_

#include <list>
#include <map>
#include <optional>
#include <string_view>
#include <vector>

#include <efi/runtime-services.h>
#include <efi/variable/variable.h>
#include <fbl/vector.h>
#include <gmock/gmock.h>

#include "mock_protocol_base.h"

namespace efi {

// Runtime services EFI stubs.
//
// The runtime services EFI table is complicated enough that it would be
// difficult to fake out all the APIs properly. Instead, we provide these stubs
// to allow tests to easily mock out the functionality they need, either with
// gmock or by subclassing and implementing the functions they need.
//
// Some of the more trivial functionality will be implemented, but can still
// be overridden by subclasses.
//
// Tests that are willing to use gmock should generally prefer to use
// MockRuntimeServices instead, which hooks up the proper mock wrappers and adds
// some additional utility functions.
class StubRuntimeServices {
 private:
  // Not copyable or movable.
  StubRuntimeServices(const StubRuntimeServices&) = delete;
  StubRuntimeServices& operator=(const StubRuntimeServices&) = delete;

 public:
  // IMPORTANT: only ONE StubRuntimeServices can exist at a time. Since this is
  // intended to be a global singleton in EFI this shouldn't be a problem,
  // but if a test does attempt to create a second it will cause an exit().
  StubRuntimeServices();
  virtual ~StubRuntimeServices();

  // Returns the underlying efi_runtime_services struct.
  efi_runtime_services* services() { return &services_; }

  void SetVariables(const std::list<Variable>& vars);
  void UnsetVariables();

  // EFI function implementations.

  virtual efi_status GetTime(efi_time* time, efi_time_capabilities* capabilities) {
    return EFI_UNSUPPORTED;
  }
  virtual efi_status SetTime(efi_time* time) { return EFI_UNSUPPORTED; }
  virtual efi_status GetWakeupTime(bool* enabled, bool* pending, efi_time* time) {
    return EFI_UNSUPPORTED;
  }
  virtual efi_status SetWakeupTime(bool enable, efi_time* time) { return EFI_UNSUPPORTED; }
  virtual efi_status SetVirtualAddressMap(size_t memory_map_size, size_t desc_size,
                                          uint32_t desc_version,
                                          efi_memory_descriptor* virtual_map) {
    return EFI_UNSUPPORTED;
  }
  virtual efi_status ConvertPointer(size_t debug_disposition, void** addr) {
    return EFI_UNSUPPORTED;
  }
  virtual efi_status GetVariable(char16_t* var_name, efi_guid* vendor_guid, uint32_t* attributes,
                                 size_t* data_size, void* data);
  virtual efi_status GetNextVariableName(size_t* var_name_size, char16_t* var_name,
                                         efi_guid* vendor_guid);
  virtual efi_status SetVariable(char16_t* var_name, efi_guid* vendor_guid, uint32_t attributes,
                                 size_t data_size, const void* data) {
    return EFI_UNSUPPORTED;
  }
  virtual efi_status GetNextHighMonotonicCount(uint32_t* high_count) { return EFI_UNSUPPORTED; }
  virtual efi_status ResetSystem(efi_reset_type reset_type, efi_status reset_status,
                                 size_t data_size, void* reset_data) {
    return EFI_UNSUPPORTED;
  }
  virtual efi_status UpdateCapsule(efi_capsule_header** capsule_header_array, size_t capsule_count,
                                   efi_physical_addr scatter_gather_list) {
    return EFI_UNSUPPORTED;
  }
  virtual efi_status QueryCapsuleCapabilities(efi_capsule_header** capsule_header_array,
                                              size_t capsule_count, uint64_t* max_capsule_size,
                                              efi_reset_type* reset_type) {
    return EFI_UNSUPPORTED;
  }
  virtual efi_status QueryVariableInfo(uint32_t attributes, uint64_t* max_var_storage_size,
                                       uint64_t* remaining_var_storage_size,
                                       uint64_t* max_var_size);

 private:
  efi_runtime_services services_;

  std::optional<std::list<Variable>> vars_;
  std::list<Variable>::const_iterator var_it_;
};

// Subclasses StubRuntimeServices to mock out methods using gmock.
//
// This will likely be the most common way to test runtime services, but gmock
// is significantly more complicated than gtest and some projects may prefer
// to avoid it, so the base class is still available for direct use.
class MockRuntimeServices : public StubRuntimeServices {
 public:
  MOCK_METHOD(efi_status, GetTime, (efi_time * time, efi_time_capabilities* capabilities),
              (override));

  MOCK_METHOD(efi_status, SetTime, (efi_time * time), (override));

  MOCK_METHOD(efi_status, GetWakeupTime, (bool* enabled, bool* pending, efi_time* time),
              (override));

  MOCK_METHOD(efi_status, SetWakeupTime, (bool enable, efi_time* time), (override));

  MOCK_METHOD(efi_status, SetVirtualAddressMap,
              (size_t memory_map_size, size_t desc_size, uint32_t desc_version,
               efi_memory_descriptor* virtual_map),
              (override));

  MOCK_METHOD(efi_status, ConvertPointer, (size_t debug_disposition, void** addr), (override));

  MOCK_METHOD(efi_status, GetVariable,
              (char16_t* var_name, efi_guid* vendor_guid, uint32_t* attributes, size_t* data_size,
               void* data),
              (override));

  MOCK_METHOD(efi_status, GetNextVariableName,
              (size_t * var_name_size, char16_t* var_name, efi_guid* vendor_guid), (override));

  MOCK_METHOD(efi_status, SetVariable,
              (char16_t* var_name, efi_guid* vendor_guid, uint32_t attributes, size_t data_size,
               const void* data),
              (override));

  MOCK_METHOD(efi_status, GetNextHighMonotonicCount, (uint32_t * high_count), (override));

  MOCK_METHOD(efi_status, ResetSystem,
              (efi_reset_type reset_type, efi_status reset_status, size_t data_size,
               void* reset_data),
              (override));

  MOCK_METHOD(efi_status, UpdateCapsule,
              (efi_capsule_header * *capsule_header_array, size_t capsule_count,
               efi_physical_addr scatter_gather_list),
              (override));

  MOCK_METHOD(efi_status, QueryCapsuleCapabilities,
              (efi_capsule_header * *capsule_header_array, size_t capsule_count,
               uint64_t* max_capsule_size, efi_reset_type* reset_type),
              (override));

  MOCK_METHOD(efi_status, QueryVariableInfo,
              (uint32_t attributes, uint64_t* max_var_storage_size,
               uint64_t* remaining_var_storage_size, uint64_t* max_var_size),
              (override));
};

}  // namespace efi

#endif  // ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_STUB_RUNTIME_SERVICES_H_
