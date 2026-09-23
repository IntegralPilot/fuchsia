// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"os"
	"path/filepath"
	"testing"
)

func TestParseGoMod(t *testing.T) {
	tempDir := t.TempDir()
	goModPath := filepath.Join(tempDir, "go.mod")

	content := `module go.fuchsia.dev/fuchsia

go 1.21.1

require (
	cloud.google.com/go/storage v1.27.0
	github.com/google/go-cmp v0.5.9
)

require github.com/spf13/pflag v1.0.5
`
	if err := os.WriteFile(goModPath, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	readmes, err := ParseGoMod(goModPath)
	if err != nil {
		t.Fatalf("ParseGoMod failed: %v", err)
	}

	expected := map[string]string{
		"storage": "vendor/cloud.google.com/go/storage",
		"go-cmp":  "vendor/github.com/google/go-cmp",
		"pflag":   "vendor/github.com/spf13/pflag",
	}

	if len(readmes) != len(expected) {
		t.Errorf("Expected %d readmes, got %d", len(expected), len(readmes))
	}

	for _, r := range readmes {
		if loc, ok := expected[r.Name]; ok {
			if r.Location != loc {
				t.Errorf("For %s: expected location %q, got %q", r.Name, loc, r.Location)
			}
		} else {
			t.Errorf("Unexpected module found: %s", r.Name)
		}
	}
}
