use std::{
    collections::HashMap,
    ffi::{CString, c_char, c_double, c_float, c_int, c_ulong, c_void},
    path::{Path, PathBuf},
};

use libloading::Library;
use polar_h10_core::AccSample;

use crate::{
    CustomFormulaConfig, MetricSpec, SourcePalette, VERNIER_BREATHING_OUTLET_KEY,
    VERNIER_RAW_OUTLET_KEY, VernierMiniSelection, VernierStreamSchema, custom_output_stream_name,
    encode_vernier_raw_rows, output_stream_name,
    provenance::{PolarRespirationProvenance, VernierBreathingProvenance},
    vernier_breathing_stream_name, vernier_raw_stream_name,
};
use vernier_gdx_core::{SampleEncoding, SensorSamples};

const MINI_POLAR_COMBINED_KEY: &str = "__mini_polar_combined";
const MINI_VERNIER_COMBINED_KEY: &str = "__mini_vernier_combined";
const MINI_STATUS_KEY: &str = "__mini_signal_status";
const MINI_POLAR_FIXED_CHANNELS: usize = 7;

type StreamInfo = *mut c_void;
type Outlet = *mut c_void;
type XmlElement = *mut c_void;
type CreateStreamInfo = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    c_double,
    c_int,
    *const c_char,
) -> StreamInfo;
type DestroyStreamInfo = unsafe extern "C" fn(StreamInfo);
type CreateOutlet = unsafe extern "C" fn(StreamInfo, c_int, c_int) -> Outlet;
type DestroyOutlet = unsafe extern "C" fn(Outlet);
type PushSample = unsafe extern "C" fn(Outlet, *const c_float, c_double, c_int) -> c_int;
type PushSampleDouble = unsafe extern "C" fn(Outlet, *const c_double, c_double, c_int) -> c_int;
type PushChunk = unsafe extern "C" fn(Outlet, *const c_float, c_ulong, c_double, c_int) -> c_int;
type LocalClock = unsafe extern "C" fn() -> c_double;
type GetDescription = unsafe extern "C" fn(StreamInfo) -> XmlElement;
type AppendChild = unsafe extern "C" fn(XmlElement, *const c_char) -> XmlElement;
type AppendChildValue =
    unsafe extern "C" fn(XmlElement, *const c_char, *const c_char) -> XmlElement;

struct LslApi {
    _library: Library,
    create_streaminfo: CreateStreamInfo,
    destroy_streaminfo: DestroyStreamInfo,
    create_outlet: CreateOutlet,
    destroy_outlet: DestroyOutlet,
    push_sample: PushSample,
    push_sample_double: PushSampleDouble,
    push_chunk: Option<PushChunk>,
    local_clock: LocalClock,
    get_description: Option<GetDescription>,
    append_child: Option<AppendChild>,
    append_child_value: Option<AppendChildValue>,
}

// liblsl documents outlets as usable across threads. Function pointers remain
// valid because their dynamic Library is owned by the same value.
unsafe impl Send for LslApi {}

impl LslApi {
    fn load(bundled_library: Option<&Path>) -> Result<Self, String> {
        let mut errors = Vec::new();
        let mut candidates = bundled_library
            .map(Path::to_path_buf)
            .into_iter()
            .collect::<Vec<_>>();
        candidates.extend(
            ["liblsl.so", "liblsl.dylib", "lsl.dll"]
                .into_iter()
                .map(PathBuf::from),
        );
        for candidate in candidates {
            // SAFETY: The library is retained for the lifetime of every symbol.
            match unsafe { Library::new(&candidate) } {
                Ok(library) => {
                    // SAFETY: Names and signatures are from liblsl's stable C API.
                    unsafe {
                        let create_streaminfo = *library
                            .get::<CreateStreamInfo>(b"lsl_create_streaminfo\0")
                            .map_err(|error| error.to_string())?;
                        let destroy_streaminfo = *library
                            .get::<DestroyStreamInfo>(b"lsl_destroy_streaminfo\0")
                            .map_err(|error| error.to_string())?;
                        let create_outlet = *library
                            .get::<CreateOutlet>(b"lsl_create_outlet\0")
                            .map_err(|error| error.to_string())?;
                        let destroy_outlet = *library
                            .get::<DestroyOutlet>(b"lsl_destroy_outlet\0")
                            .map_err(|error| error.to_string())?;
                        let push_sample = *library
                            .get::<PushSample>(b"lsl_push_sample_ftp\0")
                            .map_err(|error| error.to_string())?;
                        let push_sample_double = *library
                            .get::<PushSampleDouble>(b"lsl_push_sample_dtp\0")
                            .map_err(|error| error.to_string())?;
                        // Chunk push has been present for years, but keeping it
                        // optional preserves compatibility with older system LSL
                        // installs. Bundled builds always use the immediate chunk
                        // path below.
                        let push_chunk = library
                            .get::<PushChunk>(b"lsl_push_chunk_ftp\0")
                            .ok()
                            .map(|symbol| *symbol);
                        let local_clock = *library
                            .get::<LocalClock>(b"lsl_local_clock\0")
                            .map_err(|error| error.to_string())?;
                        let get_description = library
                            .get::<GetDescription>(b"lsl_get_desc\0")
                            .ok()
                            .map(|symbol| *symbol);
                        let append_child = library
                            .get::<AppendChild>(b"lsl_append_child\0")
                            .ok()
                            .map(|symbol| *symbol);
                        let append_child_value = library
                            .get::<AppendChildValue>(b"lsl_append_child_value\0")
                            .ok()
                            .map(|symbol| *symbol);
                        return Ok(Self {
                            _library: library,
                            create_streaminfo,
                            destroy_streaminfo,
                            create_outlet,
                            destroy_outlet,
                            push_sample,
                            push_sample_double,
                            push_chunk,
                            local_clock,
                            get_description,
                            append_child,
                            append_child_value,
                        });
                    }
                }
                Err(error) => errors.push(format!("{}: {error}", candidate.display())),
            }
        }
        Err(format!("liblsl not found ({})", errors.join("; ")))
    }
}

struct LslOutlet {
    handle: Outlet,
    rate_hz: f64,
    last_newest_timestamp: Option<f64>,
}

impl LslOutlet {
    fn monotonic_newest(
        &mut self,
        candidate: f64,
        record_count: usize,
        explicit_period_seconds: Option<f64>,
    ) -> f64 {
        let period = explicit_period_seconds
            .filter(|period| period.is_finite() && *period > 0.0)
            .or_else(|| (self.rate_hz > 0.0).then_some(1.0 / self.rate_hz))
            .unwrap_or(f64::EPSILON);
        let newest = self.last_newest_timestamp.map_or(candidate, |previous| {
            candidate.max(previous + period * record_count.max(1) as f64)
        });
        self.last_newest_timestamp = Some(newest);
        newest
    }

    fn monotonic_sparse_row(&mut self, candidate: f64) -> f64 {
        // A sparse outlet interleaves raw and derived batches whose source
        // times overlap. Advance only the conflicting row, not another full
        // sample period, or its LSL clock runs faster than real time.
        let timestamp = self
            .last_newest_timestamp
            .map_or(candidate, |previous| candidate.max(previous + 0.000_001));
        self.last_newest_timestamp = Some(timestamp);
        timestamp
    }
}

unsafe impl Send for LslOutlet {}

pub(crate) struct LslPublisher {
    bundled_library: Option<PathBuf>,
    api: Option<LslApi>,
    outlets: HashMap<String, LslOutlet>,
    source_clock: crate::SensorClockMap,
    status: String,
    scratch: Vec<f32>,
    scratch_double: Vec<f64>,
}

