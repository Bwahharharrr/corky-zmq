# Corky ZMQ Service

A robust ZeroMQ-based messaging service that implements both pub/sub and request/reply patterns in a single application, providing flexible network communication patterns for distributed systems.

## Overview

Corky ZMQ Service is a high-performance message broker and proxy implemented in Rust using the ZeroMQ (ØMQ) library. It provides both publish/subscribe capabilities via an XSUB/XPUB proxy and request/reply patterns through a ROUTER/DEALER broker.

## Key Features

- **Combined Proxy and Broker** - Runs both XSUB/XPUB proxy and ROUTER/DEALER broker in a single service
- **Configurable Endpoints** - All socket endpoints are configurable through configuration file
- **Descriptive Socket Naming** - Clear naming conventions for socket types based on their purpose
- **Robust Logging** - Comprehensive logging with configurable log levels and output paths
- **Auto-Recovery** - Built-in error handling with automatic retry capability
- **Easy Installation/Uninstallation** - Scripts for easy system or user-level installation

## Architecture

The service consists of two main components:

1. **XSUB/XPUB Proxy**
   - Binds to ports 5557 (XSUB) and 5558 (XPUB) by default
   - Forwards messages between publishers and subscribers
   - Runs in a dedicated background thread with auto-restart capability

2. **ROUTER/DEALER Broker**
   - Implements three socket patterns:
     - **Client-to-Client Direct Messaging Router** - Handles three-frame messages (client_id, empty delimiter, payload) for direct client-to-client communication
     - **Client-Facing Router** - Receives messages from clients in a broker pattern
     - **Worker-Facing Dealer** - Distributes work to backend workers
   - Polls all sockets efficiently in the main thread

## Installation

The service includes a comprehensive installation script that handles all aspects of deployment:

```bash
./install.sh
```

The installer provides two options:
1. **System installation** (recommended for production)
   - Logs stored in `/var/log/corky/`
   - Requires sudo privileges
   - Sets up system-wide service and log rotation

2. **User installation** (for development/testing)
   - Logs stored in `~/.corky/logs/`
   - No sudo required
   - Sets up user-level service and log rotation

The installation process:
- Builds the executable from source
- Creates necessary directories
- Installs the executable to `~/.corky/bin/corky-service-zmq`
- Sets up configuration
- Adds the bin directory to PATH
- Configures log rotation
- Creates appropriate systemd service files

## Uninstallation

To remove the service:

```bash
./uninstall.sh
```

The uninstaller provides a safe removal process:
- Stops any running services
- Removes only the Corky ZMQ service files
- Preserves shared resources that might be used by other Corky services
- Gives you control over whether to retain logs

## Configuration

Configuration is managed through a TOML file located at `~/.corky/config.toml`. An example configuration is provided in `example.config.toml`.

```toml
# Logging Configuration
[logging]
file_path = "/var/log/corky-service.log"  # Log file path
level = "info"                           # Log level: trace, debug, info, warn, error

# Network Configuration
[network]
# ZMQ XSUB socket endpoint (Proxy)
proxy_xsub_endpoint = "tcp://*:5557"

# ZMQ XPUB socket endpoint (Proxy)
proxy_xpub_endpoint = "tcp://*:5558"

# ZMQ client-to-client direct messaging router endpoint
client_to_client_endpoint = "tcp://*:6565"

# ZMQ client-facing router endpoint (Broker)
client_facing_endpoint = "tcp://*:5559"

# ZMQ worker-facing dealer endpoint (Broker)
worker_facing_endpoint = "tcp://*:5560"
```

All network endpoints have sensible defaults if not specified in the configuration.

## Service Management

After installation, you can manage the service using systemd:

### System-level installation:
```bash
sudo systemctl start corky-zmq.service
sudo systemctl stop corky-zmq.service
sudo systemctl status corky-zmq.service
```

### User-level installation:
```bash
systemctl --user start corky-zmq.service
systemctl --user stop corky-zmq.service
systemctl --user status corky-zmq.service
```

## Development

### Dependencies

The service uses the following Rust crates:
- `zmq`: ZeroMQ messaging library
- `log` and `fern`: Logging framework
- `chrono`: Date and time functionality
- `toml` and `serde`: Configuration parsing
- `dirs`: Cross-platform directory handling

### Building from Source

```bash
cargo build --release
```

## License

[Add your license information here]

## Contributing

[Add your contribution guidelines here]
