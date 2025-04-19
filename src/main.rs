use std::thread;
use std::time::Duration;
use zmq;
use std::path::PathBuf;
use std::fs;
use std::process;

use log::{info, error, warn};
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

// -----------------------------------------------------------------------------
// Logging initialization using `fern`
// -----------------------------------------------------------------------------
fn setup_logger(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    // Get log level from config
    let log_level = match config.logging.level.to_lowercase().as_str() {
        "trace" => log::LevelFilter::Trace,
        "debug" => log::LevelFilter::Debug,
        "info" => log::LevelFilter::Info,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        _ => log::LevelFilter::Info, // Default to Info if level is invalid
    };

    // Configure `fern` logger to log to both stdout and a file:
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{}][{}] {}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log_level)
        .chain(std::io::stdout())
        .chain(fern::log_file(&config.logging.file_path)?)
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
                    // Example: log the frames received
                    info!("(Broker) Received from client_to_client_direct_messaging_router: {:?}", msg);

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
                        warn!(
                            "(Broker) Unexpected client_to_client_direct_messaging_router message format: {} frames, {:?}",
                            msg.len(),
                            msg
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
