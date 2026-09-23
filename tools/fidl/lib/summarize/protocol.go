// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package summarize

import (
	"fmt"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

// addProtocols adds the protocols to the elements list.
func (s *summarizer) addProtocols(protocols []fidlgen.Protocol) {
	for _, p := range protocols {
		for _, m := range p.Methods {
			s.addElement(newMethod(&s.symbols, p.Name, m))
		}
		s.addElement(&protocol{
			named:    named{name: Name(p.Name)},
			protocol: p,
		})
	}
}

// registerProtocolNames registers the names of all protocols in the FIDL IR.
func (s *summarizer) registerProtocolNames(protocols []fidlgen.Protocol) {
	for _, p := range protocols {
		// This will become useful when deliberating channel syntax.
		s.symbols.addProtocol(p.Name)
	}
}

// protocol represents an element of the protocol type.
type protocol struct {
	named
	notMember
	protocol fidlgen.Protocol
}

func (p *protocol) Serialize() ElementStr {
	e := p.named.Serialize()
	e.Kind = ProtocolKind
	e.Openness = openness(p.protocol.Openness)
	transport := p.protocol.OverTransport()
	switch transport {
	case "Channel":
		e.Transport = channelTransport
	case "Driver":
		e.Transport = driverTransport
	case "Banjo":
		e.Transport = banjoTransport
	case "Syscall":
		e.Transport = syscallTransport
	default:
		panic(fmt.Sprintf("API summary tool does not support the '%s' transport", transport))
	}
	return e
}

// method represents an Element for a protocol method.
type method struct {
	membership isMember
	method     fidlgen.Method
}

// newMethod creates a new protocol method element.
func newMethod(s *symbolTable, parent fidlgen.EncodedCompoundIdentifier, m fidlgen.Method) *method {
	out := &method{
		membership: *newIsMember(s, parent, m.Name, fidlgen.ProtocolDeclType, nil /* default value */),
		method:     m,
	}
	return out
}

// Name implements Element.
func (m *method) Name() Name {
	return m.membership.Name()
}

// Member implements Element.
func (m method) Member() bool {
	return m.membership.Member()
}

func (m *method) Serialize() ElementStr {
	e := m.membership.Serialize()
	e.Kind = ProtocolMemberKind
	if m.method.IsStrict() {
		e.Strictness = isStrict
	} else {
		e.Strictness = isFlexible
	}
	e.Ordinal = Ordinal(fmt.Sprint(m.method.Ordinal))
	if m.method.HasRequest && !m.method.HasResponse {
		e.Direction = isOneWay
		if m.method.RequestPayload != nil {
			e.Request = Type(m.method.RequestPayload.Identifier)
		}
	} else if m.method.HasRequest && m.method.HasResponse {
		e.Direction = isTwoWay
		if m.method.RequestPayload != nil {
			e.Request = Type(m.method.RequestPayload.Identifier)
		}
		if m.method.ResponsePayload != nil {
			if m.method.HasResultUnion() {
				e.Response = m.membership.symbolTable.fidlTypeString(*m.method.ValueType)
				if m.method.HasError {
					e.Error = m.membership.symbolTable.fidlTypeString(*m.method.ErrorType)
				}
			} else {
				e.Response = Type(m.method.ResponsePayload.Identifier)
			}
		}
	} else if !m.method.HasRequest && m.method.HasResponse {
		e.Direction = isEvent
		// TODO(https://fxbug.dev/42156522): This looks like a typo, but it's not. We set
		// e.Request because we're moving towards terminology where "request"
		// means "initial message", not "client-to-server message". But the JSON
		// IR models an event as a method that has a response but no request.
		// We can remove this comment once the JSON IR is changed too.
		if m.method.ResponsePayload != nil {
			e.Request = Type(m.method.ResponsePayload.Identifier)
		}
	} else {
		panic("both HasRequest and HasResponse are false")
	}
	return e
}
