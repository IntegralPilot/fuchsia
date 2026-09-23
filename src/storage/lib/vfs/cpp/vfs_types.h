// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_VFS_TYPES_H_
#define SRC_STORAGE_LIB_VFS_CPP_VFS_TYPES_H_

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/zx/result.h>
#include <zircon/availability.h>
#include <zircon/compiler.h>

#include <cstdint>
#include <optional>
#include <type_traits>

// The filesystem server exposes various FIDL protocols on top of the Vnode abstractions. This
// header defines some helper types composed with the fuchsia.io protocol types to better express
// API requirements. These type names should start with "Vnode" to reduce confusion with their FIDL
// counterparts.
namespace fs {

class Vnode;

// All io1 OpenFlags that correspond to connection rights.
constexpr fuchsia_io::OpenFlags kAllIo1Rights = fuchsia_io::OpenFlags::kRightReadable |
                                                fuchsia_io::OpenFlags::kRightWritable |
                                                fuchsia_io::OpenFlags::kRightExecutable;

// All io2 Rights that allow a connection to modify the filesystem.
constexpr fuchsia_io::Rights kAllMutableIo2Rights = fuchsia_io::Rights::kWriteBytes |
                                                    fuchsia_io::Rights::kModifyDirectory |
                                                    fuchsia_io::Rights::kUpdateAttributes;

// Specifies the type of object when creating new nodes.
enum class CreationType : uint8_t {
  kFile = 0,
  kDirectory = 1,
  // Max value used for fuzzing.
  kMaxValue = kDirectory,
};

// Identifies a single type of node protocol where required for protocol negotiation/resolution.
enum class VnodeProtocol : uint8_t {
  kNode = 0,  // All Vnodes support fuchsia.io/Node, so it does not have an explicit representation.
  kService = uint64_t{fuchsia_io::NodeProtocolKinds::kConnector},
  kDirectory = uint64_t{fuchsia_io::NodeProtocolKinds::kDirectory},
  kFile = uint64_t{fuchsia_io::NodeProtocolKinds::kFile},
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  kSymlink = uint64_t{fuchsia_io::NodeProtocolKinds::kSymlink},
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
};

// Options specified during opening and cloning.
struct DeprecatedOptions {
  fuchsia_io::OpenFlags flags;
  fuchsia_io::Rights rights;

  // Translates the io1 flags passed by the client into an equivalent set of io2 protocols.
  constexpr fuchsia_io::NodeProtocolKinds protocols() const {
    constexpr fuchsia_io::NodeProtocolKinds kSupportedIo1Protocols =
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
        // Symlinks are not supported via io1.
        fuchsia_io::NodeProtocolKinds::kMask ^ fuchsia_io::NodeProtocolKinds::kSymlink;
#else
        fuchsia_io::NodeProtocolKinds::kMask;
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    if (flags & fuchsia_io::OpenFlags::kDirectory) {
      return fuchsia_io::NodeProtocolKinds::kDirectory;
    }
    if (flags & fuchsia_io::OpenFlags::kNotDirectory) {
      return kSupportedIo1Protocols ^ fuchsia_io::NodeProtocolKinds::kDirectory;
    }
    return kSupportedIo1Protocols;
  }

  // Converts from fuchsia.io/Directory.Open1 flags to |DeprecatedOptions|. Note that in io1,
  // certain operations were unprivileged so they may be implicitly added to the resulting `rights`.
  static zx::result<DeprecatedOptions> FromOpen1Flags(fuchsia_io::OpenFlags flags);

  // Converts from fuchsia.io/Directory.Clone flags to |DeprecatedOptions|.
  static zx::result<DeprecatedOptions> FromCloneFlags(fuchsia_io::OpenFlags flags,
                                                      VnodeProtocol protocol);

  // Converts from |DeprecatedOptions| to fuchsia.io flags.
  fuchsia_io::OpenFlags ToIoV1Flags() const;
};

fuchsia_io::OpenFlags RightsToOpenFlags(fuchsia_io::Rights rights);

using VnodeAttributesQuery = fuchsia_io::NodeAttributesQuery;

// Objective information about a filesystem node, used to implement |Vnode::GetAttributes|.
// Filesystems should only report those attributes which it has support for.
//
// Note that only attributes for which existing filesystems support are currently implemented.
// Additional attributes can be supported by adding them to this struct, and updating the
// |NodeAttributeBuilder::Build()| function accordingly.
struct VnodeAttributes {
  std::optional<uint64_t> id;
  std::optional<uint64_t> content_size;
  std::optional<uint64_t> storage_size;
  std::optional<uint64_t> link_count;

