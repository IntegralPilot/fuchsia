// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/lib/virtio/virtio-abi.h"

namespace virtio_abi {

const char* ControlTypeToString(ControlType type) {
  switch (type) {
    case ControlType::kGetDisplayInfoCommand:
      return "Command: GetDisplayInfo";

    case ControlType::kCreate2DResourceCommand:
      return "Command: Create2DResource";

    case ControlType::kDestroyResourceCommand:
      return "Command: DestroyResource";

    case ControlType::kSetScanoutCommand:
      return "Command: SetScanout";

    case ControlType::kFlushResourceCommand:
      return "Command: FlushResource";

    case ControlType::kTransfer2DResourceToHostCommand:
      return "Command: Tranfer2DResourceToHost";

    case ControlType::kAttachResourceBackingCommand:
      return "Command: AttachResourceBacking";

    case ControlType::kDetachResourceBackingCommand:
      return "Command: DetachResourceBacking";

    case ControlType::kGetCapabilitySetInfoCommand:
      return "Command: GetCapabilitySetInfo";

    case ControlType::kGetCapabilitySetCommand:
      return "Command: GetCapabilitySet";

    case ControlType::kGetExtendedDisplayIdCommand:
      return "Command: GetEDID";

    case ControlType::kAssignResourceUuidCommand:
      return "Command: AssignResourceUUID";

    case ControlType::kCreateBlobCommand:
      return "Command: CreateBlob";

    case ControlType::kSetScanoutBlobCommand:
      return "Command: SetScanoutBlob";

    case ControlType::kEmptyResponse:
      return "Response: Empty";

    case ControlType::kDisplayInfoResponse:
      return "Response: DisplayInfo";

    case ControlType::kCapabilitySetInfoResponse:
      return "Response: CapabilitySetInfo";

    case ControlType::kCapabilitySetResponse:
      return "Response: CapabilitySet";

    case ControlType::kExtendedDisplayIdResponse:
      return "Response: EDID";

    case ControlType::kResourceUuidResponse:
      return "Response: ResourceUUID";

    case ControlType::kMapInfoResponse:
      return "Response: MapInfo";

    case ControlType::kUnspecifiedError:
      return "Error: Unspecified";

    case ControlType::kOutOfMemoryError:
      return "Error: OutOfMemory";

    case ControlType::kInvalidScanoutIdError:
      return "Error: InvalidScanoutID";

    case ControlType::kInvalidResourceIdError:
      return "Error: InvalidResourceID";

    case ControlType::kInvalidContextIdError:
      return "Error: InvalidContextID";

    case ControlType::kInvalidParameterError:
      return "Error: InvalidParameter";

    case ControlType::kUpdateCursorCommand:
      return "Command: UpdateCursor";

    case ControlType::kMoveCursorCommand:
      return "Command: SetCursorPosition";
  }

  return "(unknown)";
}

}  // namespace virtio_abi
