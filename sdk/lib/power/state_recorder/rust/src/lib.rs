// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Standardized reporting of time series data via Inspect and trace. It supports recording of
//! **enum states** and **numeric states**.
//!
//! For example use, see the [example code][strc].
//!
//! For the intro to the library, see the [README.md][rdme].
//!
//! [rdme]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/power/state_recorder/README.md
//! [strc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/power/state_recorder
//!

use fuchsia_inspect::{self as inspect, ArrayProperty, Inspector, Property};
use fuchsia_sync::Mutex;
use fuchsia_trace as ftrace;
use futures_util::FutureExt;
use std::cmp::{Eq, max, min};
pub use std::collections::HashMap;
pub use std::ffi::{CStr, CString};
use std::fmt::{Debug, Display};
use std::fs::{self as fs, OpenOptions};
use std::hash::Hash;
use std::io::Write as OtherWrite;
use std::marker::PhantomData;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};
use strum::IntoEnumIterator;
use zx;

static CSTR_POOL: LazyLock<Mutex<HashMap<String, &'static CStr>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lazily creates &'static CStr values.
///
/// Each reference is backed by a CString value that is leaked (and can thus never be deallocated)
/// to achieve static lifetime. Each value is indexed by its corresponding `str` value, so a given
/// value will only be created once.
///
/// Errors:
///  - StateRecorderError::IncompatibleString: The provided string could not be converted to a
///    CString.
fn lazy_static_cstr(s: &str) -> Result<&'static CStr, StateRecorderError> {
    let mut pool = CSTR_POOL.lock();

    // If the string is already in our pool, return the existing CStr.
    if let Some(existing_cstr) = pool.get(s) {
        return Ok(existing_cstr);
    }

    // Create the CString and leak it in a box to give it static lifetime.
    let c_string = CString::new(s)
        .map_err(|_| StateRecorderError::IncompatibleString(s.to_owned()))?
        .into_boxed_c_str();

    // We are going to leak the `c_string`, which may trip up the LeakSanitizer. So we need to
    // explicitly disable and enable when we're running in the sanitizer variant.
    //
    // Note that the variant is named variant_asan (for AddressSanitizer), but the specific
    // sanitizer we are targeting is lsan (LeakSanitizer), which is enabled as part of the asan
    // variant.
    #[cfg(any(feature = "variant_asan", feature = "variant_hwasan"))]
    fn disable_lsan() {
        unsafe extern "C" {
            fn __lsan_disable();
        }
        unsafe {
            __lsan_disable();
        }
    }

    #[cfg(not(any(feature = "variant_asan", feature = "variant_hwasan")))]
    fn disable_lsan() {}

    #[cfg(any(feature = "variant_asan", feature = "variant_hwasan"))]
    fn enable_lsan() {
        unsafe extern "C" {
            fn __lsan_enable();
        }
        unsafe {
            __lsan_enable();
        }
    }

    #[cfg(not(any(feature = "variant_asan", feature = "variant_hwasan")))]
    fn enable_lsan() {}

    disable_lsan();
    let static_cstr: &'static CStr = Box::leak(c_string);
    enable_lsan();

    pool.insert(s.to_owned(), static_cstr);

    Ok(static_cstr)
}

static ROOT_NODE_NAME: &str = "power_observability_state_recorders";
pub const FORMAT_VERSION: &str = "2.0";

// StateRecorderManager for use with the singleton inspector.
static SINGLETON_MANAGER: LazyLock<Arc<Mutex<StateRecorderManager>>> =
    LazyLock::new(|| StateRecorderManager::new(inspect::component::inspector()));

pub fn manager() -> Arc<Mutex<StateRecorderManager>> {
    SINGLETON_MANAGER.clone()
}

#[derive(thiserror::Error, Debug)]
pub enum StateRecorderError {
    #[error("The name \"{0}\" is already in use")]
    DuplicateName(String),
    #[error("String \"{0}\" cannot be converted to a CString")]
    IncompatibleString(String),
    #[error("Invalid options: {0}")]
    InvalidOptions(String),
}

/// Manages the parent node shared by StateRecorder instances, providing protection against name
/// collisions.
pub struct StateRecorderManager {
    pub node: inspect::Node,
    // Represents a set, but implemented using a Vec due to expected small number of elements.
    names_in_use: Vec<String>,
}

impl StateRecorderManager {
    pub fn new(inspector: &inspect::Inspector) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            node: inspector.root().create_child(ROOT_NODE_NAME),
            names_in_use: Vec::new(),
        }))
    }

    fn register_name(&mut self, name: &str) -> Result<(), StateRecorderError> {
        if self.names_in_use.iter().any(|s| s == name) {
            return Err(StateRecorderError::DuplicateName(name.to_owned()));
        }
        self.names_in_use.push(name.to_owned());
        Ok(())
    }

    fn unregister_name(&mut self, name: &str) {
        match self.names_in_use.iter().position(|s| s == name) {
            Some(index) => {
                self.names_in_use.remove(index);
            }
            None => {
                log::error!("unregister_name called with nonexistent name \"{}\"", name);
            }
        }
    }
}

// Helpers for sharing logic between Recorders
fn register_with_manager(
    manager: &Arc<Mutex<StateRecorderManager>>,
    name: &str,
) -> Result<inspect::Node, StateRecorderError> {
    let mut manager = manager.lock();
    if let Err(e) = manager.register_name(name) {
        return Err(e);
    }
    Ok(manager.node.create_child(name))
}

/// The number of entries per Inspect array shard.
///
/// In Inspect VMO format, the maximum allocation block size is Order 7 (2048 bytes).
/// Subtracting an 8-byte block header and an 8-byte array metadata header leaves 2032 bytes
/// for elements. For 64-bit entries (8 bytes each), a single array block can hold at most
/// 254 slots ((2048 - 16) / 8 = 254). 200 is chosen as a round capacity safely under this limit
/// so that each shard fits within a single max-order Inspect VMO block.
pub const SHARD_CAPACITY: usize = 200;

#[derive(Debug)]
pub enum InspectValueArray {
    Uint(inspect::UintArrayProperty),
    Int(inspect::IntArrayProperty),
    Double(inspect::DoubleArrayProperty),
}

impl InspectValueArray {
    pub fn record_property(self, node: &inspect::Node) {
        match self {
            Self::Uint(p) => node.record(p),
            Self::Int(p) => node.record(p),
            Self::Double(p) => node.record(p),
        }
    }
}

#[derive(Debug)]
pub struct InspectShard {
    _node: inspect::Node,
    pub times: inspect::IntArrayProperty,
    pub values: InspectValueArray,
}

impl InspectShard {
    pub fn new(
        parent_node: &inspect::Node,
        shard_index: usize,
        size: usize,
        create_values: impl FnOnce(&inspect::Node, usize) -> InspectValueArray,
    ) -> Self {
        let node = parent_node.create_child(shard_index.to_string());
        let times = node.create_int_array("times", size);
        let values = create_values(&node, size);
        Self { _node: node, times, values }
    }
}

#[derive(Debug)]
pub struct EagerShardedBuffer {
    _history_node: inspect::Node,
    _shards_node: inspect::Node,
    current_index: inspect::UintProperty,
    current_size: inspect::UintProperty,
    shards: Vec<InspectShard>,
    capacity: usize,
    index_tracker: usize,
    size_tracker: usize,
}

impl EagerShardedBuffer {
    pub fn new(
        parent_node: &inspect::Node,
        capacity: usize,
        create_values: impl Fn(&inspect::Node, usize) -> InspectValueArray,
    ) -> Self {
        let history_node = parent_node.create_child("history");
        let current_index = history_node.create_uint("current_index", 0);
        let current_size = history_node.create_uint("current_size", 0);
        let shards_node = history_node.create_child("shards");

        let active_capacity = max(capacity, 1);
        let total_shards = (active_capacity + SHARD_CAPACITY - 1) / SHARD_CAPACITY;
        let mut shards = Vec::with_capacity(total_shards);

        for s in 0..total_shards {
            let start_idx = s * SHARD_CAPACITY;
            let end_idx = min(start_idx + SHARD_CAPACITY, capacity);
            let shard_size = max(end_idx - start_idx, 1);
            shards
                .push(InspectShard::new(&shards_node, s, shard_size, |n, sz| create_values(n, sz)));
        }

        Self {
            _history_node: history_node,
            _shards_node: shards_node,
            current_index,
            current_size,
            shards,
            capacity,
            index_tracker: 0,
            size_tracker: 0,
        }
    }

    pub fn record<T>(
        &mut self,
        timestamp_ns: i64,
        val: T,
        set_val: impl FnOnce(&InspectValueArray, usize, T),
    ) {
        if self.capacity == 0 {
            return;
        }
        let shard_idx = self.index_tracker / SHARD_CAPACITY;
        let slot_idx = self.index_tracker % SHARD_CAPACITY;

        let shard = &self.shards[shard_idx];
        shard.times.set(slot_idx, timestamp_ns);
        set_val(&shard.values, slot_idx, val);

        self.index_tracker = (self.index_tracker + 1) % self.capacity;
        self.size_tracker = min(self.size_tracker + 1, self.capacity);

        self.current_index.set(self.index_tracker as u64);
        self.current_size.set(self.size_tracker as u64);
    }
}

