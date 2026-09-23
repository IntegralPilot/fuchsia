// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_UBER_STRUCT_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_UBER_STRUCT_H_

#include <memory>
#include <memory_resource>
#include <string>
#include <unordered_map>

#include "src/ui/scenic/lib/allocation/image_metadata.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/transform_graph.h"
#include "src/ui/scenic/lib/flatland/transform_handle.h"
#include "src/ui/scenic/lib/types/rotate_flip.h"

#include <glm/glm.hpp>
#include <glm/mat3x3.hpp>

namespace flatland {

// The presented state of a single Flatland2 layer: what to draw, plus the properties
// which say where and how to draw it.  `UberStruct::layers` maps each `LayerHandle` to
// one of these, and `UberStruct::layer_stacks` lists the handles which make up each
// stack.  The same layer may appear in more than one stack; the map holds a single entry
// per handle.
//
// Both Flatland API versions produce these.  Flatland2 sessions populate them directly;
// Flatland1 sessions go through the facade, which represents each piece of legacy
// image/rect content as a single-layer stack.
//
// This is a snapshot of client-specified state, not render-ready state.  Values are
// stored as the client set them; resolution against the scene graph (inherited opacity,
// clipping, the global transform) happens in `ComputeGlobalResolvedLayers()`, which emits
// the `ResolvedLayer`s that the renderer and display path consume.
//
// The session-side counterpart is `LayerObject` (flatland_session_types.h), which holds
// the properties of every composition mode simultaneously; at `Present()` only the active
// mode's properties are copied here.  Consequently `content` says everything about what
// the layer draws: there is no inactive-mode state in the snapshot.  Like the rest of
// `UberStruct`, this must remain a pure value type: no references to external resources,
// no session-lifetime state.
struct UberStructLayer {
  // Properties which are used regardless of the layer's composition mode.
  struct CommonProperties {
    // Where the layer is drawn, in the local coordinate space of the transform that the
    // layer's stack is attached to.  Content is implicitly scaled to fit this rectangle;
    // ancestor transforms may scale and position it further.  Zero-sized by default: the
    // layer contributes nothing to the frame until a rect with non-zero width and height
    // is set.
    types::Rectangle display_rect;

    // Layer-wide opacity in [0..1], combined multiplicatively with the opacity inherited
    // from ancestor transforms.  Distinct from content alpha: it does not modify the
    // alpha of `SolidColorModeProperties::color`, nor the alpha channel of image pixels.
    float opacity = 1.f;

    // How the layer is composited against what is already behind it.  The blend mode stored
    // here (authored by the Flatland session) may differ from the `ResolvedLayer` blend mode
    // for rendering; factors include:
    //   - effective opacity at that point in the global scene graph
    //   - semantic differences between Flatland 1/2 APIs
    //   - optimization for solid color content
    // See details in `ComputeGlobalResolvedLayers()` and `ResolveBlendAndOpacity()`.
    types::BlendMode blend_mode = types::BlendMode::kReplace();

    bool operator==(const CommonProperties&) const = default;
  };

  // Properties which are used only when the layer is in image mode.
  struct ImageModeProperties {
    // The region of the image to sample from, in unnormalized image (texel) coordinates.
    // Always stored fully resolved: a layer displaying the whole image stores the image's
    // full extent, never a sentinel value.
    types::RectangleF sample_rect;

    // Flip/rotation applied to the sampled region before it is fitted to
    // `CommonProperties::display_rect`.  Stored in `types::RotateFlip`'s own convention
    // (rotate counter-clockwise, then reflect, matching the display coordinator); the
    // Flatland APIs speak flip-then-rotate, and `RotateFlip::From()` converts at the
    // boundary.
    types::RotateFlip transform = types::RotateFlip::kIdentity();

    // The image to sample, or `kInvalidImageId` if no image is bound.  A layer with no
    // bound image contributes nothing to the frame.
    allocation::GlobalImageId image_id = allocation::kInvalidImageId;

    // Dimensions of `image_id`, in texels.  Mirrored here so that consumers of the
    // snapshot need not consult the image registry.
    uint32_t image_width = 0, image_height = 0;

    bool operator==(const ImageModeProperties&) const = default;
  };

