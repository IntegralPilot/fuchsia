// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package ir

import (
	"fmt"
	"reflect"

	"golang.org/x/exp/slices"
)

func Merge(input []All) All {
	var output All
	for _, elem := range input {
		output.EncodeSuccess = append(output.EncodeSuccess, elem.EncodeSuccess...)
		output.DecodeSuccess = append(output.DecodeSuccess, elem.DecodeSuccess...)
		output.EncodeFailure = append(output.EncodeFailure, elem.EncodeFailure...)
		output.DecodeFailure = append(output.DecodeFailure, elem.DecodeFailure...)
		output.Benchmark = append(output.Benchmark, elem.Benchmark...)
	}
	return output
}

func FilterByLanguage(input All, language Language) All {
	shouldKeep := func(allowlist *[]Language, denylist *[]Language, skiplist *[]Language) bool {
		if denylist != nil && slices.Contains(*denylist, language) {
			return false
		}
		if skiplist != nil && slices.Contains(*skiplist, language) {
			return false
		}
		if allowlist != nil {
			return slices.Contains(*allowlist, language)
		}
		if _, ok := defaultDenyLanguages[language]; ok {
			return false
		}
		return true
	}
	var output All
	for _, def := range input.EncodeSuccess {
		if shouldKeep(def.BindingsAllowlist, def.BindingsDenylist, nil) {
			output.EncodeSuccess = append(output.EncodeSuccess, def)
		}
	}
	for _, def := range input.DecodeSuccess {
		if shouldKeep(def.BindingsAllowlist, def.BindingsDenylist, nil) {
			output.DecodeSuccess = append(output.DecodeSuccess, def)
		}
	}
	for _, def := range input.EncodeFailure {
		if shouldKeep(def.BindingsAllowlist, def.BindingsDenylist, def.BindingsSkip) {
			output.EncodeFailure = append(output.EncodeFailure, def)
		}
	}
	for _, def := range input.DecodeFailure {
		if shouldKeep(def.BindingsAllowlist, def.BindingsDenylist, nil) {
			output.DecodeFailure = append(output.DecodeFailure, def)
		}
	}
	for _, def := range input.Benchmark {
		if shouldKeep(def.BindingsAllowlist, def.BindingsDenylist, nil) {
			output.Benchmark = append(output.Benchmark, def)
		}
	}
	return output
}

type OutputType string

const (
	OutputTypeConformance OutputType = "conformance"
	OutputTypeBenchmark   OutputType = "benchmark"
	OutputTypeMeasureTape OutputType = "measure_tape"
)

func ValidateByOutputType(input All, outputType OutputType) error {
	forbid := func(fields map[string]any) error {
		for name, field := range fields {
			value := reflect.ValueOf(field)
			if value.Len() > 0 {
				return fmt.Errorf("encountered %s which is invalid for output type %q", name, outputType)
			}
		}
		return nil
	}
	switch outputType {
	case OutputTypeConformance, OutputTypeMeasureTape:
		return forbid(map[string]any{
			"Benchmark": input.Benchmark,
		})
	case OutputTypeBenchmark:
		return forbid(map[string]any{
			"EncodeSuccess": input.EncodeSuccess,
			"DecodeSuccess": input.DecodeSuccess,
			"EncodeFailure": input.EncodeFailure,
			"DecodeFailure": input.DecodeFailure,
		})
	default:
		panic(fmt.Sprintf("unexpected output type: %s", outputType))
	}
}

func TypeFromValue(value Value) string {
	record, ok := value.(RecordLike)
	if !ok {
		panic(fmt.Sprintf("cannot extract type name from: %T", value))
	}
	return record.TypeName()
}

func GetHandlesFromHandleDispositions(handleDispositions []HandleDisposition) []Handle {
	var handles []Handle
	for _, handleDisposition := range handleDispositions {
		handles = append(handles, handleDisposition.Handle)
	}
	return handles
}

// GetUnusedHandles returns the list of handles from the input slice that do not
// appear in the provided Value
func GetUnusedHandles(value Value, handles []Handle) []Handle {
	usedHandles := make(map[Handle]struct{})
	populateUsedHandles(value, usedHandles)

	var unused []Handle
	for _, handle := range handles {
		if _, ok := usedHandles[handle]; !ok {
			unused = append(unused, handle)
		}
	}
	return unused
}

func populateUsedHandles(value Value, seen map[Handle]struct{}) {
	switch value := value.(type) {
	case AnyHandle:
		seen[value.GetHandle()] = struct{}{}
	case Record:
		for _, field := range value.Fields {
			populateUsedHandles(field.Value, seen)
		}
	case UnknownData:
		for _, handle := range value.Handles {
			seen[handle] = struct{}{}
		}
	case []Value:
		for _, item := range value {
			populateUsedHandles(item, seen)
		}
	}
}