fn build_sharded_history_inspector<T>(
    items: &[(i64, T)],
    create_values: impl Fn(&inspect::Node, usize) -> InspectValueArray,
    set_value: impl Fn(&InspectValueArray, usize, &T),
) -> Inspector {
    let inspector = Inspector::default();
    let local_root = inspector.root();
    let size = items.len();

    local_root.record_uint("current_index", 0);
    local_root.record_uint("current_size", size as u64);

    let shards_node = local_root.create_child("shards");
    let active_capacity = max(size, 1);
    let num_shards = (active_capacity + SHARD_CAPACITY - 1) / SHARD_CAPACITY;

    for s in 0..num_shards {
        let shard_start_idx = s * SHARD_CAPACITY;
        let shard_end_idx = min(shard_start_idx + SHARD_CAPACITY, size);
        let shard_size =
            if shard_end_idx > shard_start_idx { shard_end_idx - shard_start_idx } else { 0 };

        let shard_node = shards_node.create_child(s.to_string());
        let times_prop = shard_node.create_int_array("times", max(shard_size, 1));
        let values_prop = create_values(&shard_node, max(shard_size, 1));

        for i in 0..shard_size {
            let (ts, val) = &items[shard_start_idx + i];
            times_prop.set(i, *ts);
            set_value(&values_prop, i, val);
        }

        shard_node.record(times_prop);
        values_prop.record_property(&shard_node);
        shards_node.record(shard_node);
    }

    local_root.record(shards_node);
    inspector
}

fn setup_lazy_recording_backend<T, FCreate, FSet>(
    node: &inspect::Node,
    options: &RecorderOptions,
    create_values: FCreate,
    set_value: FSet,
) -> Result<(RecorderHistory<T>, Option<PersistenceHandler<T>>), StateRecorderError>
where
    T: Copy + std::fmt::Debug + std::fmt::Display + std::str::FromStr + Send + Sync + 'static,
    FCreate: Fn(&inspect::Node, usize) -> InspectValueArray + Send + Sync + 'static,
    FSet: Fn(&InspectValueArray, usize, &T) + Send + Sync + Clone + 'static,
{
    let create_values_arc = Arc::new(create_values);

    let shared_buffer = if let Some(config) = &options.persistence {
        let (handler, buffer) = PersistenceHandler::new(config.clone(), options.capacity);

        // Handle Previous Boot Node
        let prev_data = PersistenceHandler::<T>::read_log(&config.previous_path);
        if !prev_data.is_empty() {
            let data_arc = Arc::new(prev_data);
            let create_values = create_values_arc.clone();
            let set_value = set_value.clone();
            node.record_lazy_child("previous_boot_history", move || {
                let data = data_arc.clone();
                let create_values = create_values.clone();
                let set_value = set_value.clone();
                async move {
                    Ok(build_sharded_history_inspector(
                        &data,
                        move |n, s| create_values(n, s),
                        &set_value,
                    ))
                }
                .boxed()
            });
        }
        (Some(handler), buffer)
    } else {
        (None, Arc::new(Mutex::new(TimestampRingBuffer::<T>::with_capacity(options.capacity))))
    };

    // reset_info
    let buffer_cloned = shared_buffer.1.clone();
    node.record_lazy_child("reset_info", move || {
        let history = buffer_cloned.clone();
        async move {
            let inspector = Inspector::default();
            let node = inspector.root();
            let (count, last_reset_ns) = history.lock().get_reset_info();
            node.record_int("count", count as i64);
            node.record_int("last_reset_ns", last_reset_ns);
            Ok(inspector)
        }
        .boxed()
    });

    // history
    let buffer_cloned = shared_buffer.1.clone();
    let create_values = create_values_arc;
    node.record_lazy_child("history", move || {
        let history = buffer_cloned.clone();
        let create_values = create_values.clone();
        let set_value = set_value.clone();
        async move {
            let items: Vec<(i64, T)> = history.lock().iter().collect();
            Ok(build_sharded_history_inspector(&items, move |n, s| create_values(n, s), &set_value))
        }
        .boxed()
    });

    Ok((RecorderHistory::Lazy(shared_buffer.1), shared_buffer.0))
}

fn setup_eager_recording_backend<T, FCreate>(
    node: &inspect::Node,
    options: &RecorderOptions,
    create_values: FCreate,
) -> Result<(RecorderHistory<T>, Option<PersistenceHandler<T>>), StateRecorderError>
where
    T: Copy + std::fmt::Debug,
    FCreate: Fn(&inspect::Node, usize) -> InspectValueArray + Send + Sync + 'static,
{
    if options.persistence.is_some() {
        return Err(StateRecorderError::InvalidOptions(
            "Persistence not supported in eager mode".to_string(),
        ));
    }

    node.record_child("reset_info", |node| {
        node.record_int("count", 0);
        node.record_int("last_reset_ns", zx::BootInstant::get().into_nanos());
    });

    let eager_buffer = EagerShardedBuffer::new(node, options.capacity, create_values);
    Ok((RecorderHistory::Eager(eager_buffer), None))
}

fn setup_recording_backend<T, FCreate, FSet>(
    node: &inspect::Node,
    options: &RecorderOptions,
    create_values: FCreate,
    set_value: FSet,
) -> Result<(RecorderHistory<T>, Option<PersistenceHandler<T>>), StateRecorderError>
where
    T: Copy + std::fmt::Debug + std::fmt::Display + std::str::FromStr + Send + Sync + 'static,
    FCreate: Fn(&inspect::Node, usize) -> InspectValueArray + Send + Sync + 'static,
    FSet: Fn(&InspectValueArray, usize, &T) + Send + Sync + Clone + 'static,
{
    if options.lazy_record {
        setup_lazy_recording_backend(node, options, create_values, set_value)
    } else {
        setup_eager_recording_backend(node, options, create_values)
    }
}

/// Supertrait that combines traits an enum type must satisfy to be compatible with StateRecorder.
pub trait RecordableEnum:
    Copy + Debug + Display + Eq + Hash + IntoEnumIterator + Into<u64> + Send + Sync
{
}
impl<T: Copy + Debug + Display + Eq + Hash + IntoEnumIterator + Into<u64> + Send + Sync>
    RecordableEnum for T
{
}

// To simplify lookups, StateRecorder stores each state name as both CStr (for tracing) and
// String (for Inspect).
#[derive(Clone)]
struct StateName {
    trace_name: &'static CStr,
    // This is wrapped in an Arc so that StateRecorder can clone a reference to it that is separated
    // from a borrow of `self`.
    //
    // The alternative -- while preserving `Send` for StateRecorder -- would be to wrap
    // StateRecorder::trace_state_event and StateRecorder::history in Mutexes.
    inspect_name: Arc<String>,
}

/// Records time series data for an named-u64 value state. This is best-suited for categorical
/// observations, where the name of the state and not a numeric value will be most relevant for
/// diagnostic and forensic purposes.
pub struct NamedU64StateRecorder {
    manager: Arc<Mutex<StateRecorderManager>>,
    name: String,
    trace_category: &'static CStr,
    state_names: HashMap<u64, StateName>,
    history: RecorderHistory<u64>,
    persistence: Option<PersistenceHandler<u64>>,
    _root_node: inspect::Node,
    vthread: ftrace::VThread<String>,
    current_state_trace_name: Option<&'static CStr>,
}

impl std::fmt::Debug for NamedU64StateRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamedU64StateRecorder")
            .field("metadata", &self.name)
            .field("trace_category", &self.trace_category)
            .field("history", &self.history)
            .finish()
    }
}

impl Drop for NamedU64StateRecorder {
    fn drop(&mut self) {
        self.manager.lock().unregister_name(&self.name);
    }
}

impl NamedU64StateRecorder {
    /// Creates a new NamedU64StateRecorder with a given name and a map of u64 to state names.
    ///
    /// See `RecorderOptions` for more details on options that can be specified.
    ///
    /// Errors:
    ///   - StateRecorderError::DuplicateName: `metadata.name` is already in use by a StateRecorder
    ///     associated with `manager`.
    ///   - StateRecorderError::IncompatibleString: Either `name` or the display name of a state
    ///     cannot be converted to a CString.
    ///   - StateRecorderError::InvalidOptions: `options` is invalid for the given mode.
    pub fn new(
        name: String,
        trace_category: &'static CStr,
        state_names_map: HashMap<u64, String>,
        options: RecorderOptions,
    ) -> Result<Self, StateRecorderError> {
        let manager = options.manager.clone().unwrap_or_else(|| SINGLETON_MANAGER.clone());
        let node = register_with_manager(&manager, &name)?;

        // Build up the map of u64 to state names, returning an error if any name is not a valid
        // str.
        let mut state_names = HashMap::new();
        for (value, name_str) in state_names_map {
            let inspect_name = Arc::new(name_str);
            let trace_name = lazy_static_cstr(&inspect_name)?;
            state_names.insert(value, StateName { inspect_name, trace_name });
        }

        node.record_child("metadata", |metadata_node| {
            metadata_node.record_string("format_version", FORMAT_VERSION);
            metadata_node.record_string("name", &name);
            metadata_node.record_string("type", "enum");
            metadata_node.record_child("states", |states_node| {
                for (state_value, state_name) in state_names.iter() {
                    states_node.record_uint(state_name.inspect_name.as_ref(), *state_value);
                }
            });
        });

        let create_values = |n: &inspect::Node, sz: usize| {
            InspectValueArray::Uint(n.create_uint_array("values", sz))
        };
        let set_value = |arr: &InspectValueArray, idx: usize, val: &u64| {
            if let InspectValueArray::Uint(a) = arr {
                a.set(idx, *val);
            }
        };

        let (history, persistence) =
            setup_recording_backend(&node, &options, create_values, set_value)?;

        let vthread = ftrace::VThread::new(name.clone(), ftrace::Id::new().into());

        Ok(Self {
            manager,
            name,
            trace_category,
            state_names,
            history,
            persistence,
            _root_node: node,
            vthread,
            current_state_trace_name: None,
        })
    }

