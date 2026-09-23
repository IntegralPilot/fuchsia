// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package testsharder

import (
	"encoding/json"
	"errors"
	"fmt"
	"slices"

	"go.fuchsia.dev/fuchsia/tools/build"
)

// ValidateTests validates a list of test specs against a list of available test platforms.
func ValidateTests(specs []build.TestSpec, supportedPlatforms []build.DimensionSet) error {
	var errs []error
	for _, spec := range specs {
		errs = append(errs, validateTest(spec, supportedPlatforms))
	}
	return errors.Join(errs...)
}

func validateTest(spec build.TestSpec, supportedPlatforms []build.DimensionSet) error {
	if spec.Test.Name == "" {
		return fmt.Errorf("test spec has empty name: %+v", spec)
	}
	if spec.Test.Path == "" && spec.PackageURL == "" {
		return fmt.Errorf("test %q has no path or package URL set", spec.Test.Name)
	}
	if spec.Test.OS == "" {
		return fmt.Errorf("test %q has no OS set", spec.Test.Name)
	}

	var badEnvs []build.Environment
	for _, env := range spec.Envs {
		// A test's environment is valid if its dimensions are a subset of the
		// dimensions of one of the supported platforms.
		if !slices.ContainsFunc(supportedPlatforms, env.Dimensions.IsSubset) {
			badEnvs = append(badEnvs, env)
		}
	}
	if len(badEnvs) > 0 {
		var envsStr string
		if b, err := json.Marshal(badEnvs); err == nil {
			envsStr = string(b)
		} else {
			// json.Marshal should never fail, but add a fallback just in case.
			envsStr = fmt.Sprintf("%+v", badEnvs)
		}
		return fmt.Errorf(
			"the following environments of test %q were malformed or did not match any available test platforms: %s",
			spec.Test.Name, envsStr)
	}
	return nil
}
