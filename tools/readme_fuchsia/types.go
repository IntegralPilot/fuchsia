// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme_fuchsia

// LineSpan represents a 1-indexed start and end line range in a file.
type LineSpan struct {
	StartLine int `json:"start_line,omitempty"`
	EndLine   int `json:"end_line,omitempty"`
}

// Finding represents a structured finding for SHAC / static analysis.
// It implements the error interface so validation functions can return Findings as errors.
type Finding struct {
	FilePath     string   `json:"filepath,omitempty"`
	Line         int      `json:"line,omitempty"`
	EndLine      int      `json:"end_line,omitempty"`
	Col          int      `json:"col,omitempty"`
	EndCol       int      `json:"end_col,omitempty"`
	Level        string   `json:"level"`
	Message      string   `json:"message"`
	Replacements []string `json:"replacements,omitempty"`
}

func (f Finding) Error() string {
	return f.Message
}

// Readme represents a parsed README.fuchsia file.
type Readme struct {
	FilePath   string   `readme:"-" json:"-"`
	BlockIndex int      `readme:"-" json:"-"`
	BlockSpan  LineSpan `readme:"-" json:"-"`

	Name                     string `readme:"Name"`
	URL                      string `readme:"URL"`
	OriginalURL              string `readme:"Original URL"`
	CPEPrefix                string `readme:"CPEPrefix"`
	Version                  string `readme:"Version"`
	UpstreamGit              string `readme:"Upstream Git"`
	Revision                 string `readme:"Revision"`
	UpstreamRevision         string `readme:"Upstream Revision,Upstream revision"`
	SecurityCritical         string `readme:"Security Critical"`
	FirstParty               string `readme:"First Party"`
	LicenseAndroidCompatible string `readme:"License Android Compatible"`
	Location                 string `readme:"Location"`

	Licenses             []string `readme:"License" separator:","`
	LicenseFiles         []string `readme:"License File" separator:","`
	GeneratedNoticeFiles []string `readme:"Generated Notice File" separator:","`
	NonLicenseFiles      []string `readme:"Non-License File" separator:","`

	UnknownFields []UnknownField `readme:"-"`

	Description        string `readme:"Description" multiline:"true"`
	LocalModifications string `readme:"Local Modifications,Modifications" multiline:"true"`
	Deprecated         string `readme:"Deprecated" multiline:"true"`

	Spans map[string]LineSpan `readme:"-" json:"-"`
}

// UnknownField represents an unrecognized Key: Value pair found in the README.
type UnknownField struct {
	Key   string   `json:"key"`
	Value string   `json:"value"`
	Span  LineSpan `json:"span"`
}