    fn get_state_name(&self, val: u64) -> StateName {
        static UNKNOWN_NAME: LazyLock<StateName> = LazyLock::new(|| StateName {
            trace_name: c"<Unknown>",
            inspect_name: Arc::new("<Unknown>".to_string()),
        });
        self.state_names.get(&val).unwrap_or(&UNKNOWN_NAME).clone()
    }

    pub fn record(&mut self, val: u64) {
        static CACHE: ftrace::trace_site_t = ftrace::trace_site_t::new(0);
        let context = ftrace::TraceCategoryContext::acquire_cached(self.trace_category, &CACHE);

        if let Some(context) = context.as_ref() {
            if let Some(name) = self.current_state_trace_name {
                ftrace::vthread_duration_end(context, &name, &self.vthread, &[]);
            }
        }

        let StateName { trace_name, .. } = self.get_state_name(val);
        self.current_state_trace_name = Some(trace_name);

        if let Some(context) = context.as_ref() {
            ftrace::vthread_duration_begin(context, &trace_name, &self.vthread, &[]);
        }

        let timestamp = zx::BootInstant::get().into_nanos();

        // If Persistence is on (Lazy), the handler OWNS the buffer update.
        if let Some(handler) = &mut self.persistence {
            // Updates the shared buffer inside its lock and handle persistence.
            handler.append(timestamp, val);
        } else {
            // Update manually
            match &mut self.history {
                RecorderHistory::Eager(history) => {
                    history.record(timestamp, val, |arr, idx, v| {
                        if let InspectValueArray::Uint(a) = arr {
                            a.set(idx, v);
                        }
                    });
                }
                RecorderHistory::Lazy(history) => {
                    history.lock().insert(timestamp, val);
                }
            }
        }
    }
}

/// Records time series data for an enum-valued state. This is best-suited for categorical
/// observations, where the name of the state and not a numeric value will be most relevant for
/// diagnostic and forensic purposes.
pub struct EnumStateRecorder<T: RecordableEnum> {
    inner: NamedU64StateRecorder,
    _phantom: PhantomData<T>,
}

impl<T: RecordableEnum> std::fmt::Debug for EnumStateRecorder<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnumStateRecorder").field("inner", &self.inner).finish()
    }
}

impl<T: RecordableEnum + 'static> EnumStateRecorder<T> {
    /// Creates a new EnumStateRecorder with a given name.
    ///
    /// See `RecorderOptions` for more details on options that can be specified.
    ///
    /// Errors:
    ///   - StateRecorderError::DuplicateName: `metadata.name` is already in use by a StateRecorder
    ///     associated with `manager`.
    ///   - StateRecorderError::IncompatibleString: Either `name` or the display name of a state
    ///     cannot be converted to a CString.
    ///   - StateRecorderError::InvalidOptions: `options` is invalid for the given mode.
    pub fn new(
        name: String,
        trace_category: &'static CStr,
        options: RecorderOptions,
    ) -> Result<Self, StateRecorderError> {
        let mut map = HashMap::new();
        for variant in T::iter() {
            map.insert(variant.into(), variant.to_string());
        }
        let inner = NamedU64StateRecorder::new(name, trace_category, map, options)?;

        Ok(Self { inner, _phantom: PhantomData })
    }

    pub fn record(&mut self, state_enum: T) {
        self.inner.record(state_enum.into());
    }
}

/// To be recordable, a numeric type must, in essence, be able to widen into a trace-compatible
/// type and an Inspect-compatible type. Users are not expected to implement this trait; this
/// module implements it for common numeric types below.
pub trait RecordableNumericType:
    Copy + Debug + Display + FromStr + Sized + Send + Sync + 'static
{
    type TraceType: ftrace::ArgValue;

    fn trace_value(&self) -> Self::TraceType;
    fn record(&self, node: &inspect::Node, name: &str);
    fn record_range(range: &(Self, Self), node: &inspect::Node);
    fn create_array_property(node: &inspect::Node, name: &str, size: usize) -> InspectValueArray;
    fn set_array_value(array: &InspectValueArray, index: usize, val: Self);
}

macro_rules! impl_recordable_numeric_type {
    ($numeric_type:ty, $trace_type:ty, u64) => {
        impl RecordableNumericType for $numeric_type {
            type TraceType = $trace_type;

            fn trace_value(&self) -> Self::TraceType {
                *self as Self::TraceType
            }
            fn record(&self, node: &inspect::Node, name: &str) {
                node.record_uint(name, *self as u64);
            }
            fn record_range(range: &(Self, Self), node: &inspect::Node) {
                node.record_uint("min_inc", range.0 as u64);
                node.record_uint("max_inc", range.1 as u64);
            }
            fn create_array_property(
                node: &inspect::Node,
                name: &str,
                size: usize,
            ) -> InspectValueArray {
                InspectValueArray::Uint(node.create_uint_array(name, size))
            }
            fn set_array_value(array: &InspectValueArray, index: usize, val: Self) {
                if let InspectValueArray::Uint(arr) = array {
                    arr.set(index, val as u64);
                }
            }
        }
    };
    ($numeric_type:ty, $trace_type:ty, i64) => {
        impl RecordableNumericType for $numeric_type {
            type TraceType = $trace_type;

            fn trace_value(&self) -> Self::TraceType {
                *self as Self::TraceType
            }
            fn record(&self, node: &inspect::Node, name: &str) {
                node.record_int(name, *self as i64);
            }
            fn record_range(range: &(Self, Self), node: &inspect::Node) {
                node.record_int("min_inc", range.0 as i64);
                node.record_int("max_inc", range.1 as i64);
            }
            fn create_array_property(
                node: &inspect::Node,
                name: &str,
                size: usize,
            ) -> InspectValueArray {
                InspectValueArray::Int(node.create_int_array(name, size))
            }
            fn set_array_value(array: &InspectValueArray, index: usize, val: Self) {
                if let InspectValueArray::Int(arr) = array {
                    arr.set(index, val as i64);
                }
            }
        }
    };
    ($numeric_type:ty, $trace_type:ty, f64) => {
        impl RecordableNumericType for $numeric_type {
            type TraceType = $trace_type;

            fn trace_value(&self) -> Self::TraceType {
                *self as Self::TraceType
            }
            fn record(&self, node: &inspect::Node, name: &str) {
                node.record_double(name, *self as f64);
            }
            fn record_range(range: &(Self, Self), node: &inspect::Node) {
                node.record_double("min_inc", range.0 as f64);
                node.record_double("max_inc", range.1 as f64);
            }
            fn create_array_property(
                node: &inspect::Node,
                name: &str,
                size: usize,
            ) -> InspectValueArray {
                InspectValueArray::Double(node.create_double_array(name, size))
            }
            fn set_array_value(array: &InspectValueArray, index: usize, val: Self) {
                if let InspectValueArray::Double(arr) = array {
                    arr.set(index, val as f64);
                }
            }
        }
    };
}

impl_recordable_numeric_type!(u8, u32, u64);
impl_recordable_numeric_type!(u16, u32, u64);
impl_recordable_numeric_type!(u32, u32, u64);
impl_recordable_numeric_type!(u64, u64, u64);
impl_recordable_numeric_type!(i8, i32, i64);
impl_recordable_numeric_type!(i16, i32, i64);
impl_recordable_numeric_type!(i32, i32, i64);
impl_recordable_numeric_type!(i64, i64, i64);
impl_recordable_numeric_type!(f32, f64, f64);
impl_recordable_numeric_type!(f64, f64, f64);

/// Units supported by NumericStateRecorder. The `units!` macro is recommended for construction.
///
/// Bytes and bit-rates are specifically not included yet because they invite the question of
/// whether they should be restricted to binary prefixes. We'll address that once we instrument a
/// specific use case.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Units {
    Amps(Option<DecimalPrefix>),
    AmpHours(Option<DecimalPrefix>),
    Hertz(Option<DecimalPrefix>),
    Joules(Option<DecimalPrefix>),
    Seconds(Option<DecimalPrefix>),
    Watts(Option<DecimalPrefix>),
    Volts(Option<DecimalPrefix>),
    Celsius(Option<DecimalPrefix>),
    Number(Option<DecimalPrefix>),
    Percent,
}

impl Display for Units {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn write_helper(
            f: &mut std::fmt::Formatter<'_>,
            prefix: &Option<DecimalPrefix>,
            unit_str: &str,
        ) -> std::fmt::Result {
            match prefix {
                Some(p) => write!(f, "{}{}", p, unit_str),
                None => write!(f, "{}", unit_str),
            }
        }

