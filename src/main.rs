use std::thread;
use std::time::Duration;
use zmq;
use std::fs;
use std::process;

use colored::*;
use log::{info, error, warn, debug, Level};
use chrono::Local;
use serde::Deserialize;
use dirs;
use toml;

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

#[derive(Deserialize, Default, Clone)]
struct NetworkConfig {
    #[serde(default = "default_proxy_xsub_endpoint")]
    proxy_xsub_endpoint: String,
    #[serde(default = "default_proxy_xpub_endpoint")]
    proxy_xpub_endpoint: String,
    #[serde(default = "default_client_to_client_endpoint")]
    client_to_client_endpoint: String,
    #[serde(default = "default_client_facing_endpoint")]
    client_facing_endpoint: String,
    #[serde(default = "default_worker_facing_endpoint")]
    worker_facing_endpoint: String,
}

// Default functions for network configuration
fn default_proxy_xsub_endpoint() -> String {
    "tcp://*:5557".to_string()
}

fn default_proxy_xpub_endpoint() -> String {
    "tcp://*:5558".to_string()
}

fn default_client_to_client_endpoint() -> String {
    "tcp://*:6565".to_string()
}

fn default_client_facing_endpoint() -> String {
    "tcp://*:5559".to_string()
}

fn default_worker_facing_endpoint() -> String {
    "tcp://*:5560".to_string()
}

// -----------------------------------------------------------------------------
// Load configuration
// -----------------------------------------------------------------------------
fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    // Get home directory and construct config path
    let home_dir = dirs::home_dir().ok_or("Could not determine home directory")?;
    let config_path = home_dir.join(".corky").join("config.toml");
    
    // Check if config file exists
    if !config_path.exists() {
        eprintln!("Configuration file not found at: {}", config_path.display());
        eprintln!("Please get the latest configuration from GitHub.");
        process::exit(1);
    }
    
    // Read and parse config file
    let config_content = fs::read_to_string(&config_path)?;
    let config: Config = toml::from_str(&config_content)?;
    
    Ok(config)
}



// Helper to format JSON values with pretty printing and trimming
fn format_value(value: &serde_json::Value, depth: usize) -> String {
    match value {
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                return "[]".to_string();
            }
            
            // For top-level arrays, show more items before truncating
            let (max_items, show_ellipsis) = if depth <= 1 {
                (10, arr.len() > 15)  // Show first 10 items if more than 15
            } else {
                (3, arr.len() > 6)     // For nested arrays, be more aggressive with truncation
            };
            
            let mut result = String::from("[");
            let len = arr.len();
            
            // Always show first few items
            let show_items = len.min(max_items);
            for (i, item) in arr.iter().take(show_items).enumerate() {
                if i > 0 {
                    result.push_str(", ");
                }
                result.push_str(&format_value(item, depth + 1));
            }
            
            // Add ellipsis if there are more items
            if show_ellipsis && len > show_items {
                result.push_str(", ... ");
                
                // Show last few items if we have enough
                if len > show_items * 2 {
                    for i in (len - max_items.min(3))..len {
                        result.push_str(", ");
                        result.push_str(&format_value(&arr[i], depth + 1));
                    }
                } else if len > show_items {
                    // If not too many, just show the rest
                    for i in show_items..len {
                        result.push_str(", ");
                        result.push_str(&format_value(&arr[i], depth + 1));
                    }
                }
            } else if len > show_items {
                // If we're not showing ellipsis but still have more items
                for i in show_items..len {
                    result.push_str(", ");
                    result.push_str(&format_value(&arr[i], depth + 1));
                }
            }
            
            result.push(']');
            result
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            
            // For top-level objects, show more fields
            let max_items = if depth <= 1 { 10 } else { 3 };  // Show more fields at top level
            let mut result = String::from("{\n");
            let len = map.len();
            let items: Vec<_> = map.iter().collect();
            
            // Show first few items
            let show_items = len.min(max_items);
            for (i, (k, v)) in items.iter().take(show_items).enumerate() {
                if i > 0 {
                    result.push_str(",\n");
                }
                result.push_str(&"  ".repeat(depth + 1));
                result.push_str(&format!("\"{}\": {}", k, format_value(v, depth + 1)));
            }
            
            // Add ellipsis if there are more items
            if len > max_items {
                result.push_str(",\n");
                result.push_str(&"  ".repeat(depth + 1));
                result.push_str("...");
                
                // For top level, show some key fields at the end if they exist
                if depth <= 1 {
                    let important_keys = ["symbol", "id", "type", "status"];
                    for key in important_keys {
                        if let Some(v) = map.get(key) {
                            result.push_str(",\n");
                            result.push_str(&"  ".repeat(depth + 1));
                            result.push_str(&format!("\"{}\": {}", key, format_value(v, depth + 1)));
                        }
                    }
                }
            }
            
            result.push('\n');
            result.push_str(&"  ".repeat(depth));
            result.push('}');
            result
        }
        serde_json::Value::String(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        _ => value.to_string(),
    }
}

