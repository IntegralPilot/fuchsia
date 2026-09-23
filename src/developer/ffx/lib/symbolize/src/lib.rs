// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! Library for symbolizing addresses from Fuchsia programs.

mod global_init;

use bitflags::bitflags;
use ffx_config::EnvironmentContext;
use fuchsia_sync::Mutex;
use std::os::raw::{c_char, c_void};
use std::ptr::NonNull;

/// A symbolizer for program counters.
#[derive(Debug)]
pub struct Symbolizer {
    next_id: u64,

    /// Owned pointer to the C++ symbolizer.
    inner: NonNull<symbolizer_sys::symbolizer_SymbolizerImpl>,

    // Ensure curl is initialized for any periods where this process instance may be downloading
    // symbols.
    _global_init: global_init::GlobalInitHandle,
}

static SYMBOLIZER_LOCK: Mutex<()> = Mutex::new(());

impl Symbolizer {
    /// Create a new symbolizer instance with a specific ffx context. Normally only needed in tests.
    pub fn with_context(context: &EnvironmentContext) -> Result<Self, CreateSymbolizerError> {
        symbol_index::ensure_symbol_index_registered(&context)?;

        let _global_init = global_init::GlobalInitHandle::new();

        let _guard = SYMBOLIZER_LOCK.lock();

        // SAFETY: basic FFI call without invariants.
        let inner = NonNull::new(unsafe { symbolizer_sys::symbolizer_new() })
            .expect("symbolizer pointer must have been allocated");

        Ok(Self { next_id: 0, inner, _global_init })
    }

    /// Reset the symbolizer, clear cached module and mapping information.
    pub fn reset(&mut self) {
        unsafe {
            symbolizer_sys::symbolizer_reset(self.inner.as_ptr());
        }
    }

    /// Add a new module from the process, returning a unique ID that can be used to associate
    /// multiple mappings with the same module.
    pub fn add_module(&mut self, name: &str, build_id: &[u8]) -> ModuleId {
        let build_id_str = hex::encode(build_id);
        let id = ModuleId(self.next_id);
        self.next_id += 1;

        // SAFETY: self.inner was allocated by symbolizer_new and has not been freed. The pointers
        // derived from name and build_id_str are live and valid for the lengths passed.
        unsafe {
            symbolizer_sys::symbolizer_add_module(
                self.inner.as_ptr(),
                id.0,
                name.as_ptr().cast::<c_char>(),
                name.len(),
                build_id_str.as_ptr().cast::<c_char>(),
                build_id_str.len(),
            );
        }

        id
    }

    /// Add a new mapping for a given module in the process.
    pub fn add_mapping(
        &mut self,
        id: ModuleId,
        details: MappingDetails,
    ) -> Result<(), AddMappingError> {
        let flags_str = details.flags.to_string();

        // SAFETY: self.inner was allocated by symbolizer_new and has not been freed, and the
        // pointers to flags_str are valid for the duration of this call.
        let status = unsafe {
            symbolizer_sys::symbolizer_add_mapping(
                self.inner.as_ptr(),
                id.0,
                details.start_addr,
                details.size,
                details.vaddr,
                flags_str.as_ptr().cast::<c_char>(),
                flags_str.len(),
            )
        };

        match status {
            symbolizer_sys::MappingStatus_Ok => Ok(()),
            symbolizer_sys::MappingStatus_InconsistentBaseAddress => {
                Err(AddMappingError::InconsistentBaseAddress)
            }
            symbolizer_sys::MappingStatus_InvalidModuleId => Err(AddMappingError::InvalidModuleId),
            unknown => panic!(
                "Bindings to symbolizer library are out of sync, unknown error code {unknown}"
            ),
        }
    }

