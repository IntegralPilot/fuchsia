// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel_docgen

import (
	"bytes"
	"testing"

	pb "go.fuchsia.dev/fuchsia/tools/bazel-docgen/third_party/stardoc"
)

const EXPECTED_RULE_OUTPUT_WITH_ATTRIBUTES string = `
[TOC]

# foo

docs for foo

## **ATTRIBUTES**

| Name  | Description | Type | Mandatory | Default |
| :------------- | :------------- | :------------- | :------------- | :------------- |
| first | - | UNKNOWN | optional | - |
| name | A unique name for this target. | <a href="https://bazel.build/concepts/labels#target-names">Name</a> | required | - |

`

func TestRenderRuleInfoWithAttributes(t *testing.T) {
	out := bytes.NewBufferString("")

	rule := pb.RuleInfo{
		RuleName:  "foo",
		DocString: "docs for foo",
		Attribute: []*pb.AttributeInfo{
			{Name: "first"},
			{
				Name: "name", DocString: "A unique name for this target.",
				Type: pb.AttributeType_NAME, Mandatory: true},
		},
		Test:       false,
		Executable: false,
	}

	var renderer MarkdownRenderer
	renderer.RenderRuleInfo(&rule, out)

	if out.String() != EXPECTED_RULE_OUTPUT_WITH_ATTRIBUTES {
		t.Fatalf("Output does not match: '%s' != '%s'", out.String(), EXPECTED_RULE_OUTPUT_WITH_ATTRIBUTES)
	}
}
