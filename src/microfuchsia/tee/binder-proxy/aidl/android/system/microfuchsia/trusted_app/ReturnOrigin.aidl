/*
 * Copyright 2024 The Fuchsia Authors
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */
package android.system.microfuchsia.trusted_app;

enum ReturnOrigin {
  COMMUNICATION = 0,
  TRUSTED_OS = 1,
  TRUSTED_APPLICATION = 2,
}