  std::optional<uint64_t> creation_time;
  std::optional<uint64_t> modification_time;
  std::optional<uint64_t> access_time;
  std::optional<uint64_t> change_time;

  // POSIX Compatibility Attributes
  std::optional<uint32_t> mode;
  std::optional<uint32_t> uid;
  std::optional<uint32_t> gid;
  std::optional<uint64_t> rdev;

  // Compare two |VnodeAttributes| instances for equality.
  bool operator==(const VnodeAttributes& other) const {
    return id == other.id && content_size == other.content_size &&
           storage_size == other.storage_size && link_count == other.link_count &&
           creation_time == other.creation_time && modification_time == other.modification_time &&
           access_time == other.access_time && change_time == other.change_time &&
           mode == other.mode && uid == other.uid && gid == other.gid && rdev == other.rdev;
  }

  // Converts from |VnodeAttributes| to fuchsia.io v1 |NodeAttributes|.
  fuchsia_io::wire::NodeAttributes ToIoV1NodeAttributes(const fs::Vnode& vnode) const;
};

// A request to update pieces of the |VnodeAttributes| via |Vnode::SetAttributes|. Filesystems may
// only support a sub-set of all possible attributes.
//
// Note that only attributes for which existing filesystems support are currently implemented.
// Additional attributes can be supported by adding them to this struct, and updating the
// |FromIo1()| and |FromIo2()| functions accordingly.
struct VnodeAttributesUpdate {
  std::optional<uint64_t> creation_time;
  std::optional<uint64_t> modification_time;
  std::optional<uint64_t> access_time;

  // POSIX Compatibility Attributes
  std::optional<uint32_t> mode;
  std::optional<uint32_t> uid;
  std::optional<uint32_t> gid;
  std::optional<uint64_t> rdev;

  // Return a set of flags representing those attributes which we want to update.
  constexpr VnodeAttributesQuery Query() const {
    VnodeAttributesQuery query;
    if (creation_time) {
      query |= VnodeAttributesQuery::kCreationTime;
    }
    if (modification_time) {
      query |= VnodeAttributesQuery::kModificationTime;
    }
    if (mode) {
      query |= VnodeAttributesQuery::kMode;
    }
    if (uid) {
      query |= VnodeAttributesQuery::kUid;
    }
    if (gid) {
      query |= VnodeAttributesQuery::kGid;
    }
    if (rdev) {
      query |= VnodeAttributesQuery::kRdev;
    }
    if (access_time) {
      query |= VnodeAttributesQuery::kAccessTime;
    }
    return query;
  }

  static constexpr VnodeAttributesUpdate FromIo1(const fuchsia_io::wire::NodeAttributes& attrs,
                                                 fuchsia_io::NodeAttributeFlags flags) {
    VnodeAttributesUpdate attr_update;
    if (flags & fuchsia_io::NodeAttributeFlags::kCreationTime) {
      attr_update.creation_time = attrs.creation_time;
    }
    if (flags & fuchsia_io::NodeAttributeFlags::kModificationTime) {
      attr_update.modification_time = attrs.modification_time;
    }
    return attr_update;
  }

