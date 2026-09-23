// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;
use std::collections::HashMap;

use fidl_ir::HandleSubtype;

#[derive(Deserialize)]
pub struct Config {
    #[serde(default)]
    pub is_common: bool,
    pub common_lib: Option<String>,
    pub emit_debug_impls: bool,
    pub encode_trait_path: String,
    pub decode_trait_path: String,
    pub crate_prefix: String,
    pub resource_bindings: ResourceBindings,
}

#[derive(Deserialize)]
pub struct HandleResourceBinding {
    wire_path_template: String,
    optional_wire_path_template: String,
    natural_path_template: String,
}

impl HandleResourceBinding {
    fn handle_subtype_name(subtype: HandleSubtype) -> &'static str {
        match subtype {
            HandleSubtype::None => "NullableHandle",
            HandleSubtype::Process => "Process",
            HandleSubtype::Thread => "Thread",
            HandleSubtype::Vmo => "Vmo",
            HandleSubtype::Channel => "Channel",
            HandleSubtype::Event => "Event",
            HandleSubtype::Port => "Port",
            HandleSubtype::Interrupt => "Interrupt",
            HandleSubtype::PciDevice => "PciDevice",
            HandleSubtype::DebugLog => "DebugLog",
            HandleSubtype::Socket => "Socket",
            HandleSubtype::Resource => "Resource",
            HandleSubtype::EventPair => "EventPair",
            HandleSubtype::Job => "Job",
            HandleSubtype::Vmar => "Vmar",
            HandleSubtype::Fifo => "Fifo",
            HandleSubtype::Guest => "Guest",
            HandleSubtype::Vcpu => "Vcpu",
            HandleSubtype::Timer => "Timer",
            HandleSubtype::Iommu => "Iommu",
            HandleSubtype::Bti => "Bti",
            HandleSubtype::Profile => "Profile",
            HandleSubtype::Pmt => "Pmt",
            HandleSubtype::Pager => "Pager",
            HandleSubtype::Exception => "Exception",
            HandleSubtype::Clock => "Clock",
            HandleSubtype::Stream => "Stream",
            HandleSubtype::Iob => "Iob",
            HandleSubtype::Counter => "Counter",
            // These handle subtypes do not have newtypes in zx yet. Generate them as handles.
            HandleSubtype::SuspendToken | HandleSubtype::Msi => "NullableHandle",
        }
    }

    pub fn wire_path(&self, subtype: HandleSubtype) -> String {
        self.wire_path_template.replace("{subtype}", Self::handle_subtype_name(subtype))
    }

    pub fn optional_wire_path(&self, subtype: HandleSubtype) -> String {
        self.optional_wire_path_template.replace("{subtype}", Self::handle_subtype_name(subtype))
    }

    pub fn natural_path(&self, subtype: HandleSubtype) -> String {
        self.natural_path_template.replace("{subtype}", Self::handle_subtype_name(subtype))
    }
}

#[derive(Deserialize)]
pub struct ResourceBinding {
    pub wire_path: String,
    pub optional_wire_path: String,
    pub natural_path: String,
}

#[derive(Deserialize)]
pub struct ResourceBindings {
    // Maps resource identifier (e.g. "zx/Handle") to binding types.
    handles: HashMap<String, HandleResourceBinding>,
    // The resource identifier to use for unrecognized handles.
    default_handle: String,
    // Maps protocol transport (e.g. "Channel") to binding types.
    endpoints: HashMap<String, ResourceBinding>,
    // The protocol transport to use for unrecognized transports.
    default_endpoint: String,
}

impl ResourceBindings {
    pub fn handle(&self, handle: &str) -> &HandleResourceBinding {
        self.handles.get(handle).or_else(|| self.handles.get(&self.default_handle)).unwrap()
    }

    pub fn endpoint(&self, transport: &str) -> &ResourceBinding {
        self.endpoints
            .get(transport)
            .or_else(|| self.endpoints.get(&self.default_endpoint))
            .unwrap()
    }
}
