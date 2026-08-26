use std::{
    collections::HashMap,
    ffi::{CString, c_char, c_double, c_float, c_int, c_ulong, c_void},
    path::{Path, PathBuf},
};

use libloading::Library;
use polar_h10_core::AccSample;

use crate::{
    CustomFormulaConfig, MetricSpec, VERNIER_BREATHING_OUTLET_KEY, VERNIER_RAW_OUTLET_KEY,
    VernierStreamSchema, custom_output_stream_name, encode_vernier_raw_rows, output_stream_name,
    vernier_breathing_stream_name, vernier_raw_stream_name,
};
use vernier_gdx_core::{SampleEncoding, SensorSamples};

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
}

unsafe impl Send for LslOutlet {}

pub(crate) struct LslPublisher {
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
                api: Some(api),
                outlets: HashMap::new(),
                source_clock: crate::SensorClockMap::default(),
                status: "Ready".into(),
                scratch: Vec::with_capacity(512),
                scratch_double: Vec::with_capacity(512),
            },
            Err(error) => Self {
                api: None,
                outlets: HashMap::new(),
                source_clock: crate::SensorClockMap::default(),
                status: error,
                scratch: Vec::with_capacity(512),
                scratch_double: Vec::with_capacity(512),
            },
        }
    }

    pub(crate) fn status(&self) -> &str {
        &self.status
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

    pub(crate) fn add_outlet(&mut self, base_name: &str, spec: MetricSpec) {
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
        append_stream_metadata(api, info, spec);
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

    pub(crate) fn add_custom_outlet(&mut self, base_name: &str, formula: &CustomFormulaConfig) {
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
        append_custom_metadata(api, info, formula);
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

    pub(crate) fn add_vernier_outlets(&mut self, base_name: &str, schema: &VernierStreamSchema) {
        self.add_vernier_raw_outlet(base_name, schema);
        let raw_ready = self.outlets.contains_key(VERNIER_RAW_OUTLET_KEY);
        let raw_status = self.status.clone();
        self.add_vernier_breathing_outlet(base_name, schema);
        let breathing_ready = self.outlets.contains_key(VERNIER_BREATHING_OUTLET_KEY);
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

    fn add_vernier_raw_outlet(&mut self, base_name: &str, schema: &VernierStreamSchema) {
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
        append_vernier_raw_metadata(api, info, schema);
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

    fn add_vernier_breathing_outlet(&mut self, base_name: &str, schema: &VernierStreamSchema) {
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
        append_vernier_breathing_metadata(api, info, schema);
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
}

fn append_stream_metadata(api: &LslApi, info: StreamInfo, spec: MetricSpec) {
    let (Some(get_description), Some(append_child), Some(append_child_value)) = (
        api.get_description,
        api.append_child,
        api.append_child_value,
    ) else {
        return;
    };
    // SAFETY: `info` remains live until after outlet creation, and every C
    // string below lives through its individual liblsl call.
    let description = unsafe { get_description(info) };
    if description.is_null() {
        return;
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

    let Ok(channels_name) = CString::new("channels") else {
        return;
    };
    let channels = unsafe { append_child(description, channels_name.as_ptr()) };
    if channels.is_null() {
        return;
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
}

fn append_vernier_raw_metadata(api: &LslApi, info: StreamInfo, schema: &VernierStreamSchema) {
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

fn append_vernier_breathing_metadata(api: &LslApi, info: StreamInfo, schema: &VernierStreamSchema) {
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
    append_value(
        append_child_value,
        description,
        "processing",
        "Causal 30 s force range; 5th/95th percentiles after 20 finite samples; clamp to 0-1.",
    );
    append_value(
        append_child_value,
        description,
        "nonfinite_policy",
        "Hold the last derived value; exact non-finite input remains in the raw stream.",
    );
    append_value(
        append_child_value,
        description,
        "inhale_direction",
        "increasing",
    );
    append_value(
        append_child_value,
        description,
        "interpretation",
        "Relative belt-force waveform, not lung volume or a clinical measurement.",
    );
    let Ok(channels_name) = CString::new("channels") else {
        return;
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

fn append_custom_metadata(api: &LslApi, info: StreamInfo, formula: &CustomFormulaConfig) {
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

fn append_value(append_child_value: AppendChildValue, parent: XmlElement, name: &str, value: &str) {
    let (Ok(name), Ok(value)) = (CString::new(name), CString::new(value)) else {
        return;
    };
    // SAFETY: parent is owned by the live streaminfo and strings live through
    // the call. liblsl returns a child owned by the same XML document.
    unsafe { append_child_value(parent, name.as_ptr(), value.as_ptr()) };
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
