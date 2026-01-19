use std::fs;
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use log::{debug, error, info, warn};
use serde::Deserialize;
use serde_json::{self, Value};

//
// ------------------------------- Constants -----------------------------------
//

const POLL_TIMEOUT_MS: i64 = 10; // poll timeout for low latency
const RETRY_BACKOFF_MS: u64 = 3000; // backoff between connection retries (ms)

const BYTES_PREVIEW_LEN: usize = 20; // byte preview length for non-UTF8 parts
const MAX_OBJECT_KEYS: usize = 10; // keys to show when trimming top-level objects

const DEFAULT_PROXY_XSUB_ENDPOINT: &str = "tcp://*:5557";
const DEFAULT_PROXY_XPUB_ENDPOINT: &str = "tcp://*:5558";
const DEFAULT_CLIENT_TO_CLIENT_ENDPOINT: &str = "tcp://*:6565";
const DEFAULT_CLIENT_FACING_ENDPOINT: &str = "tcp://*:5559";
const DEFAULT_WORKER_FACING_ENDPOINT: &str = "tcp://*:5560";

// Poll index constants for broker
const IDX_DIRECT_ROUTER: usize = 0;
const IDX_CLIENT_ROUTER: usize = 1;
const IDX_WORKER_DEALER: usize = 2;

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

// Preferred keys to keep when trimming large top-level objects
const IMPORTANT_KEYS: &[&str] = &[
    "id", "symbol", "ticker", "type", "status", "desc", "timeframe", "title",
];

//
// ------------------------------- Config --------------------------------------
//

#[derive(Deserialize, Clone, Default)]
struct Config {
    #[serde(default)]
    logging: LoggingConfig,
    #[serde(default)]
    network: NetworkConfig,
}