        match self {
            Units::Amps(prefix) => write_helper(f, prefix, "A"),
            Units::AmpHours(prefix) => write_helper(f, prefix, "AH"),
            Units::Hertz(prefix) => write_helper(f, prefix, "Hz"),
            Units::Joules(prefix) => write_helper(f, prefix, "J"),
            Units::Seconds(prefix) => write_helper(f, prefix, "s"),
            Units::Watts(prefix) => write_helper(f, prefix, "W"),
            Units::Volts(prefix) => write_helper(f, prefix, "V"),
            Units::Celsius(prefix) => write_helper(f, prefix, "C"),
            Units::Number(prefix) => write_helper(f, prefix, "#"),
            Units::Percent => write!(f, "%"),
        }
    }
}

/// Decimal prefixes for use with certain `Units`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DecimalPrefix {
    Nano,
    Micro,
    Milli,
    Centi,
    Deci,
    Kilo,
    Mega,
    Giga,
}

impl Display for DecimalPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecimalPrefix::Nano => write!(f, "n"),
            DecimalPrefix::Micro => write!(f, "u"),
            DecimalPrefix::Milli => write!(f, "m"),
            DecimalPrefix::Centi => write!(f, "c"),
            DecimalPrefix::Deci => write!(f, "d"),
            DecimalPrefix::Kilo => write!(f, "k"),
            DecimalPrefix::Mega => write!(f, "M"),
            DecimalPrefix::Giga => write!(f, "G"),
        }
    }
}

/// Assembles fully-specified measurement units for NumericStateRecorder, combining a base unit
/// with an optional prefix.
///
/// Examples:
///     - units!(Volt)
///     - units!(Percent)
///     - units!(Kilo, Hertz)
///     - units!(Milli, Amp)
#[macro_export]
macro_rules! units {
    (Percent) => {
        $crate::Units::Percent
    };
    ($base_unit:ident) => {
        $crate::Units::$base_unit(None)
    };
    ($prefix:ident, $base_unit:ident) => {
        $crate::Units::$base_unit(Some($crate::DecimalPrefix::$prefix))
    };
}

// Holds information for persistence
#[derive(Clone, Debug)]
pub struct PersistenceOptions {
    /// Unique name for this recorder (e.g., "battery_level").
    name: String,
    /// For current history.
    current_path: String,
    /// For previous history.
    previous_path: String,
    // A temporary name for current history to achieve atomic persistence.
    rename_path: String,
}

impl PersistenceOptions {
    // Unique name and path to storage and path to volatile directory.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            current_path: format!("/data/{}.csv", name),
            previous_path: format!("/tmp/{}.csv", name),
            rename_path: format!("/data/{}.tmp", name),
            name,
        }
    }

    pub fn storage_dir(mut self, dir: &str) -> Self {
        self.current_path = format!("{}/{}.csv", dir, self.name);
        self.rename_path = format!("{}/{}.tmp", dir, self.name);
        self
    }

    pub fn volatile_dir(mut self, dir: &str) -> Self {
        self.previous_path = format!("{}/{}.csv", dir, self.name);
        self
    }

    // Helper to generate paths
    fn paths(&self) -> (&str, &str, &str) {
        (&self.current_path, &self.previous_path, &self.rename_path)
    }
}

/// Handles persistence using TimestampRingBuffer as the backing store to save memory.
struct PersistenceHandler<T: Copy> {
    config: PersistenceOptions,
    // We reuse TimestampRingBuffer for memory-optimized storage (i32 offsets)
    buffer: Arc<Mutex<TimestampRingBuffer<T>>>,
}

impl<T: Copy + FromStr + Display> PersistenceHandler<T> {
    fn new(
        config: PersistenceOptions,
        capacity: usize,
    ) -> (Self, Arc<Mutex<TimestampRingBuffer<T>>>) {
        let (curr, prev, _) = config.paths();

        // Perform rotation before loading data
        Self::prepare_files(&curr, &prev);

        // Load any data remaining in 'current' (crash recovery)
        let initial_data = Self::read_log(&curr);

        // 3. Hydrate our internal ring buffer
        let mut buffer = TimestampRingBuffer::with_capacity(capacity);
        for (ts, val) in initial_data {
            buffer.insert(ts, val);
        }

        let shared_buffer = Arc::new(Mutex::new(buffer));

        (Self { config, buffer: shared_buffer.clone() }, shared_buffer)
    }

    /// Handles the rotation logic:
    /// If PREV doesn't exist (reboot), move CURR to PREV.
    /// If PREV exists (crash), leave CURR alone (it contains valuable pre-crash data).
    fn prepare_files(curr_path: &str, prev_path: &str) {
        if Path::new(prev_path).exists() {
            // Previous file exists -> Crash recovery.
            // Do not overwrite it. Do not touch current file.
            log::warn!("Not moving history, {} already exists", prev_path);
            return;
        }

        // Move content by reading then writing it for moves from /data to /tmp.
        let Ok(content) = std::fs::read_to_string(curr_path).map_err(|e| {
            log::info!("Could not read current history from {}, not moving: {}", curr_path, e);
        }) else {
            return;
        };

        if let Err(e) = std::fs::write(prev_path, &content) {
            log::warn!("Could not write previous boot history to {}: {}", prev_path, e);
            return;
        }

        if let Err(e) = std::fs::File::create(curr_path) {
            log::warn!("Could not clear current boot history at {}: {}", curr_path, e);
        }
    }

    fn flush(&self, buffer_guard: &TimestampRingBuffer<T>) {
        let (curr, _, temp) = self.config.paths();
        let try_write = || -> std::io::Result<()> {
            let mut file =
                OpenOptions::new().write(true).create(true).truncate(true).open(&temp)?;

            // Iterate the ring buffer (which converts internal 32-bit offsets back to 64-bit TS)
            for (ts, val) in buffer_guard.iter() {
                writeln!(file, "{},{}", ts, val)?;
            }

            file.sync_data()?;
            fs::rename(&temp, &curr)?;
            Ok(())
        };

        if let Err(e) = try_write() {
            log::error!("StateRecorder: Persist failed for {}: {:?}", self.config.name, e);
        }
    }

    /// Appends data to memory and syncs to disk.
    fn append(&mut self, timestamp: i64, value: T) {
        let mut guard = self.buffer.lock();
        guard.insert(timestamp, value);
        self.flush(&guard);
    }

    /// Static helper to read log from disk into a vector.
    fn read_log(path: &str) -> Vec<(i64, T)> {
        let Ok(content) = fs::read_to_string(path) else {
            return Vec::new();
        };
        content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let mut parts = line.splitn(2, ',');
                let ts = parts.next()?.trim().parse::<i64>().ok()?;
                let val = parts.next()?.trim().parse::<T>().ok()?;
                Some((ts, val))
            })
            .collect()
    }
}

/// Options for NumericStateRecorder and EnumStateRecorder
#[derive(Default)]
pub struct RecorderOptions {
    // If true, recorder will lazily record values to inspect. Otherwise, will record eagerly.
    pub lazy_record: bool,
    /// Maximum number of recorded values to store on a rolling basis.
    pub capacity: usize,
    /// Optional. If not set, the Recorder will be linked to this module's singleton
    /// StateRecorderManager, which in turn corresponds to the singleton Inspector.
    /// If set, the manager supplied here will be used.
    pub manager: Option<Arc<Mutex<StateRecorderManager>>>,
    // Optional persistence config
    pub persistence: Option<PersistenceOptions>,
}

#[derive(Debug)]
enum RecorderHistory<T: Copy + Debug> {
    Eager(EagerShardedBuffer),
    Lazy(Arc<Mutex<TimestampRingBuffer<T>>>),
}

#[derive(Debug)]
/// A fixed-size ring buffer with timestamps for each insertion.
/// All input and output are in nanoseconds, but will be rounded down to
/// the nearest millisecond and stored as milliseconds internally.
/// When the capacity is reached, insertions will wrap around and continue
/// from the beginning of the buffer. There is a maximum delta of ~24.8 days
/// between insertions. If this maximum is exceeded, the buffer will drop
/// all data except for the newest insertion.
struct TimestampRingBuffer<T: Copy> {
    /// Initial timestamp in milliseconds, used as basis for offsets.
    start_timestamp_ms: i64,
    /// Last timestamp inserted, in milliseconds.
    last_timestamp_ms: i64,
    /// Index where the next element should be inserted.
    next_index: usize,
    /// Store timestamps as millisecond offsets from `last_timestamp_ms`.
    offset_ms: Vec<i32>,
    /// Data to be stored in the buffer.
    data: Vec<T>,
    /// Number of times the buffer has been reset (due to max delta exceeded).
    reset_count: u32,
    /// Timestamp of the last buffer reset
    last_reset_ms: i64,
}

const NANOSECONDS_PER_MILLISECOND: i64 = 1_000_000;

fn ms_to_ns(ms: i64) -> i64 {
    ms * NANOSECONDS_PER_MILLISECOND
}

fn ns_to_ms(ns: i64) -> i64 {
    ns / NANOSECONDS_PER_MILLISECOND
}

impl<T: Copy> TimestampRingBuffer<T> {
    fn with_capacity(capacity: usize) -> Self {
        let now_ms = ns_to_ms(zx::BootInstant::get().into_nanos());
        Self {
            start_timestamp_ms: now_ms,
            last_timestamp_ms: now_ms,
            next_index: 0,
            offset_ms: Vec::with_capacity(capacity),
            data: Vec::with_capacity(capacity),
            reset_count: 0,
            last_reset_ms: now_ms,
        }
    }

