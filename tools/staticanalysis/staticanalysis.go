// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package staticanalysis

// Finding is a common schema that static analysis tools can emit that's easy
// for shac checks to parse.
type Finding struct {
	// Category is the text that will be used as the header of the Gerrit
	// comment emitted for this finding.
	//
	// TODO(olivernewman): Pass this through to shac somehow.
	Category string `json:"category"`

	// Message is a human-readable description of the finding, e.g. "variable
	// foo is not defined".
	Message string `json:"message"`

	// Path is the path to the file within a fuchsia checkout, using forward
	// slashes as delimiters, e.g. "src/foo/bar.cc"
	//
	// If omitted, the finding will apply to the change's commit message.
	Path string `json:"path"`

	// Line is the starting line of the chunk of the file that the finding
	// applies to (1-indexed, inclusive).
	//
	// If omitted, the finding will apply to the entire file and a top-level
	// file comment will be emitted.
	Line int `json:"line"`

	// EndLine is the ending line of the chunk of the file that the finding
	// applies to (1-indexed, inclusive).
	//
	// If set, must be greater than or equal to StartLine.
	//
	// If omitted, Endline is assumed to be equal to StartLine.
	EndLine int `json:"end_line"`

	// Col is the index of the first character within StartLine that the finding
	// applies to (1-indexed, inclusive).
	//
	// If omitted, the finding will apply to the entire line.
	Col int `json:"col"`

	// EndCol is the index of the last character within EndLine that the
	// finding applies to (1-indexed, exclusive).
	EndCol int `json:"end_col"`

	// Replacements is a list of possible strings that could replace the text
	// highlighted by the finding.
	Replacements []string `json:"replacements"`
}
