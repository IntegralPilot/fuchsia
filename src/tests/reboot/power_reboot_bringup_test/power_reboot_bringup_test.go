// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"testing"

	"go.fuchsia.dev/fuchsia/src/tests/reboot/reboottest"
)

// Test that "power reboot" will reboot the system.
//
// It's important to also test that bringup reboots cleanly because bringup
// doesn't have storage. Bringup must shutdown cleanly without some components
// that normally live in storage (like PowerManager).
func TestPowerReboot(t *testing.T) {
	reboottest.RebootWithCommand(t, "power reboot", reboottest.CleanReboot, reboottest.Reboot, reboottest.NoCrash)
}