    /// Resolve a single address.
    pub fn resolve_addr(&self, addr: u64) -> Result<Vec<ResolvedLocation>, ResolveError> {
        struct LocationCallbackContext {
            locations: Vec<ResolvedLocation>,
        }

        let mut context = LocationCallbackContext { locations: vec![] };

        /// Callback for collecting the locations resolved by the symbolizer. Can't be a closure
        /// because it needs to be passed as a function pointer to C which uses an explicit callback
        /// argument instead of implicitly including the context in the closure.
        ///
        /// # Safety
        ///
        /// `context` must be a unique pointer to `context` on the stack above. May only be called
        /// inside `Symbolizer::resolve_addr`.
        unsafe extern "C" fn location_callback(
            location: *const symbolizer_sys::symbolizer_location_t,
            context: *mut c_void,
        ) {
            // SAFETY: provided by the safety contract of the function
            let context: &mut LocationCallbackContext =
                unsafe { (context as *mut LocationCallbackContext).as_mut().unwrap() };

            // SAFETY: the C++ side of the callback interface guarantees that the location pointer
            // meets the safety requirements of this function.
            if let Some(resolved_location) = unsafe { ResolvedLocation::from_raw(location) } {
                context.locations.push(resolved_location)
            };
        }

        // SAFETY: self.inner was allocated by process_new and has not been freed. The provided
        // callback will not mutate the provided locations or dereference null pointers.
        let result = unsafe {
            symbolizer_sys::symbolizer_resolve_address(
                self.inner.as_ptr(),
                addr,
                Some(location_callback),
                &mut context as *mut _ as *mut c_void,
            )
        };

        match result {
            symbolizer_sys::ResolveAddressStatus_Ok => {
                if !context.locations.is_empty() {
                    Ok(context.locations)
                } else {
                    Err(ResolveError::SymbolNotFound)
                }
            }
            symbolizer_sys::ResolveAddressStatus_NoOverlappingModule => {
                Err(ResolveError::NoOverlappingModule)
            }
            symbolizer_sys::ResolveAddressStatus_SymbolFileUnavailable => {
                Err(ResolveError::SymbolFileUnavailable)
            }
            _ => unreachable!("all the cases should have already been handled"),
        }
    }
}

impl Drop for Symbolizer {
    fn drop(&mut self) {
        // SAFETY: the pointer was allocated by symbolizer_new and has not been freed.
        unsafe { symbolizer_sys::symbolizer_free(self.inner.as_mut()) }
    }
}

/// Errors that can occur when creating the symbolizer.
#[derive(Debug, thiserror::Error)]
pub enum CreateSymbolizerError {
    /// No global environment context available.
    #[error("ffx couldn't find a global environment context.")]
    NoFfxEnvironmentContext,

    /// Couldn't retrieve an SDK for ffx.
    #[error("ffx couldn't find an SDK to use: {}", _0)]
    NoSdkAvailable(#[source] anyhow::Error),

    /// Couldn't register a symbol index.
    #[error("ffx couldn't register the symbol index: {}", _0)]
    SymbolIndexRegistration(#[from] symbol_index::SymbolIndexError),
}

/// Errors that can occur when adding mappings.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AddMappingError {
    /// A module's mapping information got out of sync in the symbolizer.
    #[error("Provided mapping does not have consistent start/offset.")]
    InconsistentBaseAddress,

    /// A mapping couldn't be added because it referenced a module the symbolizer didn't understand.
    #[error("Invalid module ID provided, likely from a different Symbolizer instance.")]
    InvalidModuleId,
}

/// Errors that can occur when resolving addresses.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ResolveError {
    /// The C++ side did not populate any results at all for an address. Usually caused by an
    /// address not overlapping with any mappings.
    #[error("Provided address does not correspond to a mapped module.")]
    NoOverlappingModule,

    /// The C++ side did not find a function at the given address.
    #[error("Provided address does not correspond to a function.")]
    SymbolNotFound,

    /// The symbol file that would provide debug information about the requested address could not
    /// be loaded.
    #[error("Symbol file is not available.")]
    SymbolFileUnavailable,
}

/// A single source location resolved from an address.
#[derive(Clone, PartialEq)]
pub struct ResolvedLocation {
    /// The function name of the location.
    pub function: String,
    /// The source file for the referenced function.
    pub file_and_line: Option<(String, u32)>,
    /// The name of the library (if any) in which this location is found.
    pub library: Option<String>,
    /// The offset within the library's file where this location is found.
    pub library_offset: u64,
}