#[derive(Deserialize, Clone)]
struct LoggingConfig {
    level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
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

fn load_config() -> Result<Config, String> {
    let home_dir = match dirs::home_dir() {
        Some(dir) => dir,
        None => return Err("Could not determine home directory".to_string()),
    };
    let config_path = home_dir.join(".corky").join("config.toml");

    if !config_path.exists() {
        return Err(format!(
            "Configuration file not found at: {}",
            config_path.display()
        ));
    }

    let config_content = fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config: {}", e))?;
    let config: Config = toml::from_str(&config_content)
        .map_err(|e| format!("Failed to parse config: {}", e))?;
    Ok(config)
}

//
// ----------------------------- Logger setup ----------------------------------
//

fn setup_logger(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    // Prefer RUST_LOG; otherwise fall back to config.logging.level.
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
// -------------------------- Socket configuration -----------------------------
//

fn configure_socket(socket: &zmq::Socket) -> Result<(), zmq::Error> {
    socket.set_sndhwm(10_000)?; // Send high water mark (default 1000)
    socket.set_rcvhwm(10_000)?; // Receive high water mark
    socket.set_linger(1000)?; // 1s linger on close
    socket.set_tcp_keepalive(1)?; // Enable TCP keepalive
    socket.set_tcp_keepalive_idle(60)?;
    socket.set_tcp_keepalive_intvl(10)?;
    Ok(())
}

//
// --------------------- Recursive JSON array cropping -------------------------
//

// Robust "row-like" detection: treat as list-of-arrays if >=80% of sampled
// elements are arrays. Sample up to 32 elements, evenly spread.
fn is_mostly_arrays(arr: &[Value]) -> bool {
    let len = arr.len();
    if len == 0 {
        return false;
    }
    let sample_n = len.clamp(1, 32);
    let step = len.div_ceil(sample_n);
    let mut arrays = 0usize;
    let mut taken = 0usize;
    let mut i = 0usize;
    while i < len && taken < sample_n {
        if matches!(arr[i], Value::Array(_)) {
            arrays += 1;
        }
        taken += 1;
        i += step;
    }
    arrays * 100 >= taken * 80
}

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
            let row_like = is_mostly_arrays(arr);
            let (min_len, head, tail) = if depth == 0 {
                (OUTER_MIN_CROP_LEN, OUTER_HEAD, OUTER_TAIL)
            } else if row_like {
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

                // 1) Insert prioritized keys in order, if present.
                for k in IMPORTANT_KEYS {
                    if let Some(v) = map.get(*k) {
                        if trimmed.len() < MAX_OBJECT_KEYS {
                            trimmed.insert((*k).to_string(), crop_value(v, depth + 1));
                        }
                    }
                }
                // 2) Fill the remaining budget with other keys in map order.
                for (k, v) in map.iter() {
                    if trimmed.len() >= MAX_OBJECT_KEYS {
                        break;
                    }
                    if !trimmed.contains_key(k) {
                        trimmed.insert(k.clone(), crop_value(v, depth + 1));
                    }
                }
                // 3) Ellipsis marker with remaining count.
                if map.len() > trimmed.len() {
                    trimmed.insert(
                        "...".to_string(),
                        Value::String(format!("{} more keys", map.len() - trimmed.len())),
                    );
                }

                Value::Object(trimmed)
            } else {
                let mut new_map = serde_json::Map::with_capacity(map.len());
                for (k, v) in map.iter() {
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
    configure_socket(&xsub_socket)?;
    xsub_socket.bind(&config.network.proxy_xsub_endpoint)?;
    info!("(Proxy) XSUB bound to {}", config.network.proxy_xsub_endpoint);

    let xpub_socket = context.socket(zmq::XPUB)?;
    configure_socket(&xpub_socket)?;
    xpub_socket.bind(&config.network.proxy_xpub_endpoint)?;
    info!("(Proxy) XPUB bound to {}", config.network.proxy_xpub_endpoint);

    info!("(Proxy) Starting XSUB/XPUB forwarder...");
    zmq::proxy(&xpub_socket, &xsub_socket)?;
    Ok(())
}

//
// ------------------------------ Broker ---------------------------------------
//

fn forward_message(src: &zmq::Socket, dst: &zmq::Socket, src_name: &str, dst_name: &str) {
    match src.recv_multipart(0) {
        Ok(message) => {
            if log::log_enabled!(log::Level::Debug) {
                debug!(
                    "(Broker) Forwarding {} -> {}: {}",
                    src_name,
                    dst_name,
                    format_message(&message)
                );
            }
            if let Err(e) = dst.send_multipart(&message, 0) {
                error!(
                    "(Broker) Error forwarding {} -> {}: {}",
                    src_name, dst_name, e
                );
            }
        }
        Err(zmq::Error::EINTR) => {
            // Interrupted by signal, not an error
        }
        Err(e) => error!("(Broker) Error receiving from {}: {}", src_name, e),
    }
}

fn route_direct_message(router: &zmq::Socket) {
    match router.recv_multipart(0) {
        Ok(msg) => {
            if log::log_enabled!(log::Level::Debug) {
                debug!(
                    "(Broker) Received from direct_router: {}",
                    format_message(&msg)
                );
            }

            if msg.len() == 3 {
                let client_id = &msg[0];
                let empty = &msg[1];
                let payload = &msg[2];

                if let Err(e) = router.send_multipart([empty, client_id, payload], 0) {
                    error!("(Broker) Error sending to direct_router: {}", e);
                }
            } else {
                warn!(
                    "(Broker) Unexpected direct_router message ({} frames): {}",
                    msg.len(),
                    format_message(&msg)
                );
            }
        }
        Err(zmq::Error::EINTR) => {
            // Interrupted by signal, not an error
        }
        Err(e) => error!("(Broker) Error receiving from direct_router: {}", e),
    }
}

fn run_broker(
    context: &zmq::Context,
    config: &Config,
    shutdown: &Arc<AtomicBool>,
) -> Result<(), zmq::Error> {
    // (1) ROUTER for direct client<->client messaging
    let direct_router = context.socket(zmq::ROUTER)?;
    configure_socket(&direct_router)?;
    direct_router.bind(&config.network.client_to_client_endpoint)?;
    info!(
        "(Broker) direct_router (ROUTER) bound to {}",
        config.network.client_to_client_endpoint
    );

    // (2) Client-facing ROUTER (frontend)
    let client_router = context.socket(zmq::ROUTER)?;
    configure_socket(&client_router)?;
    client_router.bind(&config.network.client_facing_endpoint)?;
    info!(
        "(Broker) client_router (ROUTER) bound to {}",
        config.network.client_facing_endpoint
    );

    // (3) Worker-facing DEALER (backend)
    let worker_dealer = context.socket(zmq::DEALER)?;
    configure_socket(&worker_dealer)?;
    worker_dealer.bind(&config.network.worker_facing_endpoint)?;
    info!(
        "(Broker) worker_dealer (DEALER) bound to {}",
        config.network.worker_facing_endpoint
    );

    info!("(Broker) Broker loop started. Polling for messages...");

    let mut poll_items = [
        direct_router.as_poll_item(zmq::POLLIN),
        client_router.as_poll_item(zmq::POLLIN),
        worker_dealer.as_poll_item(zmq::POLLIN),
    ];

    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("(Broker) Shutdown requested...");
            return Ok(());
        }

        match zmq::poll(&mut poll_items, POLL_TIMEOUT_MS) {
            Ok(_) => {}
            Err(zmq::Error::EINTR) => continue, // Signal interrupted, just retry
            Err(e) => return Err(e),
        }

        for (idx, poll_item) in poll_items.iter().enumerate() {
            if !poll_item.is_readable() {
                continue;
            }
            match idx {
                IDX_DIRECT_ROUTER => route_direct_message(&direct_router),
                IDX_CLIENT_ROUTER => forward_message(
                    &client_router,
                    &worker_dealer,
                    "client_router",
                    "worker_dealer",
                ),
                IDX_WORKER_DEALER => forward_message(
                    &worker_dealer,
                    &client_router,
                    "worker_dealer",
                    "client_router",
                ),
                unexpected => {
                    error!("(Broker) Unexpected poll index {}, skipping", unexpected);
                }
            }
        }
    }
}

//
// --------------------------------- main --------------------------------------
//

fn main() {
    let result = panic::catch_unwind(|| {
        run_main();
    });

    if let Err(e) = result {
        eprintln!("FATAL: Panic caught: {:?}", e);
        std::process::exit(1);
    }
}

fn run_main() {
    // 1) Load configuration and initialize logging
    let config = load_config().unwrap_or_else(|e| {
        eprintln!("Config error: {}. Using defaults.", e);
        Config::default()
    });
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        std::process::exit(1);
    }
    info!("ZMQ Combined Proxy & Broker (Rust Version) - Starting...");

