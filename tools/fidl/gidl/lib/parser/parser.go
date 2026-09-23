// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package parser

import (
	"fmt"
	"io"
	"sort"
	"strconv"
	"strings"
	"text/scanner"

	"go.fuchsia.dev/fuchsia/tools/fidl/gidl/lib/ir"
	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
	"golang.org/x/exp/slices"
)

type Parser struct {
	scanner    scanner.Scanner
	lookaheads []token
	config     Config
	// Used to validate that handles are defined and used exactly once.
	handles map[ir.Handle]handleInfo
}

type Config struct {
	// All supported languages, used to validate bindings allowlist/denylist.
	Languages []ir.Language
	// All supported wire formats, used to validate `bytes` sections.
	WireFormats []ir.WireFormat
}

type handleInfo struct {
	// True if defined in the 'handle_defs' section.
	defined bool
	// Number of uses in the 'value' section.
	usesInValue int
	// Number of uses in the 'decode' function in the 'value' section. This
	// number is included in usesInValue.
	usesInValueDecodeFunction int
	// Number of uses in the 'handles' or 'handle_dispositions' section.
	usesInHandles int
}

func mergeHandleInfo(a, b handleInfo) handleInfo {
	return handleInfo{
		defined:                   a.defined || b.defined,
		usesInValue:               a.usesInValue + b.usesInValue,
		usesInValueDecodeFunction: a.usesInValueDecodeFunction + b.usesInValueDecodeFunction,
		usesInHandles:             a.usesInHandles + b.usesInHandles,
	}
}

func NewParser(name string, input io.Reader, config Config) *Parser {
	var p Parser
	p.scanner.Filename = name
	p.scanner.Init(input)
	p.config = config
	// This is reset in parseSection. We need to set it here too so that tests
	// for functions like parseHandleDefs don't panic on nil map assignment.
	p.handles = make(map[ir.Handle]handleInfo)
	return &p
}

func (p *Parser) Parse() (ir.All, error) {
	var result ir.All
	for !p.peekTokenKind(tEof) {
		if err := p.parseSection(&result); err != nil {
			return ir.All{}, err
		}
	}
	return result, nil
}

type tokenKind uint

const (
	_ tokenKind = iota
	tEof
	tText
	tString
	tLacco
	tRacco
	tComma
	tColon
	tPlus
	tNeg
	tLparen
	tRparen
	tEqual
	tLsquare
	tRsquare
	tHash
)

var tokenKindStrings = []string{
	"<invalid>",
	"<eof>",
	"<text>",
	"<string>",
	"{",
	"}",
	",",
	":",
	"+",
	"-",
	"(",
	")",
	"=",
	"[",
	"]",
	"#",
}

var (
	isUnitTokenKind = make(map[tokenKind]struct{})
	textToTokenKind = make(map[string]tokenKind)
)

func init() {
	for index, text := range tokenKindStrings {
		if strings.HasPrefix(text, "<") && strings.HasSuffix(text, ">") {
			continue
		}
		kind := tokenKind(index)
		isUnitTokenKind[kind] = struct{}{}
		textToTokenKind[text] = kind
	}
}

func (kind tokenKind) String() string {
	if index := int(kind); index < len(tokenKindStrings) {
		return tokenKindStrings[index]
	}
	return fmt.Sprintf("%d", kind)
}

type token struct {
	kind         tokenKind
	value        string
	line, column int
}

func (t token) String() string {
	if _, ok := isUnitTokenKind[t.kind]; ok {
		return t.kind.String()
	} else {
		return t.value
	}
}

type bodyElement uint

const (
	_ bodyElement = iota
	isType
	isValue
	isBytes
	isHandles
	isHandleDispositions
	isHandleDefs
	isErr
	isBindingsAllowlist
	isBindingsDenylist
	isBindingsSkip
	isEnableSendEventBenchmark
	isEnableEchoCallBenchmark
)

func (kind bodyElement) String() string {
	switch kind {
	case isType:
		return "type"
	case isValue:
		return "value"
	case isBytes:
		return "bytes"
	case isHandles:
		return "handles"
	case isHandleDispositions:
		return "handle_dispositions"
	case isHandleDefs:
		return "handle_defs"
	case isErr:
		return "err"
	case isBindingsAllowlist:
		return "bindings_allowlist"
	case isBindingsDenylist:
		return "bindings_denylist"
	case isBindingsSkip:
		return "bindings_skip"
	case isEnableSendEventBenchmark:
		return "enable_send_event_benchmark"
	case isEnableEchoCallBenchmark:
		return "enable_echo_call_benchmark"
	default:
		panic("unsupported kind")
	}
}

type encodingData struct {
	WireFormat ir.WireFormat
	Bytes      []byte

	// At most one of Handle or HandleDispositions will be non-empty.
	Handles            []ir.Handle
	HandleDispositions []ir.HandleDisposition
}

func toIrEncodings(v []encodingData) []ir.Encoding {
	var out []ir.Encoding
	for _, e := range v {
		if e.HandleDispositions != nil {
			panic("toIrEncodings should not be called when handle_dispositions is allowed")
		}
		out = append(out, ir.Encoding{
			WireFormat: e.WireFormat,
			Bytes:      e.Bytes,
			Handles:    e.Handles,
		})
	}
	return out
}

func toIrHandleDispositionEncodings(v []encodingData) []ir.HandleDispositionEncoding {
	var out []ir.HandleDispositionEncoding
	for _, e := range v {
		if e.HandleDispositions != nil {
			out = append(out, ir.HandleDispositionEncoding{
				WireFormat:         e.WireFormat,
				Bytes:              e.Bytes,
				HandleDispositions: e.HandleDispositions,
			})
		} else {
			var handleDispositions []ir.HandleDisposition
			for _, handle := range e.Handles {
				handleDispositions = append(handleDispositions, ir.HandleDisposition{
					Handle: handle,
					Type:   fidlgen.ObjectTypeNone,
					Rights: fidlgen.HandleRightsSameRights,
				})
			}
			out = append(out, ir.HandleDispositionEncoding{
				WireFormat:         e.WireFormat,
				Bytes:              e.Bytes,
				HandleDispositions: handleDispositions,
			})
		}
	}
	return out
}

