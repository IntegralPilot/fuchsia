// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_IMAGE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_IMAGE_H_

#include <lib/inspect/cpp/inspect.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <fbl/intrusive_container_utils.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/mutex.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

#include "src/graphics/display/drivers/coordinator/client-id.h"
#include "src/graphics/display/drivers/coordinator/id-map.h"
#include "src/graphics/display/drivers/coordinator/image-lifecycle-listener.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"

namespace display_coordinator {

class Image;

// Manages the lifetime of an imported sysmem pixel buffer.
//
// Instances are not thread-safe. Concurrent access must be synchronized
// externally.
class Image : public fbl::RefCounted<Image>,
              public IdMappable<fbl::RefPtr<Image>, display::ImageId> {
 private:
  // Private forward declaration.
  template <typename PtrType, typename TagType>
  struct DoublyLinkedListTraits;

  // Private typename aliases for DoublyLinkedList definition.
  using DoublyLinkedListPointer = fbl::RefPtr<Image>;
  using DefaultDoublyLinkedListTraits =
      DoublyLinkedListTraits<DoublyLinkedListPointer, fbl::DefaultObjectTag>;

 public:
  // This defines the specific type of fbl::DoublyLinkedList that an Image can
  // be placed into. Any intrusive container that can hold an Image must be of
  // type Image::DoublyLinkedList.
  //
  // Note that the default fbl::DoublyLinkedList doesn't work in this case, due
  // to the intrusive linked list node is guarded by a mutex.
  using DoublyLinkedList = fbl::DoublyLinkedList<DoublyLinkedListPointer, fbl::DefaultObjectTag,
                                                 fbl::SizeOrder::N, DefaultDoublyLinkedListTraits>;

  // `lifecycle_listener` must be non-null, and must outlive the Image.
  Image(ImageLifecycleListener* listener, const display::ImageMetadata& metadata,
        display::ImageId id, display::DriverImageId driver_id, inspect::Node* parent_node,
        ClientId client_id);

  Image(const Image&) = delete;
  Image(Image&&) = delete;
  Image& operator=(const Image&) = delete;
  Image& operator=(Image&&) = delete;

  ~Image();

  display::DriverImageId driver_id() const { return driver_id_; }
  const display::ImageMetadata& metadata() const { return metadata_; }

  // The client that owns the image.
  ClientId client_id() const { return client_id_; }

  void set_latest_driver_config_stamp(display::DriverConfigStamp driver_config_stamp) {
    latest_driver_config_stamp_ = driver_config_stamp;
  }
  display::DriverConfigStamp latest_driver_config_stamp() const {
    return latest_driver_config_stamp_;
  }

  void set_latest_client_config_stamp(display::ConfigStamp stamp) {
    latest_client_config_stamp_ = stamp;
  }
  display::ConfigStamp latest_client_config_stamp() const { return latest_client_config_stamp_; }

  // Disposed images do not release engine driver-side resources on destruction.
  //
  // This state is necessary for safely shutting down an engine driver. When
  // that happens, the driver may still be presenting some images. We want to
  // clear out our data structures, but cannot call ReleaseImage() on those
  // images.
  void MarkDisposed() { disposed_ = true; }

  // Checks if the Image is in a DoublyLinkedList container.
  // TODO(https://fxbug.dev/317914671): investigate whether storing Images in doubly-linked lists
  //                                    continues to be desirable.
  bool InDoublyLinkedList() const;

  // Removes the Image from the DoublyLinkedList. The Image must be in a
  // DoublyLinkedList when this is called.
  DoublyLinkedListPointer RemoveFromDoublyLinkedList();

 private:
  // This defines the node trait used by the fbl::DoublyLinkedList that an Image
  // can be placed in. PointerType and TagType are required for template
  // argument resolution purpose in `fbl::DoublyLinkedList`.
  template <typename PointerType, typename TagType>
  struct DoublyLinkedListTraits {
   public:
    static auto& node_state(Image& obj) { return obj.doubly_linked_list_node_state_; }
  };
  friend DoublyLinkedListTraits<DoublyLinkedListPointer, fbl::DefaultObjectTag>;

  void InitializeInspect(inspect::Node* parent_node);

  // This NodeState allows the Image to be placed in an intrusive
  // Image::DoublyLinkedList which can be either a Client's waiting image
  // list, or the Controller's presented image list.
  //
  // The presented image list is protected with the controller mutex, and the
  // waiting list is only accessed on the loop and thus is not generally
  // protected. However, transfers between the lists are protected by the
  // controller mutex.
  fbl::DoublyLinkedListNodeState<DoublyLinkedListPointer,
                                 fbl::NodeOptions::AllowRemoveFromContainer>
      doubly_linked_list_node_state_;

  const display::DriverImageId driver_id_;
  const display::ImageMetadata metadata_;

  ImageLifecycleListener& lifecycle_listener_;
  const ClientId client_id_;

  // Stamp of the latest applied display configuration that uses this image.
  display::DriverConfigStamp latest_driver_config_stamp_ = display::kInvalidDriverConfigStamp;

  // Stamp of the latest display configuration in Client (the DisplayController
  // FIDL service) that uses this image.
  //
  // Note that for an image, it is possible that its |latest_client_config_stamp_|
  // doesn't match the |latest_controller_config_stamp_|. This could happen when
  // a client configuration sets a new layer image but the new image is not
  // ready yet, so the controller has to keep using the old image.
  display::ConfigStamp latest_client_config_stamp_ = display::kInvalidConfigStamp;

  // If true, ReleaseImage() will not be called on image destruction.
  bool disposed_ = false;

  inspect::Node node_;
  inspect::ValueList properties_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_IMAGE_H_
