// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_FAKE_BUFFER_COLLECTION_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_FAKE_BUFFER_COLLECTION_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>

#include <gtest/gtest.h>

namespace intel_display {

// Used by `FakeBufferCollection`.
struct FakeBufferCollectionConfig {
  // If true, the `FakeBufferCollection` allows BufferCollection constraints set by the client in
  // `SetConstraints()` to include a `cpu_domain_supported` constraint in its
  // `buffer_memory_constraints`.
  // Otherwise, it crashes when the client provides such constraints in `SetConstraints()`.
  bool cpu_domain_supported = true;

  // If true, the `FakeBufferCollection` allows BufferCollection constraints set by the client in
  // `SetConstraints()` to include a `ram_domain_supported` constraint in its
  // `buffer_memory_constraints`.
  // Otherwise, it crashes when the client provides such constraints in `SetConstraints()`.
  bool ram_domain_supported = true;

  // If true, the `FakeBufferCollection` allows BufferCollection constraints set by the client in
  // `SetConstraints()` to include a `inaccessible_domain_supported` constraint in its
  // `buffer_memory_constraints`.
  // Otherwise, it crashes when the client provides such constraints in `SetConstraints()`.
  bool inaccessible_domain_supported = true;

  // Width of the created image, if the client doesn't specify in its `image_format_constraints`.
  int width_fallback_px = 1;

  // Height of the created image, if the client doesn't specify in its `image_format_constraints`.
  int height_fallback_px = 1;

  int bytes_per_row_divisor = 1;
  fuchsia_images2::wire::PixelFormatModifier format_modifier =
      fuchsia_images2::wire::PixelFormatModifier::kLinear;
};

// A fake BufferCollection implementation that does not share constraints
// with other BufferCollections, i.e. it allocates memory using only the
// constraints set to this BufferCollection.
//
// TODO(https://fxbug.dev/42072949): Consider creating and using a unified set of sysmem
// testing doubles instead of writing mocks for each display driver test.
class FakeBufferCollection : public fidl::testing::WireTestBase<fuchsia_sysmem2::BufferCollection> {
 public:
  explicit FakeBufferCollection(const FakeBufferCollectionConfig& config);
  ~FakeBufferCollection() override;

  bool HasConstraints() const { return !constraints_.IsEmpty(); }

  // fuchsia_sysmem2::BufferCollection:
  void SetConstraints(SetConstraintsRequestView request,
                      SetConstraintsCompleter::Sync& _completer) override;
  void CheckAllBuffersAllocated(CheckAllBuffersAllocatedCompleter::Sync& completer) override;
  void WaitForAllBuffersAllocated(WaitForAllBuffersAllocatedCompleter::Sync& completer) override;

  // fidl::testing::WireTestBase:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override;

 private:
  fidl::Arena<fidl::kDefaultArenaInitialCapacity> arena_;
  FakeBufferCollectionConfig config_;
  fuchsia_sysmem2::BufferCollectionConstraints constraints_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_FAKE_BUFFER_COLLECTION_H_