type body struct {
	Type                     string
	Value                    ir.RecordLike
	Encodings                []encodingData
	HandleDefs               []ir.HandleDef
	Err                      ir.ErrorCode
	BindingsAllowlist        *[]ir.Language
	BindingsDenylist         *[]ir.Language
	BindingsSkip             *[]ir.Language
	EnableSendEventBenchmark bool
	EnableEchoCallBenchmark  bool
}

func (b *body) addEncoding(e encodingData) error {
	for i := range b.Encodings {
		if b.Encodings[i].WireFormat == e.WireFormat {
			if e.Bytes != nil {
				if b.Encodings[i].Bytes != nil {
					return fmt.Errorf("bytes already set")
				}
				b.Encodings[i].Bytes = e.Bytes
			}
			if e.Handles != nil {
				if b.Encodings[i].Handles != nil {
					return fmt.Errorf("handles already set")
				}
				if b.Encodings[i].HandleDispositions != nil {
					return fmt.Errorf("cannot add handles when handle dispositions is set")
				}
				b.Encodings[i].Handles = e.Handles
			}
			if e.HandleDispositions != nil {
				if b.Encodings[i].Handles != nil {
					return fmt.Errorf("cannot add handles when handle dispositions is set")
				}
				if b.Encodings[i].HandleDispositions != nil {
					return fmt.Errorf("handle dispositions already set")
				}
				b.Encodings[i].HandleDispositions = e.HandleDispositions
			}
			return nil
		}
	}
	b.Encodings = append(b.Encodings, e)
	return nil
}

type scope struct {
	allowed          allowedFeatures
	inDecodeFunction bool
}

type allowedFeatures struct {
	// Allow specifying rights in the `handle_defs` section, e.g.
	// `#0 = event(rights: basic)` as opposed to `#0 = event()`.
	handleDefRights bool
	// Allow using the restrict function in the `value` section, e.g.
	// `restrict(#0, type: event, rights: basic)` as opposed to `#0`.
	restrictFunction bool
	// Allow using the decode function in the `value` section, e.g.
	// `decode({type = MyStruct, bytes = { v2 = [ ... ] } })`.
	decodeFunction bool
}

type sectionMetadata struct {
	requiredKinds map[bodyElement]struct{}
	optionalKinds map[bodyElement]struct{}
	allowed       allowedFeatures
	mustUseHandle func(kind bodyElement, info handleInfo) bool
	setter        func(name string, body body, loc ir.SourceLocation, all *ir.All)
}

