// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

package agents

import (
	"testing"
)

func TestIsAgentEnvWithLookup(t *testing.T) {
	// Case 1: No variables set.
	lookupNone := func(key string) (string, bool) {
		return "", false
	}
	if isAgentEnvWithLookup(lookupNone) {
		t.Error("expected false when no environment variables are set")
	}

	// Case 2: Each baked variable set individually.
	for _, targetVar := range bakedAgentVars {
		t.Run(targetVar, func(t *testing.T) {
			lookup := func(key string) (string, bool) {
				if key == targetVar {
					return "1", true
				}
				return "", false
			}
			if !isAgentEnvWithLookup(lookup) {
				t.Errorf("expected true when %s is set", targetVar)
			}
		})
	}
}
