// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"fmt"

	"go.fuchsia.dev/fuchsia/tools/build"
)

// AddFFXDeps selects and adds the files needed by ffx to provision a device
// or launch an emulator to the shard's list of dependencies.
func AddFFXDeps(s *Shard, buildDir string, tools build.Tools) error {
	if len(s.Tests) == 0 {
		return fmt.Errorf("shard %s has no tests", s.Name)
	}
	subtools := []string{"package", "product", "test", "log", "assembly", "monitor", "usb-driver", "forward"}
	if s.Env.TargetsEmulator() {
		subtools = append(subtools, "emu")
	}
	platform, err := testBotPlatform(s.HostCPU)
	if err != nil {
		return err
	}
	s.AddDeps(getSubtoolDeps(s, tools, platform, subtools))
	ffxTool, err := tools.LookupTool(platform, "ffx")
	if err != nil {
		return err
	}
	s.AddDeps([]string{ffxTool.Path})
	return nil
}

func getSubtoolDeps(s *Shard, tools build.Tools, platform string, subtools []string) []string {
	var deps []string
	for _, s := range subtools {
		if subtool, err := tools.LookupTool(platform, fmt.Sprintf("ffx-%s", s)); err == nil {
			deps = append(deps, subtool.Path)
			deps = append(deps, subtool.RuntimeFiles...)
		}
	}
	return deps
}