// Check if a JSON value is a flat array of numbers (or timestamps)
fn is_flat_number_array(value: &serde_json::Value) -> bool {
    if let serde_json::Value::Array(arr) = value {
        if arr.is_empty() {
            return false;
        }
        // Check if all elements are numbers or arrays of numbers
        for item in arr {
            match item {
                serde_json::Value::Number(_) => {}
                serde_json::Value::Array(nested) => {
                    for n in nested {
                        if !n.is_number() {
                            return false;
                        }
                    }
                }
                _ => return false,
            }
        }
        true
    } else {
        false
    }
}

// Format a single message part with colors
fn format_message_part(part: &[u8]) -> String {
    // Try to parse as JSON first
    if let Ok(mut parsed) = serde_json::from_slice::<serde_json::Value>(part) {
        // Special handling for flat number arrays - show more elements
        if is_flat_number_array(&parsed) {
            if let serde_json::Value::Array(arr) = &mut parsed {
                // For large arrays, show more elements before truncating
                if arr.len() > 10 {
                    let first_five: Vec<_> = arr.iter().take(5).cloned().collect();
                    let last_five: Vec<_> = arr.iter().skip(arr.len() - 5).cloned().collect();
                    let mut new_arr = Vec::with_capacity(12);
                    new_arr.extend(first_five);
                    new_arr.push(serde_json::Value::String(
                        format!(" ... ({} more items) ... ", arr.len() - 10).yellow().to_string()
                    ));
                    new_arr.extend(last_five);
                    *arr = new_arr;
                }
            }
        }
        return format_value(&parsed, 0);
    }
    
    // Try to convert to UTF-8 string
    if let Ok(s) = std::str::from_utf8(part) {
        // If it looks like a JSON array or object, try to format it
        if s.trim().starts_with('[') || s.trim().starts_with('{') {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                return format_value(&parsed, 0);
            }
        }
        return format!("\"{}\"", s.green());
    }
    
    // If not valid UTF-8, show as byte array with length
    if part.len() > 20 {
        format!("{}[{} bytes: {:?}...]{}", 
            "[".dimmed(), 
            part.len().to_string().yellow(), 
            &part[..20], 
            "]".dimmed()
        )
    } else {
        format!("{:?}", part).dimmed().to_string()
    }
}

// Format a single ZMQ message part with colors
fn format_zmq_message_part(msg: &[u8]) -> String {
    let content = format_message_part(msg);
    
    // Add some color to the message based on content
    if content.starts_with('{') || content.starts_with('[') {
        // Likely JSON - already formatted with colors
        content
    } else if content.starts_with('"') {
        // String message
        content.green().to_string()
    } else if content.contains("bytes") {
        // Binary data
        content.dimmed().to_string()
    } else {
        // Other content
        content.cyan().to_string()
    }
}