var sections = map[string]sectionMetadata{
	"success": {
		requiredKinds: map[bodyElement]struct{}{isValue: {}, isBytes: {}},
		optionalKinds: map[bodyElement]struct{}{
			isHandles: {}, isHandleDefs: {}, isBindingsAllowlist: {}, isBindingsDenylist: {},
		},
		allowed: allowedFeatures{},
		mustUseHandle: func(kind bodyElement, info handleInfo) bool {
			return kind == isValue || kind == isHandles
		},
		setter: func(name string, body body, loc ir.SourceLocation, all *ir.All) {
			encodeSuccess := ir.EncodeSuccess{
				Name:              name,
				Value:             body.Value,
				Encodings:         toIrHandleDispositionEncodings(body.Encodings),
				HandleDefs:        body.HandleDefs,
				BindingsAllowlist: body.BindingsAllowlist,
				BindingsDenylist:  body.BindingsDenylist,
				CheckHandleRights: false,
				SourceLocation:    loc,
			}
			all.EncodeSuccess = append(all.EncodeSuccess, encodeSuccess)
			decodeSuccess := ir.DecodeSuccess{
				Name:              name,
				Value:             body.Value,
				Encodings:         toIrEncodings(body.Encodings),
				HandleDefs:        body.HandleDefs,
				BindingsAllowlist: body.BindingsAllowlist,
				BindingsDenylist:  body.BindingsDenylist,
				SourceLocation:    loc,
			}
			all.DecodeSuccess = append(all.DecodeSuccess, decodeSuccess)
		},
	},
	"encode_success": {
		requiredKinds: map[bodyElement]struct{}{isValue: {}, isBytes: {}},
		optionalKinds: map[bodyElement]struct{}{
			isHandleDispositions: {}, isHandleDefs: {}, isBindingsAllowlist: {}, isBindingsDenylist: {},
		},
		allowed: allowedFeatures{
			handleDefRights: true,
			decodeFunction:  true,
		},
		mustUseHandle: func(kind bodyElement, info handleInfo) bool {
			switch kind {
			case isValue:
				return true
			case isHandleDispositions:
				// Generally input handles must appear in the output, but
				// handles in the 'decode' function might have been discarded.
				return info.usesInValueDecodeFunction == 0
			default:
				return false
			}
		},
		setter: func(name string, body body, loc ir.SourceLocation, all *ir.All) {
			result := ir.EncodeSuccess{
				Name:              name,
				Value:             body.Value,
				Encodings:         toIrHandleDispositionEncodings(body.Encodings),
				HandleDefs:        body.HandleDefs,
				BindingsAllowlist: body.BindingsAllowlist,
				BindingsDenylist:  body.BindingsDenylist,
				CheckHandleRights: true, SourceLocation: loc,
			}
			all.EncodeSuccess = append(all.EncodeSuccess, result)
		},
	},
	"decode_success": {
		requiredKinds: map[bodyElement]struct{}{isValue: {}, isBytes: {}},
		optionalKinds: map[bodyElement]struct{}{
			isHandles: {}, isHandleDefs: {}, isBindingsAllowlist: {}, isBindingsDenylist: {},
		},
		allowed: allowedFeatures{
			handleDefRights:  true,
			restrictFunction: true,
		},
		mustUseHandle: func(kind bodyElement, info handleInfo) bool {
			// All handles must appear in 'handles', but not necessarily in
			// 'value', since they could be unknowns that are discarded.
			return kind == isHandles
		},
		setter: func(name string, body body, loc ir.SourceLocation, all *ir.All) {
			result := ir.DecodeSuccess{
				Name:              name,
				Value:             body.Value,
				Encodings:         toIrEncodings(body.Encodings),
				HandleDefs:        body.HandleDefs,
				BindingsAllowlist: body.BindingsAllowlist,
				BindingsDenylist:  body.BindingsDenylist, SourceLocation: loc,
			}
			all.DecodeSuccess = append(all.DecodeSuccess, result)
		},
	},
	"encode_failure": {
		requiredKinds: map[bodyElement]struct{}{isValue: {}, isErr: {}},
		optionalKinds: map[bodyElement]struct{}{
			isHandleDefs: {}, isBindingsAllowlist: {}, isBindingsDenylist: {}, isBindingsSkip: {},
		},
		allowed: allowedFeatures{
			handleDefRights: true,
			decodeFunction:  true,
		},
		mustUseHandle: func(kind bodyElement, info handleInfo) bool {
			return kind == isValue
		},
		setter: func(name string, body body, loc ir.SourceLocation, all *ir.All) {
			result := ir.EncodeFailure{
				Name:              name,
				Value:             body.Value,
				HandleDefs:        body.HandleDefs,
				Err:               body.Err,
				BindingsAllowlist: body.BindingsAllowlist,
				BindingsDenylist:  body.BindingsDenylist,
				BindingsSkip:      body.BindingsSkip, SourceLocation: loc,
			}
			all.EncodeFailure = append(all.EncodeFailure, result)
		},
	},
	"decode_failure": {
		requiredKinds: map[bodyElement]struct{}{isType: {}, isBytes: {}, isErr: {}},
		optionalKinds: map[bodyElement]struct{}{
			isHandles: {}, isHandleDefs: {}, isBindingsAllowlist: {}, isBindingsDenylist: {},
		},
		allowed: allowedFeatures{
			handleDefRights: true,
		},
		mustUseHandle: func(kind bodyElement, info handleInfo) bool {
			return kind == isHandles
		},
		setter: func(name string, body body, loc ir.SourceLocation, all *ir.All) {
			result := ir.DecodeFailure{
				Name:              name,
				Type:              body.Type,
				Encodings:         toIrEncodings(body.Encodings),
				HandleDefs:        body.HandleDefs,
				Err:               body.Err,
				BindingsAllowlist: body.BindingsAllowlist,
				BindingsDenylist:  body.BindingsDenylist,
				SourceLocation:    loc,
			}
			all.DecodeFailure = append(all.DecodeFailure, result)
		},
	},
	"benchmark": {
		requiredKinds: map[bodyElement]struct{}{isValue: {}},
		optionalKinds: map[bodyElement]struct{}{
			isHandleDefs: {}, isBindingsAllowlist: {}, isBindingsDenylist: {},
			isEnableSendEventBenchmark: {}, isEnableEchoCallBenchmark: {},
		},
		allowed: allowedFeatures{},
		mustUseHandle: func(kind bodyElement, info handleInfo) bool {
			return kind == isValue
		},
		setter: func(name string, body body, loc ir.SourceLocation, all *ir.All) {
			benchmark := ir.Benchmark{
				Name:                     name,
				Value:                    body.Value,
				HandleDefs:               body.HandleDefs,
				BindingsAllowlist:        body.BindingsAllowlist,
				BindingsDenylist:         body.BindingsDenylist,
				EnableSendEventBenchmark: body.EnableSendEventBenchmark,
				EnableEchoCallBenchmark:  body.EnableEchoCallBenchmark,
				SourceLocation:           loc,
			}
			all.Benchmark = append(all.Benchmark, benchmark)
		},
	},
}

type handleSlice []ir.Handle

func (h handleSlice) Len() int           { return len(h) }
func (h handleSlice) Less(i, j int) bool { return h[i] < h[j] }
func (h handleSlice) Swap(i, j int)      { h[i], h[j] = h[j], h[i] }

func (p *Parser) parseSection(all *ir.All) error {
	section, name, loc, err := p.parsePreamble()
	if err != nil {
		return err
	}
	p.handles = make(map[ir.Handle]handleInfo)
	bodyTok, err := p.peekToken()
	if err != nil {
		return err
	}
	scope := scope{
		allowed: section.allowed,
	}
	body, err := p.parseBody(section.requiredKinds, section.optionalKinds, scope)
	if err != nil {
		return err
	}
	// Sort handles first so that errors are deterministic.
	var handles []ir.Handle
	for h := range p.handles {
		handles = append(handles, h)
	}
	sort.Sort(handleSlice(handles))
	for _, h := range handles {
		info := p.handles[h]
		if !info.defined {
			return p.newParseError(bodyTok, "missing definition for handle #%d", h)
		}
		if info.usesInValue > 1 {
			return p.newParseError(bodyTok, "handle #%d used more than once in 'value' section", h)
		}
		if info.usesInHandles > 1 {
			return p.newParseError(bodyTok, "handle #%d used more than once in 'handles' section", h)
		}
		if info.usesInValue == 0 && info.usesInHandles == 0 {
			return p.newParseError(bodyTok, "unused handle #%d", h)
		}
		if section.mustUseHandle(isValue, info) && info.usesInValue == 0 {
			return p.newParseError(bodyTok, "handle #%d does not occur in the 'value' section", h)
		}
		if section.mustUseHandle(isHandles, info) && info.usesInHandles == 0 {
			return p.newParseError(bodyTok, "handle #%d does not occur in the 'handles' section", h)
		}
		if section.mustUseHandle(isHandleDispositions, info) && info.usesInHandles == 0 {
			return p.newParseError(bodyTok, "handle #%d does not occur in the 'handle_dispositions' section", h)
		}
	}
	p.handles = nil
	section.setter(name, body, loc, all)
	return nil
}

