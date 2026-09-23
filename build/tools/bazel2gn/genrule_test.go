// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel2gn_test

import (
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestGenruleConversion(t *testing.T) {
	for _, tc := range []struct {
		name   string
		bazel  string
		wantGN string
	}{
		{
			name: "simple genrule",
			bazel: `genrule(
    name = "gen-json-schema",
    srcs = ["schema.json"],
    outs = ["json_schema.cc"],
    cmd = "$(location :gen-json-schema.sh) $@ $<",
    tools = [":gen-json-schema.sh"],
    visibility = [":__pkg__"],
)
`,
			wantGN: `action("gen-json-schema") {
	sources = [
		"schema.json",
	]
	outputs = [
		"json_schema.cc",
	]
	script = "gen-json-schema.sh"
	args = [
	] + rebase_path(outputs, root_build_dir) + [
	] + rebase_path(sources, root_build_dir) + [
	]
	visibility = [
		":*",
	]
}`,
		},
		{
			name: "genrule cmd with extra flags",
			bazel: `genrule(
    name = "foo",
    srcs = ["in1.txt", "in2.txt"],
    outs = ["out1.txt", "out2.txt"],
    cmd = "$(location //tools:my_tool) --foo --inputs $< --bar --outputs $@ --baz --qux",
    tools = ["//tools:my_tool"],
)
`,
			wantGN: `action("foo") {
	sources = [
		"in1.txt",
		"in2.txt",
	]
	outputs = [
		"out1.txt",
		"out2.txt",
	]
	script = "//tools/my_tool"
	args = [
		"--foo",
		"--inputs",
	] + rebase_path(sources, root_build_dir) + [
		"--bar",
		"--outputs",
	] + rebase_path(outputs, root_build_dir) + [
		"--baz",
		"--qux",
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

func TestGenRuleConversionError(t *testing.T) {
	for _, tc := range []struct {
		name  string
		bazel string
	}{
		{
			name: "genrule_cmd_not_string",
			bazel: `genrule(
    name = "foo",
    cmd = 123,
)`,
		},
		{
			name: "genrule_cmd_no_location",
			bazel: `genrule(
    name = "foo",
    cmd = "echo hello",
)`,
		},
		{
			name: "genrule_cmd_malformed_location",
			bazel: `genrule(
    name = "foo",
    cmd = "$(location //foo:bar",
)`,
		},
		{
			name: "genrule_cmd_empty",
			bazel: `genrule(
    name = "foo",
    cmd = "",
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
