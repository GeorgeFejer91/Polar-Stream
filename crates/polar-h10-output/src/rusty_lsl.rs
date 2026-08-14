use std::{
    collections::HashMap,
    io::ErrorKind,
    net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket},
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use polar_h10_core::AccSample;
use rusty_lsl::{
    ACCEPTED_FEATURE_LOCK_FINGERPRINT, DOCUMENTED_IPV4_MULTICAST_GROUP,
    DOCUMENTED_IPV4_MULTICAST_PORT, MetadataNodeInput, MetadataTree, MetadataTreeLimits,
    MetadataXmlProjectionLimits, NominalSampleRate, PersistentFloat32Outlet,
    PersistentFloat32OutletActivation, PersistentFloat32OutletId, PersistentFloat32OutletLimits,
    PersistentFloat32OutletRegistrationError, PersistentFloat32OutletRegistry,
    PersistentFloat32OutletRegistryLimits, PersistentFloat32OutletServiceCreateError,
    PersistentFloat32OutletServiceLimits, PersistentFloat32StreamInfo,
    PersistentFloat32StreamInfoInput, PersistentFloat32StreamInfoLimits, RawSourceTimestamp,
    RuntimeActivationAdmission, RuntimeActivationSelection, RuntimeModule,
    ShortInfoQueryWireLimits, ShortInfoResponderActivation, ShortInfoResponseEnvelopeLimits,
    StreamDescriptorLimits, StreamHandshakeActivation, StreamHandshakeIdentity,
    StreamHandshakeLimits, StreamInfoObservedAdmissionLimits, StreamInfoObservedDocumentLimit,
    StreamInfoObservedDocumentParseLimit, StreamInfoStaticXmlLimits, StreamInfoVolatileFieldLimits,
    StreamInfoVolatileXmlLimits, TimestampedFloat32SampleActivation,
    TimestampedFloat32SampleLimits, XmlCharacterDataLimit, XmlElementTreeLimits, XmlNameLimit,
    XmlTextLimit, admit_runtime_activation, persistent_float32_local_clock,
};

use crate::{MetricSpec, output_stream_name};

const RUSTY_LSL_REVISION: &str = "74f7d0ea2cce9b3d049ea24602527a5f52360554";
const ACTIVATION_CONSUMER: &str = "polar-stream-rusty-lsl-optional-backend-v1";
const INTERFACE_ENV: &str = "POLAR_STREAM_RUSTY_LSL_IPV4";
const MAX_OUTLETS: usize = 64;
const MAX_RECORDS_PER_CHUNK: usize = 256;
const MAX_CONSUMERS_PER_OUTLET: usize = 4;
const MAX_STREAM_INFO_BYTES: usize = 32 * 1024;
const OUTLET_BIND_ATTEMPTS: usize = 4;

static NEXT_OUTLET_UID: AtomicU64 = AtomicU64::new(1);

struct OutletEntry {
    id: PersistentFloat32OutletId,
    channels: usize,
    rate_hz: f64,
}

pub(crate) struct RustyLslPublisher {
    admission: Option<RuntimeActivationAdmission>,
    registry: Option<PersistentFloat32OutletRegistry>,
    outlets: HashMap<String, OutletEntry>,
    status: String,
    values: Vec<f32>,
    timestamps: Vec<RawSourceTimestamp>,
    cancelled: AtomicBool,
    discovery_override: Option<UdpSocket>,
    advertised_ipv4: Ipv4Addr,
    initialization_failed: bool,
}

impl RustyLslPublisher {
    pub(crate) fn new(_bundled_library: Option<PathBuf>) -> Self {
        match advertised_ipv4() {
            Ok(interface) => Self::with_optional_discovery(None, interface),
            Err(message) => Self::failed(message),
        }
    }