// Format a complete ZMQ message (multiple parts)
fn format_zmq_message(parts: &[Vec<u8>]) -> String {
    if parts.is_empty() {
        return "[empty message]".to_string();
    }

    let parts_formatted: Vec<String> = parts
        .iter()
        .map(|part| format_zmq_message_part(part))
        .collect();

    if parts_formatted.len() == 1 {
        parts_formatted[0].clone()
    } else {
        let parts_str: Vec<&str> = parts_formatted.iter().map(|s| s.as_str()).collect();
        format!("{}[{}]{}", 
            "[".dimmed(),
            parts_str.join(" ").yellow(),
            "]".dimmed()
        )
    }
}

// Helper function to get color for log level
fn get_level_color(level: &log::Level) -> ColoredString {
    match level {
        Level::Error => "ERROR".red().bold(),
        Level::Warn => "WARN ".yellow().bold(),
        Level::Info => "INFO ".green(),
        Level::Debug => "DEBUG".blue(),
        Level::Trace => "TRACE".magenta(),
    }
}

// Helper function to colorize JSON values
fn colorize_json(value: &str) -> String {
    // This is a simple implementation that colors JSON strings, numbers, and keywords
    // For a more complete implementation, you might want to parse the JSON and color each part
    let mut result = String::with_capacity(value.len() * 2);
    let mut in_string = false;
    let mut in_number = false;
    let mut in_keyword = false;
    let mut buffer = String::new();

    for c in value.chars() {
        match c {
            '"' => {
                if in_string {
                    // End of string
                    result.push_str(&format!("{}\"", buffer.green()));
                    buffer.clear();
                } else {
                    // Start of string
                    if !buffer.is_empty() {
                        result.push_str(&buffer);
                        buffer.clear();
                    }
                    result.push('"');
                }
                in_string = !in_string;
            }
            '0'..='9' | '.' | '-' | '+' | 'e' | 'E' => {
                if !in_string && !in_number {
                    in_number = true;
                    buffer.push(c);
                } else if in_number {
                    buffer.push(c);
                } else {
                    result.push(c);
                }
            }
            't' | 'r' | 'u' | 'a' | 'l' | 's' | 'n' | 'f' => {
                if !in_string && !in_keyword {
                    in_keyword = true;
                    buffer.push(c);
                } else if in_keyword {
                    buffer.push(c);
                    if buffer == "true" || buffer == "false" || buffer == "null" {
                        result.push_str(&buffer.blue().to_string());
                        buffer.clear();
                        in_keyword = false;
                    }
                } else {
                    result.push(c);
                }
            }
            _ => {
                if in_number && !c.is_digit(10) && c != '.' {
                    result.push_str(&buffer.yellow().to_string());
                    buffer.clear();
                    in_number = false;
                }
                result.push(c);
            }
        }
    }
    
    // Add any remaining buffer
    if !buffer.is_empty() {
        if in_number {
            result.push_str(&buffer.yellow().to_string());
        } else if in_keyword {
            result.push_str(&buffer.blue().to_string());
        } else {
            result.push_str(&buffer);
        }
    }
    
    result
}

// -----------------------------------------------------------------------------
// Logging initialization using `fern`
// -----------------------------------------------------------------------------
fn setup_logger(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    // Enable colored output
    colored::control::set_override(true);
    
    // Get log level from config
    let log_level = match config.logging.level.to_lowercase().as_str() {
        "trace" => log::LevelFilter::Trace,
        "debug" => log::LevelFilter::Debug,
        "info" => log::LevelFilter::Info,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        _ => log::LevelFilter::Info, // Default to Info if level is invalid
    };

    // Configure `fern` logger to log only to stdout
    fern::Dispatch::new()
        .format(move |out, message, record| {
            let level = record.level();
            let target = if !record.target().is_empty() {
                record.target()
            } else {
                record.module_path().unwrap_or_default()
            };
            
            // Format the log line with colors
            let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let level_str = get_level_color(&level);
            let target_str = target.cyan().dimmed();
            
            out.finish(format_args!(
                "{} {} [{}] {}",
                timestamp.dimmed(),
                level_str,
                target_str,
                message
            ))
        })
        .level(log_level)
        .chain(std::io::stdout())
        .apply()?;
    
    Ok(())
}

