/*
 * Copyright 2024 The Fuchsia Authors
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */
package android.system.microfuchsia.trusted_app;

import android.system.microfuchsia.trusted_app.OpResult;
import android.system.microfuchsia.trusted_app.ParameterSet;

interface ITrustedAppSession {
  OpResult invokeCommand(int commandID, in ParameterSet params);
}