    fn with_optional_discovery(
        discovery_override: Option<UdpSocket>,
        advertised_ipv4: Ipv4Addr,
    ) -> Self {
        match runtime_admission() {
            Ok(admission) => Self {
                admission: Some(admission),
                registry: None,
                outlets: HashMap::new(),
                status: format!(
                    "Optional Rusty LSL backend on {advertised_ipv4} ({})",
                    short_revision()
                ),
                values: Vec::with_capacity(MAX_RECORDS_PER_CHUNK * 3),
                timestamps: Vec::with_capacity(MAX_RECORDS_PER_CHUNK),
                cancelled: AtomicBool::new(false),
                discovery_override,
                advertised_ipv4,
                initialization_failed: false,
            },
            Err(message) => Self::failed(message),
        }
    }

    fn failed(message: String) -> Self {
        Self {
            admission: None,
            registry: None,
            outlets: HashMap::new(),
            status: message,
            values: Vec::with_capacity(MAX_RECORDS_PER_CHUNK * 3),
            timestamps: Vec::with_capacity(MAX_RECORDS_PER_CHUNK),
            cancelled: AtomicBool::new(false),
            discovery_override: None,
            advertised_ipv4: Ipv4Addr::LOCALHOST,
            initialization_failed: true,
        }
    }

    pub(crate) fn status(&self) -> &str {
        &self.status
    }

    pub(crate) fn clear(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(registry) = self.registry.take() {
            let _ = registry.close();
        }
        self.outlets.clear();
        self.values.clear();
        self.timestamps.clear();
        self.cancelled = AtomicBool::new(false);
        self.initialization_failed = self.admission.is_none();
        if self.admission.is_some() {
            self.status = format!(
                "Optional Rusty LSL backend on {} ({})",
                self.advertised_ipv4,
                short_revision()
            );
        }
    }

    pub(crate) fn add_outlet(&mut self, base_name: &str, spec: MetricSpec) {
        if let Err(message) = self.try_add_outlet(base_name, spec) {
            self.initialization_failed = true;
            self.status = message;
        }
    }

    fn try_add_outlet(&mut self, base_name: &str, spec: MetricSpec) -> Result<(), String> {
        self.ensure_registry()?;
        let output_name = output_stream_name(base_name, spec.id)
            .ok_or_else(|| format!("Unknown output module: {}", spec.id))?;
        let channels = usize::try_from(spec.channels)
            .ok()
            .filter(|channels| *channels > 0)
            .ok_or_else(|| format!("Invalid channel count for {}", spec.label))?;
        let nominal_rate = if spec.rate_hz > 0.0 {
            NominalSampleRate::regular_hz(spec.rate_hz)
                .map_err(|error| format!("Rusty LSL sample rate rejected: {error:?}"))?
        } else {
            NominalSampleRate::irregular()
        };
        let outlet_limits =
            PersistentFloat32OutletLimits::new(MAX_RECORDS_PER_CHUNK, MAX_CONSUMERS_PER_OUTLET)
                .map_err(|error| format!("Rusty LSL outlet limits rejected: {error:?}"))?;
        let mut id = None;
        for attempt in 1..=OUTLET_BIND_ATTEMPTS {
            let handshake_limits = handshake_limits();
            let identity = StreamHandshakeIdentity::new(
                next_uid(),
                "polar-stream".into(),
                format!("polar-h10-{output_name}"),
                "default".into(),
                handshake_limits,
            )
            .map_err(|error| format!("Rusty LSL identity rejected: {error:?}"))?;
            let listener = TcpListener::bind((self.advertised_ipv4, 0))
                .map_err(|error| format!("Rusty LSL outlet bind failed: {error}"))?;
            let outlet = PersistentFloat32Outlet::new(
                self.outlet_activation()?,
                listener,
                identity,
                handshake_limits,
                sample_limits(),
                channels,
                outlet_limits,
            )
            .map_err(|error| format!("Rusty LSL outlet creation failed: {error:?}"))?;
            let stream_info = PersistentFloat32StreamInfo::compose(
                &outlet,
                self.advertised_ipv4,
                PersistentFloat32StreamInfoInput::new(
                    output_name.clone(),
                    spec.stream_type.into(),
                    nominal_rate,
                    stream_metadata(spec)?,
                ),
                stream_info_limits(),
            )
            .map_err(|error| format!("Rusty LSL stream info rejected: {error:?}"))?;
            let registration = self
                .registry
                .as_mut()
                .ok_or_else(|| "Rusty LSL registry is unavailable".to_owned())?
                .register_stream_info(outlet, stream_info);
            match registration {
                Ok(registered) => {
                    id = Some(registered);
                    break;
                }
                Err(error) if retryable_timedata_bind(&error) && attempt < OUTLET_BIND_ATTEMPTS => {
                }
                Err(error) => {
                    return Err(format!(
                        "Rusty LSL outlet registration failed after {attempt} attempt(s): {error:?}"
                    ));
                }
            }
        }
        let id = id.ok_or_else(|| "Rusty LSL outlet registration exhausted retries".to_owned())?;
        self.outlets.insert(
            spec.id.into(),
            OutletEntry {
                id,
                channels,
                rate_hz: spec.rate_hz,
            },
        );
        self.refresh_status();
        Ok(())
    }

