// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"strings"
	"testing"
)

func TestFormat(t *testing.T) {
	readmeText := `Name: awesome_lib
URL: https://github.com/awesome/lib
Version: 1.2.3
Upstream Git: https://github.com/awesome/lib.git
Security Critical: yes

License: Apache-2.0, MIT
License File: LICENSE
License File: third_party/NOTICE

Some Unfamiliar Key: Some weird value
Another Unfamiliar Key: Wow

Description:
  An awesome library for doing awesome things.

  It spans lines!

Local Modifications:
  None.
`

	readmes, err := Parse([]byte(readmeText))
	if err != nil {
		t.Fatalf("Failed to parse: %v", err)
	}

	formatted := Format(readmes)

	// Since formatting might normalize whitespace and line breaks slightly differently,
	// we just trim leading/trailing newlines for comparison.
	formatted = strings.TrimSpace(formatted)
	expected := strings.TrimSpace(readmeText)

	if formatted != expected {
		t.Errorf("Formatted output does not match expected.\nEXPECTED:\n---\n%s\n---\nGOT:\n---\n%s\n---", expected, formatted)
	}
}

func TestFormat_DependencyDivider(t *testing.T) {
	readmeText := `Name: Parent Project
URL: http://parent

License File: LICENSE

-------------------- DEPENDENCY DIVIDER --------------------

Name: Vendored Sub Project
URL: http://subproject
Location: third_party/sub

License: MIT
License File: third_party/sub/LICENSE
`
	readmes, err := Parse([]byte(readmeText))
	if err != nil {
		t.Fatalf("Failed to parse: %v", err)
	}

	formatted := Format(readmes)
	formatted = strings.TrimSpace(formatted)
	expected := strings.TrimSpace(readmeText)

	if formatted != expected {
		t.Errorf("Formatted output does not match expected.\nEXPECTED:\n---\n%s\n---\nGOT:\n---\n%s\n---", expected, formatted)
	}
}
