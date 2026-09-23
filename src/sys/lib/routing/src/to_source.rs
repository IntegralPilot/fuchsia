// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust::{
    DebugRegistration, ExposeDecl, ExposeDeclCommon, NativeIntoFidl, OfferDecl, OfferDeclCommon,
    ResolverRegistration, RunnerRegistration, UseDecl, UseDeclCommon,
};
use fidl_fuchsia_component_decl as fdecl;

pub trait ToSource {
    fn to_source(&self) -> fdecl::Ref;
}

impl ToSource for UseDecl {
    fn to_source(&self) -> fdecl::Ref {
        self.source().clone().native_into_fidl()
    }
}

impl ToSource for OfferDecl {
    fn to_source(&self) -> fdecl::Ref {
        self.source().clone().native_into_fidl()
    }
}

impl ToSource for ExposeDecl {
    fn to_source(&self) -> fdecl::Ref {
        self.source().clone().native_into_fidl()
    }
}

impl ToSource for DebugRegistration {
    fn to_source(&self) -> fdecl::Ref {
        let DebugRegistration::Protocol(debug) = self;
        debug.source.clone().native_into_fidl()
    }
}

impl ToSource for RunnerRegistration {
    fn to_source(&self) -> fdecl::Ref {
        self.source.clone().native_into_fidl()
    }
}

impl ToSource for ResolverRegistration {
    fn to_source(&self) -> fdecl::Ref {
        self.source.clone().native_into_fidl()
    }
}
