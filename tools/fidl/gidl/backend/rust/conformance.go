// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package rust

import (
	"bytes"
	_ "embed"
	"fmt"
	"slices"
	"text/template"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/config"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/mixer"
	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/rust"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

var (
	//go:embed conformance.tmpl
	conformanceTmplText string

	conformanceTmpl = template.Must(template.New("conformanceTmpl").Parse(conformanceTmplText))
)

type conformanceTmplInput struct {
	EncodeSuccessCases []encodeSuccessCase
	DecodeSuccessCases []decodeSuccessCase
	EncodeFailureCases []encodeFailureCase
	DecodeFailureCases []decodeFailureCase
}

type encodeSuccessCase struct {
	Name, Context, HandleDefs, ValueType, Value, Bytes, Handles, HandleDispositions string
	IsResource                                                                      bool
}

type decodeSuccessCase struct {
	Name, Context, HandleDefs, ValueType, ValueVar, Bytes, Handles, UnusedHandles, EqualityCheck string
}

type encodeFailureCase struct {
	Name, Context, HandleDefs, ValueType, Value, ErrorCode string
	IsResource                                             bool
}

type decodeFailureCase struct {
	Name, Context, HandleDefs, ValueType, Bytes, Handles, ErrorCode string
}

// GenerateConformanceTests generates Rust tests.
func GenerateConformanceTests(gidl ir.All, fidl fidlgen.Root, config config.GeneratorConfig) ([]byte, error) {
	schema := mixer.BuildSchema(fidl)
	encodeSuccessCases, err := encodeSuccessCases(gidl.EncodeSuccess, schema)
	if err != nil {
		return nil, err
	}
	decodeSuccessCases, err := decodeSuccessCases(gidl.DecodeSuccess, schema)
	if err != nil {
		return nil, err
	}
	encodeFailureCases, err := encodeFailureCases(gidl.EncodeFailure, schema)
	if err != nil {
		return nil, err
	}
	decodeFailureCases, err := decodeFailureCases(gidl.DecodeFailure, schema)
	if err != nil {
		return nil, err
	}
	input := conformanceTmplInput{
		EncodeSuccessCases: encodeSuccessCases,
		DecodeSuccessCases: decodeSuccessCases,
		EncodeFailureCases: encodeFailureCases,
		DecodeFailureCases: decodeFailureCases,
	}
	var buf bytes.Buffer
	err = conformanceTmpl.Execute(&buf, input)
	return buf.Bytes(), err
}

func encodeSuccessCases(gidlEncodeSuccesses []ir.EncodeSuccess, schema mixer.Schema) ([]encodeSuccessCase, error) {
	var encodeSuccessCases []encodeSuccessCase
	for _, encodeSuccess := range gidlEncodeSuccesses {
		decl, err := schema.ExtractDeclarationEncodeSuccess(encodeSuccess.Value, encodeSuccess.HandleDefs)
		if err != nil {
			return nil, fmt.Errorf("encode success %s: %s", encodeSuccess.Name, err)
		}
		valueType := declName(decl)
		value := visit(encodeSuccess.Value, decl)
		for _, encoding := range encodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			newCase := encodeSuccessCase{
				Name:       testCaseName(encodeSuccess.Name, encoding.WireFormat),
				Context:    encodingContext(encoding.WireFormat),
				HandleDefs: buildHandleDefs(encodeSuccess.HandleDefs),
				ValueType:  valueType,
				Value:      value,
				Bytes:      rust.BuildBytes(encoding.Bytes),
				IsResource: decl.IsResourceType(),
			}
			if len(newCase.HandleDefs) != 0 {
				if encodeSuccess.CheckHandleRights {
					newCase.HandleDispositions = buildRawHandleDispositions(encoding.HandleDispositions)
				} else {
					newCase.Handles = buildRawHandles(encoding.HandleDispositions)
				}
			}
			encodeSuccessCases = append(encodeSuccessCases, newCase)
		}
	}
	return encodeSuccessCases, nil
}

