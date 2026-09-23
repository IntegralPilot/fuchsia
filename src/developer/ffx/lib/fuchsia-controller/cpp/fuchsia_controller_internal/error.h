// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_ERROR_H_
#define SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_ERROR_H_

#include <Python.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <string>

namespace error {

// TODO(https://fxbug.dev/42077810): This has been copied from zircon code, as vdso doesn't build
// this for host.
extern std::string zx_status_get_string(zx_status_t status);

// The (singleton) type of the ZxStatus error type. Must be initialized using ZxStatusType_Create().
extern PyTypeObject* ZxStatusType;

// Creates the ZxStatus error type. Must only be run once.
extern PyTypeObject* ZxStatusType_Create();

// The (singleton) type of the FcTransportStatus error type. Must be initialized using
// FcTransportStatusType_Create().
extern PyTypeObject* FcTransportStatusType;

// Creates the FcTransportStatus error type. Must only be run once.
extern PyTypeObject* FcTransportStatusType_Create();

}  // namespace error

#endif  // SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FUCHSIA_CONTROLLER_INTERNAL_ERROR_H_