  static constexpr VnodeAttributesUpdate FromIo2(
      const fuchsia_io::wire::MutableNodeAttributes& attrs) {
    VnodeAttributesUpdate attr_update;
    if (attrs.has_creation_time()) {
      attr_update.creation_time = attrs.creation_time();
    }
    if (attrs.has_modification_time()) {
      attr_update.modification_time = attrs.modification_time();
    }
    if (attrs.has_mode()) {
      attr_update.mode = attrs.mode();
    }
    if (attrs.has_uid()) {
      attr_update.uid = attrs.uid();
    }
    if (attrs.has_gid()) {
      attr_update.gid = attrs.gid();
    }
    if (attrs.has_rdev()) {
      attr_update.rdev = attrs.rdev();
    }
    if (attrs.has_access_time()) {
      attr_update.access_time = attrs.access_time();
    }
    return attr_update;
  }
};

// Indicates if and when a new object should be created when opening a node.
enum class CreationMode : uint8_t {
  // Never create an object. Will return `ZX_ERR_NOT_FOUND` if there is no existing object.
  kNever,
  // Create a new object if one doesn't already exist, otherwise open the existing object.
  kAllowExisting,
  // Always create an object. Will return `ZX_ERR_ALREADY_EXISTS` if one already exists.
  kAlways,
};

/// The format of directory entries returned by fuchsia.io/Directory.ReadDirents.
struct DirectoryEntry {
  /// The entry's inode number.
  uint64_t ino;
  /// Length of the entry's `name` field.
  uint8_t name_length;
  /// The entry type for the entry (e.g. DT_REG).
  fuchsia_io::DirentType type;
  /// Null-terminated name of the entry of size `name_length`.
  char name[0];
} __PACKED;

static_assert(sizeof(DirectoryEntry) == 10);
static_assert(std::is_standard_layout_v<DirectoryEntry>);
static_assert(std::is_trivially_copyable_v<DirectoryEntry>);
static_assert(std::has_unique_object_representations_v<DirectoryEntry>);

namespace internal {

// Downscope |rights| to only include those operations which |protocol| supports, or those which are
// applicable to child nodes. This follows the principle of least privilege.
fuchsia_io::Rights DownscopeRights(fuchsia_io::Rights rights, VnodeProtocol protocol);

// Determines the protocol to use for serving a connection, based on the |supported| protocols for
// a node, and those which were |requested|.
//
// Note that this function is not part of the |Vnode| interface. The fuchsia.io protocol does not
// define a specific order of protocol resolution when |requested| is ambiguous, but we define a
// strict mapping here to enforce consistency across the Fuchsia VFS libraries.
zx::result<VnodeProtocol> NegotiateProtocol(fuchsia_io::NodeProtocolKinds supported,
                                            fuchsia_io::NodeProtocolKinds requested);

// Determines the protocol to use for serving a connection, based on the |supported| protocols for
// a node, and those which were requested in |flags|.
zx::result<VnodeProtocol> NegotiateProtocol(fuchsia_io::Flags flags,
                                            fuchsia_io::NodeProtocolKinds supported);

// Synthesizes a set of POSIX mode bits using a node's supported protocols and abilities.
// This implementation mirrors that of |get_posix_mode| in fdio.
//
// Unlike the ZXIO implementation, this function is *only* used for synthesizing the mode bits
// reported by the io1 GetAttrs method. Callers should use the io2 GetAttributes method to get
// an accurate representation of the mode bits.
uint32_t GetPosixMode(fuchsia_io::NodeProtocolKinds protocols, fuchsia_io::Abilities abilities);

constexpr fuchsia_io::NodeProtocolKinds GetProtocols(fuchsia_io::Flags flags) {
  using fuchsia_io::Flags;
  using fuchsia_io::NodeProtocolKinds;
  // If the caller didn't specify a protocol, allow any.
  if (!(flags & fuchsia_io::kMaskKnownProtocols)) {
    return NodeProtocolKinds::kMask;
  }
  if (flags & Flags::kProtocolService) {
    return NodeProtocolKinds::kConnector;
  }
  NodeProtocolKinds protocols;
  if (flags & Flags::kProtocolDirectory) {
    protocols |= NodeProtocolKinds::kDirectory;
  }
  if (flags & Flags::kProtocolFile) {
    protocols |= NodeProtocolKinds::kFile;
  }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (flags & Flags::kProtocolSymlink) {
    protocols |= NodeProtocolKinds::kSymlink;
  }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  return protocols;
}

constexpr fuchsia_io::Rights FlagsToRights(fuchsia_io::Flags flags) {
  using fuchsia_io::Flags;
  using fuchsia_io::Rights;
  return static_cast<Rights>(static_cast<uint64_t>(flags) & static_cast<uint64_t>(Rights::kMask));
}

constexpr fuchsia_io::Flags RightsToFlags(fuchsia_io::Rights rights) {
  using fuchsia_io::Flags;
  using fuchsia_io::Rights;
  return static_cast<Flags>(static_cast<uint64_t>(rights));
}

constexpr CreationMode CreationModeFromFidl(fuchsia_io::OpenFlags flags) {
  if (flags & fuchsia_io::OpenFlags::kCreateIfAbsent) {
    return CreationMode::kAlways;
  }
  if (flags & fuchsia_io::OpenFlags::kCreate) {
    return CreationMode::kAllowExisting;
  }
  return CreationMode::kNever;
}

constexpr CreationMode CreationModeFromFidl(fuchsia_io::Flags flags) {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  // We traverse the path, then lookup and create the last segment. This is used to determine if the
  // last segment is to be created. When creating an unnamed temporary file, it is created in the
  // last segment. And so it must already exist, we pretend that the creation mode for the last
  // segment is '"never".
  if ((flags & fuchsia_io::Flags::kFlagCreateAsUnnamedTemporary) ||
      (flags & fuchsia_io::Flags::kFlagCreateAsUnnamedTemporary)) {
    return CreationMode::kNever;
  }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (flags & fuchsia_io::Flags::kFlagMustCreate) {
    return CreationMode::kAlways;
  }
  if (flags & fuchsia_io::Flags::kFlagMaybeCreate) {
    return CreationMode::kAllowExisting;
  }
  return CreationMode::kNever;
}

}  // namespace internal

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_VFS_TYPES_H_
