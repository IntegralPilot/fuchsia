// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/vfs_types.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <sys/stat.h>
#include <zircon/availability.h>
#include <zircon/errors.h>

#include <cstdint>

#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

// Verify that permission flags align with the Rights enumeration.
static_assert(static_cast<uint64_t>(fio::Rights::kConnect) ==
              static_cast<uint64_t>(fio::Flags::kPermConnect));
static_assert(static_cast<uint64_t>(fio::Rights::kReadBytes) ==
              static_cast<uint64_t>(fio::Flags::kPermReadBytes));
static_assert(static_cast<uint64_t>(fio::Rights::kWriteBytes) ==
              static_cast<uint64_t>(fio::Flags::kPermWriteBytes));
static_assert(static_cast<uint64_t>(fio::Rights::kExecute) ==
              static_cast<uint64_t>(fio::Flags::kPermExecute));
static_assert(static_cast<uint64_t>(fio::Rights::kGetAttributes) ==
              static_cast<uint64_t>(fio::Flags::kPermGetAttributes));
static_assert(static_cast<uint64_t>(fio::Rights::kUpdateAttributes) ==
              static_cast<uint64_t>(fio::Flags::kPermUpdateAttributes));
static_assert(static_cast<uint64_t>(fio::Rights::kEnumerate) ==
              static_cast<uint64_t>(fio::Flags::kPermEnumerate));
static_assert(static_cast<uint64_t>(fio::Rights::kTraverse) ==
              static_cast<uint64_t>(fio::Flags::kPermTraverse));
static_assert(static_cast<uint64_t>(fio::Rights::kModifyDirectory) ==
              static_cast<uint64_t>(fio::Flags::kPermModifyDirectory));

