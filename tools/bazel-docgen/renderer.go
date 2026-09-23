// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel_docgen

import (
	pb "go.fuchsia.dev/fuchsia/tools/bazel-docgen/third_party/stardoc"
	"io"
)

type SupportedDocType int

const (
	DocTypeRule = iota
	DocTypeProvider
	DocTypeStarlarkFunction
	DocTypeRepositoryRule
)

type TOCEntry struct {
	Title       string           `yaml:",omitempty"`
	Path        string           `yaml:",omitempty"`
	Heading     string           `yaml:",omitempty"`
	Sections    []TOCEntry       `yaml:"section,omitempty"`
	Description string           `yaml:"-"`
	DocType     SupportedDocType `yaml:"-"`
}

func NewTOCEntry(title string, filename string, description string, docType SupportedDocType, mkPathFn func(string) string) TOCEntry {
	// TODO: add a description to use in the README
	return TOCEntry{
		Title:       title,
		Path:        mkPathFn(filename),
		Description: description,
		DocType:     docType,
	}
}

type Renderer interface {
	RenderRuleInfo(*pb.RuleInfo, io.Writer) error
	RenderProviderInfo(*pb.ProviderInfo, io.Writer) error
	RenderStarlarkFunctionInfo(*pb.StarlarkFunctionInfo, io.Writer) error
	RenderRepositoryRuleInfo(*pb.RepositoryRuleInfo, io.Writer) error
	RenderReadme(*[]TOCEntry, io.Writer) error
}
