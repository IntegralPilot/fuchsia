// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package zither

import (
	"fmt"
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

func WriteReadme(path string, summary LibrarySummary) error {
	content := []string{fmt.Sprintf("# %s", summary.Library.String()), ""}
	for _, line := range summary.Documentation {
		// Annoyingly, FIDL comments encode the space separating the comment
		// delimiter ("///") from the actual content.
		content = append(content, strings.TrimPrefix(line, " "))
	}
	// A blank line at the end of the file for good measure.
	content = append(content, "")
	return fidlgen.WriteFileIfChanged(path, []byte(strings.Join(content, "\n")))
}