namespace fs {

namespace {

constexpr DeprecatedOptions FlagsToConnectionOptions(fio::OpenFlags flags) {
  DeprecatedOptions options;
  // Filter out io1 OpenFlags.RIGHT_* flags, translated to io2 Rights below.
  options.flags = flags & ~kAllIo1Rights;

  // Using Open1 requires GET_ATTRIBUTES as this is not expressible via |fio::OpenFlags|.
  options.rights = fio::Rights::kGetAttributes;

  // Approximate a set of io2 Rights corresponding to what is expected by |flags|.
  if (!(options.flags & fio::OpenFlags::kNodeReference)) {
    options.rights |= fio::Rights::kTraverse;
    if (flags & fio::OpenFlags::kRightReadable) {
      options.rights |= fio::kRStarDir;
    }
    if (flags & fio::OpenFlags::kRightWritable) {
      options.rights |= fio::kWStarDir;
    }
    if (flags & fio::OpenFlags::kRightExecutable) {
      options.rights |= fio::kXStarDir;
    }
  }

  return options;
}

}  // namespace

zx::result<DeprecatedOptions> DeprecatedOptions::FromOpen1Flags(fio::OpenFlags flags) {
  if ((flags & fio::OpenFlags::kNodeReference) &&
      (flags - fio::kOpenFlagsAllowedWithNodeReference)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if ((flags & fio::OpenFlags::kNotDirectory) && (flags & fio::OpenFlags::kDirectory)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if (flags & fio::OpenFlags::kCloneSameRights) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  if ((flags & fio::OpenFlags::kTruncate) && !(flags & fio::OpenFlags::kRightWritable)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return zx::ok(FlagsToConnectionOptions(flags));
}

zx::result<DeprecatedOptions> DeprecatedOptions::FromCloneFlags(fio::OpenFlags flags,
                                                                VnodeProtocol protocol) {
  constexpr fio::OpenFlags kValidCloneFlags = kAllIo1Rights | fio::OpenFlags::kAppend |
                                              fio::OpenFlags::kDescribe |
                                              fio::OpenFlags::kCloneSameRights;
  // Any flags not present in |kValidCloneFlags| should be ignored.
  flags &= kValidCloneFlags;

  // If CLONE_SAME_RIGHTS is specified, the client cannot request any specific rights.
  if ((flags & fio::OpenFlags::kCloneSameRights) && (flags & kAllIo1Rights)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Ensure we map the request to the correct flags based on the connection's protocol.
  switch (protocol) {
    case fs::VnodeProtocol::kNode: {
      flags |= fio::OpenFlags::kNodeReference;
      break;
    }
    case fs::VnodeProtocol::kDirectory: {
      flags |= fio::OpenFlags::kDirectory;
      break;
    }
    default: {
      flags |= fio::OpenFlags::kNotDirectory;
      break;
    }
  }

  DeprecatedOptions options = FlagsToConnectionOptions(flags);

  // Downscope the rights specified by |flags| to match those that were granted to this node
  // based on |protocol|. io1 OpenFlags expand to a set of rights which may not be compatible
  // with this protocol (e.g. OpenFlags::RIGHT_WRITABLE grants Rights::MODIFY_DIRECTORY, but this is
  // not applicable to files which do not have this right).
  options.rights = internal::DownscopeRights(options.rights, protocol);

  return zx::ok(options);
}

fio::OpenFlags DeprecatedOptions::ToIoV1Flags() const { return flags | RightsToOpenFlags(rights); }

fio::OpenFlags RightsToOpenFlags(fio::Rights rights) {
  fio::OpenFlags flags = {};
  // Map io2 rights to io1 flags only if all constituent io2 rights are present.
  if ((rights & fio::kRStarDir) == fio::kRStarDir) {
    flags |= fio::OpenFlags::kRightReadable;
  }
  if ((rights & fio::kWStarDir) == fio::kWStarDir) {
    flags |= fio::OpenFlags::kRightWritable;
  }
  if ((rights & fio::kXStarDir) == fio::kXStarDir) {
    flags |= fio::OpenFlags::kRightExecutable;
  }
  return flags;
}

fio::wire::NodeAttributes VnodeAttributes::ToIoV1NodeAttributes(const fs::Vnode& vnode) const {
  // Filesystems that don't support hard links typically report a value of 1 for the link count.
  constexpr uint64_t kDefaultLinkCount = 1;
  return fio::wire::NodeAttributes{
      .mode = mode.has_value() ? *mode
                               : internal::GetPosixMode(vnode.GetProtocols(), vnode.GetAbilities()),
      .id = id.value_or(fio::wire::kInoUnknown),
      .content_size = content_size.value_or(0),
      .storage_size = storage_size.value_or(0),
      .link_count = link_count.value_or(kDefaultLinkCount),
      .creation_time = creation_time.value_or(0),
      .modification_time = modification_time.value_or(0)};
}

namespace internal {

fio::Rights DownscopeRights(fio::Rights rights, VnodeProtocol protocol) {
  switch (protocol) {
    case VnodeProtocol::kDirectory: {
      // Directories support all rights.
      return rights;
    }
    case VnodeProtocol::kFile: {
      return rights & (fio::Rights::kReadBytes | fio::Rights::kWriteBytes | fio::Rights::kExecute |
                       fio::Rights::kGetAttributes | fio::Rights::kUpdateAttributes);
    }
    case VnodeProtocol::kNode: {
      // Node connections only support GET_ATTRIBUTES.
      return rights & fio::Rights::kGetAttributes;
    }
    default: {
      // Remove all rights from unknown or unsupported node types.
      return {};
    }
  }
}

namespace {

constexpr fio::NodeProtocolKinds FlagsToProtocols(fio::Flags flags) {
  fio::NodeProtocolKinds protocols;
  if (flags & fio::Flags::kProtocolDirectory) {
    protocols |= fio::NodeProtocolKinds::kDirectory;
  }
  if (flags & fio::Flags::kProtocolFile) {
    protocols |= fio::NodeProtocolKinds::kFile;
  }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (flags & fio::Flags::kProtocolSymlink) {
    protocols |= fio::NodeProtocolKinds::kSymlink;
  }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (flags & fio::Flags::kProtocolService) {
    protocols |= fio::NodeProtocolKinds::kConnector;
  }
  return protocols;
}

}  // namespace

zx::result<VnodeProtocol> NegotiateProtocol(fio::Flags flags, fio::NodeProtocolKinds supported) {
  using fio::NodeProtocolKinds;

  const NodeProtocolKinds requested = FlagsToProtocols(flags);

  if (flags & fio::Flags::kProtocolNode) {
    if (!requested || (requested & supported)) {
      return zx::ok(VnodeProtocol::kNode);
    }
  } else {
    // Remove protocols that were not requested from the set of supported protocols. If no protocols
    // were requested, any protocol is acceptable.
    if (requested) {
      supported = supported & requested;
    }
    // Attempt to negotiate a protocol for the connection based on the following order. The
    // fuchsia.io protocol does not enforce a particular order for resolution, and when callers
    // specify multiple protocols, they must be prepared to accept any that were set in the request.
    if (supported & NodeProtocolKinds::kConnector) {
      return zx::ok(VnodeProtocol::kService);
    }
    if (supported & NodeProtocolKinds::kDirectory) {
      return zx::ok(VnodeProtocol::kDirectory);
    }
    if (supported & NodeProtocolKinds::kFile) {
      return zx::ok(VnodeProtocol::kFile);
    }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    if (supported & NodeProtocolKinds::kSymlink) {
      return zx::ok(VnodeProtocol::kSymlink);
    }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  }
  // If we failed to resolve a protocol, we determine what error to return from a combination of the
  // type of node and the protocols which were requested.
  if ((requested & NodeProtocolKinds::kDirectory) && !(supported & NodeProtocolKinds::kDirectory)) {
    return zx::error(ZX_ERR_NOT_DIR);
  }
  if ((requested & NodeProtocolKinds::kFile) && !(supported & NodeProtocolKinds::kFile)) {
    return zx::error(ZX_ERR_NOT_FILE);
  }
  return zx::error(ZX_ERR_WRONG_TYPE);
}

zx::result<VnodeProtocol> NegotiateProtocol(fio::NodeProtocolKinds supported,
                                            fio::NodeProtocolKinds requested) {
  using fio::NodeProtocolKinds;
  // Remove protocols that were not requested from the set of supported protocols.
  supported = supported & requested;
  // Attempt to negotiate a protocol for the connection based on the following order. The fuchsia.io
  // protocol does not enforce a particular order for resolution, and when callers specify multiple
  // protocols, they must be prepared to accept any that were set in the request.
  if (supported & NodeProtocolKinds::kConnector) {
    return zx::ok(VnodeProtocol::kService);
  }
  if (supported & NodeProtocolKinds::kDirectory) {
    return zx::ok(VnodeProtocol::kDirectory);
  }
  if (supported & NodeProtocolKinds::kFile) {
    return zx::ok(VnodeProtocol::kFile);
  }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (supported & NodeProtocolKinds::kSymlink) {
    return zx::ok(VnodeProtocol::kSymlink);
  }
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  // If we failed to resolve a protocol, we determine what error to return from a combination of the
  // type of node and the protocols which were requested.
  if ((requested & NodeProtocolKinds::kDirectory) && !(supported & NodeProtocolKinds::kDirectory)) {
    return zx::error(ZX_ERR_NOT_DIR);
  }
  if ((requested & NodeProtocolKinds::kFile) && !(supported & NodeProtocolKinds::kFile)) {
    return zx::error(ZX_ERR_NOT_FILE);
  }
  return zx::error(ZX_ERR_WRONG_TYPE);
}

uint32_t GetPosixMode(fio::NodeProtocolKinds protocols, fio::Abilities abilities) {
  uint32_t mode = 0;
  if (protocols & fio::NodeProtocolKinds::kDirectory) {
    mode |= S_IFDIR;
    if (abilities & fio::Abilities::kEnumerate) {
      mode |= S_IRUSR;
    }
    if (abilities & fio::Abilities::kModifyDirectory) {
      mode |= S_IWUSR;
    }
    if (abilities & fio::Abilities::kTraverse) {
      mode |= S_IXUSR;
    }
  } else {
    mode |= S_IFREG;
    if (abilities & fio::Abilities::kReadBytes) {
      mode |= S_IRUSR;
    }
    if (abilities & fio::Abilities::kWriteBytes) {
      mode |= S_IWUSR;
    }
    if (abilities & fio::Abilities::kExecute) {
      mode |= S_IXUSR;
    }
  }
  return mode;
}

}  // namespace internal

}  // namespace fs
