// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel_docgen

import (
	pb "go.fuchsia.dev/fuchsia/tools/bazel-docgen/third_party/stardoc"
	"io"
	"text/template"
)

type MarkdownRenderer struct{}

func NewMarkdownRenderer() MarkdownRenderer {
	return MarkdownRenderer{}
}

func (r MarkdownRenderer) RenderRuleInfo(rule *pb.RuleInfo, out io.Writer) error {
	return render(rule, out, NewRuleTemplate)
}

func (r MarkdownRenderer) RenderProviderInfo(rule *pb.ProviderInfo, out io.Writer) error {
	return render(rule, out, NewProviderTemplate)
}

func (r MarkdownRenderer) RenderStarlarkFunctionInfo(rule *pb.StarlarkFunctionInfo, out io.Writer) error {
	return render(rule, out, NewStarlarkFunctionTemplate)
}

func (r MarkdownRenderer) RenderRepositoryRuleInfo(rule *pb.RepositoryRuleInfo, out io.Writer) error {
	return render(rule, out, NewRepositoryRuleTemplate)
}

func (r MarkdownRenderer) RenderReadme(toc *[]TOCEntry, out io.Writer) error {
	return render(toc, out, NewREADMETemplate)
}

func render(v any, out io.Writer, templateFunc func() (*template.Template, error)) error {
	t, err := templateFunc()
	if err == nil {
		err = t.Execute(out, v)
	}
	return err
}
