// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.examples/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/hlcpp_conversion.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_types.h>
#include <fuchsia/sysmem2/cpp/fidl.h>
#include <zircon/availability.h>

// The testing library only works on Fuchsia.
#ifdef __Fuchsia__
#include <fidl/fuchsia.sysmem2/cpp/test_base.h>

class FakeSysmemAllocator : public fidl::testing::TestBase<fuchsia_sysmem2::Allocator> {
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {}
  void handle_unknown_method(::fidl::UnknownMethodMetadata<fuchsia_sysmem2::Allocator> metadata,
                             ::fidl::UnknownMethodCompleter::Sync& completer) override {}
};
#endif

int main() {
  // Use a C++ natural bindings method that is not defined in a header file.
  fuchsia_sysmem2::wire::BufferCollectionConstraints cpp_constraints;
  [[maybe_unused]] bool is_empty_cpp = cpp_constraints.IsEmpty();
  assert(is_empty_cpp);

#ifdef __Fuchsia__
  // Use a type from the testing library. It only contains headers so this is
  // sufficient.
  FakeSysmemAllocator fake_allocator;
#endif

  // Use a type from the HLCPP conversion library. It only contains headers so
  // this is sufficient.
  [[maybe_unused]] struct fidl::internal::NaturalToHLCPPTraits<
      ::fuchsia_images2::PixelFormatModifier> conversion_traits;

  // Use an HLCPP bindings method that is not defined in a header file.
  fuchsia::sysmem2::BufferCollectionConstraints hlcpp_constraints;
  [[maybe_unused]] bool is_empty_hlcpp = hlcpp_constraints.IsEmpty();
  assert(is_empty_hlcpp);

  // Verify the FIDL and Clang API levels are consistent and working correctly.
#if FUCHSIA_API_LEVEL_AT_LEAST(PLATFORM)
  // Both aliases are available in the platform build.
  fuchsia_examples::AvailableUntilApiLevelNext before_next = 10;
  fuchsia_examples::AvailableFromApiLevelNext y_value = 10000 + before_next;
#elif FUCHSIA_API_LEVEL_AT_LEAST(NEXT)  // NEVER_REPLACE_NEXT
  fuchsia_examples::AvailableFromApiLevelNext y_value = 10000;
#else
  fuchsia_examples::AvailableUntilApiLevelNext y_value = 30;
#endif

  fuchsia_math::Vec vec(1, y_value);
  return vec.x() + vec.y();
}
