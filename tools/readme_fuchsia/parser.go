// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme_fuchsia

import (
	"bufio"
	"bytes"
	"fmt"
	"os"
	"reflect"
	"sort"
	"strings"
)

const dependencyDivider = "-------------------- DEPENDENCY DIVIDER --------------------"

type fieldMeta struct {
	Index     int
	Multiline bool
	Separator string
	IsSlice   bool
}

var directiveMap map[string]fieldMeta

func init() {
	directiveMap = make(map[string]fieldMeta)
	t := reflect.TypeOf(Readme{})
	for i := 0; i < t.NumField(); i++ {
		f := t.Field(i)
		readmeTag := f.Tag.Get("readme")
		if readmeTag == "" || readmeTag == "-" {
			continue
		}

		multiline := f.Tag.Get("multiline") == "true"
		separator := f.Tag.Get("separator")
		isSlice := f.Type.Kind() == reflect.Slice

		// The readme tag can be comma-separated aliases, e.g. "Local Modifications,Modifications"
		aliases := strings.Split(readmeTag, ",")
		for _, alias := range aliases {
			directiveMap[alias] = fieldMeta{
				Index:     i,
				Multiline: multiline,
				Separator: separator,
				IsSlice:   isSlice,
			}
		}
	}
}

// ParseFile reads a README.fuchsia file from disk and parses it into a slice of Readme structs.
func ParseFile(path string) ([]*Readme, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	readmes, err := Parse(data)
	if err != nil {
		return nil, err
	}
	for _, r := range readmes {
		r.FilePath = path
	}
	return readmes, nil
}

// deduplicateAndSort takes a slice of strings, trims whitespace, removes empties and duplicates, and sorts them.
func deduplicateAndSort(items []string) []string {
	seen := make(map[string]bool)
	var result []string
	for _, item := range items {
		trimmed := strings.TrimSpace(item)
		if trimmed != "" && !seen[trimmed] {
			seen[trimmed] = true
			result = append(result, trimmed)
		}
	}
	sort.Strings(result)
	return result
}

// Parse extracts a slice of Readme structs from the given byte array,
// splitting by the standard DEPENDENCY DIVIDER if present.
func Parse(data []byte) ([]*Readme, error) {
	var readmes []*Readme

	newReadme := func(blockIdx, startLine int) (*Readme, reflect.Value) {
		r := &Readme{
			BlockIndex: blockIdx,
			BlockSpan:  LineSpan{StartLine: startLine, EndLine: startLine},
			Spans:      make(map[string]LineSpan),
		}
		return r, reflect.ValueOf(r).Elem()
	}

	readme, readmeVal := newReadme(0, 1)
	readmes = append(readmes, readme)

	var currentKey string
	var currentKeyStartLine int
	var currentValue strings.Builder
	lineNum := 0

	scanner := bufio.NewScanner(bytes.NewReader(data))
	for scanner.Scan() {
		lineNum++
		line := scanner.Text()
		trimmed := strings.TrimSpace(line)

		// Check for dependency divider
		if trimmed == dependencyDivider {
			readme.BlockSpan.EndLine = lineNum - 1
			currentKey = ""
			currentValue.Reset()
			readme, readmeVal = newReadme(len(readmes), lineNum)
			readmes = append(readmes, readme)
			continue
		}

		readme.BlockSpan.EndLine = lineNum

		// Check if we are currently inside a multi-line field
		inMultiline := false
		if currentKey != "" {
			if meta, ok := directiveMap[currentKey]; ok && meta.Multiline {
				inMultiline = true
			}
		}

		// Skip empty lines or comments ONLY if we are not actively parsing a multi-line field.
		if (trimmed == "" || strings.HasPrefix(trimmed, "#")) && !inMultiline {
			continue
		}

		isRootDirective := false
		cleanLine := strings.TrimLeft(line, " \t->")
		parts := strings.SplitN(cleanLine, ":", 2)

		if len(parts) == 2 {
			key := strings.TrimSpace(parts[0])
			value := strings.TrimSpace(parts[1])

			if meta, ok := directiveMap[key]; ok {
				isRootDirective = true
				fieldVal := readmeVal.Field(meta.Index)

				if meta.IsSlice {
					// Split by separator (usually comma)
					items := strings.Split(value, meta.Separator)
					for _, item := range items {
						itemStr := strings.TrimSpace(item)
						if itemStr != "" {
							fieldVal.Set(reflect.Append(fieldVal, reflect.ValueOf(itemStr)))
							readme.Spans[itemStr] = LineSpan{StartLine: lineNum, EndLine: lineNum}
						}
					}
					readme.Spans[key] = LineSpan{StartLine: lineNum, EndLine: lineNum}
					currentKey = ""
				} else {
					currentKey = key
					currentKeyStartLine = lineNum
					currentValue.Reset()
					currentValue.WriteString(value)
					fieldVal.SetString(currentValue.String())
					readme.Spans[key] = LineSpan{StartLine: lineNum, EndLine: lineNum}
				}
				continue
			} else if !inMultiline {
				// We hit a Key: Value pair, it's not a known directive, and we are NOT
				// currently inside a multi-line field. This is an unknown field.
				// We only record it if it's not a legacy ignored field.
				if key != "License Type" && key != "License File URL" && key != "License Reference" && key != "Non-License File Explanation" && key != "Notes" && key != "Source File" {
					readme.UnknownFields = append(readme.UnknownFields, UnknownField{
						Key:   key,
						Value: value,
						Span:  LineSpan{StartLine: lineNum, EndLine: lineNum},
					})
				}
				currentKey = ""
				isRootDirective = true
				continue
			}
		}

		// Continuation of a multi-line value
		if !isRootDirective && inMultiline {
			if trimmed == "" {
				currentValue.WriteString("\n")
			} else {
				// Strip up to 2 spaces of indentation from continuation lines
				unindented := line
				if strings.HasPrefix(unindented, "  ") {
					unindented = unindented[2:]
				} else if strings.HasPrefix(unindented, " ") {
					unindented = unindented[1:]
				}
				currentValue.WriteString("\n" + strings.TrimRight(unindented, " \t\r\n"))
			}
			readmeVal.Field(directiveMap[currentKey].Index).SetString(currentValue.String())
			readme.Spans[currentKey] = LineSpan{StartLine: currentKeyStartLine, EndLine: lineNum}
		}
	}

	if err := scanner.Err(); err != nil {
		return nil, fmt.Errorf("error scanning README bytes: %w", err)
	}

	// Deduplicate and sort all list fields (e.g., License Files) dynamically.
	for _, r := range readmes {
		rVal := reflect.ValueOf(r).Elem()
		for i := 0; i < rVal.NumField(); i++ {
			f := rVal.Field(i)
			if f.Kind() == reflect.Slice && f.Type().Elem().Kind() == reflect.String && rVal.Type().Field(i).Name != "UnknownFields" {
				if f.Len() > 0 {
					var strSlice []string
					for j := 0; j < f.Len(); j++ {
						strSlice = append(strSlice, f.Index(j).String())
					}
					sorted := deduplicateAndSort(strSlice)
					f.Set(reflect.MakeSlice(f.Type(), len(sorted), len(sorted)))
					for j, s := range sorted {
						f.Index(j).SetString(s)
					}
				}
			}
		}
	}

	return readmes, nil
}
