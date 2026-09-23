// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package zither

import (
	"path/filepath"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

// CIncludeNamespace gives the default 'include' namespace for the headers
// of bindings generated for a given FIDL library and C family backend.
func CIncludeNamespace(lib fidlgen.LibraryName, backend string) string {
	str := strings.TrimPrefix(lib.String(), "zither.")
	return filepath.Join(str, backend)
}

// CHeaderPath gives the header path within the 'include' namespace associated
// within a given FIDL source file and C family backend.
func CHeaderPath(summary FileSummary, backend, includeNamespace string) string {
	if includeNamespace == "" {
		includeNamespace = CIncludeNamespace(summary.Library, backend)
	}
	return filepath.Join(includeNamespace, summary.Name()+".h")
}

// CHeaderGuard gives the header guard value for a C family backend.
func CHeaderGuard(summary FileSummary, backend, includeNamespace string) string {
	path := CHeaderPath(summary, backend, includeNamespace)
	for _, c := range []string{".", string(filepath.Separator), "-"} {
		path = strings.ReplaceAll(path, c, "_")
	}
	return fidlgen.ConstNameToAllCapsSnake(path) + "_"
}
