// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn_test

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestTagsConversion(t *testing.T) {
	for _, tc := range []struct {
		name   string
		bazel  string
		wantGN string
	}{
		{
			name: "sync assert_no_deps",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    tags = ["assert_no_deps=//third_party/rust_crates/vendor:anyhow"],
)
`,
			wantGN: `rustc_library("lib") {
	sources = [
		"src/lib.rs",
	]
	assert_no_deps = [
		"//third_party/rust_crates:anyhow",
	]
}`,
		},
		{
			name: "sync multiple assert_no_deps and ignore other tags",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    tags = [
        "manual",
        "assert_no_deps=//third_party/rust_crates/vendor:anyhow",
        "noclippy",
        "assert_no_deps=//src/lib/foo:bar",
    ],
)
`,
			wantGN: `rustc_library("lib") {
	sources = [
		"src/lib.rs",
	]
	assert_no_deps = [
		"//third_party/rust_crates:anyhow",
		"//src/lib/foo:bar",
	]
}`,
		},
		{
			name: "ignore tags if no assert_no_deps",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    tags = [
        "manual",
        "noclippy",
    ],
)
`,
			wantGN: `rustc_library("lib") {
	sources = [
		"src/lib.rs",
	]
}`,
		},
		{
			name: "sync assert_no_deps with __pkg__ and __subpackages__",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    tags = [
        "assert_no_deps=//src/lib/foo:__pkg__",
        "assert_no_deps=//src/lib/bar:__subpackages__",
    ],
)
`,
			wantGN: `rustc_library("lib") {
	sources = [
		"src/lib.rs",
	]
	assert_no_deps = [
		"//src/lib/foo:*",
		"//src/lib/bar/*",
	]
}`,
		},
		{
			name: "sync assert_no_deps with local __pkg__ and __subpackages__",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    tags = [
        "assert_no_deps=:__pkg__",
        "assert_no_deps=:__subpackages__",
    ],
)
`,
			wantGN: `rustc_library("lib") {
	sources = [
		"src/lib.rs",
	]
	assert_no_deps = [
		":*",
		"./*",
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

func TestTagsConversionErrors(t *testing.T) {
	for _, tc := range []struct {
		name  string
		bazel string
	}{
		{
			name: "tags not list literal",
			bazel: `rustc_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    tags = select({
        "//conditions:default": ["assert_no_deps=//third_party/rust_crates/vendor:anyhow"],
    }),
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