    fn ensure_registry(&mut self) -> Result<(), String> {
        if self.initialization_failed {
            return Err(self.status.clone());
        }
        if self.registry.is_some() {
            return Ok(());
        }
        let discovery = if let Some(discovery) = self.discovery_override.take() {
            discovery
        } else {
            let discovery =
                UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DOCUMENTED_IPV4_MULTICAST_PORT))
                    .map_err(|error| format!("Rusty LSL discovery bind failed: {error}"))?;
            discovery
                .join_multicast_v4(&DOCUMENTED_IPV4_MULTICAST_GROUP, &self.advertised_ipv4)
                .map_err(|error| format!("Rusty LSL discovery join failed: {error}"))?;
            discovery
        };
        let registry = PersistentFloat32OutletRegistry::new_prebound(
            self.responder_activation()?,
            self.advertised_ipv4,
            discovery,
            PersistentFloat32OutletRegistryLimits::new(MAX_OUTLETS, service_limits())
                .map_err(|error| format!("Rusty LSL registry limits rejected: {error:?}"))?,
        )
        .map_err(|error| format!("Rusty LSL registry creation failed: {error:?}"))?;
        self.registry = Some(registry);
        Ok(())
    }

    fn outlet_activation(&self) -> Result<PersistentFloat32OutletActivation, String> {
        let admission = self.admission.as_ref().ok_or_else(|| self.status.clone())?;
        let handshake = StreamHandshakeActivation::new(
            admission
                .capability(RuntimeModule::StreamHandshake)
                .ok_or_else(|| "Rusty LSL stream-handshake capability is absent".to_owned())?,
        )
        .map_err(|error| format!("Rusty LSL handshake activation failed: {error:?}"))?;
        let sample = TimestampedFloat32SampleActivation::new(
            admission
                .capability(RuntimeModule::TimestampedFloat32Sample)
                .ok_or_else(|| "Rusty LSL Float32 capability is absent".to_owned())?,
            handshake,
        )
        .map_err(|error| format!("Rusty LSL sample activation failed: {error:?}"))?;
        PersistentFloat32OutletActivation::new(
            admission
                .capability(RuntimeModule::PersistentFloat32Outlet)
                .ok_or_else(|| "Rusty LSL persistent-outlet capability is absent".to_owned())?,
            sample,
        )
        .map_err(|error| format!("Rusty LSL outlet activation failed: {error:?}"))
    }

    fn responder_activation(&self) -> Result<ShortInfoResponderActivation, String> {
        let admission = self.admission.as_ref().ok_or_else(|| self.status.clone())?;
        ShortInfoResponderActivation::new(
            admission
                .capability(RuntimeModule::ShortInfoDiscoveryResponder)
                .ok_or_else(|| "Rusty LSL discovery capability is absent".to_owned())?,
        )
        .map_err(|error| format!("Rusty LSL discovery activation failed: {error:?}"))
    }

    pub(crate) fn poll(&mut self) -> Option<String> {
        let registry = self.registry.as_mut()?;
        match registry.poll(&self.cancelled) {
            Ok(result) => {
                if !result.is_idle() {
                    self.refresh_status();
                }
                None
            }
            Err(error) => self.record_poll_error(format!("Rusty LSL poll failed: {error:?}")),
        }
    }

    pub(crate) fn push_scalar(&mut self, id: &str, value: f32) {
        self.push_values(id, &[value], 1);
    }

    pub(crate) fn push_scalar_series<I>(&mut self, id: &str, values: I)
    where
        I: IntoIterator<Item = f32>,
    {
        self.values.clear();
        self.values.extend(values);
        if self.values.is_empty() {
            return;
        }
        self.push_buffered(id, 1);
    }

    pub(crate) fn push_accelerometer(&mut self, samples: &[AccSample]) {
        self.values.clear();
        self.values.reserve(samples.len().saturating_mul(3));
        for sample in samples {
            self.values.extend([
                f32::from(sample.x_mg),
                f32::from(sample.y_mg),
                f32::from(sample.z_mg),
            ]);
        }
        if self.values.is_empty() {
            return;
        }
        self.push_buffered("raw_acc", 3);
    }

    fn push_buffered(&mut self, id: &str, channels: usize) {
        let values = std::mem::take(&mut self.values);
        self.push_values(id, &values, channels);
        self.values = values;
    }

    fn push_values(&mut self, id: &str, values: &[f32], channels: usize) {
        let Some(entry) = self.outlets.get(id) else {
            return;
        };
        let (outlet_id, expected_channels, rate_hz) = (entry.id, entry.channels, entry.rate_hz);
        if channels != expected_channels || !values.len().is_multiple_of(channels) {
            self.status = format!("Rusty LSL rejected malformed {id} chunk shape");
            return;
        }
        let records = values.len() / channels;
        if records == 0 || records > MAX_RECORDS_PER_CHUNK {
            self.status = format!("Rusty LSL rejected {id} chunk with {records} records");
            return;
        }
        self.timestamps.clear();
        let newest = persistent_float32_local_clock();
        for index in 0..records {
            let backfill = if rate_hz > 0.0 {
                (records - index - 1) as f64 / rate_hz
            } else {
                0.0
            };
            let Ok(timestamp) = RawSourceTimestamp::new(newest - backfill) else {
                self.status = format!("Rusty LSL could not timestamp {id}");
                return;
            };
            self.timestamps.push(timestamp);
        }
        let Some(registry) = self.registry.as_mut() else {
            return;
        };
        match registry.try_push_chunk(outlet_id, values, &self.timestamps, &self.cancelled) {
            Some(Ok(report)) if report.failed_consumers() == 0 => {}
            Some(Ok(report)) => {
                self.status = format!(
                    "Rusty LSL evicted {} stalled consumer(s) for {id}",
                    report.failed_consumers()
                );
            }
            Some(Err(error)) => {
                self.status = format!("Rusty LSL {id} push failed: {error:?}");
            }
            None => {
                self.status = format!("Rusty LSL outlet disappeared for {id}");
            }
        }
    }

    fn refresh_status(&mut self) {
        let consumers = self.registry.as_ref().map_or(0, |registry| {
            self.outlets
                .values()
                .filter_map(|entry| registry.outlet_health(entry.id))
                .map(|health| health.connected_consumers())
                .sum::<usize>()
        });
        self.status = format!(
            "Optional Rusty LSL backend on {} · {} stream(s) · {consumers} consumer(s) · {}",
            self.advertised_ipv4,
            self.outlets.len(),
            short_revision()
        );
    }

    fn record_poll_error(&mut self, message: String) -> Option<String> {
        if self.status == message {
            None
        } else {
            self.status.clone_from(&message);
            Some(message)
        }
    }

    #[cfg(test)]
    fn new_prebound_for_test(discovery: UdpSocket) -> Self {
        Self::with_optional_discovery(Some(discovery), Ipv4Addr::LOCALHOST)
    }

    #[cfg(test)]
    fn test_endpoint(&self, id: &str) -> Option<std::net::SocketAddr> {
        let entry = self.outlets.get(id)?;
        self.registry.as_ref()?.outlet_local_address(entry.id)
    }

    #[cfg(test)]
    fn test_outlet_health(&self, id: &str) -> Option<rusty_lsl::PersistentFloat32OutletHealth> {
        let entry = self.outlets.get(id)?;
        self.registry.as_ref()?.outlet_health(entry.id)
    }
}

