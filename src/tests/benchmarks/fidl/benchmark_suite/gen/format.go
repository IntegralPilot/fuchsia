// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"strings"

	"go.fuchsia.dev/fuchsia/src/tests/benchmarks/fidl/benchmark_suite/gen/config"
)

func formatComment(comment string) string {
	if comment == "" {
		return ""
	}
	parts := strings.Split(strings.TrimSpace(comment), "\n")
	var builder strings.Builder
	for _, part := range parts {
		builder.WriteString("// " + strings.TrimSpace(part) + "\n")
	}
	return builder.String()
}

func formatBindingList(bindings []config.Binding) string {
	if len(bindings) == 0 {
		return ""
	}
	strs := make([]string, len(bindings))
	for i, binding := range bindings {
		strs[i] = string(binding)
	}
	return "[" + strings.Join(strs, ", ") + "]"
}
