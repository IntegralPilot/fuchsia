// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package rust_next

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
	Name, HandleDefs, ValueType, WireValueType, Value, Bytes, RawHandles, HandleDispositions string
	IsResource                                                                               bool
}

type decodeSuccessCase struct {
	Name, HandleDefs, ValueType, ValueVar, Bytes, Handles, HandleValues, UnusedHandles, EqualityCheck, WireValueType string
	IsResource, IsStatic                                                                                             bool
}

type encodeFailureCase struct {
	Name, HandleDefs, ValueType, Value, ErrorPattern string
	IsResource                                       bool
}

type decodeFailureCase struct {
	Name, HandleDefs, ValueType, Bytes, Handles, HandleValues, ErrorPattern, WireValueType string
	IsStatic                                                                               bool
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
		wireValueType := wireStructDeclName(decl)
		value := visit(encodeSuccess.Value, decl)
		for _, encoding := range encodeSuccess.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			newCase := encodeSuccessCase{
				Name:          testCaseName(encodeSuccess.Name, encoding.WireFormat),
				HandleDefs:    buildHandleDefs(encodeSuccess.HandleDefs),
				ValueType:     valueType,
				WireValueType: wireValueType,
				Value:         value,
				Bytes:         rust.BuildBytes(encoding.Bytes),
				IsResource:    decl.IsResourceType(),
			}
			if len(newCase.HandleDefs) != 0 {
				if encodeSuccess.CheckHandleRights {
					newCase.HandleDispositions = buildRawHandleDispositions(encoding.HandleDispositions)
				} else {
					newCase.RawHandles = buildRawHandles(encoding.HandleDispositions)
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
		wireValueType := wireStructDeclName(decl)
		valueVar := "_value"
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
				HandleDefs:    buildHandleDefs(decodeSuccess.HandleDefs),
				ValueType:     valueType,
				WireValueType: wireValueType,
				ValueVar:      valueVar,
				Bytes:         rust.BuildBytes(encoding.Bytes),
				Handles:       buildHandles(encoding.Handles),
				HandleValues:  buildHandleValues(encoding.Handles),
				UnusedHandles: unusedHandles,
				EqualityCheck: equalityCheck,
				IsResource:    decl.IsResourceType(),
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
		errorPattern, err := encodeErrorPattern(encodeFailure.Err)
		if err != nil {
			return nil, fmt.Errorf("encode failure %s: %s", encodeFailure.Name, err)
		}
		valueType := declName(decl)
		value := visit(encodeFailure.Value, decl)

		for _, wireFormat := range supportedWireFormats {
			encodeFailureCases = append(encodeFailureCases, encodeFailureCase{
				Name:         testCaseName(encodeFailure.Name, wireFormat),
				HandleDefs:   buildHandleDefs(encodeFailure.HandleDefs),
				ValueType:    valueType,
				Value:        value,
				ErrorPattern: errorPattern,
				IsResource:   decl.IsResourceType(),
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
		errorPattern, err := decodeErrorPattern(decodeFailure.Err)
		if err != nil {
			return nil, fmt.Errorf("decode failure %s: %s", decodeFailure.Name, err)
		}
		valueType := declName(decl)
		wireValueType := wireStructDeclName(decl)
		for _, encoding := range decodeFailure.Encodings {
			if !wireFormatSupported(encoding.WireFormat) {
				continue
			}
			decodeFailureCases = append(decodeFailureCases, decodeFailureCase{
				Name:          testCaseName(decodeFailure.Name, encoding.WireFormat),
				HandleDefs:    buildHandleDefs(decodeFailure.HandleDefs),
				ValueType:     valueType,
				WireValueType: wireValueType,
				Bytes:         rust.BuildBytes(encoding.Bytes),
				Handles:       buildHandles(encoding.Handles),
				HandleValues:  buildHandleValues(encoding.Handles),
				ErrorPattern:  errorPattern,
			})
		}
	}
	return decodeFailureCases, nil
}

func testCaseName(baseName string, wireFormat ir.WireFormat) string {
	if wireFormat != ir.V2WireFormat {
		panic("Only V2 wire format is supported")
	}
	return fidlgen.ToSnakeCase(baseName)
}

var supportedWireFormats = []ir.WireFormat{
	ir.V2WireFormat,
}

func wireFormatSupported(wireFormat ir.WireFormat) bool {
	return slices.Contains(supportedWireFormats, wireFormat)
}

var encodeErrorPatternNames = map[ir.ErrorCode]string{
	ir.NonNullableTypeWithNullValue: "EncodeError::InvalidRequiredHandle",
	ir.StrictBitsUnknownBit:         "EncodeError::InvalidStrictBits",
	ir.StringTooLong:                "EncodeError::Validation(ValidationError::StringTooLong{ .. })",
	ir.CountExceedsLimit:            "EncodeError::Validation(ValidationError::VectorTooLong{ .. })",
	ir.FlexibleUnionUnknownField:    "EncodeError::UnknownUnionOrdinal(_)",
}

func encodeErrorPattern(code ir.ErrorCode) (string, error) {
	if str, ok := encodeErrorPatternNames[code]; ok {
		return str, nil
	}
	return "", fmt.Errorf("no rust error string defined for encode error code %s", code)
}

var decodeErrorPatternMap = map[ir.ErrorCode]string{
	ir.CountExceedsLimit:                  "DecodeError::Validation(ValidationError::VectorTooLong{ .. })",
	ir.EnvelopeBytesExceedMessageLength:   "DecodeError::InvalidEnvelopeSize(_)",
	ir.EnvelopeHandlesExceedMessageLength: "DecodeError::InvalidEnvelopeSize(_)",
	ir.ExceededMaxOutOfLineDepth:          `"TODO: ExceededMaxOutOfLineDepth"`,
	ir.IncorrectHandleType:                "DecodeError::ExpectedDriverHandle", // probably wrong
	ir.InvalidBoolean:                     "DecodeError::InvalidBool(_)",
	ir.InvalidEmptyStruct:                 "DecodeError::InvalidUnit(_)",
	ir.InvalidHandlePresenceIndicator:     "DecodeError::InvalidHandlePresence(_)",
	ir.InvalidInlineBitInEnvelope:         `"TODO: InvalidInlineBitInEnvelope"`,
	ir.InvalidInlineMarkerInEnvelope:      `DecodeError::InvalidEnvelopeFlags(_)`,
	ir.InvalidNumBytesInEnvelope:          "DecodeError::InvalidEnvelopeSize(_)",
	ir.InvalidNumHandlesInEnvelope:        "DecodeError::IncorrectNumberOfHandlesConsumed{ .. }",
	ir.InvalidPaddingByte:                 `DecodeError::InvalidPadding`,
	ir.InvalidPresenceIndicator:           "DecodeError::InvalidPointerPresence(_)",
	ir.MissingRequiredHandleRights:        `"TODO: MissingRequiredHandleRights"`,
	ir.NonEmptyStringWithNullBody:         "DecodeError::InvalidOptionalSize(_) | DecodeError::RequiredValueAbsent",
	ir.NonEmptyVectorWithNullBody:         "DecodeError::InvalidOptionalSize(_) | DecodeError::RequiredValueAbsent",
	ir.NonNullableTypeWithNullValue:       "DecodeError::RequiredValueAbsent | DecodeError::RequiredHandleAbsent",
	ir.StrictBitsUnknownBit:               "DecodeError::InvalidBits{ .. }",
	ir.StrictEnumUnknownValue:             "DecodeError::InvalidEnumOrdinal(_)",
	ir.StrictUnionUnknownField:            "DecodeError::InvalidUnionOrdinal(_)",
	ir.StringCountExceeds32BitLimit:       "DecodeError::Validation(ValidationError::StringTooLong{ .. })",
	ir.StringNotUtf8:                      "DecodeError::InvalidUtf8(_)",
	ir.StringTooLong:                      "DecodeError::Validation(ValidationError::StringTooLong{ .. })", // probably should have a different error
	ir.TableCountExceeds32BitLimit:        "DecodeError::VectorTooLong{ .. }",
	ir.TooFewBytes:                        "DecodeError::InsufficientData",
	ir.TooFewHandles:                      "DecodeError::InsufficientHandles",
	ir.TooManyBytesInMessage:              "DecodeError::ExtraBytes{ .. }",
	ir.TooManyHandlesInMessage:            "DecodeError::ExtraHandles{ .. }",
	ir.UnionFieldNotSet:                   "DecodeError::InvalidUnionEnvelope",
	ir.VectorCountExceeds32BitLimit:       "DecodeError::Validation(ValidationError::VectorTooLong{ .. })",
}

func decodeErrorPattern(code ir.ErrorCode) (string, error) {
	if str, ok := decodeErrorPatternMap[code]; ok {
		return str, nil
	}
	return "", fmt.Errorf("no rust error string defined for decode error code %s", code)
}