impl Drop for RustyLslPublisher {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(registry) = self.registry.take() {
            let _ = registry.close();
        }
    }
}

fn retryable_timedata_bind(error: &PersistentFloat32OutletRegistrationError) -> bool {
    matches!(
        error,
        PersistentFloat32OutletRegistrationError::Service(
            PersistentFloat32OutletServiceCreateError::BindTimedata(
                ErrorKind::AddrInUse | ErrorKind::PermissionDenied
            )
        )
    )
}

fn advertised_ipv4() -> Result<Ipv4Addr, String> {
    if let Some(value) = std::env::var_os(INTERFACE_ENV) {
        let value = value
            .into_string()
            .map_err(|_| format!("{INTERFACE_ENV} must contain UTF-8 text"))?;
        return parse_advertised_ipv4(value.trim())
            .map_err(|message| format!("{INTERFACE_ENV} is invalid: {message}"));
    }

    // A connected UDP socket selects the operating system's route without
    // sending a datagram. Querying that local address gives the concrete IPv4
    // interface liblsl peers must use for discovery, timedata, and TCP data.
    let route = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|error| format!("Rusty LSL interface probe bind failed: {error}"))?;
    route
        .connect((
            DOCUMENTED_IPV4_MULTICAST_GROUP,
            DOCUMENTED_IPV4_MULTICAST_PORT,
        ))
        .map_err(|error| format!("Rusty LSL interface route selection failed: {error}"))?;
    let address = route
        .local_addr()
        .map_err(|error| format!("Rusty LSL interface inspection failed: {error}"))?;
    match address.ip() {
        IpAddr::V4(value) => parse_advertised_ipv4(&value.to_string())
            .map_err(|message| format!("Rusty LSL selected interface is invalid: {message}")),
        IpAddr::V6(_) => Err("Rusty LSL requires a concrete IPv4 interface".into()),
    }
}