    fn insert(&mut self, timestamp_ns: i64, value: T) {
        if self.offset_ms.capacity() == 0 {
            return;
        }
        let timestamp_ms = ns_to_ms(timestamp_ns);
        // Attempt to down-convert the offset from last_timestamp_ms to an i32
        let offset_ms = match i32::try_from(timestamp_ms - self.last_timestamp_ms) {
            Ok(offset_ms) => offset_ms,
            Err(_) => {
                // Offset from last_timestamp_ms exceeds maximum allowable,
                // reset the buffer.
                self.offset_ms.clear();
                self.data.clear();
                self.start_timestamp_ms = timestamp_ms;
                self.next_index = 0;
                self.reset_count += 1;
                self.last_reset_ms = self.start_timestamp_ms;
                0
            }
        };
        if self.offset_ms.len() < self.offset_ms.capacity() {
            // Buffer isn't full yet, just append.
            self.offset_ms.push(offset_ms);
            self.data.push(value);
        } else {
            // Buffer is full, shift `start_timestamp_ms` forward by the oldest
            // offset, then overwrite that entry with the new data.
            self.start_timestamp_ms += self.offset_ms[self.next_index] as i64;
            self.offset_ms[self.next_index] = offset_ms;
            self.data[self.next_index] = value;
        }
        self.last_timestamp_ms = timestamp_ms;
        self.next_index = (self.next_index + 1) % self.offset_ms.capacity();
    }

    /// Returns the reset count, and the timestamp of the last reset in nanoseconds.
    fn get_reset_info(&self) -> (u32, i64) {
        (self.reset_count, ms_to_ns(self.last_reset_ms))
    }

    /// Returns an Iterator of (timestamp in nanoseconds, T), starting
    /// from the oldest entry.
    fn iter(&self) -> TimestampRingBufferIter<'_, T> {
        TimestampRingBufferIter::new(self)
    }
}

struct TimestampRingBufferIter<'a, T: Copy> {
    buffer: &'a TimestampRingBuffer<T>,
    index: usize,
    last_timestamp_ms: i64,
}

impl<'a, T: Copy> TimestampRingBufferIter<'a, T> {
    fn new(buffer: &'a TimestampRingBuffer<T>) -> Self {
        Self { buffer, index: 0, last_timestamp_ms: buffer.start_timestamp_ms }
    }
}

/// Iterate over the wrapped buffer, returning (timestamp in nanoseconds, T),
/// starting from the oldest entry.
impl<T: Copy> Iterator for TimestampRingBufferIter<'_, T> {
    type Item = (i64, T);

    fn next(&mut self) -> Option<(i64, T)> {
        if self.index >= self.buffer.offset_ms.len() {
            return None;
        }
        // Start from the oldest insertion and wrap around.
        let index = (self.index + self.buffer.next_index) % self.buffer.offset_ms.len();
        let timestamp_ms = self.last_timestamp_ms + self.buffer.offset_ms[index] as i64;
        self.index += 1;
        self.last_timestamp_ms = timestamp_ms;
        Some((ms_to_ns(timestamp_ms), self.buffer.data[index]))
    }
}

pub struct NumericStateRecorder<T: RecordableNumericType> {
    manager: Arc<Mutex<StateRecorderManager>>,
    name: String,
    trace_category: &'static CStr,
    trace_name: &'static CStr,
    units: String,
    history: RecorderHistory<T>,
    persistence: Option<PersistenceHandler<T>>,
    _root_node: inspect::Node,
    trace_id: ftrace::Id,
    _phantom: PhantomData<T>,
}

impl<T: RecordableNumericType> NumericStateRecorder<T> {
    /// Creates a new NumericStateRecorder.
    ///
    /// See `RecorderOptions` for more details on options that can be specified.
    ///
    /// Errors:
    ///   - StateRecorderError::DuplicateName: `metadata.name` is already in use by a StateRecorder
    ///     associated with `manager`.
    ///   - StateRecorderError::IncompatibleString: Either `name` or the display name of a state
    ///     cannot be converted to a CString.
    ///   - StateRecorderError::InvalidOptions: `options` is invalid for the given mode.
    pub fn new(
        name: String,
        trace_category: &'static CStr,
        units: Units,
        range: Option<(T, T)>,
        options: RecorderOptions,
    ) -> Result<Self, StateRecorderError> {
        let manager = options.manager.clone().unwrap_or_else(|| SINGLETON_MANAGER.clone());
        let node = register_with_manager(&manager, &name)?;

        let trace_name = lazy_static_cstr(&name)?;
        let units_str = format!("{}", units);

        node.record_child("metadata", |metadata_node| {
            metadata_node.record_string("format_version", FORMAT_VERSION);
            metadata_node.record_string("name", &name);
            metadata_node.record_string("type", "numeric");
            metadata_node.record_string("units", &units_str);
            match range {
                Some(r) => metadata_node.record_child("range", |node| T::record_range(&r, node)),
                None => metadata_node.record_string("range", "<Unspecified>"),
            }
        });

        let create_values =
            |n: &inspect::Node, sz: usize| T::create_array_property(n, "values", sz);
        let set_value = |arr: &InspectValueArray, idx: usize, val: &T| {
            T::set_array_value(arr, idx, *val);
        };

        let (history, persistence) =
            setup_recording_backend(&node, &options, create_values, set_value)?;

        Ok(Self {
            manager,
            name,
            trace_category,
            trace_name,
            units: units_str,
            history,
            persistence,
            _root_node: node,
            trace_id: ftrace::Id::new(),
            _phantom: PhantomData,
        })
    }

    pub fn record(&mut self, state_value: T) {
        let timestamp = zx::BootInstant::get().into_nanos();

        ftrace::counter!(
            self.trace_category,
            self.trace_name,
            self.trace_id.into(),
            &self.units.to_string() => state_value.trace_value()
        );

        // If Persistence is on (Lazy), the handler OWNS the shared buffer update.
        if let Some(handler) = &mut self.persistence {
            handler.append(timestamp, state_value);
        } else {
            match &mut self.history {
                RecorderHistory::Eager(history) => {
                    history.record(timestamp, state_value, |arr, idx, val| {
                        T::set_array_value(arr, idx, val);
                    });
                }
                RecorderHistory::Lazy(history) => {
                    history.lock().insert(timestamp, state_value);
                }
            }
        }
    }
}

impl<T: RecordableNumericType> Drop for NumericStateRecorder<T> {
    fn drop(&mut self) {
        self.manager.lock().unregister_name(&self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyIntProperty, AnyProperty, assert_data_tree};
    use fuchsia_inspect::Inspector;
    use strum_macros::{Display, EnumIter, EnumString};
    use test_case::test_case;

    #[derive(Copy, Clone, Debug, Display, EnumIter, EnumString, Eq, PartialEq, Hash)]
    #[repr(u8)]
    enum SwitchState {
        OFF = 0,
        ON = 1,
    }

    impl From<SwitchState> for u64 {
        fn from(value: SwitchState) -> Self {
            value as Self
        }
    }

    #[fuchsia::test]
    async fn test_timestamp_ring_buffer() {
        let mut buffer = TimestampRingBuffer::<i32>::with_capacity(3);
        let start_ms = buffer.start_timestamp_ms;

        let t1 = (ms_to_ns(start_ms + 1000), 1);
        // t2's timestamp is before t1, which will result in a negative offset.
        let t2 = (ms_to_ns(start_ms + 900), 2);
        let t3 = (ms_to_ns(start_ms + 3000), 3);

        buffer.insert(t1.0, t1.1);
        buffer.insert(t2.0, t2.1);
        buffer.insert(t3.0, t3.1);

        assert_eq!(vec![t1, t2, t3], buffer.iter().collect::<Vec<_>>());
        assert_eq!((0, ms_to_ns(start_ms)), buffer.get_reset_info());

        // Buffer is already at capacity, so this should overwrite the first element.
        let t4 = (ms_to_ns(start_ms + 4000), 4);
        buffer.insert(t4.0, t4.1);
        assert_eq!(vec![t2, t3, t4], buffer.iter().collect::<Vec<_>>());
        assert_eq!((0, ms_to_ns(start_ms)), buffer.get_reset_info());
    }

    #[fuchsia::test]
    async fn test_timestamp_ring_buffer_resets_on_maximum_offset() {
        let mut buffer = TimestampRingBuffer::<i32>::with_capacity(3);
        let start_ms = buffer.start_timestamp_ms;

        const MAX_OFFSET_MS: i64 = i32::MAX as i64;
        let t1 = (ms_to_ns(start_ms + 1000), 1);
        let t2 = (t1.0 + ms_to_ns(MAX_OFFSET_MS), 2);

        buffer.insert(t1.0, t1.1);
        buffer.insert(t2.0, t2.1);

        assert_eq!(vec![t1, t2], buffer.iter().collect::<Vec<_>>());
        assert_eq!((0, ms_to_ns(start_ms)), buffer.get_reset_info());

        // This should exceed the maximum allowable timestamp offset,
        // causing the buffer to reset.
        let t3 = (t2.0 + ms_to_ns(MAX_OFFSET_MS + 1), 3);
        buffer.insert(t3.0, t3.1);
        assert_eq!(vec![t3], buffer.iter().collect::<Vec<_>>());
        assert_eq!((1, t3.0), buffer.get_reset_info());
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_enum_off_on(lazy_record: bool) {
        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        let mut recorder = EnumStateRecorder::new(
            "my_switch".into(),
            c"power_test",
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager),
                persistence: None,
            },
        )
        .unwrap();

        recorder.record(SwitchState::OFF);
        recorder.record(SwitchState::ON);
        recorder.record(SwitchState::OFF);
        recorder.record(SwitchState::ON);
        if lazy_record {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_switch: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_switch",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 4u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![0u64, 1u64, 0u64, 1u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_switch: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_switch",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 4u64,
                            current_size: 4u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![0u64, 1u64, 0u64, 1u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_multiple_recorders(lazy_record: bool) {
        #[derive(Copy, Clone, Debug, Display, EnumIter, EnumString, Eq, PartialEq, Hash)]
        #[repr(u8)]
        enum EnablementState {
            DISABLED = 0,
            ENABLED = 1,
        }
        impl From<EnablementState> for u64 {
            fn from(value: EnablementState) -> Self {
                value as Self
            }
        }

        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        let mut recorder_0 = EnumStateRecorder::new(
            "switch_0".into(),
            c"power_test",
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager.clone()),
                persistence: None,
            },
        )
        .unwrap();
        let mut recorder_1 = EnumStateRecorder::new(
            "switch_1".into(),
            c"power_test",
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager),
                persistence: None,
            },
        )
        .unwrap();
        recorder_0.record(SwitchState::OFF);
        recorder_0.record(SwitchState::ON);
        recorder_1.record(EnablementState::ENABLED);
        recorder_1.record(EnablementState::DISABLED);