impl LslPublisher {
    pub(crate) fn new(bundled_library: Option<PathBuf>) -> Self {
        match LslApi::load(bundled_library.as_deref()) {
            Ok(api) => Self {
                bundled_library,
                api: Some(api),
                outlets: HashMap::new(),
                source_clock: crate::SensorClockMap::default(),
                status: "Ready".into(),
                scratch: Vec::with_capacity(512),
                scratch_double: Vec::with_capacity(512),
            },
            Err(error) => Self {
                bundled_library,
                api: None,
                outlets: HashMap::new(),
                source_clock: crate::SensorClockMap::default(),
                status: error,
                scratch: Vec::with_capacity(512),
                scratch_double: Vec::with_capacity(512),
            },
        }
    }

    pub(crate) fn fresh(&self) -> Self {
        Self::new(self.bundled_library.clone())
    }

    pub(crate) fn inherit_source_clock(&mut self, previous: &mut Self) {
        self.source_clock = std::mem::take(&mut previous.source_clock);
    }

    pub(crate) fn status(&self) -> &str {
        &self.status
    }

    pub(crate) fn outlet_count(&self) -> usize {
        self.outlets.len()
    }

    pub(crate) fn clear(&mut self) {
        if let Some(api) = &self.api {
            for (_, outlet) in self.outlets.drain() {
                // SAFETY: Handles were created by this API and are destroyed once.
                unsafe { (api.destroy_outlet)(outlet.handle) };
            }
        } else {
            self.outlets.clear();
        }
    }

    pub(crate) fn add_outlet_with_palette(
        &mut self,
        base_name: &str,
        spec: MetricSpec,
        palette: Option<&SourcePalette>,
        respiration_provenance: Option<&PolarRespirationProvenance>,
    ) {
        let Some(api) = &self.api else { return };
        let Some(output_name) = output_stream_name(base_name, spec.id) else {
            return;
        };
        let Ok(name) = CString::new(output_name.as_str()) else {
            return;
        };
        let Ok(stream_type) = CString::new(spec.stream_type) else {
            return;
        };
        let Ok(source) = CString::new(format!("polar-h10-{output_name}")) else {
            return;
        };
        // cf_float32 == 1 in the public lsl_channel_format_t enum.
        // SAFETY: C strings live through these calls and info is checked below.
        let info = unsafe {
            (api.create_streaminfo)(
                name.as_ptr(),
                stream_type.as_ptr(),
                spec.channels,
                spec.rate_hz,
                1,
                source.as_ptr(),
            )
        };
        if info.is_null() {
            self.status = format!("Could not create {} stream", spec.label);
            return;
        }
        if !append_stream_metadata(api, info, spec, palette, respiration_provenance) {
            unsafe { (api.destroy_streaminfo)(info) };
            self.status = format!(
                "Could not attach required processing metadata to {}",
                spec.label
            );
            return;
        }
        // SAFETY: info is live; create_outlet copies its metadata.
        let outlet = unsafe { (api.create_outlet)(info, 0, 360) };
        unsafe { (api.destroy_streaminfo)(info) };
        if outlet.is_null() {
            self.status = format!("Could not open {} outlet", spec.label);
            return;
        }
        self.outlets.insert(
            spec.id.into(),
            LslOutlet {
                handle: outlet,
                rate_hz: spec.rate_hz,
                last_newest_timestamp: None,
            },
        );
        self.status = format!("Publishing {} stream(s)", self.outlets.len());
    }

    pub(crate) fn add_custom_outlet_with_palette(
        &mut self,
        base_name: &str,
        formula: &CustomFormulaConfig,
        palette: Option<&SourcePalette>,
    ) {
        let Some(api) = &self.api else { return };
        let output_name = custom_output_stream_name(base_name, formula);
        let Ok(name) = CString::new(output_name.as_str()) else {
            return;
        };
        let Ok(stream_type) = CString::new(formula.source.stream_type()) else {
            return;
        };
        let Ok(source) = CString::new(format!("polar-h10-formula-{}", formula.id)) else {
            return;
        };
        let info = unsafe {
            (api.create_streaminfo)(
                name.as_ptr(),
                stream_type.as_ptr(),
                1,
                formula.source.rate_hz(),
                1,
                source.as_ptr(),
            )
        };
        if info.is_null() {
            self.status = format!("Could not create {} stream", formula.name);
            return;
        }
        append_custom_metadata(api, info, formula, palette);
        let outlet = unsafe { (api.create_outlet)(info, 0, 360) };
        unsafe { (api.destroy_streaminfo)(info) };
        if outlet.is_null() {
            self.status = format!("Could not open {} outlet", formula.name);
            return;
        }
        self.outlets.insert(
            formula.id.clone(),
            LslOutlet {
                handle: outlet,
                rate_hz: formula.source.rate_hz(),
                last_newest_timestamp: None,
            },
        );
        self.status = format!("Publishing {} stream(s)", self.outlets.len());
    }

    pub(crate) fn add_vernier_outlets(
        &mut self,
        base_name: &str,
        schema: &VernierStreamSchema,
        palette: Option<&SourcePalette>,
        raw: bool,
        breathing: bool,
    ) {
        if raw {
            self.add_vernier_raw_outlet(base_name, schema, palette);
        }
        let raw_ready = !raw || self.outlets.contains_key(VERNIER_RAW_OUTLET_KEY);
        let raw_status = self.status.clone();
        if breathing {
            self.add_vernier_breathing_outlet(base_name, schema, palette);
        }
        let breathing_ready = !breathing || self.outlets.contains_key(VERNIER_BREATHING_OUTLET_KEY);
        let breathing_status = self.status.clone();
        if raw_ready && breathing_ready {
            return;
        }

        let destroy_outlet = self.api.as_ref().map(|api| api.destroy_outlet);
        for key in [VERNIER_RAW_OUTLET_KEY, VERNIER_BREATHING_OUTLET_KEY] {
            if let Some(outlet) = self.outlets.remove(key)
                && let Some(destroy_outlet) = destroy_outlet
            {
                unsafe { destroy_outlet(outlet.handle) };
            }
        }
        self.status = format!(
            "Vernier LSL outlet setup failed (raw: {}; breathing: {})",
            if raw_ready { "ready" } else { &raw_status },
            if breathing_ready {
                "ready"
            } else {
                &breathing_status
            }
        );
    }

    fn add_vernier_raw_outlet(
        &mut self,
        base_name: &str,
        schema: &VernierStreamSchema,
        palette: Option<&SourcePalette>,
    ) {
        let Some(api) = &self.api else { return };
        let output_name = vernier_raw_stream_name(base_name);
        let (Ok(name), Ok(stream_type), Ok(source)) = (
            CString::new(output_name.as_str()),
            CString::new("VernierRaw"),
            CString::new(format!("polar-stream-vernier-raw-{output_name}")),
        ) else {
            return;
        };
        let Ok(channels) = c_int::try_from(schema.raw_channel_count()) else {
            self.status = "Invalid aggregate Vernier channel count".into();
            return;
        };
        // cf_double64 == 2 in the public lsl_channel_format_t enum.
        let info = unsafe {
            (api.create_streaminfo)(
                name.as_ptr(),
                stream_type.as_ptr(),
                channels,
                0.0,
                2,
                source.as_ptr(),
            )
        };
        if info.is_null() {
            self.status = "Could not create aggregate Vernier raw stream".into();
            return;
        }
        append_vernier_raw_metadata(api, info, schema, palette);
        let outlet = unsafe { (api.create_outlet)(info, 0, 360) };
        unsafe { (api.destroy_streaminfo)(info) };
        if outlet.is_null() {
            self.status = "Could not open aggregate Vernier raw outlet".into();
            return;
        }
        self.outlets.insert(
            VERNIER_RAW_OUTLET_KEY.into(),
            LslOutlet {
                handle: outlet,
                rate_hz: 0.0,
                last_newest_timestamp: None,
            },
        );
        self.status = format!("Publishing {} stream(s)", self.outlets.len());
    }