func decodeSuccessCases(gidlDecodeSuccesses []ir.DecodeSuccess, schema mixer.Schema) ([]decodeSuccessCase, error) {
	var decodeSuccessCases []decodeSuccessCase
	for _, decodeSuccess := range gidlDecodeSuccesses {
		decl, err := schema.ExtractDeclaration(decodeSuccess.Value, decodeSuccess.HandleDefs)
		if err != nil {
			return nil, fmt.Errorf("decode success %s: %s", decodeSuccess.Name, err)
		}
		valueType := declName(decl)
		valueVar := "value"
		equalityCheck := buildEqualityCheck(valueVar, decodeSuccess.Value, decl)
		for _, encoding := range decodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			unusedHandles := ""
			if h := ir.GetUnusedHandles(decodeSuccess.Value, encoding.Handles); len(h) != 0 {
				unusedHandles = buildHandles(h)
			}
			decodeSuccessCases = append(decodeSuccessCases, decodeSuccessCase{
				Name:          testCaseName(decodeSuccess.Name, encoding.WireFormat),
				Context:       encodingContext(encoding.WireFormat),
				HandleDefs:    buildHandleDefs(decodeSuccess.HandleDefs),
				ValueType:     valueType,
				ValueVar:      valueVar,
				Bytes:         rust.BuildBytes(encoding.Bytes),
				Handles:       buildHandles(encoding.Handles),
				UnusedHandles: unusedHandles,
				EqualityCheck: equalityCheck,
			})
		}
	}
	return decodeSuccessCases, nil
}

func encodeFailureCases(gidlEncodeFailures []ir.EncodeFailure, schema mixer.Schema) ([]encodeFailureCase, error) {
	var encodeFailureCases []encodeFailureCase
	for _, encodeFailure := range gidlEncodeFailures {
		decl, err := schema.ExtractDeclarationUnsafe(encodeFailure.Value)
		if err != nil {
			return nil, fmt.Errorf("encode failure %s: %s", encodeFailure.Name, err)
		}
		errorCode, err := rustErrorCode(encodeFailure.Err)
		if err != nil {
			return nil, fmt.Errorf("encode failure %s: %s", encodeFailure.Name, err)
		}
		valueType := declName(decl)
		value := visit(encodeFailure.Value, decl)

		for _, wireFormat := range supportedWireFormats {
			encodeFailureCases = append(encodeFailureCases, encodeFailureCase{
				Name:       testCaseName(encodeFailure.Name, wireFormat),
				Context:    encodingContext(wireFormat),
				HandleDefs: buildHandleDefs(encodeFailure.HandleDefs),
				ValueType:  valueType,
				Value:      value,
				ErrorCode:  errorCode,
				IsResource: decl.IsResourceType(),
			})
		}
	}
	return encodeFailureCases, nil
}

func decodeFailureCases(gidlDecodeFailures []ir.DecodeFailure, schema mixer.Schema) ([]decodeFailureCase, error) {
	var decodeFailureCases []decodeFailureCase
	for _, decodeFailure := range gidlDecodeFailures {
		decl, err := schema.ExtractDeclarationByName(decodeFailure.Type)
		if err != nil {
			return nil, fmt.Errorf("decode failure %s: %s", decodeFailure.Name, err)
		}
		errorCode, err := rustErrorCode(decodeFailure.Err)
		if err != nil {
			return nil, fmt.Errorf("decode failure %s: %s", decodeFailure.Name, err)
		}
		valueType := declName(decl)
		for _, encoding := range decodeFailure.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			decodeFailureCases = append(decodeFailureCases, decodeFailureCase{
				Name:       testCaseName(decodeFailure.Name, encoding.WireFormat),
				Context:    encodingContext(encoding.WireFormat),
				HandleDefs: buildHandleDefs(decodeFailure.HandleDefs),
				ValueType:  valueType,
				Bytes:      rust.BuildBytes(encoding.Bytes),
				Handles:    buildHandles(encoding.Handles),
				ErrorCode:  errorCode,
			})
		}
	}
	return decodeFailureCases, nil
}