func (p *Parser) parsePreamble() (sectionMetadata, string, ir.SourceLocation, error) {
	tok, err := p.consumeToken(tText)
	if err != nil {
		return sectionMetadata{}, "", ir.SourceLocation{}, err
	}

	loc := ir.SourceLocation{
		Filename: p.scanner.Filename,
		Line:     tok.line,
	}

	section, ok := sections[tok.value]
	if !ok {
		return sectionMetadata{}, "", loc, p.newParseError(tok, "unknown section %s", tok.value)
	}

	tok, err = p.consumeToken(tLparen)
	if err != nil {
		return sectionMetadata{}, "", loc, err
	}

	tok, err = p.consumeToken(tString)
	if err != nil {
		return sectionMetadata{}, "", loc, err
	}
	name := tok.value

	tok, err = p.consumeToken(tRparen)
	if err != nil {
		return sectionMetadata{}, "", loc, err
	}

	return section, name, loc, nil
}

func (p *Parser) parseBody(requiredKinds map[bodyElement]struct{}, optionalKinds map[bodyElement]struct{}, scope scope) (body, error) {
	var (
		result      body
		parsedKinds = make(map[bodyElement]struct{})
	)
	bodyTok, err := p.peekToken()
	if err != nil {
		return result, err
	}
	if err := p.parseCommaSeparated(tLacco, tRacco, func() error {
		return p.parseSingleBodyElement(&result, parsedKinds, scope)
	}); err != nil {
		return result, err
	}
	for requiredKind := range requiredKinds {
		if _, ok := parsedKinds[requiredKind]; !ok {
			return result, p.newParseError(bodyTok, "missing required parameter '%s'", requiredKind)
		}
	}
	for parsedKind := range parsedKinds {
		_, requiredKind := requiredKinds[parsedKind]
		_, optionalKind := optionalKinds[parsedKind]
		if !requiredKind && !optionalKind {
			return result, p.newParseError(bodyTok, "parameter '%s' does not apply to element", parsedKind)
		}
	}
	return result, nil
}

func (p *Parser) parseSingleBodyElement(result *body, all map[bodyElement]struct{}, scope scope) error {
	tok, err := p.consumeToken(tText)
	if err != nil {
		return err
	}
	if _, err := p.consumeToken(tEqual); err != nil {
		return err
	}
	var kind bodyElement
	switch tok.value {
	case "type":
		tok, err := p.consumeToken(tText)
		if err != nil {
			return err
		}
		result.Type = tok.value
		kind = isType
	case "value":
		tok, err := p.peekToken()
		if err != nil {
			return err
		}
		val, err := p.parseValue(scope)
		if err != nil {
			return err
		}
		record, ok := val.(ir.RecordLike)
		if !ok {
			// TODO(https://fxbug.dev/42069389): Change message when tables and unions are allowed.
			return p.newParseError(tok, "top-level value must be a struct; got %T", val)
		}
		result.Value = record
		kind = isValue
	case "bytes":
		encodings, err := p.parseByteSection()
		if err != nil {
			return err
		}
		for _, e := range encodings {
			if err := result.addEncoding(e); err != nil {
				return p.newParseError(tok, "%s", err)
			}
		}
		kind = isBytes
	case "handles":
		encodings, err := p.parseHandleSection(scope)
		if err != nil {
			return err
		}
		for _, e := range encodings {
			if err := result.addEncoding(e); err != nil {
				return p.newParseError(tok, "%s", err)
			}
		}
		kind = isHandles
	case "handle_dispositions":
		encodings, err := p.parseHandleDispositionSection()
		if err != nil {
			return err
		}
		for _, e := range encodings {
			if err := result.addEncoding(e); err != nil {
				return p.newParseError(tok, "%s", err)
			}
		}
		kind = isHandleDispositions
	case "handle_defs":
		handleDefs, err := p.parseHandleDefSection(scope)
		if err != nil {
			return err
		}
		result.HandleDefs = handleDefs
		kind = isHandleDefs
	case "err":
		errorCode, err := p.parseErrorCode()
		if err != nil {
			return err
		}
		result.Err = errorCode
		kind = isErr
	case "bindings_allowlist":
		languages, err := p.parseLanguageList()
		if err != nil {
			return err
		}
		result.BindingsAllowlist = &languages
		kind = isBindingsAllowlist
	case "bindings_denylist":
		languages, err := p.parseLanguageList()
		if err != nil {
			return err
		}
		result.BindingsDenylist = &languages
		kind = isBindingsDenylist
	case "bindings_skip":
		languages, err := p.parseLanguageList()
		if err != nil {
			return err
		}
		result.BindingsSkip = &languages
		kind = isBindingsSkip
	case "enable_send_event_benchmark":
		value, err := p.parseValue(scope)
		if err != nil {
			return err
		}
		boolValue, ok := value.(bool)
		if !ok {
			return p.newParseError(tok, "expected boolean value")
		}
		result.EnableSendEventBenchmark = boolValue
		kind = isEnableSendEventBenchmark
	case "enable_echo_call_benchmark":
		value, err := p.parseValue(scope)
		if err != nil {
			return err
		}
		boolValue, ok := value.(bool)
		if !ok {
			return p.newParseError(tok, "expected boolean value")
		}
		result.EnableEchoCallBenchmark = boolValue
		kind = isEnableEchoCallBenchmark
	default:
		return p.newParseError(tok, "must be type, value, bytes, err, bindings_allowlist or bindings_denylist")
	}
	if kind == 0 {
		panic("kind must be set")
	}
	if _, ok := all[kind]; ok {
		return p.newParseError(tok, "duplicate %s found", kind)
	}
	all[kind] = struct{}{}
	return nil
}

