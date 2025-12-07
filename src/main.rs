use std::fs;
use std::process;
use std::thread;
use std::time::Duration;

use log::{debug, error, info, warn};
use serde::Deserialize;
use serde_json::{self, Value};
use toml;
use zmq;

//
// ------------------------------- Constants -----------------------------------
//

const POLL_TIMEOUT_MS: i64 = 100; // poll timeout to allow periodic checks
const RETRY_ATTEMPTS: usize = 3; // max attempts for transient ZMQ ops
const RETRY_BACKOFF_MS: u64 = 3000; // backoff between retries (ms)

const BYTES_PREVIEW_LEN: usize = 20; // byte preview length for non-UTF8 parts
const MAX_OBJECT_KEYS: usize = 10; // keys to show when trimming top-level objects

const DEFAULT_PROXY_XSUB_ENDPOINT: &str = "tcp://*:5557";
const DEFAULT_PROXY_XPUB_ENDPOINT: &str = "tcp://*:5558";
const DEFAULT_CLIENT_TO_CLIENT_ENDPOINT: &str = "tcp://*:6565";
const DEFAULT_CLIENT_FACING_ENDPOINT: &str = "tcp://*:5559";
const DEFAULT_WORKER_FACING_ENDPOINT: &str = "tcp://*:5560";

// Cropping controls (arrays)
const MAX_DEPTH: usize = 2;                 // limit recursion for performance
const OUTER_HEAD: usize = 1;                // top-level array head items
const OUTER_TAIL: usize = 1;                // top-level array tail items
const OUTER_MIN_CROP_LEN: usize = 5;        // DO NOT crop arrays smaller than this at depth 0

const INNER_MIN_CROP_LEN: usize = 30;       // DO NOT crop inner arrays smaller than this
const ROW_LIST_HEAD: usize = 1;             // arrays-of-arrays (e.g., OHLCV rows) head
const ROW_LIST_TAIL: usize = 1;             // arrays-of-arrays (e.g., OHLCV rows) tail
const SCALAR_LIST_HEAD: usize = 3;          // arrays of scalars/strings head (e.g., colors)
const SCALAR_LIST_TAIL: usize = 1;          // arrays of scalars/strings tail

//
// ------------------------------- Config --------------------------------------
//

#[derive(Deserialize, Clone)]
struct Config {
    logging: LoggingConfig,
    #[serde(default)]
    network: NetworkConfig,
}

#[derive(Deserialize, Clone)]
struct LoggingConfig {
    #[allow(dead_code)]
    file_path: String,
    level: String,
}

#[derive(Deserialize, Clone)]
#[serde(default)] // missing fields inherit from NetworkConfig::default()
struct NetworkConfig {
    proxy_xsub_endpoint: String,
    proxy_xpub_endpoint: String,
    client_to_client_endpoint: String,
    client_facing_endpoint: String,
    worker_facing_endpoint: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            proxy_xsub_endpoint: DEFAULT_PROXY_XSUB_ENDPOINT.to_string(),
            proxy_xpub_endpoint: DEFAULT_PROXY_XPUB_ENDPOINT.to_string(),
            client_to_client_endpoint: DEFAULT_CLIENT_TO_CLIENT_ENDPOINT.to_string(),
            client_facing_endpoint: DEFAULT_CLIENT_FACING_ENDPOINT.to_string(),
            worker_facing_endpoint: DEFAULT_WORKER_FACING_ENDPOINT.to_string(),
        }
    }
}

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let home_dir = dirs::home_dir().ok_or("Could not determine home directory")?;
    let config_path = home_dir.join(".corky").join("config.toml");

    if !config_path.exists() {
        eprintln!("Configuration file not found at: {}", config_path.display());
        eprintln!("Please get the latest configuration from GitHub.");
        process::exit(1);
    }

    let config_content = fs::read_to_string(&config_path)?;
    let config: Config = toml::from_str(&config_content)?;
    Ok(config)
}

//
// ----------------------------- Logger setup ----------------------------------
//