// -----------------------------------------------------------------------------
// XSUB/XPUB Proxy
// -----------------------------------------------------------------------------
fn run_proxy(context: &zmq::Context, config: &Config) -> Result<(), zmq::Error> {
    // Create the XSUB socket
    let xsub_socket = context.socket(zmq::XSUB)?;
    xsub_socket.bind(&config.network.proxy_xsub_endpoint)?;
    info!("(Proxy) XSUB bound to {}", config.network.proxy_xsub_endpoint);

    // Create the XPUB socket
    let xpub_socket = context.socket(zmq::XPUB)?;
    xpub_socket.bind(&config.network.proxy_xpub_endpoint)?;
    info!("(Proxy) XPUB bound to {}", config.network.proxy_xpub_endpoint);

    info!("(Proxy) Starting XSUB/XPUB forwarder...");
    // Blocks forever forwarding messages between XPUB <--> XSUB
    zmq::proxy(&xpub_socket, &xsub_socket)?;

    // Normally `zmq::proxy` never returns unless there's an error:
    Ok(())
}

// -----------------------------------------------------------------------------
// ROUTER/DEALER Broker
// -----------------------------------------------------------------------------
fn run_broker(context: &zmq::Context, config: &Config) -> Result<(), zmq::Error> {
    // (1) Request-Reply ROUTER for handling 3-frame messages
    let client_to_client_direct_messaging_router = context.socket(zmq::ROUTER)?;
    client_to_client_direct_messaging_router.bind(&config.network.client_to_client_endpoint)?;
    info!("(Broker) client_to_client_direct_messaging_router (ROUTER) bound to {}", 
          config.network.client_to_client_endpoint);

    // (2) Client-facing ROUTER that receives messages from clients
    let client_facing_router = context.socket(zmq::ROUTER)?;
    client_facing_router.bind(&config.network.client_facing_endpoint)?;
    info!("(Broker) client_facing_router (ROUTER) bound to {}", 
          config.network.client_facing_endpoint);

    // (3) Worker-facing DEALER that distributes work to backend workers
    let worker_facing_dealer = context.socket(zmq::DEALER)?;
    worker_facing_dealer.bind(&config.network.worker_facing_endpoint)?;
    info!("(Broker) worker_facing_dealer (DEALER) bound to {}", 
          config.network.worker_facing_endpoint);

    info!("(Broker) Broker loop started. Polling for messages...");

    // We'll store poll items (one per socket).
    // Each PollItem is basically "which socket do we watch for read/write events, and how?"
    let mut poll_items = [
        client_to_client_direct_messaging_router.as_poll_item(zmq::POLLIN),
        client_facing_router.as_poll_item(zmq::POLLIN),
        worker_facing_dealer.as_poll_item(zmq::POLLIN),
    ];

    // For convenience, keep indexes as constants so the code is self-explanatory:
    const IDX_CLIENT_TO_CLIENT_DIRECT_MESSAGING_ROUTER: usize = 0;
    const IDX_CLIENT_FACING_ROUTER: usize = 1;
    const IDX_WORKER_FACING_DEALER: usize = 2;

    loop {
        // Poll indefinitely (-1). If you want a timeout, specify in ms.
        zmq::poll(&mut poll_items, -1)?;

        // (1) Request-Reply router communicator
        if poll_items[IDX_CLIENT_TO_CLIENT_DIRECT_MESSAGING_ROUTER].is_readable() {
            match client_to_client_direct_messaging_router.recv_multipart(0) {
                Ok(msg) => {
                    // Log the frames using our pretty formatter
                    let msg_str = format_zmq_message(&msg);
                    info!(
                        "(Broker) Received from {}: {}",
                        "client_to_client_direct_messaging_router".cyan(),
                        msg_str
                    );

                    // If you expect 3 frames [client_id, empty, payload], you can reorder them:
                    if msg.len() == 3 {
                        let client_id = &msg[0];
                        let empty = &msg[1];
                        let payload = &msg[2];

                        info!("(Broker) Sending back reversed envelope...");
                        if let Err(e) = client_to_client_direct_messaging_router.send_multipart(&[empty, client_id, payload], 0) {
                            error!("(Broker) Error sending to client_to_client_direct_messaging_router: {}", e);
                        }
                    } else {
                        let msg_str = format_zmq_message(&msg);
                        warn!(
                            "(Broker) Unexpected {} message format ({} frames): {}",
                            "client_to_client_direct_messaging_router".yellow().bold(),
                            msg.len().to_string().red(),
                            msg_str
                        );
                    }
                }
                Err(e) => error!("(Broker) Error receiving from client_to_client_direct_messaging_router: {}", e),
            }
        }

        // (2) Client-facing -> Worker-facing forwarding
        if poll_items[IDX_CLIENT_FACING_ROUTER].is_readable() {
            match client_facing_router.recv_multipart(0) {
                Ok(message) => {
                    let msg = format_zmq_message(&message);
                    debug!(
                        "(Broker) Forwarding message from {} to {}: {}",
                        "client_facing_router".cyan(),
                        "worker_facing_dealer".cyan(),
                        msg.yellow()
                    );
                    if let Err(e) = worker_facing_dealer.send_multipart(&message, 0) {
                        error!("(Broker) Error forwarding to worker_facing_dealer: {}", e);
                    }
                }
                Err(e) => error!("(Broker) Error receiving from client_facing_router: {}", e),
            }
        }

        // (3) Worker-facing -> Client-facing forwarding
        if poll_items[IDX_WORKER_FACING_DEALER].is_readable() {
            match worker_facing_dealer.recv_multipart(0) {
                Ok(message) => {
                    // Log the forwarded message
                    let msg = format_zmq_message(&message);
                    debug!(
                        "(Broker) Forwarding message from {} to {}: {}",
                        "worker_facing_dealer".cyan(),
                        "client_facing_router".cyan(),
                        msg.yellow()
                    );
                    if let Err(e) = client_facing_router.send_multipart(&message, 0) {
                        error!("(Broker) Error forwarding to client_facing_router: {}", e);
                    }
                }
                Err(e) => error!("(Broker) Error receiving from worker_facing_dealer: {}", e),
            }
        }
    }
}