    fn add_vernier_breathing_outlet(
        &mut self,
        base_name: &str,
        schema: &VernierStreamSchema,
        palette: Option<&SourcePalette>,
    ) {
        let Some(api) = &self.api else { return };
        let output_name = vernier_breathing_stream_name(base_name);
        let (Ok(name), Ok(stream_type), Ok(source)) = (
            CString::new(output_name.as_str()),
            CString::new("Respiration"),
            CString::new(format!("polar-stream-vernier-breathing-{output_name}")),
        ) else {
            return;
        };
        let info = unsafe {
            (api.create_streaminfo)(
                name.as_ptr(),
                stream_type.as_ptr(),
                1,
                0.0,
                1,
                source.as_ptr(),
            )
        };
        if info.is_null() {
            self.status = "Could not create Vernier breathing stream".into();
            return;
        }
        if !append_vernier_breathing_metadata(api, info, schema, palette) {
            unsafe { (api.destroy_streaminfo)(info) };
            self.status =
                "Could not attach required processing metadata to Vernier breathing stream".into();
            return;
        }
        let outlet = unsafe { (api.create_outlet)(info, 0, 360) };
        unsafe { (api.destroy_streaminfo)(info) };
        if outlet.is_null() {
            self.status = "Could not open Vernier breathing outlet".into();
            return;
        }
        self.outlets.insert(
            VERNIER_BREATHING_OUTLET_KEY.into(),
            LslOutlet {
                handle: outlet,
                rate_hz: 0.0,
                last_newest_timestamp: None,
            },
        );
        self.status = format!("Publishing {} stream(s)", self.outlets.len());
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_vernier_raw(
        &mut self,
        schema: &VernierStreamSchema,
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        sequence: u64,
        dropped_before: u64,
        device_drop_reports_before: u64,
        decode_latency_ns: u64,
        encoding: SampleEncoding,
        sensors: &[SensorSamples],
    ) {
        let row_count = encode_vernier_raw_rows(
            &mut self.scratch_double,
            schema,
            host_receive_timestamp_ns,
            sample_period_us,
            sequence,
            dropped_before,
            device_drop_reports_before,
            decode_latency_ns,
            encoding,
            sensors,
        );
        let (Some(api), Some(outlet)) = (&self.api, self.outlets.get_mut(VERNIER_RAW_OUTLET_KEY))
        else {
            return;
        };
        let channels = schema.raw_channel_count();
        if row_count == 0 || self.scratch_double.len() != row_count.saturating_mul(channels) {
            return;
        }
        let local_now = unsafe { (api.local_clock)() };
        let newest = self
            .source_clock
            .map_newest(host_receive_timestamp_ns, local_now);
        let period_seconds =
            (sample_period_us > 0).then_some(f64::from(sample_period_us) / 1_000_000.0);
        let newest = outlet.monotonic_newest(newest, row_count, period_seconds);
        for (row, values) in self.scratch_double.chunks_exact(channels).enumerate() {
            let backfill = if sample_period_us > 0 {
                (row_count - row - 1) as f64 * f64::from(sample_period_us) / 1_000_000.0
            } else {
                0.0
            };
            let result = unsafe {
                (api.push_sample_double)(outlet.handle, values.as_ptr(), newest - backfill, 1)
            };
            if result != 0 {
                self.status = format!("LSL Vernier raw push failed ({result})");
                break;
            }
        }
    }

    pub(crate) fn push_scalar_series<I>(&mut self, id: &str, values: I)
    where
        I: IntoIterator<Item = f32>,
    {
        self.scratch.clear();
        self.scratch.extend(values);
        if self.scratch.is_empty() {
            return;
        }
        self.push_notification(id, 1, None);
    }

    pub(crate) fn push_scalar_series_at<I>(&mut self, id: &str, values: I, sensor_timestamp_ns: u64)
    where
        I: IntoIterator<Item = f32>,
    {
        self.scratch.clear();
        self.scratch.extend(values);
        if self.scratch.is_empty() {
            return;
        }
        self.push_notification(id, 1, Some(sensor_timestamp_ns));
    }

    pub(crate) fn push_scalar_series_period_at<I>(
        &mut self,
        id: &str,
        values: I,
        newest_timestamp_ns: u64,
        sample_period_us: u32,
    ) where
        I: IntoIterator<Item = f32>,
    {
        self.scratch.clear();
        self.scratch.extend(values);
        let (Some(api), Some(outlet)) = (&self.api, self.outlets.get_mut(id)) else {
            return;
        };
        if self.scratch.is_empty() {
            return;
        }
        let local_now = unsafe { (api.local_clock)() };
        let count = self.scratch.len();
        let period_seconds = f64::from(sample_period_us) / 1_000_000.0;
        let newest = self.source_clock.map_newest(newest_timestamp_ns, local_now);
        let newest = outlet.monotonic_newest(newest, count, Some(period_seconds));
        for (index, value) in self.scratch.iter().enumerate() {
            let backfill = (count - index - 1) as f64 * f64::from(sample_period_us) / 1_000_000.0;
            let result = unsafe { (api.push_sample)(outlet.handle, value, newest - backfill, 1) };
            if result != 0 {
                self.status = format!("LSL push failed ({result})");
                break;
            }
        }
    }

    pub(crate) fn push_accelerometer_at(
        &mut self,
        samples: &[AccSample],
        sensor_timestamp_ns: u64,
    ) {
        self.scratch.clear();
        self.scratch.reserve(samples.len().saturating_mul(3));
        for sample in samples {
            self.scratch.extend([
                f32::from(sample.x_mg),
                f32::from(sample.y_mg),
                f32::from(sample.z_mg),
            ]);
        }
        self.push_notification("raw_acc", 3, Some(sensor_timestamp_ns));
    }

    /// Immediately forwards one already-arrived BLE notification. This does
    /// not accumulate data or wait for a timer: the chunk call simply replaces
    /// many C FFI calls with one. Its timestamp denotes the newest sample and
    /// liblsl derives earlier sample times from the declared nominal rate.
    fn push_notification(&mut self, id: &str, channels: usize, sensor_timestamp_ns: Option<u64>) {
        let (Some(api), Some(outlet)) = (&self.api, self.outlets.get_mut(id)) else {
            return;
        };
        if self.scratch.is_empty() || !self.scratch.len().is_multiple_of(channels) {
            return;
        }
        // SAFETY: Function pointers come from the retained library, the buffer
        // lives through each call, and its length is a channel-count multiple.
        let local_now = unsafe { (api.local_clock)() };
        let sample_count = self.scratch.len() / channels;
        let newest = sensor_timestamp_ns.map_or(local_now, |sensor_timestamp_ns| {
            self.source_clock.map_newest(sensor_timestamp_ns, local_now)
        });
        let newest = outlet.monotonic_newest(newest, sample_count, None);
        let result = if let Some(push_chunk) = api.push_chunk {
            unsafe {
                push_chunk(
                    outlet.handle,
                    self.scratch.as_ptr(),
                    self.scratch.len() as c_ulong,
                    newest,
                    1,
                )
            }
        } else {
            let mut result = 0;
            for (index, values) in self.scratch.chunks_exact(channels).enumerate() {
                let backfill = if outlet.rate_hz > 0.0 {
                    (sample_count - index - 1) as f64 / outlet.rate_hz
                } else {
                    0.0
                };
                result = unsafe {
                    (api.push_sample)(outlet.handle, values.as_ptr(), newest - backfill, 1)
                };
                if result != 0 {
                    break;
                }
            }
            result
        };
        if result != 0 {
            self.status = format!("LSL push failed ({result})");
        }
    }

    fn add_combined_outlet(
        &mut self,
        descriptor: MiniCombinedOutletDescriptor<'_>,
        channels: &[MiniCombinedChannel],
    ) {
        let Some(api) = &self.api else { return };
        if channels.is_empty() || channels.len() > c_int::MAX as usize {
            self.status = "Invalid combined LSL channel count".into();
            return;
        }
        let (Ok(name), Ok(stream_type), Ok(source)) = (
            CString::new(descriptor.name),
            CString::new(descriptor.stream_type),
            CString::new(descriptor.source_id),
        ) else {
            self.status = "Combined LSL stream metadata contains invalid text".into();
            return;
        };
        let info = unsafe {
            (api.create_streaminfo)(
                name.as_ptr(),
                stream_type.as_ptr(),
                channels.len() as c_int,
                0.0,
                descriptor.channel_format,
                source.as_ptr(),
            )
        };
        if info.is_null() {
            self.status = "Could not create combined mini stream".into();
            return;
        }
        append_mini_combined_metadata(api, info, descriptor, channels);
        let outlet = unsafe { (api.create_outlet)(info, 0, 360) };
        unsafe { (api.destroy_streaminfo)(info) };
        if outlet.is_null() {
            self.status = "Could not open combined mini outlet".into();
            return;
        }
        self.outlets.insert(
            descriptor.key.into(),
            LslOutlet {
                handle: outlet,
                rate_hz: 0.0,
                last_newest_timestamp: None,
            },
        );
        self.status = format!("Publishing {} stream(s)", self.outlets.len());
    }

    fn push_float_rows_at_key(
        &mut self,
        key: &str,
        rows: &[f32],
        channels: usize,
        newest_timestamp_ns: Option<u64>,
        period_seconds: Option<f64>,
    ) {
        let (Some(api), Some(outlet)) = (&self.api, self.outlets.get_mut(key)) else {
            return;
        };
        if rows.is_empty() || channels == 0 || !rows.len().is_multiple_of(channels) {
            return;
        }
        let row_count = rows.len() / channels;
        let local_now = unsafe { (api.local_clock)() };
        let newest = newest_timestamp_ns
            .filter(|timestamp| *timestamp != 0)
            .map_or(local_now, |timestamp| {
                self.source_clock.map_newest(timestamp, local_now)
            });
        for (index, values) in rows.chunks_exact(channels).enumerate() {
            let backfill =
                period_seconds.map_or(0.0, |period| (row_count - index - 1) as f64 * period);
            let timestamp = outlet.monotonic_sparse_row(newest - backfill);
            let result = unsafe { (api.push_sample)(outlet.handle, values.as_ptr(), timestamp, 1) };
            if result != 0 {
                self.status = format!("LSL combined push failed ({result})");
                break;
            }
        }
    }

    fn push_double_rows_at_key(
        &mut self,
        key: &str,
        rows: &[f64],
        channels: usize,
        newest_timestamp_ns: Option<u64>,
        period_seconds: Option<f64>,
    ) {
        let (Some(api), Some(outlet)) = (&self.api, self.outlets.get_mut(key)) else {
            return;
        };
        if rows.is_empty() || channels == 0 || !rows.len().is_multiple_of(channels) {
            return;
        }
        let row_count = rows.len() / channels;
        let local_now = unsafe { (api.local_clock)() };
        let newest = newest_timestamp_ns
            .filter(|timestamp| *timestamp != 0)
            .map_or(local_now, |timestamp| {
                self.source_clock.map_newest(timestamp, local_now)
            });
        for (index, values) in rows.chunks_exact(channels).enumerate() {
            let backfill =
                period_seconds.map_or(0.0, |period| (row_count - index - 1) as f64 * period);
            let timestamp = outlet.monotonic_sparse_row(newest - backfill);
            let result =
                unsafe { (api.push_sample_double)(outlet.handle, values.as_ptr(), timestamp, 1) };
            if result != 0 {
                self.status = format!("LSL combined push failed ({result})");
                break;
            }
        }
    }
}

#[derive(Clone, Debug)]
struct MiniCombinedChannel {
    label: String,
    unit: String,
    stream_type: String,
    detail: String,
}

impl MiniCombinedChannel {
    fn new(label: &str, unit: &str, stream_type: &str, detail: &str) -> Self {
        Self {
            label: label.into(),
            unit: unit.into(),
            stream_type: stream_type.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Copy)]
struct MiniCombinedOutletDescriptor<'a> {
    key: &'a str,
    name: &'a str,
    stream_type: &'a str,
    channel_format: c_int,
    source_id: &'a str,
    application: &'a str,
    manufacturer: &'a str,
    model: &'a str,
    role: &'a str,
    palette: Option<&'a SourcePalette>,
}

pub struct MiniStatusOutput {
    inner: std::sync::Mutex<LslPublisher>,
}

impl MiniStatusOutput {
    pub fn health(&self) -> String {
        self.inner.lock().map(|lsl| lsl.status().to_string()).unwrap_or_else(|_| "Signal-status output lock failed".into())
    }

