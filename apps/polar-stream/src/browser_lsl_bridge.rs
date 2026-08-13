//! Authenticated loopback adapter from the hosted browser UI to native LSL.
//!
//! Browsers do not expose the UDP multicast and TCP socket primitives used by
//! LSL. This module keeps those sockets native while accepting bounded signal
//! notifications from the canonical GitHub Pages UI on an ephemeral loopback
//! port. A per-launch bearer token is delivered only in the URL fragment, and
//! the first authenticated browser client exclusively owns that bridge.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use polar_h10_core::AccSample;
use polar_h10_output::{MetricSpec, MetricValue, OutputConfig, OutputHealth, OutputRouter};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Semaphore, watch},
};
use uuid::Uuid;

const PROTOCOL_VERSION: u16 = 1;
const PAGES_ORIGIN: &str = "https://georgefejer91.github.io";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_EVENTS_PER_BATCH: usize = 64;
const MAX_SAMPLES_PER_EVENT: usize = 1_024;
const MAX_CONCURRENT_REQUESTS: usize = 8;
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct BrowserLslBridge {
    bundled_lsl: Option<PathBuf>,
    running: Mutex<Option<RunningBridge>>,
}

struct RunningBridge {
    shutdown: watch::Sender<bool>,
    _task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub(crate) struct BrowserBridgeLaunch {
    pub(crate) port: u16,
    pub(crate) token: String,
}

impl BrowserLslBridge {
    pub(crate) fn new(bundled_lsl: Option<PathBuf>) -> Self {
        Self {
            bundled_lsl,
            running: Mutex::new(None),
        }
    }