    // 2) Set up signal handling for graceful shutdown
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_handler = Arc::clone(&shutdown);
    if let Err(e) = ctrlc::set_handler(move || {
        info!("Shutdown signal received...");
        shutdown_handler.store(true, Ordering::SeqCst);
    }) {
        error!("Failed to set Ctrl-C handler: {}. Graceful shutdown via Ctrl-C disabled.", e);
    }

    // 3) Create a global ZMQ context
    let context = zmq::Context::new();

    // 4) Start XSUB/XPUB proxy in a background thread
    let ctx_for_proxy = context.clone();
    let config_for_proxy = config.clone();
    let shutdown_proxy = Arc::clone(&shutdown);
    let proxy_thread = thread::Builder::new()
        .name("proxy-thread".to_string())
        .spawn(move || {
            while !shutdown_proxy.load(Ordering::SeqCst) {
                match run_proxy(&ctx_for_proxy, &config_for_proxy) {
                    Ok(_) => break,
                    Err(_) if shutdown_proxy.load(Ordering::SeqCst) => break,
                    Err(e) => {
                        error!("(Proxy) Error: {}", e);
                        thread::sleep(Duration::from_millis(RETRY_BACKOFF_MS));
                    }
                }
            }
        });

    let proxy_handle = match proxy_thread {
        Ok(handle) => Some(handle),
        Err(e) => {
            error!("(Main) Failed to spawn proxy thread: {}. Running without proxy.", e);
            None
        }
    };

    // 5) Run the broker loop with auto-recovery
    let shutdown_broker = Arc::clone(&shutdown);
    while !shutdown_broker.load(Ordering::SeqCst) {
        match run_broker(&context, &config, &shutdown_broker) {
            Ok(_) => break,
            Err(_) if shutdown_broker.load(Ordering::SeqCst) => break,
            Err(e) => {
                error!("(Broker) Error: {}", e);
                thread::sleep(Duration::from_millis(RETRY_BACKOFF_MS));
            }
        }
    }

    // 6) Join the proxy thread on exit
    if let Some(handle) = proxy_handle {
        let _ = handle.join();
    }
    info!("(Main) Graceful shutdown complete.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_crop_small_top_level_triplet() {
        // ["qb-python", "rustcharts", ["ok", {...}]]
        let v = json!(["qb-python", "rustcharts", ["ok", { "x": 1 }]]);
        let s = format_json_pretty(&v);
        assert!(
            !s.contains("... ("),
            "should not crop small outer arrays; got: {s}"
        );
    }

    #[test]
    fn crop_large_row_list_first_and_last() {
        // data: many rows -> expect 1 head, ellipsis, 1 tail
        let mut rows: Vec<Value> = Vec::new();
        for i in 0..35 {
            rows.push(json!([i, i + 1, i + 2, i + 3, i + 4, i + 5]));
        }
        let v = json!({ "data": rows });
        let s = format_json_pretty(&v);
        assert!(
            s.matches("... (").count() >= 1,
            "expected an ellipsis for large data arrays"
        );
        assert!(s.contains("[0, 1, 2, 3, 4, 5]"));
        assert!(s.contains("[34, 35, 36, 37, 38, 39]"));
    }

    #[test]
    fn crop_large_scalar_list_show_head_tail() {
        let colors: Vec<Value> = (0..64)
            .map(|i| Value::String(format!("#{:06X}", i)))
            .collect();
        let v = json!({ "candle_colors": colors });
        let s = format_json_pretty(&v);
        assert!(
            s.matches("... (").count() >= 1,
            "expected an ellipsis for large scalar arrays"
        );
    }
}