fn setup_logger(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default());
    if std::env::var("RUST_LOG").is_err() {
        let level = match config.logging.level.to_lowercase().as_str() {
            "trace" => log::LevelFilter::Trace,
            "debug" => log::LevelFilter::Debug,
            "info" => log::LevelFilter::Info,
            "warn" => log::LevelFilter::Warn,
            "error" => log::LevelFilter::Error,
            _ => log::LevelFilter::Info,
        };
        builder.filter_level(level);
    }
    builder.init();
    Ok(())
}

//
// -------------------------- Lightweight retry --------------------------------
//

fn retry<F, T, E>(mut op: F, attempts: usize, backoff: Duration, name: &str) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
    E: std::fmt::Display,
{
    let mut try_no = 0usize;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => {
                try_no += 1;
                error!("{} failed (attempt {}): {}", name, try_no, e);
                if try_no >= attempts {
                    return Err(e);
                }
                thread::sleep(backoff);
            }
        }
    }
}

//
// --------------------- Recursive JSON array cropping -------------------------
//

fn format_json_pretty(value: &Value) -> String {
    let cropped = crop_value(value, 0);
    serde_json::to_string_pretty(&cropped).unwrap_or_else(|_| cropped.to_string())
}

fn crop_value(value: &Value, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return value.clone();
    }

    match value {
        Value::Array(arr) => {
            // Decide strategy based on depth and element "shape"
            let is_list_of_arrays = arr.iter().all(|v| matches!(v, Value::Array(_)));
            let (min_len, head, tail) = if depth == 0 {
                (OUTER_MIN_CROP_LEN, OUTER_HEAD, OUTER_TAIL)
            } else if is_list_of_arrays {
                (INNER_MIN_CROP_LEN, ROW_LIST_HEAD, ROW_LIST_TAIL)
            } else {
                (INNER_MIN_CROP_LEN, SCALAR_LIST_HEAD, SCALAR_LIST_TAIL)
            };

            // If small or near head+tail window, don't crop — but still recurse.
            if arr.len() < min_len || arr.len() <= head + tail {
                return Value::Array(
                    arr.iter().map(|v| crop_value(v, depth + 1)).collect::<Vec<_>>(),
                );
            }

            // Crop large lists only.
            let mut out = Vec::with_capacity(head + 1 + tail);
            for v in arr.iter().take(head) {
                out.push(crop_value(v, depth + 1));
            }
            let omitted = arr.len() - (head + tail);
            out.push(Value::String(format!("... ({} more) ...", omitted)));
            let tail_start = arr.len() - tail;
            for v in arr.iter().skip(tail_start) {
                out.push(crop_value(v, depth + 1));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            // Trim only *top-level* objects by key count; always recurse into values.
            if depth == 0 && map.len() > MAX_OBJECT_KEYS {
                let mut trimmed = serde_json::Map::with_capacity(MAX_OBJECT_KEYS + 1);
                for (k, v) in map.iter().take(MAX_OBJECT_KEYS) {
                    trimmed.insert(k.clone(), crop_value(v, depth + 1));
                }
                trimmed.insert(
                    "...".to_string(),
                    Value::String(format!("{} more keys", map.len() - MAX_OBJECT_KEYS)),
                );
                Value::Object(trimmed)
            } else {
                let mut new_map = serde_json::Map::with_capacity(map.len());
                for (k, v) in map {
                    new_map.insert(k.clone(), crop_value(v, depth + 1));
                }
                Value::Object(new_map)
            }
        }
        _ => value.clone(),
    }
}

//
// ------------------------ Message formatting ---------------------------------
//

fn try_parse_json_bytes(part: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Value>(part).ok()
}

fn try_parse_json_str(s: &str) -> Option<Value> {
    serde_json::from_str::<Value>(s).ok()
}