    pub(crate) async fn start(&self) -> Result<BrowserBridgeLaunch, String> {
        let mut running = self.running.lock().await;
        // One explicit desktop launch owns one browser producer. Replacing the
        // session prevents two copied tabs from interleaving research samples.
        running.take();

        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .map_err(|error| format!("Could not bind the browser LSL bridge: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("Could not read the browser LSL bridge port: {error}"))?
            .port();
        let token = Uuid::new_v4().simple().to_string();
        let expected_host = format!("127.0.0.1:{port}");
        let output = Arc::new(OutputRouter::with_bundled_lsl(self.bundled_lsl.clone()));
        let state = Arc::new(BridgeState::default());
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task_token = token.clone();
        let task = tokio::spawn(async move {
            serve(
                listener,
                expected_host,
                task_token,
                output,
                state,
                shutdown_rx,
            )
            .await;
        });
        *running = Some(RunningBridge {
            shutdown,
            _task: task,
        });
        Ok(BrowserBridgeLaunch { port, token })
    }
}

impl Drop for RunningBridge {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

#[derive(Default)]
struct BridgeState {
    client_id: StdMutex<Option<String>>,
    lsl_enabled: AtomicBool,
    accepted_events: AtomicU64,
    accepted_samples: AtomicU64,
}

async fn serve(
    listener: TcpListener,
    expected_host: String,
    token: String,
    output: Arc<OutputRouter>,
    state: Arc<BridgeState>,
    mut shutdown: watch::Receiver<bool>,
) {
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else {
                    continue;
                };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue;
                };
                let expected_host = expected_host.clone();
                let token = token.clone();
                let output = output.clone();
                let state = state.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = handle_connection(stream, &expected_host, &token, &output, &state).await;
                });
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    expected_host: &str,
    token: &str,
    output: &OutputRouter,
    state: &BridgeState,
) -> Result<(), String> {
    let request = match tokio::time::timeout(REQUEST_READ_TIMEOUT, read_request(&mut stream)).await
    {
        Ok(Ok(request)) => request,
        Ok(Err(error)) => {
            write_response(
                &mut stream,
                HttpResponse::json(400, None, &ErrorResponse::new(error)),
            )
            .await?;
            return Ok(());
        }
        Err(_) => {
            write_response(
                &mut stream,
                HttpResponse::json(
                    408,
                    None,
                    &ErrorResponse::new("The browser bridge request timed out."),
                ),
            )
            .await?;
            return Ok(());
        }
    };
    let origin = request.header("origin").map(str::to_string);
    let response = route_request(request, expected_host, token, output, state).await;
    write_response(
        &mut stream,
        response.with_origin(origin.as_deref().filter(|value| allowed_origin(value))),
    )
    .await
}

async fn route_request(
    request: HttpRequest,
    expected_host: &str,
    token: &str,
    output: &OutputRouter,
    state: &BridgeState,
) -> HttpResponse {
    let Some(origin) = request.header("origin") else {
        return HttpResponse::json(
            403,
            None,
            &ErrorResponse::new("An explicit browser origin is required."),
        );
    };
    if !allowed_origin(origin) {
        return HttpResponse::json(
            403,
            None,
            &ErrorResponse::new("That browser origin is not allowed."),
        );
    }
    if request.header("host") != Some(expected_host) {
        return HttpResponse::json(
            421,
            Some(origin),
            &ErrorResponse::new("The bridge Host header was not accepted."),
        );
    }
    if request.method == "OPTIONS" {
        return HttpResponse::empty(204, Some(origin));
    }
    let expected_authorization = format!("Bearer {token}");
    if request.header("authorization") != Some(expected_authorization.as_str()) {
        return HttpResponse::json(
            401,
            Some(origin),
            &ErrorResponse::new("Browser bridge authorization failed."),
        );
    }
    let Some(client_id) = request.header("x-polar-bridge-client") else {
        return HttpResponse::json(
            401,
            Some(origin),
            &ErrorResponse::new("A browser bridge client identifier is required."),
        );
    };
    if !valid_client_id(client_id) {
        return HttpResponse::json(
            400,
            Some(origin),
            &ErrorResponse::new("The browser bridge client identifier is invalid."),
        );
    }
    let client_allowed = state
        .client_id
        .lock()
        .map(|mut active| match active.as_deref() {
            Some(current) => current == client_id,
            None => {
                *active = Some(client_id.to_string());
                true
            }
        })
        .unwrap_or(false);
    if !client_allowed {
        return HttpResponse::json(
            409,
            Some(origin),
            &ErrorResponse::new("Another browser tab owns this LSL bridge session."),
        );
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/status") => HttpResponse::json(
            200,
            Some(origin),
            &StatusResponse {
                protocol_version: PROTOCOL_VERSION,
                lsl_enabled: state.lsl_enabled.load(Ordering::Relaxed),
                accepted_events: state.accepted_events.load(Ordering::Relaxed),
                accepted_samples: state.accepted_samples.load(Ordering::Relaxed),
            },
        ),
        ("POST", "/v1/config") => {
            if !has_json_content_type(&request) {
                return HttpResponse::json(
                    415,
                    Some(origin),
                    &ErrorResponse::new("A JSON Content-Type is required."),
                );
            }
            let mut envelope: ConfigEnvelope = match serde_json::from_slice(&request.body) {
                Ok(value) => value,
                Err(_) => {
                    return HttpResponse::json(
                        400,
                        Some(origin),
                        &ErrorResponse::new("The browser bridge configuration is malformed."),
                    );
                }
            };
            if envelope.protocol_version != PROTOCOL_VERSION {
                return HttpResponse::json(
                    409,
                    Some(origin),
                    &ErrorResponse::new("The browser bridge protocol version is incompatible."),
                );
            }
            envelope.config.osc_enabled = false;
            let enabled = envelope.config.lsl_enabled;
            match output.configure(envelope.config).await {
                Ok(health) => {
                    state.lsl_enabled.store(enabled, Ordering::Relaxed);
                    HttpResponse::json(
                        200,
                        Some(origin),
                        &ConfigResponse {
                            protocol_version: PROTOCOL_VERSION,
                            health,
                        },
                    )
                }
                Err(message) => HttpResponse::json(422, Some(origin), &ErrorResponse::new(message)),
            }
        }
        ("POST", "/v1/events") => {
            if !has_json_content_type(&request) {
                return HttpResponse::json(
                    415,
                    Some(origin),
                    &ErrorResponse::new("A JSON Content-Type is required."),
                );
            }
            if !state.lsl_enabled.load(Ordering::Relaxed) {
                return HttpResponse::json(
                    409,
                    Some(origin),
                    &ErrorResponse::new(
                        "Enable LSL and configure the bridge before sending events.",
                    ),
                );
            }
            let batch: EventBatch = match serde_json::from_slice(&request.body) {
                Ok(value) => value,
                Err(_) => {
                    return HttpResponse::json(
                        400,
                        Some(origin),
                        &ErrorResponse::new("The browser signal batch is malformed."),
                    );
                }
            };
            match publish_batch(output, batch) {
                Ok(summary) => {
                    state
                        .accepted_events
                        .fetch_add(summary.events, Ordering::Relaxed);
                    state
                        .accepted_samples
                        .fetch_add(summary.samples, Ordering::Relaxed);
                    HttpResponse::json(
                        202,
                        Some(origin),
                        &EventResponse {
                            protocol_version: PROTOCOL_VERSION,
                            accepted_events: summary.events,
                            accepted_samples: summary.samples,
                        },
                    )
                }
                Err(message) => HttpResponse::json(422, Some(origin), &ErrorResponse::new(message)),
            }
        }
        _ => HttpResponse::json(
            404,
            Some(origin),
            &ErrorResponse::new("The browser bridge route was not found."),
        ),
    }
}

