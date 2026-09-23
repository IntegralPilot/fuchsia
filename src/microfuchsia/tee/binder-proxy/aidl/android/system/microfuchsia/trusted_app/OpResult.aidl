/*
 * Copyright 2024 The Fuchsia Authors
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */
package android.system.microfuchsia.trusted_app;

import android.system.microfuchsia.trusted_app.ParameterSet;
import android.system.microfuchsia.trusted_app.ReturnOrigin;

parcelable OpResult {
  long returnCode;
  ReturnOrigin returnOrigin;
  // |params| will contain entries corresponding to all input parameters with directions
  // OUTPUT or INOUT. |params| will contain an empty entry for input parameters that
  // are empty or that have direction INPUT.
  ParameterSet params;
}
