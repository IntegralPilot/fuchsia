// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_COMPONENT_H_
#define SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_COMPONENT_H_

#include <fidl/fuchsia.io/cpp/wire.h>

#include "src/storage/lib/fs_management/cpp/format.h"

namespace fs_management {

class __EXPORT FsComponent {
 public:
  // Returns a component that will launch with a unique name as part of the default fs_management
  // collection.
  static FsComponent FromDiskFormat(DiskFormat df);

  // Returns a component that will launch with a unique name and the given URL.
  static FsComponent FromUrl(std::string_view url, bool is_multi_volume);

  // Returns a static child component that is using the specified well known format.
  static FsComponent StaticChild(std::string_view child_name, DiskFormat df);

  FsComponent(std::string_view url, std::string_view child_name,
              std::optional<std::string_view> collection_name, bool is_multi_volume)
      : url_(url),
        child_name_(child_name),
        collection_name_(collection_name),
        is_multi_volume_(is_multi_volume) {}

  FsComponent(FsComponent&&);
  FsComponent& operator=(FsComponent&&);

  ~FsComponent();

  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> Connect();

  // Destroys the component (if it's a dynamic child). Does nothing otherwise.
  zx::result<> DestroyChild();

  bool is_multi_volume() const { return is_multi_volume_; }
  const std::string& child_name() const { return child_name_; }
  const std::optional<std::string>& collection_name() const { return collection_name_; }

 private:
  std::string url_;
  std::string child_name_;
  std::optional<std::string> collection_name_;
  bool is_multi_volume_;
  bool connected_ = false;
};

// Connect to a filesystem component in our realm with the given |component_child_name|, optionally
// a dynamic component in the collection named |component_collection_name|. |component_child_name|
// is required. If |component_collection_name| is unset, it's assumed that the component is a
// static child.
//
// If it fails to find a component with the INSTANCE_NOT_FOUND error, and the component is a
// dynamic child (i.e. |component_collection_name| is set), then it attempts to launch a new
// instance of the component using the provided |component_url|.
//
// In all successful cases, it returns the exposed directory associated with the launched component
// instance.
zx::result<fidl::ClientEnd<fuchsia_io::Directory>> ConnectFsComponent(
    std::string_view component_url, std::string_view component_child_name,
    std::optional<std::string_view> component_collection_name);

// Destroy a filesystem component in our realm, named |component_child_name| in the collection
// |component_collection_name|. Destruction only works on dynamic components, so the collection
// name is required. If it tries to destroy a component and gets an INSTANCE_NOT_FOUND error, it
// still returns success - the end goal of having no component with this moniker is achieved.
zx::result<> DestroyFsComponent(std::string_view component_child_name,
                                std::string_view component_collection_name);

}  // namespace fs_management

#endif  // SRC_STORAGE_LIB_FS_MANAGEMENT_CPP_COMPONENT_H_
