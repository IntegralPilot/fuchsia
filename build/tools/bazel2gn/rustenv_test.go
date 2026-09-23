// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn_test

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestRustenvConversion(t *testing.T) {
	for _, tc := range []struct {
		name   string
		bazel  string
		wantGN string
	}{
		{
			name: "simple rustc_env",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    rustc_env = {
        "FOO": "bar",
        "BAZ": "qux",
    },
)
`,
			wantGN: `rustc_library("lib") {
	sources = [
		"src/lib.rs",
	]
	rustenv = [
		"FOO=bar",
		"BAZ=qux",
	]
}`,
		},
		{
			name: "rustc_env with value overwrite",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    rustc_env = {
        "FOO": "bar", # @bazel2gn:value_overwrite:overwritten_bar
        "BAZ": "qux",
    },
)
`,
			wantGN: `rustc_library("lib") {
	sources = [
		"src/lib.rs",
	]
	rustenv = [
		"FOO=overwritten_bar",
		"BAZ=qux",
	]
}`,
		},
		{
			name: "empty rustc_env",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    rustc_env = {},
)
`,
			wantGN: `rustc_library("lib") {
	sources = [
		"src/lib.rs",
	]
}`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			gotGN, err := bazelToGN(f)
			if err != nil {
				t.Fatalf("Unexpected failure converting Bazel build targets: %v", err)
			}
			if diff := cmp.Diff(gotGN, tc.wantGN); diff != "" {
				t.Errorf("Diff found after GN conversion (-got +want):\n%s\nBazel source:\n%s", diff, tc.bazel)
			}
		})
	}
}

func TestRustenvConversionErrors(t *testing.T) {
	for _, tc := range []struct {
		name  string
		bazel string
	}{
		{
			name: "rustc_env not dict literal",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    rustc_env = ["FOO=bar"],
)`,
		},
		{
			name: "rustc_env key not literal",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    rustc_env = {
        foo: "bar",
    },
)`,
		},
		{
			name: "rustc_env value not literal",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    rustc_env = {
        "FOO": select({
            "//conditions:default": "bar",
        }),
    },
)`,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := toSyntaxFile(t, tc.bazel)
			_, err := bazelToGN(f)
			if err == nil {
				t.Errorf("Unexpected success converting Bazel targets. Bazel source:\n%s", tc.bazel)
			}
		})
	}
}