func (p *Parser) parseHandleRestrict() (ir.RestrictedHandle, error) {
	if _, err := p.consumeToken(tLparen); err != nil {
		return ir.RestrictedHandle{}, err
	}
	handle, err := p.parseHandle(handleInfo{usesInValue: 1})
	if err != nil {
		return ir.RestrictedHandle{}, err
	}
	var (
		hasObjectType bool
		objectType    fidlgen.ObjectType
		hasRights     bool
		rights        fidlgen.HandleRights
	)
	for p.peekTokenKind(tComma) {
		p.nextToken()

		labelTok, err := p.consumeToken(tText)
		if err != nil {
			return ir.RestrictedHandle{}, err
		}

		if _, err := p.consumeToken(tColon); err != nil {
			return ir.RestrictedHandle{}, err
		}

		switch labelTok.value {
		case "type":
			hasObjectType = true
			subtype, err := p.parseHandleSubtype()
			if err != nil {
				return ir.RestrictedHandle{}, err
			}
			objectType = fidlgen.ObjectTypeFromHandleSubtype(subtype)
		case "rights":
			hasRights = true
			rightsTok, err := p.peekToken()
			if err != nil {
				return ir.RestrictedHandle{}, err
			}
			rights, err = p.parseHandleRights()
			if err != nil {
				return ir.RestrictedHandle{}, err
			}
			if rights == fidlgen.HandleRightsSameRights {
				return ir.RestrictedHandle{}, p.newParseError(rightsTok, "'same_rights' is not allowed in the restrict function")
			}
		default:
			return ir.RestrictedHandle{}, p.newParseError(labelTok, "unknown restrict argument: %s", labelTok.value)
		}
	}
	tok, err := p.consumeToken(tRparen)
	if err != nil {
		return ir.RestrictedHandle{}, err
	}
	if !hasObjectType {
		return ir.RestrictedHandle{}, p.newParseError(tok, "missing restrict argument 'type'")
	}
	if !hasRights {
		return ir.RestrictedHandle{}, p.newParseError(tok, "missing restrict argument 'rights'")
	}
	return ir.RestrictedHandle{
		Handle: handle,
		Type:   objectType,
		Rights: rights,
	}, nil
}

func (p *Parser) parseValue(scope scope) (ir.Value, error) {
	tok, err := p.peekToken()
	if err != nil {
		return nil, err
	}
	switch tok.kind {
	case tText:
		tok, err := p.nextToken()
		if err != nil {
			return nil, err
		}
		if '0' <= tok.value[0] && tok.value[0] <= '9' {
			return parseNum(tok, false)
		}
		if tok.value == "null" {
			return nil, nil
		}
		if tok.value == "true" {
			return true, nil
		}
		if tok.value == "false" {
			return false, nil
		}
		if tok.value == "raw_float" {
			return p.parseRawFloat()
		}
		if tok.value == "restrict" {
			if !scope.allowed.restrictFunction {
				return nil, p.newParseError(tok, "the 'restrict' function is not allowed here")
			}
			return p.parseHandleRestrict()
		}
		if tok.value == "decode" {
			if !scope.allowed.decodeFunction {
				return nil, fmt.Errorf("the 'decode' function is not allowed here")
			}
			return p.parseDecode(scope)
		}
		return p.parseRecord(tok.value, scope)
	case tLsquare:
		return p.parseSlice(scope)
	case tString:
		tok, err := p.nextToken()
		if err != nil {
			return nil, err
		}
		return tok.value, nil
	case tNeg:
		if _, err := p.nextToken(); err != nil {
			return nil, err
		}
		if tok, err := p.consumeToken(tText); err != nil {
			return nil, err
		} else {
			return parseNum(tok, true)
		}
	case tHash:
		handle, err := p.parseHandle(handleInfo{usesInValue: 1})
		if err != nil {
			return nil, err
		}
		return handle, nil
	default:
		tok, err := p.peekToken()
		if err != nil {
			return nil, err
		}
		return nil, p.newParseError(tok, "expected value")
	}
}

func parseNum(tok token, neg bool) (ir.Value, error) {
	if strings.Contains(tok.value, ".") {
		val, err := strconv.ParseFloat(tok.value, 64)
		if err != nil {
			return nil, err
		}
		if neg {
			return -val, nil
		} else {
			return val, nil
		}
	} else {
		val, err := strconv.ParseUint(tok.value, 0, 64)
		if err != nil {
			return nil, err
		}
		if neg {
			return -int64(val), nil
		} else {
			return uint64(val), nil
		}
	}
}

func (p *Parser) parseRawFloat() (ir.RawFloat, error) {
	// Already parsed raw_float token.
	if _, err := p.consumeToken(tLparen); err != nil {
		return 0, err
	}
	tok, err := p.nextToken()
	if err != nil {
		return 0, err
	}
	raw, err := strconv.ParseUint(tok.value, 0, 64)
	if err != nil {
		return 0, err
	}
	if _, err := p.consumeToken(tRparen); err != nil {
		return 0, err
	}
	return ir.RawFloat(raw), nil
}

func (p *Parser) parseRecord(name string, scope scope) (ir.Record, error) {
	obj := ir.Record{Name: name}
	err := p.parseCommaSeparated(tLacco, tRacco, func() error {
		tokFieldName, err := p.consumeToken(tText)
		if err != nil {
			return err
		}
		key := decodeFieldKey(tokFieldName.value)
		if _, err := p.consumeToken(tColon); err != nil {
			return err
		}
		var val ir.Value
		if key.IsKnown() {
			val, err = p.parseValue(scope)
		} else if p.peekTokenKind(tText) {
			tok, err := p.consumeToken(tText)
			if err != nil {
				panic(fmt.Sprintf("consume failed after peek: %s", err))
			}
			if tok.value != "null" {
				return p.newParseError(tok, "expected 'null' or '{', got '%s'", tok.value)
			}
			// This syntax, as in `MyUnion { 123: null }` is used to
			// represent a domain object that only stores the unknown
			// ordinal, not bytes and handles.
			val = nil
		} else {
			val, err = p.parseUnknownData()
		}
		if err != nil {
			return err
		}
		obj.Fields = append(obj.Fields, ir.Field{Key: key, Value: val})
		return nil
	})
	if err != nil {
		return ir.Record{}, err
	}
	return obj, nil
}

// Field can be referenced by either name or ordinal.
func decodeFieldKey(field string) ir.FieldKey {
	if ord, err := strconv.ParseInt(field, 0, 64); err == nil {
		return ir.FieldKey{UnknownOrdinal: uint64(ord)}
	}
	return ir.FieldKey{Name: field}
}