        if lazy_record {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    switch_0: {
                        metadata: {
                            format_version: "2.0",
                            name: "switch_0",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![0u64, 1u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    },
                    switch_1: {
                        metadata: {
                            format_version: "2.0",
                            name: "switch_1",
                            type: "enum",
                            states: {
                                "DISABLED": 0u64,
                                "ENABLED": 1u64,
                            }
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![1u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    switch_0: {
                        metadata: {
                            format_version: "2.0",
                            name: "switch_0",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 2u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![0u64, 1u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    },
                    switch_1: {
                        metadata: {
                            format_version: "2.0",
                            name: "switch_1",
                            type: "enum",
                            states: {
                                "DISABLED": 0u64,
                                "ENABLED": 1u64,
                            }
                        },
                        history: {
                            current_index: 2u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![1u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_enum_three_states(lazy_record: bool) {
        #[derive(Copy, Clone, Debug, Display, EnumIter, EnumString, Eq, PartialEq, Hash)]
        #[repr(u8)]
        enum FanSpeed {
            OFF = 0,
            LOW = 1,
            HIGH = 2,
        }

        impl From<FanSpeed> for u64 {
            fn from(value: FanSpeed) -> Self {
                value as Self
            }
        }

        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        let mut recorder = EnumStateRecorder::new(
            "the_best_fan".into(),
            c"power_test",
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager),
                persistence: None,
            },
        )
        .unwrap();

        recorder.record(FanSpeed::OFF);
        recorder.record(FanSpeed::LOW);
        recorder.record(FanSpeed::HIGH);
        recorder.record(FanSpeed::OFF);
        recorder.record(FanSpeed::HIGH);
        if lazy_record {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    the_best_fan: {
                        metadata: {
                            format_version: "2.0",
                            name: "the_best_fan",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "LOW": 1u64,
                                "HIGH": 2u64,
                            }
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 5u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![0u64, 1u64, 2u64, 0u64, 2u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    the_best_fan: {
                        metadata: {
                            format_version: "2.0",
                            name: "the_best_fan",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "LOW": 1u64,
                                "HIGH": 2u64,
                            }
                        },
                        history: {
                            current_index: 5u64,
                            current_size: 5u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![0u64, 1u64, 2u64, 0u64, 2u64, 0u64, 0u64, 0u64, 0u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_name_reuse_not_allowed(lazy_record: bool) {
        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        let recorder = EnumStateRecorder::<SwitchState>::new(
            "my_switch".into(),
            c"power_test",
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager.clone()),
                persistence: None,
            },
        )
        .unwrap();

        // While `recorder` is still in scope, its name cannot be reused.
        let result = EnumStateRecorder::<SwitchState>::new(
            "my_switch".into(),
            c"power_test",
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager.clone()),
                persistence: None,
            },
        );
        assert!(result.is_err());

        // After `recorder` is dropped, its name can be used again.
        drop(recorder);
        let result = EnumStateRecorder::<SwitchState>::new(
            "my_switch".into(),
            c"power_test",
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager.clone()),
                persistence: None,
            },
        );
        assert!(result.is_ok());
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_zero_capacity(lazy_record: bool) {
        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);
        let mut recorder = EnumStateRecorder::<SwitchState>::new(
            "zero_cap_switch".into(),
            c"power_test",
            RecorderOptions { lazy_record, capacity: 0, manager: Some(manager), persistence: None },
        )
        .unwrap();

        // Recording with 0 capacity should not panic.
        recorder.record(SwitchState::OFF);
        recorder.record(SwitchState::ON);
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_multi_shard_and_wraparound(lazy_record: bool) {
        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);
        let capacity = SHARD_CAPACITY + 50;
        let mut recorder = EnumStateRecorder::<SwitchState>::new(
            "multi_shard_switch".into(),
            c"power_test",
            RecorderOptions { lazy_record, capacity, manager: Some(manager), persistence: None },
        )
        .unwrap();

        // 1. Record 220 items to cross SHARD_CAPACITY (200) into the second shard.
        for i in 0..220 {
            let state = if i % 2 == 0 { SwitchState::OFF } else { SwitchState::ON };
            recorder.record(state);
        }

        let expected_shard_0: Vec<u64> = (0..SHARD_CAPACITY).map(|i| (i % 2) as u64).collect();
        let expected_shard_1: Vec<u64> = (SHARD_CAPACITY..220).map(|i| (i % 2) as u64).collect();

        if lazy_record {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    multi_shard_switch: {
                        metadata: {
                            format_version: "2.0",
                            name: "multi_shard_switch",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 220u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: expected_shard_0.clone(),
                                },
                                "1": {
                                    times: AnyProperty,
                                    values: expected_shard_1.clone(),
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            let mut expected_eager_shard_1 = expected_shard_1.clone();
            expected_eager_shard_1.resize(50, 0u64);
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    multi_shard_switch: {
                        metadata: {
                            format_version: "2.0",
                            name: "multi_shard_switch",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 220u64,
                            current_size: 220u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: expected_shard_0.clone(),
                                },
                                "1": {
                                    times: AnyProperty,
                                    values: expected_eager_shard_1,
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }

        // 2. Record items 220..300 (total 300 into capacity 250) to trigger ring buffer wraparound.
        for i in 220..300 {
            let state = if i % 2 == 0 { SwitchState::OFF } else { SwitchState::ON };
            recorder.record(state);
        }

        if lazy_record {
            let lazy_shard_0: Vec<u64> = (50..250).map(|i| (i % 2) as u64).collect();
            let lazy_shard_1: Vec<u64> = (250..300).map(|i| (i % 2) as u64).collect();
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    multi_shard_switch: {
                        metadata: {
                            format_version: "2.0",
                            name: "multi_shard_switch",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 250u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: lazy_shard_0,
                                },
                                "1": {
                                    times: AnyProperty,
                                    values: lazy_shard_1,
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            let mut eager_shard_0: Vec<u64> = Vec::with_capacity(SHARD_CAPACITY);
            for i in 250..300 {
                eager_shard_0.push((i % 2) as u64);
            }
            for i in 50..200 {
                eager_shard_0.push((i % 2) as u64);
            }
            let eager_shard_1: Vec<u64> = (200..250).map(|i| (i % 2) as u64).collect();

            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    multi_shard_switch: {
                        metadata: {
                            format_version: "2.0",
                            name: "multi_shard_switch",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 50u64,
                            current_size: 250u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: eager_shard_0,
                                },
                                "1": {
                                    times: AnyProperty,
                                    values: eager_shard_1,
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_singleton_manager(lazy_record: bool) {
        let mut recorder = EnumStateRecorder::new(
            "my_switch".into(),
            c"power_test",
            RecorderOptions { lazy_record, capacity: 10, manager: None, persistence: None },
        )
        .unwrap();

        recorder.record(SwitchState::OFF);
        recorder.record(SwitchState::ON);
        if lazy_record {
            assert_data_tree!(inspect::component::inspector(), root: {
                power_observability_state_recorders: {
                    my_switch: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_switch",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![0u64, 1u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            assert_data_tree!(inspect::component::inspector(), root: {
                power_observability_state_recorders: {
                    my_switch: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_switch",
                            type: "enum",
                            states: {
                                "OFF": 0u64,
                                "ON": 1u64,
                            }
                        },
                        history: {
                            current_index: 2u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![0u64, 1u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[fuchsia::test]
    async fn test_recorder_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<EnumStateRecorder<SwitchState>>();
    }

    async fn test_uint_numeric_type<T: RecordableNumericType>(lazy_record: bool)
    where
        T: Into<u64> + From<u8>,
    {
        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);
        let mut recorder = NumericStateRecorder::new(
            "my_stateful_thing".into(),
            c"power_test",
            units!(Percent),
            Some((T::from(0), T::from(255))),
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager),
                persistence: None,
            },
        )
        .unwrap();

        recorder.record(T::from(10));
        recorder.record(T::from(0));
        if lazy_record {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_stateful_thing: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_stateful_thing",
                            type: "numeric",
                            units: "%",
                            range: {
                                min_inc: 0u64,
                                max_inc: 255u64
                            },
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![10u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_stateful_thing: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_stateful_thing",
                            type: "numeric",
                            units: "%",
                            range: {
                                min_inc: 0u64,
                                max_inc: 255u64
                            },
                        },
                        history: {
                            current_index: 2u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![10u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_uint_numeric_types(lazy_record: bool) {
        test_uint_numeric_type::<u8>(lazy_record).await;
        test_uint_numeric_type::<u16>(lazy_record).await;
        test_uint_numeric_type::<u32>(lazy_record).await;
        test_uint_numeric_type::<u64>(lazy_record).await;
    }

    async fn test_int_numeric_type<T: RecordableNumericType>(lazy_record: bool)
    where
        T: Into<i64> + From<i8>,
    {
        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);
        let mut recorder = NumericStateRecorder::new(
            "my_stateful_thing".into(),
            c"power_test",
            units!(Number),
            Some((T::from(-128), T::from(127))),
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager),
                persistence: None,
            },
        )
        .unwrap();

        recorder.record(T::from(10));
        recorder.record(T::from(0));
        if lazy_record {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_stateful_thing: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_stateful_thing",
                            type: "numeric",
                            units: "#",
                            range: {
                                min_inc: -128i64,
                                max_inc: 127i64
                            },
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![10i64, 0i64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_stateful_thing: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_stateful_thing",
                            type: "numeric",
                            units: "#",
                            range: {
                                min_inc: -128i64,
                                max_inc: 127i64
                            },
                        },
                        history: {
                            current_index: 2u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![10i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_int_numeric_types(lazy_record: bool) {
        test_int_numeric_type::<i8>(lazy_record).await;
        test_int_numeric_type::<i16>(lazy_record).await;
        test_int_numeric_type::<i32>(lazy_record).await;
        test_int_numeric_type::<i64>(lazy_record).await;
    }

    async fn test_float_numeric_type<T: RecordableNumericType>(lazy_record: bool)
    where
        T: Into<f64> + From<u8>,
    {
        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);
        let mut recorder = NumericStateRecorder::new(
            "my_stateful_thing".into(),
            c"power_test",
            units!(Kilo, Hertz),
            Some((T::from(0), T::from(255))),
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager),
                persistence: None,
            },
        )
        .unwrap();

        recorder.record(T::from(10));
        recorder.record(T::from(0));
        if lazy_record {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_stateful_thing: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_stateful_thing",
                            type: "numeric",
                            units: "kHz",
                            range: {
                                min_inc: 0.0,
                                max_inc: 255.0
                            },
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![10.0f64, 0.0f64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_stateful_thing: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_stateful_thing",
                            type: "numeric",
                            units: "kHz",
                            range: {
                                min_inc: 0.0,
                                max_inc: 255.0
                            },
                        },
                        history: {
                            current_index: 2u64,
                            current_size: 2u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![10.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_float_numeric_types(lazy_record: bool) {
        test_float_numeric_type::<f32>(lazy_record).await;
        test_float_numeric_type::<f64>(lazy_record).await;
    }

    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_persistence_crash_recovery(lazy_record: bool) {
        use std::fs;
        use tempfile::tempdir;

        // 1. Setup isolated environment
        let dir = tempdir().unwrap();
        let storage_path = dir.path().join("data");
        let volatile_path = dir.path().join("tmp");
        fs::create_dir(&storage_path).unwrap();
        fs::create_dir(&volatile_path).unwrap();

        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        // Helper to generate options
        let create_options = |manager_ref| RecorderOptions {
            lazy_record, // Passed from test_case argument
            capacity: 10,
            manager: Some(manager_ref),
            persistence: Some(
                PersistenceOptions::new("crash_test".to_string())
                    .storage_dir(storage_path.to_str().unwrap())
                    .volatile_dir(volatile_path.to_str().unwrap()),
            ),
        };

        // 2. START RECORDER 1 (Fill data)
        {
            let mut recorder = EnumStateRecorder::<SwitchState>::new(
                "crash_test".into(),
                c"power_test",
                create_options(manager.clone()),
            )
            .unwrap();

            recorder.record(SwitchState::ON);
            recorder.record(SwitchState::OFF);

            // Scope ends, data is persisted to disk
        }

        // Verify disk content
        let curr_csv = storage_path.join("crash_test.csv");
        let content = fs::read_to_string(curr_csv).unwrap();
        // Should contain integer values (ON=1, OFF=0) in order
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2, "Expected 2 lines of recorded history");

        // First record: ON (1)
        let parts0: Vec<&str> = lines[0].split(',').collect();
        assert_eq!(parts0.len(), 2, "Invalid CSV format in line 1");
        assert_eq!(parts0[1], "1", "First record should be ON (1)");

        // Second record: OFF (0)
        let parts1: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(parts1.len(), 2, "Invalid CSV format in line 2");
        assert_eq!(parts1[1], "0", "Second record should be OFF (0)");

        // 3. FORCE "CRASH" STATE
        // Create 'Previous' file so library thinks this is a crash restart, not a reboot.
        // This forces it to READ from storage_path without overwriting it.
        let prev_csv = volatile_path.join("crash_test.csv");
        fs::write(&prev_csv, "").unwrap();

        // 4. START RECORDER 2 (Simulate Restart)
        // This triggers hydration from disk into (Lazy: Dynamic nodes) or (Eager: EagerShardedBuffer)
        let mut recorder_restarted = EnumStateRecorder::<SwitchState>::new(
            "crash_test".into(),
            c"power_test",
            create_options(manager),
        )
        .unwrap();

        // ASSERTIONS
        assert_data_tree!(inspector, root: {
            power_observability_state_recorders: {
                crash_test: {
                    metadata: {
                        format_version: "2.0",
                        name: "crash_test",
                        type: "enum",
                        states: {
                            "OFF": 0u64,
                            "ON": 1u64,
                        }
                    },
                    history: {
                        current_index: 0u64,
                        current_size: 2u64,
                        shards: {
                            "0": {
                                times: AnyProperty,
                                values: vec![1u64, 0u64],
                            }
                        }
                    },
                    reset_info: {
                        count: 0i64, // Matches both lazy (casted i64) and eager (0 literal)
                        last_reset_ns: AnyIntProperty,
                    },
                }
            }
        });

        // 5. RECORD NEW DATA
        recorder_restarted.record(SwitchState::ON);
        assert_data_tree!(inspector, root: {
            power_observability_state_recorders: {
                crash_test: {
                    metadata: {
                        format_version: "2.0",
                        name: "crash_test",
                        type: "enum",
                        states: {
                            "OFF": 0u64,
                            "ON": 1u64,
                        }
                    },
                    history: {
                        current_index: 0u64,
                        current_size: 3u64,
                        shards: {
                            "0": {
                                times: AnyProperty,
                                values: vec![1u64, 0u64, 1u64],
                            }
                        }
                    },
                    reset_info: {
                        count: 0i64,
                        last_reset_ns: AnyIntProperty,
                    },
                }
            }
        });
    }

    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_persistence_reboot(lazy_record: bool) {
        use std::fs;
        use tempfile::tempdir;

        // 1. Setup isolated environment
        let dir = tempdir().unwrap();
        let storage_path = dir.path().join("data");
        let volatile_path = dir.path().join("tmp");
        fs::create_dir(&storage_path).unwrap();
        fs::create_dir(&volatile_path).unwrap();

        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        // Helper to generate options pointing to our temp dirs
        let create_options = |manager_ref| RecorderOptions {
            lazy_record,
            capacity: 10,
            manager: Some(manager_ref),
            persistence: Some(
                PersistenceOptions::new("reboot_test".to_string())
                    .storage_dir(storage_path.to_str().unwrap())
                    .volatile_dir(volatile_path.to_str().unwrap()),
            ),
        };

        // 2. SIMULATE FRESH REBOOT STATE
        // - "Current" file exists in persistent storage (saved from previous run).
        // - "Previous" file in volatile storage is MISSING (cleared by OS reboot).
        let curr_csv = storage_path.join("reboot_test.csv");
        // Write raw CSV data simulating timestamps 1000 and 2000 with integers (ON=1, OFF=0)
        fs::write(&curr_csv, "1000,1\n2000,0\n").unwrap();

        // Ensure volatile file doesn't exist (simulating clean /tmp)
        let prev_csv = volatile_path.join("reboot_test.csv");
        assert!(!prev_csv.exists());

        // 3. START RECORDER (Trigger Logic)
        let mut recorder = EnumStateRecorder::<SwitchState>::new(
            "reboot_test".into(),
            c"power_test",
            create_options(manager),
        )
        .unwrap();

        // 4. VERIFY FILESYSTEM (Rotation)
        // The file should have been moved from 'data' to 'tmp'.
        assert!(prev_csv.exists(), "Library should have rotated curr -> prev");
        assert!(curr_csv.exists(), "Library should have create a new current file");

        let rotated_content = fs::read_to_string(&prev_csv).unwrap();
        assert_eq!(rotated_content, "1000,1\n2000,0\n");

        // 5. ASSERTIONS (Inspect)
        assert_data_tree!(inspector, root: {
            power_observability_state_recorders: {
                reboot_test: {
                    metadata: {
                        format_version: "2.0",
                        name: "reboot_test",
                        type: "enum",
                        states: {
                            "OFF": 0u64,
                            "ON": 1u64,
                        }
                    },
                    // DATA FROM FILE IS HERE (Read Only / Static)
                    previous_boot_history: {
                        current_index: 0u64,
                        current_size: 2u64,
                        shards: {
                            "0": {
                                times: vec![1000i64, 2000i64],
                                values: vec![1u64, 0u64],
                            }
                        }
                    },
                    // ACTIVE HISTORY IS EMPTY (Fresh start)
                    history: {
                        current_index: 0u64,
                        current_size: 0u64,
                        shards: {
                            "0": {
                                times: vec![0i64],
                                values: vec![0u64],
                            }
                        }
                    },
                    reset_info: {
                        count: 0i64,
                        last_reset_ns: AnyIntProperty,
                    },
                }
            }
        });

        // 6. RECORD NEW DATA AFTER REBOOT
        recorder.record(SwitchState::ON);
        recorder.record(SwitchState::OFF);
        assert_data_tree!(inspector, root: {
            power_observability_state_recorders: {
                reboot_test: {
                    metadata: {
                        format_version: "2.0",
                        name: "reboot_test",
                        type: "enum",
                        states: {
                            "OFF": 0u64,
                            "ON": 1u64,
                        }
                    },
                    // DATA FROM FILE IS HERE (Read Only / Static)
                    previous_boot_history: {
                        current_index: 0u64,
                        current_size: 2u64,
                        shards: {
                            "0": {
                                times: vec![1000i64, 2000i64],
                                values: vec![1u64, 0u64],
                            }
                        }
                    },
                    // ACTIVE HISTORY IS NOW POPULATED WITH NEW DATA
                    history: {
                        current_index: 0u64,
                        current_size: 2u64,
                        shards: {
                            "0": {
                                times: AnyProperty,
                                values: vec![1u64, 0u64],
                            }
                        }
                    },
                    reset_info: {
                        count: 0i64,
                        last_reset_ns: AnyIntProperty,
                    },
                }
            }
        });
    }

    #[test_case(false; "eager")]
    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_named_u64_recorder(lazy_record: bool) {
        use std::fs;
        use tempfile::tempdir;

        // Setup isolated persistence environment
        let dir = tempdir().unwrap();
        let storage_path = dir.path().join("data");
        let volatile_path = dir.path().join("tmp");
        fs::create_dir(&storage_path).unwrap();
        fs::create_dir(&volatile_path).unwrap();

        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        let mut map = HashMap::new();
        map.insert(100, "Hundred".to_string());
        map.insert(200, "TwoHundred".to_string());

        let persistence_opts = if lazy_record {
            Some(
                PersistenceOptions::new("my_u64_metrics_p".to_string())
                    .storage_dir(storage_path.to_str().unwrap())
                    .volatile_dir(volatile_path.to_str().unwrap()),
            )
        } else {
            None
        };

        // 1. Start Recorder and Record Data
        let mut recorder = NamedU64StateRecorder::new(
            "my_u64_metrics_p".into(),
            c"power_test",
            map.clone(),
            RecorderOptions {
                lazy_record,
                capacity: 10,
                manager: Some(manager.clone()),
                persistence: persistence_opts.clone(),
            },
        )
        .unwrap();

        recorder.record(100);
        recorder.record(200);
        recorder.record(300); // Unknown

        // 2. Verify Persistence (Lazy mode only)
        if lazy_record {
            drop(recorder); // Drop to ensure flush and release name

            let curr_csv = storage_path.join("my_u64_metrics_p.csv");
            let content = fs::read_to_string(&curr_csv).unwrap();
            let lines: Vec<&str> = content.trim().lines().collect();
            assert_eq!(lines.len(), 3, "Expected 3 lines of recorded history");

            // First record: 100
            let parts0: Vec<&str> = lines[0].split(',').collect();
            assert_eq!(parts0.len(), 2, "Invalid CSV format in line 1");
            assert_eq!(parts0[1], "100", "First record should be 100");

            // Second record: 200
            let parts1: Vec<&str> = lines[1].split(',').collect();
            assert_eq!(parts1.len(), 2, "Invalid CSV format in line 2");
            assert_eq!(parts1[1], "200", "Second record should be 200");

            // Third record: 300
            let parts2: Vec<&str> = lines[2].split(',').collect();
            assert_eq!(parts2.len(), 2, "Invalid CSV format in line 3");
            assert_eq!(parts2[1], "300", "Third record should be 300");

            // 3. Restart Recorder (Simulate Reboot)
            let mut _recorder_restarted = NamedU64StateRecorder::new(
                "my_u64_metrics_p".into(),
                c"power_test",
                map,
                RecorderOptions {
                    lazy_record,
                    capacity: 10,
                    manager: Some(manager),
                    persistence: persistence_opts,
                },
            )
            .unwrap();

            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_u64_metrics_p: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_u64_metrics_p",
                            type: "enum",
                            states: {
                                "Hundred": 100u64,
                                "TwoHundred": 200u64,
                            }
                        },
                        previous_boot_history: {
                            current_index: 0u64,
                            current_size: 3u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![100u64, 200u64, 300u64],
                                }
                            }
                        },
                        history: {
                            current_index: 0u64,
                            current_size: 0u64,
                            shards: {
                                "0": {
                                    times: vec![0i64],
                                    values: vec![0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        } else {
            // Eager mode
            // Recorder IS ALIVE here, so node exists.
            assert_data_tree!(inspector, root: {
                power_observability_state_recorders: {
                    my_u64_metrics_p: {
                        metadata: {
                            format_version: "2.0",
                            name: "my_u64_metrics_p",
                            type: "enum",
                            states: {
                                "Hundred": 100u64,
                                "TwoHundred": 200u64,
                            }
                        },
                        history: {
                            current_index: 3u64,
                            current_size: 3u64,
                            shards: {
                                "0": {
                                    times: AnyProperty,
                                    values: vec![100u64, 200u64, 300u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64],
                                }
                            }
                        },
                        reset_info: {
                            count: 0,
                            last_reset_ns: AnyIntProperty,
                        }
                    }
                }
            });
        }
    }

    #[test_case(true; "lazy")]
    #[fuchsia::test]
    async fn test_numeric_persistence_reboot(lazy_record: bool) {
        use std::fs;
        use tempfile::tempdir;

        // 1. Setup isolated persistence environment
        let dir = tempdir().unwrap();
        let storage_path = dir.path().join("data");
        let volatile_path = dir.path().join("tmp");
        fs::create_dir(&storage_path).unwrap();
        fs::create_dir(&volatile_path).unwrap();

        let inspector = Inspector::default();
        let manager = StateRecorderManager::new(&inspector);

        let create_options = |manager_ref| RecorderOptions {
            lazy_record,
            capacity: 10,
            manager: Some(manager_ref),
            persistence: Some(
                PersistenceOptions::new("num_reboot_test".to_string())
                    .storage_dir(storage_path.to_str().unwrap())
                    .volatile_dir(volatile_path.to_str().unwrap()),
            ),
        };

        // 2. SIMULATE FRESH REBOOT STATE
        // - "Current" file exists (saved from previous run).
        // - "Previous" file in volatile is MISSING.
        let curr_csv = storage_path.join("num_reboot_test.csv");
        // Write raw CSV data: time,value
        fs::write(&curr_csv, "1000,42\n2000,100\n").unwrap();

        let prev_csv = volatile_path.join("num_reboot_test.csv");
        assert!(!prev_csv.exists());

        // 3. START RECORDER
        let mut _recorder = NumericStateRecorder::new(
            "num_reboot_test".into(),
            c"power_test",
            units!(Number),
            Some((0u64, 200u64)),
            create_options(manager),
        )
        .unwrap();

        // 4. VERIFY FILESYSTEM (Rotation)
        // The file should have been moved from 'data' to 'tmp'.
        assert!(prev_csv.exists(), "Library should have rotated curr -> prev");
        assert!(curr_csv.exists(), "Library should have create a new current file");

        let rotated_content = fs::read_to_string(&prev_csv).unwrap();
        assert_eq!(rotated_content, "1000,42\n2000,100\n");

        // 5. ASSERTIONS (Inspect)
        assert_data_tree!(inspector, root: {
            power_observability_state_recorders: {
                num_reboot_test: {
                    metadata: {
                        format_version: "2.0",
                        name: "num_reboot_test",
                        type: "numeric",
                        units: "#",
                        range: {
                            min_inc: 0u64,
                            max_inc: 200u64,
                        }
                    },
                    previous_boot_history: {
                        current_index: 0u64,
                        current_size: 2u64,
                        shards: {
                            "0": {
                                times: vec![1000i64, 2000i64],
                                values: vec![42u64, 100u64],
                            }
                        }
                    },
                    history: {
                        current_index: 0u64,
                        current_size: 0u64,
                        shards: {
                            "0": {
                                times: vec![0i64],
                                values: vec![0u64],
                            }
                        }
                    },
                    reset_info: {
                        count: 0i64,
                        last_reset_ns: AnyIntProperty,
                    }
                }
            }
        });
    }
}