fn parse_advertised_ipv4(value: &str) -> Result<Ipv4Addr, String> {
    let value = value
        .parse::<Ipv4Addr>()
        .map_err(|_| "expected an IPv4 address".to_owned())?;
    if value.is_unspecified() || value.is_multicast() || value == Ipv4Addr::BROADCAST {
        return Err("address must be a concrete unicast IPv4 interface".into());
    }
    Ok(value)
}

fn runtime_admission() -> Result<RuntimeActivationAdmission, String> {
    let selections = [
        RuntimeModule::StreamHandshake,
        RuntimeModule::TimestampedFloat32Sample,
        RuntimeModule::PersistentFloat32Outlet,
        RuntimeModule::ShortInfoDiscoveryResponder,
    ]
    .map(|module| RuntimeActivationSelection::new(module.id(), module.effective_marker()));
    admit_runtime_activation(
        ACCEPTED_FEATURE_LOCK_FINGERPRINT,
        ACTIVATION_CONSUMER,
        &selections,
    )
    .map_err(|error| format!("Rusty LSL runtime activation rejected: {error:?}"))
}

fn handshake_limits() -> StreamHandshakeLimits {
    StreamHandshakeLimits::new(
        4096,
        256,
        Duration::from_millis(2),
        Duration::from_millis(100),
    )
    .expect("static Rusty LSL handshake limits must be valid")
}

fn sample_limits() -> TimestampedFloat32SampleLimits {
    TimestampedFloat32SampleLimits::new(Duration::from_millis(2), Duration::from_millis(100))
        .expect("static Rusty LSL sample limits must be valid")
}