func (p *Parser) parseDecode(scope scope) (ir.DecodedRecord, error) {
	parenTok, err := p.consumeToken(tLparen)
	if err != nil {
		return ir.DecodedRecord{}, err
	}
	required := map[bodyElement]struct{}{
		isType:  {},
		isBytes: {},
	}
	optional := map[bodyElement]struct{}{
		isHandles: {},
	}
	newscope := scope
	newscope.inDecodeFunction = true
	body, err := p.parseBody(required, optional, newscope)
	if err != nil {
		return ir.DecodedRecord{}, err
	}
	if _, err := p.consumeToken(tRparen); err != nil {
		return ir.DecodedRecord{}, err
	}
	if len(body.Encodings) == 0 {
		panic("required body elements ensure there is at least one encoding")
	}
	if len(body.Encodings) > 1 {
		return ir.DecodedRecord{}, p.newParseError(parenTok, "the decode function can only use one wire format")
	}
	return ir.DecodedRecord{
		Type: body.Type,
		Encoding: ir.Encoding{
			WireFormat: body.Encodings[0].WireFormat,
			Bytes:      body.Encodings[0].Bytes,
			Handles:    body.Encodings[0].Handles,
		},
	}, nil
}

// This is not expressed in terms of parseBody because it parses bytes/handles
// without wire formats (e.g. `bytes = [...]`, not `bytes = { v2 = [...] }`).
func (p *Parser) parseUnknownData() (ir.UnknownData, error) {
	var result ir.UnknownData
	parsedKinds := make(map[bodyElement]struct{})
	bodyTok, err := p.peekToken()
	if err != nil {
		return result, err
	}
	if err := p.parseCommaSeparated(tLacco, tRacco, func() error {
		tok, err := p.consumeToken(tText)
		if err != nil {
			return err
		}
		if _, err := p.consumeToken(tEqual); err != nil {
			return err
		}
		var kind bodyElement
		switch tok.value {
		case "bytes":
			bytes, err := p.parseByteList()
			if err != nil {
				return err
			}
			result.Bytes = bytes
			kind = isBytes
		case "handles":
			handles, err := p.parseHandleList(handleInfo{usesInValue: 1})
			if err != nil {
				return err
			}
			result.Handles = handles
			kind = isHandles
		default:
			return p.newParseError(tok, "parameter '%s' does not apply to unknown data, must be bytes", tok)
		}
		if kind == 0 {
			panic("kind must be set")
		}
		if _, ok := parsedKinds[kind]; ok {
			return p.newParseError(tok, "duplicate parameter '%s' found", kind)
		}
		parsedKinds[kind] = struct{}{}
		return nil
	}); err != nil {
		return result, err
	}
	for _, requiredKind := range []bodyElement{isBytes} {
		if _, ok := parsedKinds[requiredKind]; !ok {
			return result, p.newParseError(bodyTok, "missing required parameter '%s'", requiredKind)
		}
	}
	return result, nil
}

func (p *Parser) parseErrorCode() (ir.ErrorCode, error) {
	tok, err := p.consumeToken(tText)
	if err != nil {
		return "", err
	}
	code := ir.ErrorCode(tok.value)
	if _, ok := ir.AllErrorCodes[code]; !ok {
		return "", p.newParseError(tok, "unknown error code: %s", tok.value)
	}
	return code, nil
}

func (p *Parser) parseSlice(scope scope) ([]ir.Value, error) {
	result := make([]ir.Value, 0)
	err := p.parseCommaSeparated(tLsquare, tRsquare, func() error {
		tok, err := p.peekToken()
		if err != nil {
			return err
		}
		if tok.kind == tText && tok.value == "repeat" {
			p.consumeToken(tText)
			_, err := p.consumeToken(tLparen)
			if err != nil {
				return err
			}
			val, err := p.parseValue(scope)
			if err != nil {
				return err
			}
			_, err = p.consumeToken(tRparen)
			if err != nil {
				return err
			}
			size, err := p.parseColonSize()
			if err != nil {
				return err
			}
			for i := uint64(0); i < size; i++ {
				result = append(result, val)
			}
			return nil
		}

		val, err := p.parseValue(scope)
		if err != nil {
			return err
		}
		result = append(result, val)
		return nil
	})
	if err != nil {
		return nil, err
	}
	return result, nil
}

func (p *Parser) parseTextSlice() ([]string, error) {
	var result []string
	err := p.parseCommaSeparated(tLsquare, tRsquare, func() error {
		if tok, err := p.consumeToken(tText); err != nil {
			return err
		} else {
			result = append(result, tok.value)
			return nil
		}
	})
	if err != nil {
		return nil, err
	}
	return result, nil
}

func (p *Parser) parseLanguageList() ([]ir.Language, error) {
	var result []ir.Language
	err := p.parseCommaSeparated(tLsquare, tRsquare, func() error {
		tok, err := p.consumeToken(tText)
		if err != nil {
			return err
		}
		language := ir.Language(tok.value)
		if !slices.Contains(p.config.Languages, language) {
			var strs []string
			for _, language := range p.config.Languages {
				strs = append(strs, string(language))
			}
			return p.newParseError(tok, "invalid language '%s'; must be one of: %s",
				tok.value, strings.Join(strs, ", "))
		}
		result = append(result, language)
		return nil
	})
	if err != nil {
		return nil, err
	}
	return result, nil
}