    pub fn new(
        bundled_library: Option<PathBuf>,
        stream_name: &str,
        polar: bool,
    ) -> Result<Self, String> {
        let stream_name = crate::normalize_stream_base(stream_name)?;
        let mut lsl = LslPublisher::new(bundled_library);
        let name = format!("{stream_name}-signalStatus");
        lsl.add_combined_outlet(
            MiniCombinedOutletDescriptor {
                key: MINI_STATUS_KEY,
                name: &name,
                stream_type: "Markers",
                channel_format: 1,
                source_id: &format!("{}-{stream_name}-signal-status", if polar { "polar-mini" } else { "vernier-mini" }),
                application: if polar { "Polar Stream Mini" } else { "Vernier Stream Mini" },
                manufacturer: if polar { "Polar" } else { "Vernier" },
                model: if polar { "H10" } else { "Go Direct" },
                role: "signal_continuity_markers",
                palette: None,
            },
            &[MiniCombinedChannel::new("signal_state", "code", "Marker", "1=Bluetooth signal lost; 2=Bluetooth signal restored. No sensor samples are synthesized during a gap.")],
        );
        if lsl.outlet_count() != 1 {
            return Err(format!(
                "Mini signal-status LSL outlet was not opened: {}",
                lsl.status()
            ));
        }
        Ok(Self {
            inner: std::sync::Mutex::new(lsl),
        })
    }

    pub fn publish(&self, restored: bool) {
        if let Ok(mut lsl) = self.inner.lock() {
            lsl.push_float_rows_at_key(
                MINI_STATUS_KEY,
                &[if restored { 2.0 } else { 1.0 }],
                1,
                None,
                None,
            );
        }
    }
}

pub struct MiniCombinedOutput {
    inner: std::sync::Mutex<MiniCombinedInner>,
}

struct MiniCombinedInner {
    lsl: LslPublisher,
    stream_name: String,
    polar_metric_ids: Vec<String>,
    vernier_schema: Option<VernierStreamSchema>,
    vernier_selection: Option<VernierMiniSelection>,
}

impl MiniCombinedOutput {
    pub fn publish_signal_state(&self, restored: bool) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.vernier_schema.is_some() {
            if !inner.vernier_selection.is_some_and(|selection| selection.signal_status) { return; }
            let Some(schema) = inner.vernier_schema.as_ref() else {
                return;
            };
            let selection = inner.vernier_selection.expect("Vernier selection exists with schema");
            let mut row = vec![f64::NAN; vernier_combined_channel_count(schema, selection)];
            *row.last_mut().expect("signal state channel exists") =
                if restored { 2.0 } else { 1.0 };
            let channels = row.len();
            inner.lsl.push_double_rows_at_key(
                MINI_VERNIER_COMBINED_KEY,
                &row,
                channels,
                None,
                None,
            );
        } else if !inner.polar_metric_ids.is_empty() || inner.lsl.outlet_count() > 0 {
            let mut row = vec![f32::NAN; MINI_POLAR_FIXED_CHANNELS + inner.polar_metric_ids.len()];
            row[0] = if restored { 21.0 } else { 20.0 };
            let channels = row.len();
            inner
                .lsl
                .push_float_rows_at_key(MINI_POLAR_COMBINED_KEY, &row, channels, None, None);
        }
    }