fn service_limits() -> PersistentFloat32OutletServiceLimits {
    PersistentFloat32OutletServiceLimits::new(
        4096,
        StreamInfoObservedDocumentParseLimit::new(MAX_STREAM_INFO_BYTES)
            .expect("static stream-info parse limit must be valid"),
        StreamInfoObservedAdmissionLimits::new(
            StreamDescriptorLimits::new(128, 128, 128, 64)
                .expect("static descriptor limits must be valid"),
            MetadataTreeLimits::new(96, 8, 64, 64, 1024)
                .expect("static metadata limits must be valid"),
            StreamInfoVolatileFieldLimits::new(256, 256, 256)
                .expect("static volatile limits must be valid"),
        ),
        ShortInfoQueryWireLimits::new(512, 1024).expect("static query limits must be valid"),
        ShortInfoResponseEnvelopeLimits::new(MAX_STREAM_INFO_BYTES, MAX_STREAM_INFO_BYTES + 512)
            .expect("static response limits must be valid"),
    )
    .expect("static Rusty LSL service limits must be valid")
}

fn stream_info_limits() -> PersistentFloat32StreamInfoLimits {
    let name = XmlNameLimit::new(64).expect("static XML name limit must be valid");
    let text = XmlTextLimit::new(1024).expect("static XML text limit must be valid");
    let character_data = XmlCharacterDataLimit::new(MAX_STREAM_INFO_BYTES)
        .expect("static XML character-data limit must be valid");
    PersistentFloat32StreamInfoLimits::new(
        StreamDescriptorLimits::new(128, 128, 128, 64)
            .expect("static descriptor limits must be valid"),
        StreamInfoStaticXmlLimits::new(
            name,
            text,
            character_data,
            XmlElementTreeLimits::new(7, 2, 6, 4096).expect("static XML limits must be valid"),
        ),
        MetadataXmlProjectionLimits::new(
            name,
            text,
            character_data,
            XmlElementTreeLimits::new(96, 8, 64, 16_384)
                .expect("metadata XML limits must be valid"),
        ),
        XmlElementTreeLimits::new(112, 9, 72, 20_480)
            .expect("description XML limits must be valid"),
        StreamInfoVolatileFieldLimits::new(256, 256, 256)
            .expect("static volatile limits must be valid"),
        StreamInfoVolatileXmlLimits::new(
            name,
            text,
            character_data,
            XmlElementTreeLimits::new(12, 2, 11, 4096).expect("volatile XML limits must be valid"),
        ),
        XmlElementTreeLimits::new(144, 10, 80, MAX_STREAM_INFO_BYTES)
            .expect("ordered XML limits must be valid"),
        StreamInfoObservedDocumentLimit::new(MAX_STREAM_INFO_BYTES)
            .expect("document limit must be valid"),
    )
}

fn stream_metadata(spec: MetricSpec) -> Result<MetadataTree, String> {
    let mut nodes = Vec::with_capacity(5 + usize::try_from(spec.channels).unwrap_or(0) * 4);
    nodes.push(MetadataNodeInput::new(None, "desc".into(), None));
    nodes.push(MetadataNodeInput::new(
        Some(0),
        "manufacturer".into(),
        Some("Polar".into()),
    ));
    nodes.push(MetadataNodeInput::new(
        Some(0),
        "model".into(),
        Some("H10".into()),
    ));
    nodes.push(MetadataNodeInput::new(
        Some(0),
        "application".into(),
        Some("Polar Stream".into()),
    ));
    nodes.push(MetadataNodeInput::new(Some(0), "channels".into(), None));
    for label in channel_labels(spec) {
        let channel = nodes.len();
        nodes.push(MetadataNodeInput::new(Some(4), "channel".into(), None));
        nodes.push(MetadataNodeInput::new(
            Some(channel),
            "label".into(),
            Some(label.into()),
        ));
        nodes.push(MetadataNodeInput::new(
            Some(channel),
            "unit".into(),
            Some(spec.unit.into()),
        ));
        nodes.push(MetadataNodeInput::new(
            Some(channel),
            "type".into(),
            Some(spec.stream_type.into()),
        ));
    }
    MetadataTree::new(
        MetadataTreeLimits::new(96, 8, 64, 64, 1024).expect("static metadata limits must be valid"),
        nodes,
    )
    .map_err(|error| format!("Rusty LSL metadata rejected: {error:?}"))
}