fn format_part(part: &[u8]) -> String {
    if let Some(v) = try_parse_json_bytes(part) {
        return format_json_pretty(&v);
    }

    if let Ok(s) = std::str::from_utf8(part) {
        let t = s.trim();
        if (t.starts_with('{') && t.ends_with('}')) || (t.starts_with('[') && t.ends_with(']')) {
            if let Some(v) = try_parse_json_str(t) {
                return format_json_pretty(&v);
            }
        }
        return format!("\"{}\"", s.replace('"', "\\\""));
    }

    if part.len() > BYTES_PREVIEW_LEN {
        format!("[{} bytes: {:?}...]", part.len(), &part[..BYTES_PREVIEW_LEN])
    } else {
        format!("{:?}", part)
    }
}

fn format_message(parts: &[Vec<u8>]) -> String {
    if parts.is_empty() {
        return "[empty message]".to_string();
    }
    let rendered: Vec<String> = parts.iter().map(|p| format_part(p)).collect();
    if rendered.len() == 1 {
        rendered[0].clone()
    } else {
        format!("[{}]", rendered.join(" | "))
    }
}

//
// ------------------------------ Proxy ----------------------------------------
//

fn run_proxy(context: &zmq::Context, config: &Config) -> Result<(), zmq::Error> {
    let xsub_socket = context.socket(zmq::XSUB)?;
    xsub_socket.bind(&config.network.proxy_xsub_endpoint)?;
    info!("(Proxy) XSUB bound to {}", config.network.proxy_xsub_endpoint);

    let xpub_socket = context.socket(zmq::XPUB)?;
    xpub_socket.bind(&config.network.proxy_xpub_endpoint)?;
    info!("(Proxy) XPUB bound to {}", config.network.proxy_xpub_endpoint);

    info!("(Proxy) Starting XSUB/XPUB forwarder...");
    zmq::proxy(&xpub_socket, &xsub_socket)?;
    Ok(())
}

//
// ------------------------------ Broker ---------------------------------------
//

fn forward_or_log(src: &zmq::Socket, dst: &zmq::Socket, src_name: &str, dst_name: &str) {
    match retry(
        || src.recv_multipart(0),
        RETRY_ATTEMPTS,
        Duration::from_millis(RETRY_BACKOFF_MS),
        &format!("recv {}", src_name),
    ) {
        Ok(message) => {
            debug!(
                "(Broker) Forwarding {} -> {}: {}",
                src_name,
                dst_name,
                format_message(&message)
            );
            if let Err(e) = retry(
                || dst.send_multipart(&message, 0),
                RETRY_ATTEMPTS,
                Duration::from_millis(RETRY_BACKOFF_MS),
                &format!("send {} -> {}", src_name, dst_name),
            ) {
                error!("(Broker) Error forwarding {} -> {}: {}", src_name, dst_name, e);
            }
        }
        Err(e) => error!("(Broker) Error receiving from {}: {}", src_name, e),
    }
}

fn handle_client_to_client(router: &zmq::Socket) {
    match retry(
        || router.recv_multipart(0),
        RETRY_ATTEMPTS,
        Duration::from_millis(RETRY_BACKOFF_MS),
        "recv client_to_client_direct_messaging_router",
    ) {
        Ok(msg) => {
            info!(
                "(Broker) Received from client_to_client_direct_messaging_router: {}",
                format_message(&msg)
            );

            if msg.len() == 3 {
                let client_id = &msg[0];
                let empty = &msg[1];
                let payload = &msg[2];

                if let Err(e) = retry(
                    || router.send_multipart(&[empty, client_id, payload], 0),
                    RETRY_ATTEMPTS,
                    Duration::from_millis(RETRY_BACKOFF_MS),
                    "send client_to_client_direct_messaging_router",
                ) {
                    error!(
                        "(Broker) Error sending to client_to_client_direct_messaging_router: {}",
                        e
                    );
                }
            } else {
                warn!(
                    "(Broker) Unexpected client_to_client_direct_messaging_router message ({} frames): {}",
                    msg.len(),
                    format_message(&msg)
                );
            }
        }
        Err(e) => error!(
            "(Broker) Error receiving from client_to_client_direct_messaging_router: {}",
            e
        ),
    }
}