    pub fn polar(
        bundled_library: Option<PathBuf>,
        stream_name: &str,
        selected_outputs: &[String],
    ) -> Result<Self, String> {
        let stream_name = crate::normalize_stream_base(stream_name)?;
        let polar_metric_ids = polar_combined_metric_ids(selected_outputs);
        let mut lsl = LslPublisher::new(bundled_library);
        let channels = polar_combined_channels(&polar_metric_ids);
        lsl.add_combined_outlet(
            MiniCombinedOutletDescriptor {
                key: MINI_POLAR_COMBINED_KEY,
                name: &stream_name,
                stream_type: "PolarMini",
                channel_format: 1,
                source_id: &format!("polar-stream-mini-{stream_name}"),
                application: "Polar Stream Mini",
                manufacturer: "Polar",
                model: "H10",
                role: "single_sparse_measurement_stream",
                palette: None,
            },
            &channels,
        );
        if lsl.outlet_count() != 1 {
            return Err(format!(
                "Polar Mini combined LSL outlet was not opened: {}",
                lsl.status()
            ));
        }
        Ok(Self {
            inner: std::sync::Mutex::new(MiniCombinedInner {
                lsl,
                stream_name,
                polar_metric_ids,
                vernier_schema: None,
                vernier_selection: None,
            }),
        })
    }

    pub fn vernier(bundled_library: Option<PathBuf>, stream_name: &str, selected_outputs: &[String]) -> Result<Self, String> {
        let stream_name = crate::normalize_stream_base(stream_name)?;
        Ok(Self {
            inner: std::sync::Mutex::new(MiniCombinedInner {
                lsl: LslPublisher::new(bundled_library),
                stream_name,
                polar_metric_ids: Vec::new(),
                vernier_schema: None,
                vernier_selection: Some(VernierMiniSelection::from_ids(Some(selected_outputs))),
            }),
        })
    }

    pub fn configure_vernier_streams(
        &self,
        model_code: &str,
        sample_period_us: u32,
        sensors: &[vernier_gdx_core::SensorInfo],
    ) -> Result<VernierStreamSchema, String> {
        let schema = VernierStreamSchema::new(model_code, sample_period_us, sensors)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Combined mini output lock failed".to_string())?;
        if inner.vernier_schema.as_ref() == Some(&schema) {
            return Ok(schema);
        }
        let mut lsl = inner.lsl.fresh();
        let channels = vernier_combined_channels(&schema, inner.vernier_selection.expect("Vernier selection exists"));
        lsl.add_combined_outlet(
            MiniCombinedOutletDescriptor {
                key: MINI_VERNIER_COMBINED_KEY,
                name: &inner.stream_name,
                stream_type: "VernierMini",
                channel_format: 2,
                source_id: &format!("polar-stream-vernier-mini-{}", inner.stream_name),
                application: "Vernier Stream Mini",
                manufacturer: "Vernier",
                model: schema.model_code(),
                role: "single_sparse_measurement_stream",
                palette: None,
            },
            &channels,
        );
        if lsl.outlet_count() != 1 {
            return Err(format!(
                "Vernier Mini combined LSL outlet was not opened: {}",
                lsl.status()
            ));
        }
        lsl.inherit_source_clock(&mut inner.lsl);
        inner.lsl = lsl;
        inner.vernier_schema = Some(schema.clone());
        Ok(schema)
    }

    pub fn health(&self) -> String {
        self.inner
            .lock()
            .map(|inner| {
                if inner.lsl.outlet_count() == 0 {
                    "Waiting for device stream schema".into()
                } else {
                    inner.lsl.status().into()
                }
            })
            .unwrap_or_else(|_| "Combined mini output lock failed".into())
    }

    pub fn publish_polar_ecg(&self, sensor_timestamp_ns: u64, samples: &[i32]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let channels = MINI_POLAR_FIXED_CHANNELS + inner.polar_metric_ids.len();
        let mut rows = Vec::with_capacity(samples.len().saturating_mul(channels));
        for sample in samples {
            let mut row = vec![f32::NAN; channels];
            row[0] = 1.0;
            row[1] = *sample as f32;
            rows.extend(row);
        }
        inner.lsl.push_float_rows_at_key(
            MINI_POLAR_COMBINED_KEY,
            &rows,
            channels,
            Some(sensor_timestamp_ns),
            Some(1.0 / 130.0),
        );
    }

    pub fn publish_polar_accelerometer(&self, sensor_timestamp_ns: u64, samples: &[AccSample]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let channels = MINI_POLAR_FIXED_CHANNELS + inner.polar_metric_ids.len();
        let mut rows = Vec::with_capacity(samples.len().saturating_mul(channels));
        for sample in samples {
            let mut row = vec![f32::NAN; channels];
            row[0] = 2.0;
            row[2] = f32::from(sample.x_mg);
            row[3] = f32::from(sample.y_mg);
            row[4] = f32::from(sample.z_mg);
            rows.extend(row);
        }
        inner.lsl.push_float_rows_at_key(
            MINI_POLAR_COMBINED_KEY,
            &rows,
            channels,
            Some(sensor_timestamp_ns),
            Some(1.0 / 200.0),
        );
    }

    pub fn publish_polar_heart_rate(&self, beats_per_minute: u16, rr_intervals_ms: &[f32]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let channels = MINI_POLAR_FIXED_CHANNELS + inner.polar_metric_ids.len();
        let row_count = rr_intervals_ms.len().max(1);
        let mut rows = Vec::with_capacity(row_count.saturating_mul(channels));
        if rr_intervals_ms.is_empty() {
            let mut row = vec![f32::NAN; channels];
            row[0] = 3.0;
            row[5] = f32::from(beats_per_minute);
            rows.extend(row);
        } else {
            for rr_interval in rr_intervals_ms {
                let mut row = vec![f32::NAN; channels];
                row[0] = 3.0;
                row[5] = f32::from(beats_per_minute);
                row[6] = *rr_interval;
                rows.extend(row);
            }
        }
        inner
            .lsl
            .push_float_rows_at_key(MINI_POLAR_COMBINED_KEY, &rows, channels, None, None);
    }