  // Properties which are used only when the layer is in solid-color mode.
  struct SolidColorModeProperties {
    // The color which fills `CommonProperties::display_rect`.  Straight
    // (non-premultiplied) alpha, regardless of `blend_mode`; conversion to the
    // premultiplied form expected downstream happens in `ComputeGlobalResolvedLayers()`.
    std::array<float, 4> color = {1.f, 1.f, 1.f, 1.f};

    bool operator==(const SolidColorModeProperties&) const = default;
  };

  bool operator==(const UberStructLayer&) const = default;

  // What the layer draws: the active composition mode's properties.  `monostate` means
  // the layer is invisible; it occupies its place in the stack but contributes nothing to
  // the frame.
  std::variant<std::monostate, ImageModeProperties, SolidColorModeProperties> content;

  // Properties which apply regardless of the composition mode above.
  CommonProperties common;
};

// TODO(https://fxbug.dev/42122511): find the appropriate name for this struct.
// of that instance's presented state. Because the UberStruct represents a snapshot of the local
// state of a Flatland instance, it must be stateless. It should contain only data and no
// references to external resources.
struct UberStruct {
 private:
  // The memory resource used for allocations in this struct. Must be declared first
  // so it is destroyed last.
  std::pmr::monotonic_buffer_resource resource_;

 public:
  using InstanceMap =
      std::unordered_map<TransformHandle::InstanceId, std::shared_ptr<const UberStruct>>;

  UberStruct()
      : local_topology(&resource_),
        local_matrices(&resource_),
        local_opacity_values(&resource_),
        local_clip_regions(&resource_),
        local_hit_regions_map(&resource_),
        layer_stacks(&resource_),
        layers(&resource_),
        debug_name(&resource_) {}

  // Note: this MUST only be used to allocate memory for this UberStruct's fields
  std::pmr::memory_resource* resource() { return &resource_; }

  // The local topology of this Flatland instance.
  TransformGraph::TopologyVector local_topology;

  // The local (i.e. relative to the parent) geometric transformation matrix of each
  // TransformHandle. Handles with no entry indicate an identity matrix.
  std::pmr::unordered_map<TransformHandle, glm::mat3> local_matrices;

  // The local (i.e. relative to the parent) opacity values of each TransformHandles. Handles
  // with no entry indicate an opacity value of 1.0.
  std::pmr::unordered_map<TransformHandle, float> local_opacity_values;

  // Map of the regions of transforms that clip child content.
  std::pmr::unordered_map<TransformHandle, TransformClipRegion> local_clip_regions;

  // Map of local hit regions.
  std::pmr::unordered_map<TransformHandle, std::pmr::vector<flatland::HitRegion>>
      local_hit_regions_map;

  // Flatland2 layer stacks associated with each TransformHandle.
  std::pmr::unordered_map<TransformHandle, std::pmr::vector<LayerHandle>> layer_stacks;

  // Flatland2 layers.
  std::pmr::unordered_map<LayerHandle, UberStructLayer> layers;

  // Describes the API version of the Flatland session which authored this UberStruct.  Almost all
  // fields are interpreted identically, regardless of which API produced it (this is the whole
  // point of having Flatland1/2 share the same UberStruct schema).  However, there are a few places
  // where classic Flatland1 semantics are incompatible with the Composer3 HAL semantics that
  // Flatland2 is designed to support.
  uint32_t flatland_version = 1;

  // The ViewRef for the root (View) of this Flatland instance.
  // Can be nullptr when not attached to the scene, otherwise must be set.
  std::shared_ptr<const ViewRef> view_ref = nullptr;

  // Set from SetDebugName(). Can be empty if the client does not calls SetDebugName().
  std::pmr::string debug_name;

  // The time the UberStruct was created.
  zx::time_monotonic creation_time;

  // Test-only helper which abstracts over legacy image content, and Flatland2 layer content.
  bool HasLayerContentForTest(TransformHandle handle) const {
    return layer_stacks.contains(handle);
  }
};

std::ostream& operator<<(std::ostream& out, const UberStructLayer::ImageModeProperties& image);
std::ostream& operator<<(std::ostream& out, const UberStructLayer::SolidColorModeProperties& solid);
std::ostream& operator<<(std::ostream& out, const UberStructLayer& layer);
std::ostream& operator<<(std::ostream& out, const UberStruct& us);

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_UBER_STRUCT_H_