fn has_json_content_type(request: &HttpRequest) -> bool {
    request
        .header("content-type")
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
}

fn valid_client_id(value: &str) -> bool {
    (16..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn allowed_origin(origin: &str) -> bool {
    if origin == PAGES_ORIGIN {
        return true;
    }
    ["http://127.0.0.1:", "http://localhost:"]
        .iter()
        .any(|prefix| {
            origin
                .strip_prefix(prefix)
                .is_some_and(|port| port.parse::<u16>().is_ok())
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigEnvelope {
    protocol_version: u16,
    config: OutputConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventBatch {
    protocol_version: u16,
    sequence: u64,
    events: Vec<BrowserEvent>,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum BrowserEvent {
    Ecg {
        sensor_timestamp_ns: String,
        microvolts: Vec<i32>,
    },
    Accelerometer {
        sensor_timestamp_ns: String,
        samples: Vec<BrowserAccSample>,
    },
    Metrics {
        values: Vec<BrowserMetricValue>,
    },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserAccSample {
    x_mg: i16,
    y_mg: i16,
    z_mg: i16,
}

#[derive(Deserialize)]
struct BrowserMetricValue {
    id: String,
    value: f32,
}

struct BatchSummary {
    events: u64,
    samples: u64,
}

fn publish_batch(output: &OutputRouter, batch: EventBatch) -> Result<BatchSummary, String> {
    if batch.protocol_version != PROTOCOL_VERSION {
        return Err("The browser bridge protocol version is incompatible.".into());
    }
    if batch.sequence == 0 {
        return Err("The browser signal batch sequence must be positive.".into());
    }
    if batch.events.is_empty() || batch.events.len() > MAX_EVENTS_PER_BATCH {
        return Err(format!(
            "Each browser batch must contain 1 to {MAX_EVENTS_PER_BATCH} events."
        ));
    }

    let event_count = batch.events.len() as u64;
    let mut samples = 0_u64;
    for event in &batch.events {
        match event {
            BrowserEvent::Ecg {
                sensor_timestamp_ns,
                microvolts,
            } => {
                parse_sensor_timestamp(sensor_timestamp_ns)?;
                if microvolts.is_empty() || microvolts.len() > MAX_SAMPLES_PER_EVENT {
                    return Err(format!(
                        "Each ECG event must contain 1 to {MAX_SAMPLES_PER_EVENT} samples."
                    ));
                }
                if microvolts
                    .iter()
                    .any(|value| !(-8_388_608..=8_388_607).contains(value))
                {
                    return Err("An ECG sample exceeded the signed 24-bit Polar range.".into());
                }
                samples += microvolts.len() as u64;
            }
            BrowserEvent::Accelerometer {
                sensor_timestamp_ns,
                samples: values,
            } => {
                parse_sensor_timestamp(sensor_timestamp_ns)?;
                if values.is_empty() || values.len() > MAX_SAMPLES_PER_EVENT {
                    return Err(format!(
                        "Each accelerometer event must contain 1 to {MAX_SAMPLES_PER_EVENT} samples."
                    ));
                }
                samples += values.len() as u64;
            }
            BrowserEvent::Metrics { values } => {
                if values.is_empty() || values.len() > MAX_EVENTS_PER_BATCH {
                    return Err(format!(
                        "Each metric event must contain 1 to {MAX_EVENTS_PER_BATCH} values."
                    ));
                }
                for metric in values {
                    if metric.id.len() > 64 || MetricSpec::for_id(&metric.id).is_none() {
                        return Err(
                            "A metric identifier is not in the Polar Stream catalog.".into()
                        );
                    }
                    if !metric.value.is_finite() {
                        return Err("Metric values must be finite.".into());
                    }
                }
                samples += values.len() as u64;
            }
        }
    }

    for event in batch.events {
        match event {
            BrowserEvent::Ecg {
                sensor_timestamp_ns,
                microvolts,
            } => output.publish_ecg(parse_sensor_timestamp(&sensor_timestamp_ns)?, &microvolts),
            BrowserEvent::Accelerometer {
                sensor_timestamp_ns,
                samples,
            } => {
                let samples = samples
                    .into_iter()
                    .map(|sample| AccSample {
                        x_mg: sample.x_mg,
                        y_mg: sample.y_mg,
                        z_mg: sample.z_mg,
                    })
                    .collect::<Vec<_>>();
                output
                    .publish_accelerometer(parse_sensor_timestamp(&sensor_timestamp_ns)?, &samples);
            }
            BrowserEvent::Metrics { values } => {
                let routed = values
                    .iter()
                    .map(|metric| MetricValue {
                        id: &metric.id,
                        value: metric.value,
                    })
                    .collect::<Vec<_>>();
                output.publish_metrics(&routed);
            }
        }
    }
    Ok(BatchSummary {
        events: event_count,
        samples,
    })
}

fn parse_sensor_timestamp(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| "A sensor timestamp was not an unsigned integer.".to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    protocol_version: u16,
    lsl_enabled: bool,
    accepted_events: u64,
    accepted_samples: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigResponse {
    protocol_version: u16,
    health: OutputHealth,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventResponse {
    protocol_version: u16,
    accepted_events: u64,
    accepted_samples: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorResponse {
    code: &'static str,
    message: String,
}

impl ErrorResponse {
    fn new(message: impl Into<String>) -> Self {
        Self {
            code: "BROWSER_LSL_BRIDGE_ERROR",
            message: message.into(),
        }
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut bytes = Vec::with_capacity(4_096);
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() >= MAX_HEADER_BYTES {
            return Err("The browser bridge request headers were too large.".into());
        }
        let mut chunk = [0_u8; 4_096];
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|_| "The browser bridge request could not be read.".to_string())?;
        if count == 0 {
            return Err("The browser bridge request ended before its headers.".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    };

    let mut parsed_headers = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Request::new(&mut parsed_headers);
    match parsed.parse(&bytes[..header_end]) {
        Ok(httparse::Status::Complete(_)) => {}
        _ => return Err("The browser bridge request headers were malformed.".into()),
    }
    if parsed.version != Some(1) {
        return Err("The browser bridge requires HTTP/1.1.".into());
    }
    let method = parsed
        .method
        .ok_or_else(|| "The browser bridge request method was missing.".to_string())?
        .to_string();
    let path = parsed
        .path
        .ok_or_else(|| "The browser bridge request path was missing.".to_string())?
        .to_string();
    let mut headers = HashMap::new();
    for header in parsed.headers.iter() {
        let name = header.name.to_ascii_lowercase();
        if headers.contains_key(&name) {
            return Err("Duplicate browser bridge headers are not accepted.".into());
        }
        let value = std::str::from_utf8(header.value)
            .map_err(|_| "A browser bridge header was not UTF-8.".to_string())?
            .trim()
            .to_string();
        headers.insert(name, value);
    }
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| "The browser bridge Content-Length was invalid.".to_string())?
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err("The browser bridge request body was too large.".into());
    }
    let required = header_end + content_length;
    while bytes.len() < required {
        let mut chunk = [0_u8; 8_192];
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|_| "The browser bridge body could not be read.".to_string())?;
        if count == 0 {
            return Err("The browser bridge request body ended early.".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > header_end + MAX_BODY_BYTES {
            return Err("The browser bridge request body was too large.".into());
        }
    }
    Ok(HttpRequest {
        method,
        path,
        headers,
        body: bytes[header_end..required].to_vec(),
    })
}

struct HttpResponse {
    status: u16,
    origin: Option<String>,
    body: Vec<u8>,
    content_type: Option<&'static str>,
}

impl HttpResponse {
    fn json<T: Serialize>(status: u16, origin: Option<&str>, value: &T) -> Self {
        Self {
            status,
            origin: origin.map(str::to_string),
            body: serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
            content_type: Some("application/json; charset=utf-8"),
        }
    }

    fn empty(status: u16, origin: Option<&str>) -> Self {
        Self {
            status,
            origin: origin.map(str::to_string),
            body: Vec::new(),
            content_type: None,
        }
    }

    fn with_origin(mut self, origin: Option<&str>) -> Self {
        if self.origin.is_none() {
            self.origin = origin.map(str::to_string);
        }
        self
    }
}

async fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<(), String> {
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        409 => "Conflict",
        415 => "Unsupported Media Type",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        _ => "Error",
    };
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n",
        response.status,
        reason,
        response.body.len()
    );
    if let Some(content_type) = response.content_type {
        headers.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if let Some(origin) = response.origin {
        headers.push_str(&format!(
            "Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Authorization, Content-Type, X-Polar-Bridge-Client\r\nAccess-Control-Allow-Private-Network: true\r\n"
        ));
    }
    headers.push_str("\r\n");
    stream
        .write_all(headers.as_bytes())
        .await
        .map_err(|_| "The browser bridge response headers could not be written.".to_string())?;
    stream
        .write_all(&response.body)
        .await
        .map_err(|_| "The browser bridge response body could not be written.".to_string())?;
    stream
        .shutdown()
        .await
        .map_err(|_| "The browser bridge connection could not be closed.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_only_pages_and_loopback_development_origins() {
        assert!(allowed_origin(PAGES_ORIGIN));
        assert!(allowed_origin("http://127.0.0.1:8080"));
        assert!(allowed_origin("http://localhost:4173"));
        assert!(!allowed_origin("https://example.com"));
        assert!(!allowed_origin("http://127.0.0.1.example.com:8080"));
    }

    #[test]
    fn rejects_invalid_batches_before_publication() {
        let output = OutputRouter::new();
        let error = publish_batch(
            &output,
            EventBatch {
                protocol_version: PROTOCOL_VERSION,
                sequence: 1,
                events: vec![BrowserEvent::Metrics {
                    values: vec![BrowserMetricValue {
                        id: "not_a_metric".into(),
                        value: 1.0,
                    }],
                }],
            },
        )
        .err();
        assert_eq!(
            error.as_deref(),
            Some("A metric identifier is not in the Polar Stream catalog.")
        );
    }

    #[tokio::test]
    async fn loopback_server_requires_origin_host_and_token() {
        let bridge = BrowserLslBridge::new(None);
        let launch = bridge.start().await.unwrap();
        let response = send_test_request(
            launch.port,
            &format!(
                "GET /v1/status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: {}\r\nAuthorization: Bearer {}\r\nX-Polar-Bridge-Client: browser-test-client-1\r\nConnection: close\r\n\r\n",
                launch.port, PAGES_ORIGIN, launch.token
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(&format!("Access-Control-Allow-Origin: {PAGES_ORIGIN}")));

        let competing_tab = send_test_request(
            launch.port,
            &format!(
                "GET /v1/status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: {}\r\nAuthorization: Bearer {}\r\nX-Polar-Bridge-Client: browser-test-client-2\r\nConnection: close\r\n\r\n",
                launch.port, PAGES_ORIGIN, launch.token
            ),
        )
        .await;
        assert!(competing_tab.starts_with("HTTP/1.1 409 Conflict"));

        let denied = send_test_request(
            launch.port,
            &format!(
                "GET /v1/status HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: {}\r\nAuthorization: Bearer wrong\r\nX-Polar-Bridge-Client: browser-test-client-1\r\nConnection: close\r\n\r\n",
                launch.port, PAGES_ORIGIN
            ),
        )
        .await;
        assert!(denied.starts_with("HTTP/1.1 401 Unauthorized"));

        let preflight = send_test_request(
            launch.port,
            &format!(
                "OPTIONS /v1/config HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: {}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization,content-type,x-polar-bridge-client\r\nConnection: close\r\n\r\n",
                launch.port, PAGES_ORIGIN
            ),
        )
        .await;
        assert!(preflight.starts_with("HTTP/1.1 204 No Content"));
        assert!(preflight.contains("Access-Control-Allow-Private-Network: true"));

        let config = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "config": {
                "streamName": "Bridge-Test",
                "lslEnabled": true,
                "oscEnabled": false,
                "outputs": ["raw_ecg", "raw_acc"],
                "metricOptions": {}
            }
        })
        .to_string();
        let configured =
            send_json_test_request(launch.port, "/v1/config", &launch.token, &config).await;
        assert!(configured.starts_with("HTTP/1.1 200 OK"));

        let events = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "sequence": 1,
            "events": [
                { "kind": "ecg", "sensorTimestampNs": "42", "microvolts": [10, -11] },
                {
                    "kind": "accelerometer",
                    "sensorTimestampNs": "43",
                    "samples": [{ "xMg": 1, "yMg": -2, "zMg": 3 }]
                }
            ]
        })
        .to_string();
        let accepted =
            send_json_test_request(launch.port, "/v1/events", &launch.token, &events).await;
        assert!(accepted.starts_with("HTTP/1.1 202 Accepted"));
        assert!(accepted.contains("\"acceptedSamples\":3"));
    }

    async fn send_test_request(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    }

    async fn send_json_test_request(port: u16, path: &str, token: &str, body: &str) -> String {
        send_test_request(
            port,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {PAGES_ORIGIN}\r\nAuthorization: Bearer {token}\r\nX-Polar-Bridge-Client: browser-test-client-1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        )
        .await
    }
}