func testCaseName(baseName string, wireFormat ir.WireFormat) string {
	return fidlgen.ToSnakeCase(fmt.Sprintf("%s_%s", baseName, wireFormat))
}

var supportedWireFormats = []ir.WireFormat{
	ir.V2WireFormat,
}

func wireFormatSupported(wireFormat ir.WireFormat) bool {
	return slices.Contains(supportedWireFormats, wireFormat)
}

func encodingContext(wireFormat ir.WireFormat) string {
	switch wireFormat {
	case ir.V2WireFormat:
		return "_V2_CONTEXT"
	default:
		panic(fmt.Sprintf("unexpected wire format %v", wireFormat))
	}
}

// Rust errors are defined in src/lib/fidl/rust/fidl/src/error.rs.
var rustErrorCodeNames = map[ir.ErrorCode]string{
	ir.CountExceedsLimit:                  "VectorTooLong",
	ir.EnvelopeBytesExceedMessageLength:   "InvalidNumBytesInEnvelope",
	ir.EnvelopeHandlesExceedMessageLength: "InvalidNumHandlesInEnvelope",
	ir.ExceededMaxOutOfLineDepth:          "MaxRecursionDepth",
	ir.FlexibleUnionUnknownField:          "UnknownUnionTag",
	ir.IncorrectHandleType:                "IncorrectHandleSubtype",
	ir.InvalidBoolean:                     "InvalidBoolean",
	ir.InvalidEmptyStruct:                 "Invalid",
	ir.InvalidHandlePresenceIndicator:     "InvalidPresenceIndicator",
	ir.InvalidInlineBitInEnvelope:         "InvalidInlineBitInEnvelope",
	ir.InvalidInlineMarkerInEnvelope:      "InvalidInlineMarkerInEnvelope",
	ir.InvalidNumBytesInEnvelope:          "InvalidNumBytesInEnvelope",
	ir.InvalidNumHandlesInEnvelope:        "InvalidNumHandlesInEnvelope",
	ir.InvalidPaddingByte:                 "NonZeroPadding",
	ir.InvalidPresenceIndicator:           "InvalidPresenceIndicator",
	ir.MissingRequiredHandleRights:        "MissingExpectedHandleRights",
	ir.NonEmptyStringWithNullBody:         "UnexpectedNullRef",
	ir.NonEmptyVectorWithNullBody:         "UnexpectedNullRef",
	ir.NonNullableTypeWithNullValue:       "NotNullable",
	ir.StrictBitsUnknownBit:               "InvalidBitsValue",
	ir.StrictEnumUnknownValue:             "InvalidEnumValue",
	ir.StrictUnionUnknownField:            "UnknownUnionTag",
	ir.StringCountExceeds32BitLimit:       "OutOfRange",
	ir.StringNotUtf8:                      "Utf8Error",
	ir.StringTooLong:                      "StringTooLong",
	ir.TableCountExceeds32BitLimit:        "OutOfRange",
	ir.TooFewBytes:                        "OutOfRange",
	ir.TooFewBytesInPrimaryObject:         "OutOfRange",
	ir.TooFewHandles:                      "OutOfHandles",
	ir.TooManyBytesInMessage:              "ExtraBytes",
	ir.TooManyHandlesInMessage:            "ExtraHandles",
	ir.UnexpectedOrdinal:                  "OutOfRange",
	ir.UnionFieldNotSet:                   "UnknownUnionTag",
	ir.VectorCountExceeds32BitLimit:       "OutOfRange",
}

func rustErrorCode(code ir.ErrorCode) (string, error) {
	if str, ok := rustErrorCodeNames[code]; ok {
		return fmt.Sprintf("Error::%s", str), nil
	}
	return "", fmt.Errorf("no rust error string defined for error code %s", code)
}