fn run_broker(context: &zmq::Context, config: &Config) -> Result<(), zmq::Error> {
    // (1) ROUTER for direct client<->client messaging
    let client_to_client_direct_messaging_router = context.socket(zmq::ROUTER)?;
    client_to_client_direct_messaging_router.bind(&config.network.client_to_client_endpoint)?;
    info!(
        "(Broker) client_to_client_direct_messaging_router (ROUTER) bound to {}",
        config.network.client_to_client_endpoint
    );

    // (2) Client-facing ROUTER (frontend)
    let client_facing_router = context.socket(zmq::ROUTER)?;
    client_facing_router.bind(&config.network.client_facing_endpoint)?;
    info!(
        "(Broker) client_facing_router (ROUTER) bound to {}",
        config.network.client_facing_endpoint
    );

    // (3) Worker-facing DEALER (backend)
    let worker_facing_dealer = context.socket(zmq::DEALER)?;
    worker_facing_dealer.bind(&config.network.worker_facing_endpoint)?;
    info!(
        "(Broker) worker_facing_dealer (DEALER) bound to {}",
        config.network.worker_facing_endpoint
    );

    info!("(Broker) Broker loop started. Polling for messages...");

    let mut poll_items = [
        client_to_client_direct_messaging_router.as_poll_item(zmq::POLLIN),
        client_facing_router.as_poll_item(zmq::POLLIN),
        worker_facing_dealer.as_poll_item(zmq::POLLIN),
    ];

    const IDX_CLIENT_TO_CLIENT_DIRECT_MESSAGING_ROUTER: usize = 0;
    const IDX_CLIENT_FACING_ROUTER: usize = 1;
    const IDX_WORKER_FACING_DEALER: usize = 2;

    loop {
        zmq::poll(&mut poll_items, POLL_TIMEOUT_MS)?;

        for idx in 0..poll_items.len() {
            if !poll_items[idx].is_readable() {
                continue;
            }
            match idx {
                IDX_CLIENT_TO_CLIENT_DIRECT_MESSAGING_ROUTER => {
                    handle_client_to_client(&client_to_client_direct_messaging_router)
                }
                IDX_CLIENT_FACING_ROUTER => forward_or_log(
                    &client_facing_router,
                    &worker_facing_dealer,
                    "client_facing_router",
                    "worker_facing_dealer",
                ),
                IDX_WORKER_FACING_DEALER => forward_or_log(
                    &worker_facing_dealer,
                    &client_facing_router,
                    "worker_facing_dealer",
                    "client_facing_router",
                ),
                _ => unreachable!("invalid poll index"),
            }
        }
    }
}

//
// --------------------------------- main --------------------------------------
//

fn main() {
    // 1) Load configuration and initialize logging
    let config = load_config().expect("Failed to load configuration");
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        std::process::exit(1);
    }
    info!("ZMQ Combined Proxy & Broker (Rust Version) - Starting...");

    // 2) Create a global ZMQ context
    let context = zmq::Context::new();

    // 3) Start XSUB/XPUB proxy in a background thread
    let ctx_for_proxy = context.clone();
    let config_for_proxy = config.clone();
    let proxy_thread = thread::spawn(move || loop {
        match run_proxy(&ctx_for_proxy, &config_for_proxy) {
            Ok(_) => {
                info!("(Proxy) Stopped without error. Exiting proxy thread...");
                break;
            }
            Err(e) => {
                error!("(Proxy) Encountered an error: {}", e);
                thread::sleep(Duration::from_millis(RETRY_BACKOFF_MS));
                warn!("(Proxy) Retrying XSUB/XPUB proxy...");
            }
        }
    });

    // 4) Run the broker loop (blocks)
    if let Err(e) = run_broker(&context, &config) {
        error!("(Broker) Encountered an error: {}", e);
    }

    // 5) Join the proxy thread on exit
    let _ = proxy_thread.join();
    info!("(Main) Exiting.");
}