func (p *Parser) parseByteSection() ([]encodingData, error) {
	var res []encodingData
	firstTok, err := p.peekToken()
	if err != nil {
		return nil, err
	}
	err = p.parseWireFormatMapping(func(wireFormats []ir.WireFormat) error {
		b, err := p.parseByteList()
		if err != nil {
			return err
		}
		for _, wf := range wireFormats {
			res = append(res, encodingData{
				WireFormat: wf,
				Bytes:      b,
			})
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	if len(res) == 0 {
		return nil, p.newParseError(firstTok, "no bytes provided for any wire format")
	}
	return res, nil
}

func (p *Parser) parseByteList() ([]byte, error) {
	var bytes []byte
	err := p.parseCommaSeparated(tLsquare, tRsquare, func() error {
		tok, err := p.peekToken()
		if err != nil {
			return err
		}
		if tok.kind == tText && 'a' <= tok.value[0] && tok.value[0] <= 'z' {
			p.nextToken()
			parse, ok := p.getByteGenParser(tok.value)
			if !ok {
				return p.newParseError(tok, "invalid byte syntax: %s", tok.value)
			}
			b, err := parse()
			if err != nil {
				return err
			}
			bytes = append(bytes, b...)
		} else {
			b, err := p.parseByte()
			if err != nil {
				return err
			}
			bytes = append(bytes, b)
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	return bytes, nil
}

func (p *Parser) parseByte() (byte, error) {
	tok, err := p.consumeToken(tText)
	if err != nil {
		return 0, err
	}
	if len(tok.value) == 3 && tok.value[0] == '\'' && tok.value[2] == '\'' {
		return tok.value[1], nil
	}
	b, err := strconv.ParseUint(tok.value, 0, 8)
	if err != nil {
		return 0, p.newParseError(tok, "invalid byte syntax: %s", tok.value)
	}
	return byte(b), nil
}

func (p *Parser) parseHandleSection(scope scope) ([]encodingData, error) {
	info := handleInfo{usesInHandles: 1}
	if scope.inDecodeFunction {
		info = handleInfo{usesInValue: 1, usesInValueDecodeFunction: 1}
	}
	var res []encodingData
	err := p.parseWireFormatMapping(func(wireFormats []ir.WireFormat) error {
		h, err := p.parseHandleList(info)
		if err != nil {
			return err
		}
		for _, wf := range wireFormats {
			res = append(res, encodingData{
				WireFormat: wf,
				Handles:    h,
			})
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	return res, nil
}

func (p *Parser) parseHandleDispositionSection() ([]encodingData, error) {
	var res []encodingData
	err := p.parseWireFormatMapping(func(wireFormats []ir.WireFormat) error {
		h, err := p.parseHandleDispositionList(handleInfo{usesInHandles: 1})
		if err != nil {
			return err
		}
		for _, wf := range wireFormats {
			res = append(res, encodingData{
				WireFormat:         wf,
				HandleDispositions: h,
			})
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	return res, nil
}

func (p *Parser) parseHandleList(info handleInfo) ([]ir.Handle, error) {
	var handles []ir.Handle
	err := p.parseCommaSeparated(tLsquare, tRsquare, func() error {
		h, err := p.parseHandle(info)
		if err != nil {
			return err
		}
		handles = append(handles, h)
		return nil
	})
	if err != nil {
		return nil, err
	}
	return handles, nil
}

func (p *Parser) parseHandleDispositionList(info handleInfo) ([]ir.HandleDisposition, error) {
	var handleDispositions []ir.HandleDisposition
	err := p.parseCommaSeparated(tLsquare, tRsquare, func() error {
		if _, err := p.consumeToken(tLacco); err != nil {
			return err
		}
		handle, err := p.parseHandle(handleInfo{usesInHandles: 1})
		if err != nil {
			return err
		}
		objectType := fidlgen.ObjectTypeNone
		rights := fidlgen.HandleRightsSameRights
		for p.peekTokenKind(tComma) {
			p.nextToken()

			labelTok, err := p.consumeToken(tText)
			if err != nil {
				return err
			}

			if _, err := p.consumeToken(tColon); err != nil {
				return err
			}

			switch labelTok.value {
			case "type":
				subtype, err := p.parseHandleSubtype()
				if err != nil {
					return err
				}
				objectType = fidlgen.ObjectTypeFromHandleSubtype(subtype)
			case "rights":
				rights, err = p.parseHandleRights()
				if err != nil {
					return err
				}
			default:
				return p.newParseError(labelTok, "unknown handle disposition label: %s", labelTok.value)
			}
		}
		handleDispositions = append(handleDispositions, ir.HandleDisposition{
			Handle: handle,
			Type:   objectType,
			Rights: rights,
		})
		if _, err := p.consumeToken(tRacco); err != nil {
			return err
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	return handleDispositions, nil
}

func (p *Parser) parseHandle(info handleInfo) (ir.Handle, error) {
	tok, err := p.consumeToken(tHash)
	if err != nil {
		return 0, err
	}
	tok, err = p.consumeToken(tText)
	if err != nil {
		return 0, err
	}
	index, err := strconv.ParseInt(tok.value, 10, 0)
	if err != nil {
		return 0, p.newParseError(tok, "invalid handle syntax: %s", tok.value)
	}
	if index < 0 {
		panic("impossible because tok is tText not tNeg")
	}
	h := ir.Handle(index)
	p.handles[h] = mergeHandleInfo(p.handles[h], info)
	return h, nil
}

func (p *Parser) parseHandleDefSection(scope scope) ([]ir.HandleDef, error) {
	var res []ir.HandleDef
	expected := ir.Handle(0)
	err := p.parseCommaSeparated(tLacco, tRacco, func() error {
		tok, err := p.peekToken()
		if err != nil {
			return err
		}
		h, err := p.parseHandle(handleInfo{defined: true})
		if err != nil {
			return err
		}
		if h != expected {
			return p.newParseError(
				tok, "want #%d, got #%d (handle_defs must be #0, #1, #2, etc.)", expected, h)
		}
		expected++
		if _, err = p.consumeToken(tEqual); err != nil {
			return err
		}
		hd, err := p.parseHandleDef(scope)
		if err != nil {
			return err
		}
		res = append(res, hd)
		return nil
	})
	if err != nil {
		return nil, err
	}
	return res, nil
}

func (p *Parser) parseHandleSubtype() (fidlgen.HandleSubtype, error) {
	tok, err := p.consumeToken(tText)
	if err != nil {
		return "", err
	}
	subtype, ok := ir.HandleSubtypeByName(tok.value)
	if !ok {
		return "", p.newParseError(tok, "invalid handle subtype: %s", tok.value)
	}
	return subtype, nil
}

func (p *Parser) parseHandleRights() (fidlgen.HandleRights, error) {
	tok, err := p.consumeToken(tText)
	if err != nil {
		return 0, err
	}
	rights, ok := ir.HandleRightsByName(tok.value)
	if !ok {
		return 0, p.newParseError(tok, "expected handle right: %s", tok.value)
	}
	for p.peekTokenKind(tPlus) || p.peekTokenKind(tNeg) {
		opTok, err := p.nextToken()
		if err != nil {
			return 0, err
		}
		tok, err := p.consumeToken(tText)
		if err != nil {
			return 0, err
		}
		opRights, ok := ir.HandleRightsByName(tok.value)
		if !ok {
			return 0, p.newParseError(tok, "expected handle right: %s", tok.value)
		}
		switch opTok.kind {
		case tPlus:
			rights |= opRights
		case tNeg:
			rights &= ^opRights
		default:
			panic("unexpected state")
		}
	}
	return rights, nil
}

func (p *Parser) parseHandleDef(scope scope) (ir.HandleDef, error) {
	subtype, err := p.parseHandleSubtype()
	if err != nil {
		return ir.HandleDef{}, err
	}
	if _, err := p.consumeToken(tLparen); err != nil {
		return ir.HandleDef{}, err
	}
	rights := fidlgen.HandleRightsSameRights
	if p.peekTokenKind(tText) {
		if !scope.allowed.handleDefRights {
			tok, _ := p.peekToken()
			return ir.HandleDef{}, p.newParseError(tok, "rights are disabled for this section")
		}

		tok, err := p.consumeToken(tText)
		if err != nil {
			return ir.HandleDef{}, err
		}
		if tok.value != "rights" {
			return ir.HandleDef{}, p.newParseError(tok, "expected 'rights', got %s", tok.value)
		}
		tok, err = p.consumeToken(tColon)
		if err != nil {
			return ir.HandleDef{}, err
		}
		rights, err = p.parseHandleRights()
		if err != nil {
			return ir.HandleDef{}, err
		}
	}
	if _, err := p.consumeToken(tRparen); err != nil {
		return ir.HandleDef{}, err
	}
	return ir.HandleDef{
		Subtype: subtype,
		Rights:  rights,
	}, nil
}

func (p *Parser) parseWireFormatMapping(handler func([]ir.WireFormat) error) error {
	seenWireFormats := map[ir.WireFormat]struct{}{}
	return p.parseCommaSeparated(tLacco, tRacco, func() error {
		var wireFormats []ir.WireFormat
		for {
			tok, err := p.consumeToken(tText)
			if err != nil {
				return err
			}
			wf := ir.WireFormat(tok.value)
			if !slices.Contains(p.config.WireFormats, wf) {
				var strs []string
				for _, wireFormat := range p.config.WireFormats {
					strs = append(strs, string(wireFormat))
				}
				return p.newParseError(tok, "invalid wire format '%s'; must be one of: %s",
					tok.value, strings.Join(strs, ", "))
			}
			if _, ok := seenWireFormats[wf]; ok {
				return p.newParseError(tok, "duplicate wire format: %s", tok.value)
			}
			seenWireFormats[wf] = struct{}{}
			wireFormats = append(wireFormats, wf)
			if p.peekTokenKind(tEqual) {
				break
			}
			if _, err := p.consumeToken(tComma); err != nil {
				return err
			}
		}
		if _, err := p.consumeToken(tEqual); err != nil {
			return err
		}
		return handler(wireFormats)
	})
}

func (p *Parser) parseCommaSeparated(beginTok, endTok tokenKind, handler func() error) error {
	if _, err := p.consumeToken(beginTok); err != nil {
		return err
	}
	for !p.peekTokenKind(endTok) {
		if err := handler(); err != nil {
			return err
		}
		if !p.peekTokenKind(endTok) {
			if _, err := p.consumeToken(tComma); err != nil {
				return err
			}
		}
	}
	if _, err := p.consumeToken(endTok); err != nil {
		return err
	}
	return nil
}

func (p *Parser) consumeToken(kind tokenKind) (token, error) {
	tok, err := p.nextToken()
	if err != nil {
		return token{}, err
	} else if tok.kind != kind {
		return token{}, p.newParseError(tok, "unexpected tokenKind: want %q, got %q (value: %q)", kind, tok.kind, tok.value)
	}
	return tok, nil
}

func (p *Parser) peekTokenKind(kind tokenKind) bool {
	tok, err := p.peekToken()
	if err != nil {
		return false
	}
	return tok.kind == kind
}

func (p *Parser) peekToken() (token, error) {
	if len(p.lookaheads) == 0 {
		tok, err := p.nextToken()
		if err != nil {
			return token{}, err
		}
		p.lookaheads = append(p.lookaheads, tok)
	}
	return p.lookaheads[0], nil
}

func (p *Parser) nextToken() (token, error) {
	if len(p.lookaheads) != 0 {
		var tok token
		tok, p.lookaheads = p.lookaheads[0], p.lookaheads[1:]
		return tok, nil
	}
	return p.scanToken()
}

func (p *Parser) scanToken() (token, error) {
	// eof
	if tok := p.scanner.Scan(); tok == scanner.EOF {
		return token{tEof, "", 0, 0}, nil
	}
	pos := p.scanner.Position

	// unit tokens
	text := p.scanner.TokenText()
	if kind, ok := textToTokenKind[text]; ok {
		return token{kind, text, pos.Line, pos.Column}, nil
	}

	// string
	if text[0] == '"' {
		tok := token{tString, "", pos.Line, pos.Column}
		s, err := strconv.Unquote(text)
		if err != nil {
			return tok, p.newParseError(tok, "improperly escaped string, %s: %s", err, text)
		}
		tok.value = s
		return tok, nil
	}

	// text
	return token{tText, text, pos.Line, pos.Column}, nil
}

type parseError struct {
	input        string
	line, column int
	message      string
}

// Assert parseError implements error interface
var _ error = &parseError{}

func (err *parseError) Error() string {
	return fmt.Sprintf("%s:%d:%d: %s", err.input, err.line, err.column, err.message)
}

func (p *Parser) newParseError(tok token, format string, a ...any) error {
	return &parseError{
		input:   p.scanner.Position.Filename,
		line:    tok.line,
		column:  tok.column,
		message: fmt.Sprintf(format, a...),
	}
}