fn channel_labels(spec: MetricSpec) -> Vec<&'static str> {
    if spec.id == "raw_acc" && spec.channels == 3 {
        vec!["X", "Y", "Z"]
    } else {
        vec![spec.label]
    }
}

fn next_uid() -> String {
    let sequence = NEXT_OUTLET_UID.fetch_add(1, Ordering::Relaxed);
    let clock = persistent_float32_local_clock().to_bits();
    let raw = (u128::from(clock) << 64) | u128::from(sequence);
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (raw >> 96) as u32,
        ((raw >> 80) & 0xffff) as u16,
        ((raw >> 64) & 0x0fff) as u16,
        ((raw >> 48) & 0x0fff) as u16,
        raw & 0xffff_ffff_ffff
    )
}

fn short_revision() -> &'static str {
    &RUSTY_LSL_REVISION[..12]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polar_shapes_enter_two_bounded_outlets_without_browser_or_device_input() {
        let discovery = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut publisher = RustyLslPublisher::new_prebound_for_test(discovery);
        publisher.add_outlet("participant_07", MetricSpec::for_id("raw_ecg").unwrap());
        publisher.add_outlet("participant_07", MetricSpec::for_id("raw_acc").unwrap());

        assert_ne!(
            publisher.test_endpoint("raw_ecg"),
            publisher.test_endpoint("raw_acc")
        );
        assert!(publisher.poll().is_none());

        let ecg = (0..73).map(|value| value as f32 - 36.0).collect::<Vec<_>>();
        publisher.push_scalar_series("raw_ecg", ecg.iter().copied());
        assert_eq!(publisher.values, ecg);
        assert_eq!(publisher.timestamps.len(), 73);
        let ecg_health = publisher.test_outlet_health("raw_ecg").unwrap();
        assert_eq!(ecg_health.push_calls(), 1);
        assert_eq!(ecg_health.records_encoded(), 73);
        assert_eq!(ecg_health.complete_deliveries(), 0);

        let acc = (0..36)
            .map(|value| AccSample {
                x_mg: value,
                y_mg: -value,
                z_mg: value + 100,
            })
            .collect::<Vec<_>>();
        publisher.push_accelerometer(&acc);
        let expected_acc = acc
            .iter()
            .flat_map(|sample| {
                [
                    f32::from(sample.x_mg),
                    f32::from(sample.y_mg),
                    f32::from(sample.z_mg),
                ]
            })
            .collect::<Vec<_>>();
        assert_eq!(publisher.values, expected_acc);
        assert_eq!(publisher.timestamps.len(), 36);
        let acc_health = publisher.test_outlet_health("raw_acc").unwrap();
        assert_eq!(acc_health.push_calls(), 1);
        assert_eq!(acc_health.records_encoded(), 36);
        assert_eq!(acc_health.complete_deliveries(), 0);
        assert!(publisher.status().contains("2 stream(s)"));
    }

    #[test]
    fn advertised_interface_parser_rejects_nonconcrete_or_wrong_shape_values() {
        assert_eq!(
            parse_advertised_ipv4("192.0.2.44").unwrap(),
            Ipv4Addr::new(192, 0, 2, 44)
        );
        assert!(parse_advertised_ipv4("0.0.0.0").is_err());
        assert!(parse_advertised_ipv4("239.255.172.215").is_err());
        assert!(parse_advertised_ipv4("255.255.255.255").is_err());
        assert!(parse_advertised_ipv4("not-an-address").is_err());
    }

    #[test]
    fn clearing_a_failed_backend_preserves_its_root_diagnostic() {
        let mut publisher = RustyLslPublisher::failed("root interface failure".into());
        publisher.clear();
        assert_eq!(publisher.status(), "root interface failure");
    }
}
