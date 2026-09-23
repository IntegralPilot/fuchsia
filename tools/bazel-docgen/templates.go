// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel_docgen

import (
	"strings"
	"text/template"

	pb "go.fuchsia.dev/fuchsia/tools/bazel-docgen/third_party/stardoc"
)

const ruleTemplate = `
[TOC]

# {{ .RuleName }}

{{ .DocString }}

## **ATTRIBUTES**

| Name  | Description | Type | Mandatory | Default |
| :------------- | :------------- | :------------- | :------------- | :------------- |
{{range .Attribute }}| {{ .Name }} | {{description .DocString }} | {{attributeTypeString .Type }} | {{isMandatory .Mandatory }} | {{defaultValue .DefaultValue }} |
{{end }}
`

const providerTemplate = `
[TOC]

# {{ .ProviderName }}

{{ .DocString }}

{{if .FieldInfo }}## **FIELDS**


| Name  | Description |
| :------------- | :------------- |
{{range .FieldInfo }}| {{ .Name }} | {{description .DocString }} |
{{end}}{{end}}
`

const starlarkFunctionTemplate = `
[TOC]

# {{ .FunctionName }}

{{ .DocString }}

{{if .Parameter }}## **PARAMETERS**


| Name  | Description | Default |
| :------------- | :------------- | :------------- |
{{range .Parameter }}| {{ .Name }} | {{description .DocString }} | {{defaultValue .DefaultValue }} |
{{end}}{{end}}

{{if .Return }}
**RETURNS**

{{with .Return }}{{ .DocString }}{{end}}
{{end}}
`

const repositoryRuleTemplate = `
[TOC]

# {{ .RuleName }}

{{ .DocString }}

## **ATTRIBUTES**

| Name  | Description | Type | Mandatory | Default |
| :------------- | :------------- | :------------- | :------------- | :------------- |
{{range .Attribute }}| {{ .Name }} | {{description .DocString }} | {{attributeTypeString .Type }} | {{isMandatory .Mandatory }} | {{defaultValue .DefaultValue }} |
{{end }}

{{if .Environ}}
## **ENVIRONMENT VARIABLES**

This repository rule depends on the following environment variables:

{{range .Environ}}* {{codeBlock .}}{{end}}
{{end}}
`

const READMETemplate = `
[TOC]
<div>
<devsite-filter>
<input type="text" placeholder="Type a keyword here to filter results">
<table>
<thead><tr><th>Name</th><th>Description</th></tr></thead>
<tbody class="list">
{{range . }}<tr>
    <td><a href="{{ .Path }}">{{ .Title }}</a></td>
    <td>{{ .Description }}</td>
</tr>{{end}}
</tbody>
</table>
</devsite-filter>
</div>
`

var (
	attributeTypeURLMap = map[pb.AttributeType]string{
		pb.AttributeType_INT:               "int",
		pb.AttributeType_LABEL:             "label",
		pb.AttributeType_STRING:            "string",
		pb.AttributeType_STRING_LIST:       "string_list",
		pb.AttributeType_INT_LIST:          "int_list",
		pb.AttributeType_LABEL_LIST:        "label_list",
		pb.AttributeType_BOOLEAN:           "bool",
		pb.AttributeType_LABEL_STRING_DICT: "label_keyed_string_dict",
		pb.AttributeType_STRING_DICT:       "string_dict",
		pb.AttributeType_STRING_LIST_DICT:  "string_list_dict",
		pb.AttributeType_OUTPUT:            "output",
		pb.AttributeType_OUTPUT_LIST:       "output_list",
		pb.AttributeType_LABEL_LIST_DICT:   "label_list_dict",
	}
	attributeTypeReadableNameMap = map[pb.AttributeType]string{
		pb.AttributeType_INT:               "Integer",
		pb.AttributeType_LABEL:             "Label",
		pb.AttributeType_STRING:            "String",
		pb.AttributeType_STRING_LIST:       "String List",
		pb.AttributeType_INT_LIST:          "Integer List",
		pb.AttributeType_LABEL_LIST:        "Label List",
		pb.AttributeType_BOOLEAN:           "Boolean",
		pb.AttributeType_LABEL_STRING_DICT: "Label String Dict",
		pb.AttributeType_STRING_DICT:       "String Dict",
		pb.AttributeType_STRING_LIST_DICT:  "String List Dict",
		pb.AttributeType_OUTPUT:            "Output",
		pb.AttributeType_OUTPUT_LIST:       "Output List",
		pb.AttributeType_LABEL_LIST_DICT:   "Label List Dict",
	}
)

func defaultValue(value string) string {
	if value == "" {
		return "-"
	}
	// Return the value with backticks if there is one
	return "`" + value + "`"
}

func codeBlock(value string) string {
	return "`" + value + "`"
}

type stringReplacements struct {
	input       string
	replacement string
}

// Makes the description renderable inside of a markdown table.
// TODO: properly support code blocks.
func description(value string) string {
	if value == "" {
		return "-"
	}

	// Replace the values of the description that would cause problems rendering
	// in a table context.
	replacements := []stringReplacements{
		{"<", "&lt;"},
		{">", "&gt;"},
		{"|", "\\|"},
		{"\n", "<br>"},
	}
	value = strings.TrimSpace(value)
	for _, r := range replacements {
		value = strings.ReplaceAll(value, r.input, r.replacement)
	}
	return value
}

func attributeTypeString(t pb.AttributeType) string {
	if t == pb.AttributeType_NAME {
		return "<a href=\"https://bazel.build/concepts/labels#target-names\">Name</a>"
	} else if t == pb.AttributeType_UNKNOWN {
		return "UNKNOWN"
	} else {
		return "<a href=\"https://bazel.build/rules/lib/toplevel/attr#" + attributeTypeURLMap[t] + "\">" + attributeTypeReadableNameMap[t] + "</a>"
	}
}

func isMandatory(m bool) string {
	if m {
		return "required"
	} else {
		return "optional"
	}
}
func NewRuleTemplate() (*template.Template, error) {
	return makeTemplate("rule", ruleTemplate)
}

func NewProviderTemplate() (*template.Template, error) {
	return makeTemplate("provider", providerTemplate)
}

func NewStarlarkFunctionTemplate() (*template.Template, error) {
	return makeTemplate("starlark_function", starlarkFunctionTemplate)
}

func NewRepositoryRuleTemplate() (*template.Template, error) {
	return makeTemplate("repository_rule", repositoryRuleTemplate)
}

func NewREADMETemplate() (*template.Template, error) {
	return makeTemplate("readme", READMETemplate)
}

func makeTemplate(name string, templateString string) (*template.Template, error) {
	t := template.New(name)
	t.Funcs(
		template.FuncMap{
			"defaultValue":        defaultValue,
			"description":         description,
			"attributeTypeString": attributeTypeString,
			"isMandatory":         isMandatory,
			"codeBlock":           codeBlock,
		},
	)
	return t.Parse(templateString)
}
