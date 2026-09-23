// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ffxutil

import (
	"context"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

// Flash flashes the target.
func (f *FFXInstance) Flash(ctx context.Context, target, sshKey, productBundle string, tcp bool) error {
	configs := map[string]any{
		"discovery.mdns.enabled":          false,
		"fastboot.usb.disabled":           true,
		"discovery.timeout":               12000,
		"fastboot.flash.timeout_rate":     "1",
		"fastboot.flash.min_timeout_secs": "600",
	}

	// Set the machine format to json so that errors will get printed to stdout
	// for parsing.
	args := []string{"-v", "--machine", "json", "target", "flash", "--product-bundle", productBundle}
	if tcp {
		// Rebooting while flashing over TCP will error out.
		args = append(args, "--no-bootloader-reboot")
	}
	if sshKey != "" {
		args = append(args, "--authorized-keys", sshKey)
	}
	i := f.invoker(args).setConfigs(configs).setTarget(target).setCaptureOutput().setTimeout(0)
	err := i.run(ctx)
	s := strings.TrimSpace(i.output.String())
	// The reboot may return an error even it happened. Make note of it, but
	// continue trying to run tests.
	// LINT.IfChange
	if strings.Contains(s, "Could not reboot") {
		logger.Errorf(ctx, "Reboot returned an error. Continuing to run tests anyway.")
		return nil
	}
	// LINT.ThenChange(//src/developer/ffx/lib/fastboot/src/common/mod.rs)
	return err
}