    pub fn publish_polar_metrics_at(
        &self,
        sensor_timestamp_ns: u64,
        values: &[crate::MetricValue<'_>],
    ) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if values.is_empty() || inner.polar_metric_ids.is_empty() {
            return;
        }
        let channels = MINI_POLAR_FIXED_CHANNELS + inner.polar_metric_ids.len();
        let mut row = vec![f32::NAN; channels];
        row[0] = 10.0;
        for value in values {
            if let Some(index) = inner.polar_metric_ids.iter().position(|id| id == value.id) {
                row[MINI_POLAR_FIXED_CHANNELS + index] = value.value;
            }
        }
        if row[MINI_POLAR_FIXED_CHANNELS..]
            .iter()
            .any(|value| value.is_finite())
        {
            inner.lsl.push_float_rows_at_key(
                MINI_POLAR_COMBINED_KEY,
                &row,
                channels,
                Some(sensor_timestamp_ns),
                None,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn publish_vernier_raw(
        &self,
        host_receive_timestamp_ns: u64,
        sample_period_us: u32,
        sequence: u64,
        dropped_before: u64,
        device_drop_reports_before: u64,
        decode_latency_ns: u64,
        encoding: SampleEncoding,
        sensors: &[SensorSamples],
    ) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(schema) = inner.vernier_schema.clone() else {
            return;
        };
        let selection = inner.vernier_selection.expect("Vernier selection exists with schema");
        if !selection.raw_vernier && !selection.raw_force { return; }
        let raw_channels = if selection.raw_vernier { schema.raw_channel_count() } else { 0 };
        let force = schema.force_sensor_number().and_then(|number| sensors.iter().find(|sensor| sensor.sensor_number == number));
        let row_count = if selection.raw_vernier {
            encode_vernier_raw_rows(
                &mut inner.lsl.scratch_double, &schema, host_receive_timestamp_ns,
                sample_period_us, sequence, dropped_before, device_drop_reports_before,
                decode_latency_ns, encoding, sensors,
            )
        } else { force.map_or(0, |samples| samples.values.len()) };
        if row_count == 0 || (selection.raw_vernier && inner.lsl.scratch_double.len() != row_count * raw_channels) {
            return;
        }
        let channels = vernier_combined_channel_count(&schema, selection);
        let mut rows = Vec::with_capacity(row_count.saturating_mul(channels));
        for row in 0..row_count {
            if selection.raw_vernier {
                rows.extend_from_slice(&inner.lsl.scratch_double[row * raw_channels..(row + 1) * raw_channels]);
            }
            if selection.raw_force {
                rows.push(force.and_then(|samples| samples.values.get(row)).copied().unwrap_or(f64::NAN));
            }
            if selection.breathing { rows.push(f64::NAN); }
            if selection.signal_status { rows.push(f64::NAN); }
        }
        inner.lsl.push_double_rows_at_key(
            MINI_VERNIER_COMBINED_KEY,
            &rows,
            channels,
            Some(host_receive_timestamp_ns),
            Some(f64::from(sample_period_us) / 1_000_000.0),
        );
    }

    pub fn publish_vernier_breathing(
        &self,
        host_receive_timestamp_ns: u64,
        values_01: &[f32],
        sample_period_us: u32,
    ) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(schema) = inner.vernier_schema.as_ref() else {
            return;
        };
        let selection = inner.vernier_selection.expect("Vernier selection exists with schema");
        if !selection.breathing { return; }
        let breathing_index = if selection.raw_vernier { schema.raw_channel_count() } else { 0 }
            + usize::from(selection.raw_force);
        let channels = vernier_combined_channel_count(schema, selection);
        let mut rows = Vec::with_capacity(values_01.len().saturating_mul(channels));
        for value in values_01 {
            let mut row = vec![f64::NAN; channels];
            row[breathing_index] = f64::from(*value);
            rows.extend(row);
        }
        inner.lsl.push_double_rows_at_key(
            MINI_VERNIER_COMBINED_KEY,
            &rows,
            channels,
            Some(host_receive_timestamp_ns),
            Some(f64::from(sample_period_us) / 1_000_000.0),
        );
    }
}

fn polar_combined_metric_ids(selected_outputs: &[String]) -> Vec<String> {
    let mut ids = Vec::new();
    for id in selected_outputs {
        if matches!(
            id.as_str(),
            "raw_ecg" | "raw_acc" | "heart_rate" | "rr_interval" | "raw_force"
        ) || ids.iter().any(|known| known == id)
        {
            continue;
        }
        if MetricSpec::for_id(id).is_some() {
            ids.push(id.clone());
        }
    }
    ids
}

fn polar_combined_channels(metric_ids: &[String]) -> Vec<MiniCombinedChannel> {
    let mut channels = vec![
        MiniCombinedChannel::new(
            "event_kind",
            "code",
            "Marker",
            "1=ECG, 2=ACC, 3=HR/RR, 10=metric snapshot, 20=signal lost, 21=signal restored",
        ),
        MiniCombinedChannel::new("raw_ecg_uv", "uV", "ECG", "Polar H10 raw ECG voltage"),
        MiniCombinedChannel::new(
            "acc_x_mg",
            "mg",
            "Accelerometer",
            "Polar H10 acceleration X",
        ),
        MiniCombinedChannel::new(
            "acc_y_mg",
            "mg",
            "Accelerometer",
            "Polar H10 acceleration Y",
        ),
        MiniCombinedChannel::new(
            "acc_z_mg",
            "mg",
            "Accelerometer",
            "Polar H10 acceleration Z",
        ),
        MiniCombinedChannel::new(
            "heart_rate_bpm",
            "bpm",
            "HeartRate",
            "Polar H10 device-reported heart rate",
        ),
        MiniCombinedChannel::new(
            "rr_interval_ms",
            "ms",
            "RrInterval",
            "Polar H10 RR intervals from the standard heart-rate characteristic",
        ),
    ];
    for id in metric_ids {
        if let Some(spec) = MetricSpec::for_id(id) {
            channels.push(MiniCombinedChannel::new(
                spec.stream_suffix,
                spec.unit,
                spec.stream_type,
                spec.label,
            ));
        }
    }
    channels
}

fn vernier_combined_channel_count(schema: &VernierStreamSchema, selection: VernierMiniSelection) -> usize {
    (if selection.raw_vernier { schema.raw_channel_count() } else { 0 })
        + usize::from(selection.raw_force)
        + usize::from(selection.breathing)
        + usize::from(selection.signal_status)
}

fn vernier_combined_channels(schema: &VernierStreamSchema, selection: VernierMiniSelection) -> Vec<MiniCombinedChannel> {
    let mut channels = Vec::with_capacity(schema.raw_channel_count() + 3);
    if selection.raw_vernier {
        for sensor in schema.channels() {
            channels.push(MiniCombinedChannel::new(
                &format!("sensor_{}_{}", sensor.number, sensor.description),
                &sensor.unit,
                "RawMeasurement",
                "Vernier Go Direct metadata-exposed device channel",
            ));
        }
        for (label, unit, detail) in [
        ("sequence", "count", "Monotonic raw row sequence"),
        (
            "dropped_rows_before",
            "count",
            "Rows dropped at the bounded input queue before this row",
        ),
        (
            "device_drop_reports_before",
            "count",
            "Device-level dropped-packet reports observed before this row",
        ),
        (
            "sample_period_us",
            "microseconds",
            "Configured periodic backfill interval; zero denotes no interval",
        ),
        (
            "decode_latency_ns",
            "nanoseconds",
            "Host notification-to-decode latency",
        ),
        (
            "host_receive_timestamp_ns",
            "nanoseconds",
            "Monotonic time since the native measurement-session origin",
        ),
        (
            "encoding_code",
            "code",
            "0 = device Float32 frame; 1 = device Int32 frame",
        ),
        ] {
            channels.push(MiniCombinedChannel::new(label, unit, "RecordingDiagnostics", detail));
        }
    }
    if selection.raw_force {
        channels.push(MiniCombinedChannel::new("raw_force", "N", "RespirationForce", "Force channel compatibility copy"));
    }
    if selection.breathing {
        channels.push(MiniCombinedChannel::new("vernier_breathing_01", "0-1", "Respiration", "Explicitly derived Vernier breathing waveform"));
    }
    if selection.signal_status {
        channels.push(MiniCombinedChannel::new("signal_state", "code", "Marker", "1=Bluetooth signal lost; 2=Bluetooth signal restored. All other channels are NaN on marker rows."));
    }
    channels
}

fn append_mini_combined_metadata(
    api: &LslApi,
    info: StreamInfo,
    descriptor: MiniCombinedOutletDescriptor<'_>,
    channel_defs: &[MiniCombinedChannel],
) {
    let (Some(get_description), Some(append_child), Some(append_child_value)) = (
        api.get_description,
        api.append_child,
        api.append_child_value,
    ) else {
        return;
    };
    let description = unsafe { get_description(info) };
    if description.is_null() {
        return;
    }
    append_value(
        append_child_value,
        description,
        "manufacturer",
        descriptor.manufacturer,
    );
    append_value(append_child_value, description, "model", descriptor.model);
    append_value(
        append_child_value,
        description,
        "application",
        descriptor.application,
    );
    append_value(
        append_child_value,
        description,
        "stream_role",
        descriptor.role,
    );
    append_value(
        append_child_value,
        description,
        "sparse_encoding",
        "Channels unrelated to a measurement row are NaN and are never carried forward or interpolated.",
    );
    if descriptor.role == "single_sparse_measurement_stream" {
        append_value(
            append_child_value,
            description,
            "timestamp_policy",
            "Source-time candidates are published immediately in acquisition order. Overlapping rows from different signals are advanced only to the next monotonic microsecond. Use separate streams for exact per-signal sample timing.",
        );
    }
    append_source_palette(
        append_child,
        append_child_value,
        description,
        descriptor.palette,
    );

    let Ok(channels_name) = CString::new("channels") else {
        return;
    };
    let channels = unsafe { append_child(description, channels_name.as_ptr()) };
    if channels.is_null() {
        return;
    }
    for definition in channel_defs {
        let Ok(channel_name) = CString::new("channel") else {
            continue;
        };
        let channel = unsafe { append_child(channels, channel_name.as_ptr()) };
        if channel.is_null() {
            continue;
        }
        append_value(append_child_value, channel, "label", &definition.label);
        append_value(append_child_value, channel, "unit", &definition.unit);
        append_value(append_child_value, channel, "type", &definition.stream_type);
        append_value(
            append_child_value,
            channel,
            "description",
            &definition.detail,
        );
    }
}

fn append_stream_metadata(
    api: &LslApi,
    info: StreamInfo,
    spec: MetricSpec,
    palette: Option<&SourcePalette>,
    respiration_provenance: Option<&PolarRespirationProvenance>,
) -> bool {
    let processing_required =
        respiration_provenance.is_some_and(|_| PolarRespirationProvenance::applies_to(spec));
    let (Some(get_description), Some(append_child), Some(append_child_value)) = (
        api.get_description,
        api.append_child,
        api.append_child_value,
    ) else {
        return !processing_required;
    };
    // SAFETY: `info` remains live until after outlet creation, and every C
    // string below lives through its individual liblsl call.
    let description = unsafe { get_description(info) };
    if description.is_null() {
        return !processing_required;
    }
    let (manufacturer, model) = if spec.id == "raw_force" {
        ("Vernier", "Go Direct")
    } else {
        ("Polar", "H10")
    };
    append_value(
        append_child_value,
        description,
        "manufacturer",
        manufacturer,
    );
    append_value(append_child_value, description, "model", model);
    append_value(
        append_child_value,
        description,
        "application",
        "Polar Stream",
    );
    append_source_palette(append_child, append_child_value, description, palette);
    let processing_attached = append_polar_respiration_processing(
        append_child,
        append_child_value,
        description,
        spec,
        respiration_provenance,
    );

    let Ok(channels_name) = CString::new("channels") else {
        return processing_attached;
    };
    let channels = unsafe { append_child(description, channels_name.as_ptr()) };
    if channels.is_null() {
        return processing_attached;
    }
    for label in channel_labels(spec) {
        let Ok(channel_name) = CString::new("channel") else {
            continue;
        };
        let channel = unsafe { append_child(channels, channel_name.as_ptr()) };
        if channel.is_null() {
            continue;
        }
        append_value(append_child_value, channel, "label", label);
        append_value(append_child_value, channel, "unit", spec.unit);
        append_value(append_child_value, channel, "type", spec.stream_type);
    }
    processing_attached
}

fn append_polar_respiration_processing(
    append_child: AppendChild,
    append_child_value: AppendChildValue,
    description: XmlElement,
    spec: MetricSpec,
    provenance: Option<&PolarRespirationProvenance>,
) -> bool {
    let Some(provenance) = provenance.filter(|_| PolarRespirationProvenance::applies_to(spec))
    else {
        return true;
    };
    let Ok(name) = CString::new("processing") else {
        return false;
    };
    let processing = unsafe { append_child(description, name.as_ptr()) };
    if processing.is_null() {
        return false;
    }
    provenance
        .fields()
        .into_iter()
        .all(|field| append_value_checked(append_child_value, processing, field.name, &field.value))
}

fn append_vernier_raw_metadata(
    api: &LslApi,
    info: StreamInfo,
    schema: &VernierStreamSchema,
    palette: Option<&SourcePalette>,
) {
    let (Some(get_description), Some(append_child), Some(append_child_value)) = (
        api.get_description,
        api.append_child,
        api.append_child_value,
    ) else {
        return;
    };
    let description = unsafe { get_description(info) };
    if description.is_null() {
        return;
    }
    append_value(append_child_value, description, "manufacturer", "Vernier");
    append_value(
        append_child_value,
        description,
        "model",
        schema.model_code(),
    );
    append_value(
        append_child_value,
        description,
        "application",
        "Polar Stream",
    );
    append_source_palette(append_child, append_child_value, description, palette);
    append_value(
        append_child_value,
        description,
        "stream_role",
        "raw_measurement_recording",
    );
    append_value(
        append_child_value,
        description,
        "clock_source",
        "host_receive_time_with_configured_period_backfill",
    );
    append_value(
        append_child_value,
        description,
        "sparse_encoding",
        "Channels absent from a native device update are NaN; values are never carried forward or interpolated.",
    );
    append_value(
        append_child_value,
        description,
        "value_format",
        "Double64 losslessly contains device Float32 and Int32 values.",
    );

    let Ok(channels_name) = CString::new("channels") else {
        return;
    };
    let channels = unsafe { append_child(description, channels_name.as_ptr()) };
    if channels.is_null() {
        return;
    }
    for sensor in schema.channels() {
        let numeric_type = match sensor.numeric_type {
            vernier_gdx_core::NumericMeasurementType::Real => "Float32",
            vernier_gdx_core::NumericMeasurementType::Integer => "Int32",
            vernier_gdx_core::NumericMeasurementType::Unknown(_) => "Unknown",
        };
        let sampling_mode = match sensor.sampling_mode {
            vernier_gdx_core::SamplingMode::Periodic => "Periodic",
            vernier_gdx_core::SamplingMode::Aperiodic => "Aperiodic",
            vernier_gdx_core::SamplingMode::Unknown(_) => "Unknown",
        };
        append_vernier_channel(
            append_child,
            append_child_value,
            channels,
            &sensor.description,
            &sensor.unit,
            "RawMeasurement",
            &[
                ("sensor_number", sensor.number.to_string()),
                ("sensor_id", sensor.sensor_id.to_string()),
                ("numeric_type", numeric_type.into()),
                ("sampling_mode", sampling_mode.into()),
                ("uncertainty", sensor.uncertainty.to_string()),
                ("minimum", sensor.minimum.to_string()),
                ("maximum", sensor.maximum.to_string()),
                ("minimum_period_us", sensor.minimum_period_us.to_string()),
                ("maximum_period_us", sensor.maximum_period_us.to_string()),
                ("typical_period_us", sensor.typical_period_us.to_string()),
                (
                    "period_granularity_us",
                    sensor.period_granularity_us.to_string(),
                ),
            ],
        );
    }
    for (label, unit, detail) in [
        ("sequence", "count", "Monotonic raw row sequence"),
        (
            "dropped_rows_before",
            "count",
            "Rows dropped at the bounded input queue before this row",
        ),
        (
            "device_drop_reports_before",
            "count",
            "Device-level dropped-packet reports observed before this row",
        ),
        (
            "sample_period_us",
            "microseconds",
            "Configured periodic backfill interval; zero denotes no interval",
        ),
        (
            "decode_latency_ns",
            "nanoseconds",
            "Host notification-to-decode latency",
        ),
        (
            "host_receive_timestamp_ns",
            "nanoseconds",
            "Monotonic time since the native measurement-session origin",
        ),
        (
            "encoding_code",
            "code",
            "0 = device Float32 frame; 1 = device Int32 frame",
        ),
    ] {
        append_vernier_channel(
            append_child,
            append_child_value,
            channels,
            label,
            unit,
            "RecordingDiagnostics",
            &[("description", detail.into())],
        );
    }
}

fn append_vernier_breathing_metadata(
    api: &LslApi,
    info: StreamInfo,
    schema: &VernierStreamSchema,
    palette: Option<&SourcePalette>,
) -> bool {
    let (Some(get_description), Some(append_child), Some(append_child_value)) = (
        api.get_description,
        api.append_child,
        api.append_child_value,
    ) else {
        return false;
    };
    let description = unsafe { get_description(info) };
    if description.is_null() {
        return false;
    }
    append_value(append_child_value, description, "manufacturer", "Vernier");
    append_value(
        append_child_value,
        description,
        "model",
        schema.model_code(),
    );
    append_value(
        append_child_value,
        description,
        "application",
        "Polar Stream",
    );
    append_source_palette(append_child, append_child_value, description, palette);
    append_value(
        append_child_value,
        description,
        "stream_role",
        "derived_breathing_waveform",
    );
    append_value(
        append_child_value,
        description,
        "source",
        "GDX-RB Force (N)",
    );
    if !append_vernier_breathing_processing(append_child, append_child_value, description) {
        return false;
    }
    append_value(
        append_child_value,
        description,
        "interpretation",
        "Relative belt-force waveform, not lung volume or a clinical measurement.",
    );
    let Ok(channels_name) = CString::new("channels") else {
        return false;
    };
    let channels = unsafe { append_child(description, channels_name.as_ptr()) };
    append_vernier_channel(
        append_child,
        append_child_value,
        channels,
        "Vernier breathing waveform",
        "0-1",
        "DerivedRespiration",
        &[],
    );
    true
}

fn append_vernier_breathing_processing(
    append_child: AppendChild,
    append_child_value: AppendChildValue,
    description: XmlElement,
) -> bool {
    let Ok(name) = CString::new("processing") else {
        return false;
    };
    let processing = unsafe { append_child(description, name.as_ptr()) };
    if processing.is_null() {
        return false;
    }
    VernierBreathingProvenance
        .fields()
        .into_iter()
        .all(|field| append_value_checked(append_child_value, processing, field.name, &field.value))
}

fn append_vernier_channel(
    append_child: AppendChild,
    append_child_value: AppendChildValue,
    channels: XmlElement,
    label: &str,
    unit: &str,
    channel_type: &str,
    extra: &[(&str, String)],
) {
    if channels.is_null() {
        return;
    }
    let Ok(channel_name) = CString::new("channel") else {
        return;
    };
    let channel = unsafe { append_child(channels, channel_name.as_ptr()) };
    if channel.is_null() {
        return;
    }
    append_value(append_child_value, channel, "label", label);
    append_value(append_child_value, channel, "unit", unit);
    append_value(append_child_value, channel, "type", channel_type);
    for (name, value) in extra {
        append_value(append_child_value, channel, name, value);
    }
}

fn append_custom_metadata(
    api: &LslApi,
    info: StreamInfo,
    formula: &CustomFormulaConfig,
    palette: Option<&SourcePalette>,
) {
    let (Some(get_description), Some(append_child), Some(append_child_value)) = (
        api.get_description,
        api.append_child,
        api.append_child_value,
    ) else {
        return;
    };
    let description = unsafe { get_description(info) };
    if description.is_null() {
        return;
    }
    append_value(append_child_value, description, "manufacturer", "Polar");
    append_value(append_child_value, description, "model", "H10");
    append_value(
        append_child_value,
        description,
        "application",
        "Polar Stream",
    );
    append_source_palette(append_child, append_child_value, description, palette);

    let Ok(channels_name) = CString::new("channels") else {
        return;
    };
    let channels = unsafe { append_child(description, channels_name.as_ptr()) };
    let Ok(channel_name) = CString::new("channel") else {
        return;
    };
    let channel = unsafe { append_child(channels, channel_name.as_ptr()) };
    append_value(append_child_value, channel, "label", &formula.name);
    append_value(append_child_value, channel, "unit", &formula.unit);
    append_value(
        append_child_value,
        channel,
        "type",
        formula.source.stream_type(),
    );

    let Ok(processing_name) = CString::new("processing") else {
        return;
    };
    let processing = unsafe { append_child(description, processing_name.as_ptr()) };
    append_value(
        append_child_value,
        processing,
        "formula",
        &formula.expression,
    );
    append_value(
        append_child_value,
        processing,
        "source",
        &format!("{:?}", formula.source),
    );
    append_value(append_child_value, processing, "formula_id", &formula.id);
}

fn append_source_palette(
    append_child: AppendChild,
    append_child_value: AppendChildValue,
    description: XmlElement,
    palette: Option<&SourcePalette>,
) {
    let Some(palette) = palette else { return };
    let Ok(name) = CString::new("source_palette") else {
        return;
    };
    let node = unsafe { append_child(description, name.as_ptr()) };
    if node.is_null() {
        return;
    }
    for (field, value) in palette.metadata_fields() {
        append_value(append_child_value, node, field, value);
    }
}

fn append_value(append_child_value: AppendChildValue, parent: XmlElement, name: &str, value: &str) {
    let _ = append_value_checked(append_child_value, parent, name, value);
}

fn append_value_checked(
    append_child_value: AppendChildValue,
    parent: XmlElement,
    name: &str,
    value: &str,
) -> bool {
    let (Ok(name), Ok(value)) = (CString::new(name), CString::new(value)) else {
        return false;
    };
    // SAFETY: parent is owned by the live streaminfo and strings live through
    // the call. liblsl returns a child owned by the same XML document.
    !unsafe { append_child_value(parent, name.as_ptr(), value.as_ptr()) }.is_null()
}

fn channel_labels(spec: MetricSpec) -> Vec<&'static str> {
    if spec.id == "raw_acc" && spec.channels == 3 {
        vec!["X", "Y", "Z"]
    } else {
        vec![spec.label]
    }
}