impl ResolvedLocation {
    /// # Safety
    ///
    /// `raw` must point to a live, correctly aligned, and fully-initialized instance of a
    /// `symbolizer_location_t`.
    ///
    /// Each of the non-null pointers on the pointed-to `symbolizer_location_t` must be valid to
    /// read from for their respective `${POINTER_NAME}_len` bytes.
    unsafe fn from_raw(raw: *const symbolizer_sys::symbolizer_location_t) -> Option<Self> {
        // SAFETY: provided by the safety contract of the function
        let raw = unsafe { raw.as_ref().unwrap() };

        // SAFETY: provided by the safety contract of the function
        let function = unsafe { string_from_pointer_and_len(raw.function, raw.function_len) }?;

        // SAFETY: provided by the safety contract of the function
        let file_and_line =
            unsafe { string_from_pointer_and_len(raw.file, raw.file_len) }.map(|f| (f, raw.line));

        // SAFETY: provided by the safety contract of the function
        let library = unsafe { string_from_pointer_and_len(raw.library, raw.library_len) };

        Some(Self { function, file_and_line, library, library_offset: raw.library_offset })
    }
}

/// # Safety
///
/// If `bytes` is not null, it must be valid to read for `len` bytes.
unsafe fn string_from_pointer_and_len(bytes: *const c_char, len: usize) -> Option<String> {
    if bytes.is_null() {
        None
    } else {
        // SAFETY: this is safe per the contract of the function, and it's safe to interpret i8 as
        // u8.
        let bytes = unsafe { std::slice::from_raw_parts(bytes as *const u8, len) };
        Some(String::from_utf8_lossy(bytes).to_string())
    }
}

impl std::fmt::Debug for ResolvedLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedLocation")
            .field("function", &self.function)
            .field("file_and_line", &self.file_and_line)
            .field("library", &self.library)
            .field("library_offset", &format_args!("0x{:x}", self.library_offset))
            .finish()
    }
}

/// The ID of a module in the resolver, used to uniquely identify its mappings when symbolizing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct ModuleId(u64);

/// Details of a particular module's mapping. Each module is often mapped multiple times, once for
/// each PT_LOAD segment.
#[derive(Clone, PartialEq)]
pub struct MappingDetails {
    /// The start of the mapping in the process' address space.
    pub start_addr: u64,
    /// The size of the mapping in bytes.
    pub size: u64,
    /// The p_vaddr value of the mapping, usually the offset of the mapping within the source file.
    pub vaddr: u64,
    /// Readable/writeable/executable flags.
    pub flags: MappingFlags,
}

impl std::fmt::Debug for MappingDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MappingDetails")
            .field("start_addr", &format_args!("0x{:x}", self.start_addr))
            .field("size", &format_args!("0x{:x}", self.size))
            .field("vaddr", &format_args!("0x{:x}", self.vaddr))
            .field("flags", &self.flags)
            .finish()
    }
}

bitflags! {
    /// Flags for a module's mapping.
    #[derive(Debug, PartialEq, Clone)]
    pub struct MappingFlags: u32 {
        /// The mapping is readable.
        const READ = 0b1;
        /// The mapping is writeable.
        const WRITE = 0b10;
        /// The mapping is executable.
        const EXECUTE = 0b100;
    }
}

impl std::fmt::Display for MappingFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.contains(Self::READ) {
            write!(f, "r")?;
        }
        if self.contains(Self::WRITE) {
            write!(f, "w")?;
        }
        if self.contains(Self::EXECUTE) {
            write!(f, "x")?;
        }
        Ok(())
    }
}

/// Errors that can occur when setting mapping flags.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MappingFlagsError {
    /// A string cannot be transferred into valid MappingFlags because it contain charaters outside of rwx.
    #[error("Invalid mapping flags: {:?} provided, only r, w or x are supported.", .0)]
    InvalidFlagsInput(String),
}

impl std::str::FromStr for MappingFlags {
    type Err = MappingFlagsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut flags = MappingFlags::empty();

        for c in s.chars() {
            match c {
                'r' => flags.insert(MappingFlags::READ),
                'w' => flags.insert(MappingFlags::WRITE),
                'x' => flags.insert(MappingFlags::EXECUTE),
                _ => return Err(MappingFlagsError::InvalidFlagsInput(s.to_string())),
            }
        }

        Ok(flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_get_mappingflags_from_str() {
        let input = "rw";
        let flags = MappingFlags::from_str(input).unwrap();
        assert_eq!(flags, MappingFlags::READ | MappingFlags::WRITE);
    }

    #[test]
    fn test_invalid_get_mappingflags_from_str() {
        let input = "wrong";
        let flags = MappingFlags::from_str(input);
        assert_eq!(flags, Err(MappingFlagsError::InvalidFlagsInput("wrong".to_string())));
    }
}
