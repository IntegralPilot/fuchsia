// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"go.fuchsia.dev/fuchsia/tools/check-licenses/pipeline"
	"go.fuchsia.dev/fuchsia/tools/readme_fuchsia"
)

type Readme = readme_fuchsia.Readme
type UnknownField = readme_fuchsia.UnknownField

type ReadmeSegment = pipeline.ReadmeSegment
type ReadmeFile = pipeline.ReadmeFile

// Config defines the configuration methods needed by the readme package.
type Config interface {
	IsSkipped(absPath string) bool
	OutOfTreeReadmes() map[string]string
	HasPolicyException(policyName string, relPath string) bool
}