impl Drop for LslPublisher {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaved_sparse_rows_do_not_run_ahead_of_the_source_clock() {
        let mut outlet = LslOutlet {
            handle: std::ptr::null_mut(),
            rate_hz: 0.0,
            last_newest_timestamp: None,
        };
        let mut previous = f64::NEG_INFINITY;
        for tick in 0..100 {
            let newest = 100.0 + f64::from(tick) * 0.1;
            // One 20 Hz Vernier raw batch followed by the matching derived batch.
            for candidate in [newest - 0.05, newest, newest - 0.05, newest] {
                let published = outlet.monotonic_sparse_row(candidate);
                assert!(published > previous);
                previous = published;
            }
            assert!(previous - newest < 0.001);
        }
    }

    #[test]
    fn mini_acc_methods_have_stable_separate_names_and_single_stream_channels() {
        let ids = [
            "phan_breath_event",
            "phan_breath_rate",
            "flowborne_phase",
            "flowborne_motion_score",
        ];
        let selected = ids.map(str::to_string).to_vec();
        let channels = polar_combined_channels(&polar_combined_metric_ids(&selected));
        for (id, suffix) in [
            ("phan_breath_event", "phanBreathEvent"),
            ("phan_breath_rate", "phanBreathRate"),
            ("flowborne_phase", "flowbornePhase"),
            ("flowborne_motion_score", "flowborneMotionScore"),
        ] {
            let spec = MetricSpec::for_id(id).unwrap();
            assert_eq!(spec.suffix(), suffix);
            assert!(channels.iter().any(|channel| channel.label == suffix));
        }
    }

    unsafe extern "C" fn unavailable_child(
        _parent: XmlElement,
        _name: *const c_char,
    ) -> XmlElement {
        std::ptr::null_mut()
    }

    unsafe extern "C" fn unavailable_value(
        _parent: XmlElement,
        _name: *const c_char,
        _value: *const c_char,
    ) -> XmlElement {
        std::ptr::null_mut()
    }

    #[test]
    fn required_respiration_processing_metadata_fails_closed() {
        let provenance = PolarRespirationProvenance::new(Default::default());
        assert!(!append_polar_respiration_processing(
            unavailable_child,
            unavailable_value,
            std::ptr::null_mut(),
            MetricSpec::for_id("breathing_volume").unwrap(),
            Some(&provenance),
        ));
        assert!(append_polar_respiration_processing(
            unavailable_child,
            unavailable_value,
            std::ptr::null_mut(),
            MetricSpec::for_id("raw_acc").unwrap(),
            Some(&provenance),
        ));
    }

    #[test]
    fn required_vernier_breathing_processing_metadata_fails_closed() {
        assert!(!append_vernier_breathing_processing(
            unavailable_child,
            unavailable_value,
            std::ptr::null_mut(),
        ));
    }
}
