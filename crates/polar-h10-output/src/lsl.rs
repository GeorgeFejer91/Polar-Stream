use std::{
    collections::HashMap,
    ffi::{CString, c_char, c_double, c_float, c_int, c_ulong, c_void},
    path::{Path, PathBuf},
};

use libloading::Library;
use polar_h10_core::AccSample;

use crate::{CustomFormulaConfig, MetricSpec, custom_output_stream_name, output_stream_name};

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
    sensor_clock: crate::SensorClockMap,
}

unsafe impl Send for LslOutlet {}

pub(crate) struct LslPublisher {
    api: Option<LslApi>,
    outlets: HashMap<String, LslOutlet>,
    status: String,
    scratch: Vec<f32>,
}

impl LslPublisher {
    pub(crate) fn new(bundled_library: Option<PathBuf>) -> Self {
        match LslApi::load(bundled_library.as_deref()) {
            Ok(api) => Self {
                api: Some(api),
                outlets: HashMap::new(),
                status: "Ready".into(),
                scratch: Vec::with_capacity(512),
            },
            Err(error) => Self {
                api: None,
                outlets: HashMap::new(),
                status: error,
                scratch: Vec::with_capacity(512),
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
                sensor_clock: crate::SensorClockMap::default(),
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
                sensor_clock: crate::SensorClockMap::default(),
            },
        );
        self.status = format!("Publishing {} stream(s)", self.outlets.len());
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
        let newest = outlet
            .sensor_clock
            .map_newest(newest_timestamp_ns, local_now);
        let count = self.scratch.len();
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
        let newest = sensor_timestamp_ns.map_or(local_now, |sensor_timestamp_ns| {
            outlet
                .sensor_clock
                .map_newest(sensor_timestamp_ns, local_now)
        });
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
            let sample_count = self.scratch.len() / channels;
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