// -----------------------------------------------------------------------------
// main
// -----------------------------------------------------------------------------
fn main() {
    // 1. Initialize logging
    let config = load_config().expect("Failed to load configuration");
    if let Err(e) = setup_logger(&config) {
        eprintln!("Failed to initialize logger: {}", e);
        std::process::exit(1);
    }
    info!("ZMQ Combined Proxy & Broker (Rust Version) - Starting...");

    // 2. Create a global ZMQ context
    let context = zmq::Context::new();

    // 3. Start proxy (XSUB/XPUB forwarder) in a background thread
    let ctx_for_proxy = context.clone();
    let config_for_proxy = config.clone();
    let proxy_thread = thread::spawn(move || {
        loop {
            match run_proxy(&ctx_for_proxy, &config_for_proxy) {
                Ok(_) => {
                    // If for some reason the proxy returns Ok, break out of the loop
                    info!("(Proxy) Stopped without error. Exiting proxy thread...");
                    break;
                }
                Err(e) => {
                    error!("(Proxy) Encountered an error: {}", e);
                    // Retry after a brief pause
                    thread::sleep(Duration::from_secs(3));
                    warn!("(Proxy) Retrying XSUB/XPUB proxy...");
                }
            }
        }
    });

    // 4. Run the broker poll loop in the main thread (blocks forever, typically)
    if let Err(e) = run_broker(&context, &config) {
        error!("(Broker) Encountered an error: {}", e);
        // In a production system, you might attempt to re-init or gracefully shut down here.
    }

    // 5. (Unreachable in normal usage—broker never returns.)
    let _ = proxy_thread.join();
    info!("(Main) Exiting.");
}
