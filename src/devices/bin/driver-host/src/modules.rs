// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::loader::{Library, LoaderService};
use crate::utils::*;
use fdf_component::Incoming;
use std::collections::BTreeMap;
use std::ffi::{CString, c_void};
use std::ptr::NonNull;
use zx::Status;
use {fidl_fuchsia_data as fdata, fidl_fuchsia_driver_framework as fidl_fdf};

#[derive(Debug)]
struct Symbol {
    module_name: String,
    symbol_name: String,
    symbol: NonNull<c_void>,
}

/// SAFETY: Symbols are just static pointers to code and therefore have no thread local state.
unsafe impl Send for Symbol {}

impl Symbol {
    fn new(module_name: String, module: &Library, symbol_name: String) -> Result<Symbol, Status> {
        let symbol_name_cstr =
            CString::new(symbol_name.as_str()).map_err(|_| Status::INVALID_ARGS)?;

        // SAFETY: The symbol is valid as long as the shared library is not closed. So its
        // lifetime must track that of |library| from above. We also do a null check to ensure
        // it's a valid pointer.
        let symbol = unsafe { libc::dlsym(module.ptr.as_ptr(), symbol_name_cstr.as_ptr()) };
        let symbol = NonNull::new(symbol).ok_or(Status::NOT_FOUND)?;
        Ok(Symbol { module_name, symbol_name, symbol })
    }
}

#[derive(Debug, Default)]
pub(crate) struct ModulesAndSymbols {
    modules: Vec<Library>,
    symbols: Vec<Symbol>,
}

impl ModulesAndSymbols {
    pub(crate) async fn load(
        program: &fdata::Dictionary,
        incoming: &Incoming,
    ) -> Result<ModulesAndSymbols, Status> {
        // Step 1: parse out all modules and load them into VMOs.
        struct Module {
            vmo: zx::Vmo,
            overrides: BTreeMap<String, zx::Vmo>,
            symbols: Vec<String>,
        }
        let default = Vec::new();
        let modules = get_program_objvec(program, "modules")?.unwrap_or(&default);
        let mut module_map = BTreeMap::new();

        for module in modules {
            let mut module_name = get_program_string(module, "module_name")?;
            // Special case for compat. The syntax could allow more more generic references to other
            // fields, but we don't need that for now, so we hardcode support for one specific field.
            if module_name == "#program.compat" {
                module_name = get_program_string(program, "compat")?;
            }

            let module_vmo = get_file_vmo(incoming, module_name).await?;

            // Lookup overrides specific to this module.
            let mut override_map = BTreeMap::new();
            let default = Vec::new();
            let overrides = get_program_objvec(module, "loader_overrides")?.unwrap_or(&default);
            for override_ in overrides {
                let from = get_program_string(override_, "from")?;
                let to = get_program_string(override_, "to")?;

                let override_vmo = get_file_vmo(incoming, to).await?;
                override_map.insert(from.to_string(), override_vmo);
            }

            // Lookup symbols specific to this module.
            let symbols = get_program_strvec(module, "symbols")?.ok_or(Status::NOT_FOUND)?;
            module_map.insert(
                module_name.to_string(),
                Module { vmo: module_vmo, overrides: override_map, symbols: symbols.to_vec() },
            );
        }

        // Step 2: dlopen each module and collect relevant symbols from it.
        let mut modules_and_symbols = ModulesAndSymbols::default();
        for (module_name, module) in module_map {
            let module_library = if !module.overrides.is_empty() {
                LoaderService::install(module.overrides).await.try_load(module.vmo, &None).await?
            } else {
                LoaderService::try_load(module.vmo, &None).await?
            };

            // Find symbols.
            for symbol_name in module.symbols {
                let symbol = Symbol::new(module_name.clone(), &module_library, symbol_name)?;
                modules_and_symbols.symbols.push(symbol);
            }

            modules_and_symbols.modules.push(module_library);
        }
        Ok(modules_and_symbols)
    }

    pub(crate) fn copy_to_start_args(&self, start_args: &mut fidl_fdf::DriverStartArgs) {
        let symbols = start_args.symbols.get_or_insert_default();
        for symbol in &self.symbols {
            symbols.push(fidl_fdf::NodeSymbol {
                name: Some(symbol.symbol_name.clone()),
                address: Some(symbol.symbol.addr().get() as u64),
                module_name: Some(symbol.module_name.clone()),
                ..Default::default()
            });
        }
    }
}
