// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package codegen

import (
	"strings"
	"testing"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgentest"
)

func TestDerivesToString(t *testing.T) {
	cases := []struct {
		input    derives
		expected string
	}{
		{0, ""},
		{derivesDebug, "#[derive(Debug)]"},
		{derivesPartialOrd, "#[derive(PartialOrd)]"},
	}
	for _, ex := range cases {
		actual := ex.input.String()
		if actual != ex.expected {
			t.Errorf("%d: expected '%s', actual '%s'", ex.input, ex.expected, actual)
		}
	}
}

func TestDerivesCalculation(t *testing.T) {
	cases := []struct {
		fidl     string
		expected string
	}{
		{
			fidl:     `type MyStruct = struct { field string; };`,
			expected: "#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]",
		},
		{
			fidl:     `type MyStruct = struct { field float32; };`,
			expected: "#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]",
		},
		{
			fidl:     `type MyStruct = resource struct { field client_end:P; }; closed protocol P {};`,
			expected: "#[derive(Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]",
		},
		{
			fidl:     `type MyStruct = resource struct { field uint64; };`,
			expected: "#[derive(Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]",
		},
		{
			fidl:     `type MyStruct = resource struct {};`,
			expected: "#[derive(Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]",
		},
	}
	for _, ex := range cases {
		root := Compile(fidlgentest.EndToEndTest{T: t}.Single(`library example; `+ex.fidl), false, false, false)
		actual := root.Structs[0].Derives.String()
		if ex.expected != actual {
			t.Errorf("%s: expected %s, found %s", ex.fidl, ex.expected, actual)
		}
	}
}

func TestApiCoverageAnnotations(t *testing.T) {
	fidl := `
library example;

open protocol Echo {
	strict EchoString(struct { value string; }) -> (struct { response string; });
};
`
	ir := fidlgentest.EndToEndTest{T: t}.Single(fidl)
	tree := Compile(ir, false, false, false)

	tests := []struct {
		name        string
		apiCoverage bool
	}{
		{name: "disabled", apiCoverage: false},
		{name: "enabled", apiCoverage: true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			gen := NewGenerator("", "", false, false, "", tt.apiCoverage)
			bytes, err := gen.ExecuteTemplate("GenerateSourceFile", tree)
			if err != nil {
				t.Fatalf("ExecuteTemplate failed: %v", err)
			}
			output := string(bytes)
			expectedStart := "// api-coverage-fidl-name:example/Echo.EchoString"
			expectedEnd := "// api-coverage-fidl-name:example/Echo.EchoString:end"
			if tt.apiCoverage {
				if !strings.Contains(output, expectedStart) {
					t.Errorf("expected output to contain %q, but got:\n%s", expectedStart, output)
				}
				if !strings.Contains(output, expectedEnd) {
					t.Errorf("expected output to contain %q, but got:\n%s", expectedEnd, output)
				}
			} else {
				if strings.Contains(output, "// api-coverage-fidl-name:") {
					t.Errorf("expected output NOT to contain api-coverage comments when apiCoverage=false, got:\n%s", output)
				}
			}
		})
	}
}
