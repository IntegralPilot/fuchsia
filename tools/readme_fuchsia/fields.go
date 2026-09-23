// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme_fuchsia

import (
	"fmt"
	"reflect"
	"strings"
)

// GetField retrieves the string representation of a field by its key (canonical name or alias).
// If the field is a slice, it returns the elements joined by the field's separator (or comma).
func (r *Readme) GetField(key string) (string, bool) {
	readmeVal := reflect.ValueOf(r).Elem()

	// 1. Check known directives
	if meta, ok := directiveMap[key]; ok {
		fieldVal := readmeVal.Field(meta.Index)
		if meta.IsSlice {
			var items []string
			for i := 0; i < fieldVal.Len(); i++ {
				items = append(items, fieldVal.Index(i).String())
			}
			sep := meta.Separator
			if sep == "" {
				sep = ","
			}
			return strings.Join(items, sep+" "), true
		}
		return fieldVal.String(), true
	}

	// 2. Check unknown fields
	for _, uf := range r.UnknownFields {
		if uf.Key == key {
			return uf.Value, true
		}
	}

	return "", false
}

// SetField sets the value of a field by its key (canonical name or alias).
// If the field is a slice, it splits the value by the field's separator.
func (r *Readme) SetField(key string, value string) error {
	readmeVal := reflect.ValueOf(r).Elem()

	// 1. Check known directives
	if meta, ok := directiveMap[key]; ok {
		fieldVal := readmeVal.Field(meta.Index)
		if meta.IsSlice {
			sep := meta.Separator
			if sep == "" {
				sep = ","
			}
			parts := strings.Split(value, sep)
			var cleanParts []string
			for _, p := range parts {
				trimmed := strings.TrimSpace(p)
				if trimmed != "" {
					cleanParts = append(cleanParts, trimmed)
				}
			}
			sorted := deduplicateAndSort(cleanParts)

			sliceVal := reflect.MakeSlice(fieldVal.Type(), len(sorted), len(sorted))
			for i, s := range sorted {
				sliceVal.Index(i).SetString(s)
			}
			fieldVal.Set(sliceVal)
		} else {
			fieldVal.SetString(value)
		}
		return nil
	}

	// 2. Check/Update unknown fields
	for i, uf := range r.UnknownFields {
		if uf.Key == key {
			r.UnknownFields[i].Value = value
			return nil
		}
	}

	// 3. Add as new unknown field if not found
	r.UnknownFields = append(r.UnknownFields, UnknownField{
		Key:   key,
		Value: value,
	})
	return nil
}

// AddField adds or appends a value to a field by its key (canonical name or alias).
// If the field is a slice, it appends the value (or split parts) without overwriting existing items.
func (r *Readme) AddField(key string, value string) error {
	readmeVal := reflect.ValueOf(r).Elem()

	// 1. Check known directives
	if meta, ok := directiveMap[key]; ok {
		fieldVal := readmeVal.Field(meta.Index)
		if meta.IsSlice {
			var cleanParts []string
			for i := 0; i < fieldVal.Len(); i++ {
				cleanParts = append(cleanParts, fieldVal.Index(i).String())
			}
			sep := meta.Separator
			if sep == "" {
				sep = ","
			}
			parts := strings.Split(value, sep)
			for _, p := range parts {
				trimmed := strings.TrimSpace(p)
				if trimmed != "" {
					cleanParts = append(cleanParts, trimmed)
				}
			}
			sorted := deduplicateAndSort(cleanParts)

			sliceVal := reflect.MakeSlice(fieldVal.Type(), len(sorted), len(sorted))
			for i, s := range sorted {
				sliceVal.Index(i).SetString(s)
			}
			fieldVal.Set(sliceVal)
		} else if meta.Multiline {
			existing := fieldVal.String()
			if existing == "" {
				fieldVal.SetString(value)
			} else {
				fieldVal.SetString(existing + "\n" + value)
			}
		} else {
			existing := fieldVal.String()
			if existing == "" || existing == value {
				fieldVal.SetString(value)
			} else {
				return fmt.Errorf("field %q does not support multiple values and is already set to %q", key, existing)
			}
		}
		return nil
	}

	// 2. Check/Update unknown fields
	for _, uf := range r.UnknownFields {
		if uf.Key == key {
			if uf.Value == value {
				return nil
			}
			return fmt.Errorf("unknown field %q does not support multiple values and is already set to %q", key, uf.Value)
		}
	}

	// 3. Add as new unknown field if not found
	r.UnknownFields = append(r.UnknownFields, UnknownField{
		Key:   key,
		Value: value,
	})
	return nil
}
