#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=======================================================${NC}"
echo -e "${GREEN}Corky ZMQ Service Installer${NC}"
echo -e "${BLUE}=======================================================${NC}"

# Check if running as root
IS_ROOT=0
if [ $(id -u) -eq 0 ]; then
    IS_ROOT=1
fi

# Variables
CORKY_DIR="$HOME/.corky"
CORKY_BIN_DIR="$CORKY_DIR/bin"
CORKY_CONFIG_DIR="$CORKY_DIR"
EXECUTABLE_NAME="corky-service-zmq"
EXECUTABLE_PATH="$CORKY_BIN_DIR/$EXECUTABLE_NAME"
CONFIG_EXAMPLE="$(pwd)/example.config.toml"
CONFIG_DEST="$CORKY_CONFIG_DIR/config.toml"
LOGROTATE_CONFIG="$(pwd)/corky-logrotate.conf"
LOGROTATE_DEST="/etc/logrotate.d/corky"

# Prompt for log storage location if not running as root
if [ $IS_ROOT -eq 0 ]; then
    echo -e "\n${BLUE}Log Storage Selection${NC}"
    echo -e "${YELLOW}Corky service needs to store log files. There are two options:${NC}"
    echo -e "  ${GREEN}1) System location${NC} (/var/log/corky) - ${YELLOW}Requires sudo privileges${NC}"
    echo -e "     - Follows proper Linux standards for system services"
    echo -e "     - Logs are stored with other system logs"
    echo -e "     - Properly managed by system log rotation"
    echo -e "     - Won't fill up your home directory"
    echo -e "     - ${GREEN}Recommended for production use${NC}"
    echo -e ""
    echo -e "  ${GREEN}2) Home directory${NC} ($HOME/.corky/logs) - ${YELLOW}No sudo required${NC}"
    echo -e "     - Easy to set up without admin privileges"
    echo -e "     - Useful for development or testing"
    echo -e "     - ${RED}WARNING: May consume significant disk space in your home directory if not monitored${NC}"
    echo -e "     - ${RED}WARNING: Can potentially fill up your home partition${NC}"
    echo -e ""
    read -p "Enter your choice (1 or 2): " LOG_LOCATION_CHOICE
    
    if [ "$LOG_LOCATION_CHOICE" == "1" ]; then
        echo -e "${YELLOW}You've chosen to use the system location for logs.${NC}"
        echo -e "${YELLOW}The installer will now ask for sudo privileges to set up log directories.${NC}"
        echo ""
        
        # Run this script again with sudo if the user chose system location
        sudo "$0"
        exit 0
    elif [ "$LOG_LOCATION_CHOICE" == "2" ]; then
        echo -e "${YELLOW}You've chosen to store logs in your home directory.${NC}"
        echo -e "${RED}Remember to periodically check ~/.corky/logs to ensure it doesn't consume too much space.${NC}"
        CORKY_LOG_DIR="$CORKY_DIR/logs"
        LOG_FILE_PATH="$CORKY_LOG_DIR/corky-service.log"
        LOG_USER="$USER"
        LOG_GROUP="$USER"
    else
        echo -e "${RED}Invalid choice. Defaulting to home directory.${NC}"
        CORKY_LOG_DIR="$CORKY_DIR/logs"
        LOG_FILE_PATH="$CORKY_LOG_DIR/corky-service.log"
        LOG_USER="$USER"
        LOG_GROUP="$USER"
    fi
else
    # Root user - use system locations
    CORKY_LOG_DIR="/var/log/corky"
    LOG_FILE_PATH="$CORKY_LOG_DIR/corky-service.log"
    LOG_USER="root"
    LOG_GROUP="root"
fi

# Function to check if a directory exists, create it if not
create_dir_if_not_exists() {
    if [ ! -d "$1" ]; then
        echo -e "${YELLOW}Creating directory: $1${NC}"
        mkdir -p "$1"
        
        # Set appropriate ownership if we're root and this isn't in home
        if [ $IS_ROOT -eq 1 ] && [[ "$1" != "$HOME"* ]]; then
            chown root:root "$1"
            chmod 755 "$1"
        fi
    else
        echo -e "${GREEN}Directory already exists: $1${NC}"
    fi
}

# Step 1: Create necessary directories
echo -e "\n${BLUE}Step 1: Creating necessary directories...${NC}"
create_dir_if_not_exists "$CORKY_DIR"
create_dir_if_not_exists "$CORKY_BIN_DIR"
create_dir_if_not_exists "$CORKY_CONFIG_DIR"
create_dir_if_not_exists "$CORKY_LOG_DIR"

# Step 2: Build the executable
echo -e "\n${BLUE}Step 2: Building the executable...${NC}"
cargo build --release
if [ $? -ne 0 ]; then
    echo -e "${RED}Error: Failed to build the executable.${NC}"
    exit 1
fi

# Step 3: Copy the executable to the bin directory
echo -e "\n${BLUE}Step 3: Installing the executable...${NC}"
cp "$(pwd)/target/release/service-zmq" "$EXECUTABLE_PATH"
chmod +x "$EXECUTABLE_PATH"
echo -e "${GREEN}Executable installed at: $EXECUTABLE_PATH${NC}"

# Step 4: Copy config file if it doesn't exist
echo -e "\n${BLUE}Step 4: Setting up configuration...${NC}"
if [ ! -f "$CONFIG_DEST" ]; then
    echo -e "${YELLOW}Creating config file from example...${NC}"
    cp "$CONFIG_EXAMPLE" "$CONFIG_DEST"
    # Update the log file path in the config to point to the logs directory
    sed -i "s|/var/log/corky-service.log|$LOG_FILE_PATH|g" "$CONFIG_DEST"
    echo -e "${GREEN}Config file created at: $CONFIG_DEST${NC}"
else
    echo -e "${GREEN}Config file already exists at: $CONFIG_DEST${NC}"
    echo -e "${YELLOW}Note: The existing config file was not modified.${NC}"
    echo -e "${YELLOW}If needed, manually update the log file path to: $LOG_FILE_PATH${NC}"
fi

# Step 5: Setup PATH if needed
echo -e "\n${BLUE}Step 5: Checking PATH configuration...${NC}"
if [[ ":$PATH:" != *":$CORKY_BIN_DIR:"* ]]; then
    echo -e "${YELLOW}Adding $CORKY_BIN_DIR to PATH...${NC}"
    
    # Determine which shell configuration file to use
    SHELL_CONFIG=""
    if [ -f "$HOME/.bashrc" ]; then
        SHELL_CONFIG="$HOME/.bashrc"
    elif [ -f "$HOME/.zshrc" ]; then
        SHELL_CONFIG="$HOME/.zshrc"
    elif [ -f "$HOME/.profile" ]; then
        SHELL_CONFIG="$HOME/.profile"
    fi
    
    if [ -n "$SHELL_CONFIG" ]; then
        echo "# Added by Corky ZMQ Service Installer" >> "$SHELL_CONFIG"
        echo "export PATH=\"\$PATH:$CORKY_BIN_DIR\"" >> "$SHELL_CONFIG"
        echo -e "${GREEN}Added $CORKY_BIN_DIR to PATH in $SHELL_CONFIG${NC}"
        echo -e "${YELLOW}Please run 'source $SHELL_CONFIG' or restart your terminal to apply changes${NC}"
    else
        echo -e "${RED}Could not find a shell configuration file to update.${NC}"
        echo -e "${YELLOW}Please manually add the following line to your shell configuration:${NC}"
        echo -e "${YELLOW}export PATH=\"\$PATH:$CORKY_BIN_DIR\"${NC}"
    fi
else
    echo -e "${GREEN}$CORKY_BIN_DIR is already in PATH${NC}"
fi

# Step 6: Setup logrotate
echo -e "\n${BLUE}Step 6: Setting up log rotation...${NC}"
if [ $IS_ROOT -eq 1 ]; then
    echo -e "${YELLOW}Installing system-wide log rotation configuration...${NC}"
    
    # Create a proper system logrotate config
    cat > "$LOGROTATE_DEST" << EOF
# Logrotate configuration for Corky ZMQ Service
$CORKY_LOG_DIR/*.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
    create 0644 $LOG_USER $LOG_GROUP
    dateext
    dateformat -%Y%m%d
    postrotate
        systemctl restart corky-zmq.service >/dev/null 2>&1 || true
    endscript
}
EOF
    chmod 644 "$LOGROTATE_DEST"
    echo -e "${GREEN}Log rotation configuration installed at: $LOGROTATE_DEST${NC}"
else
    echo -e "${YELLOW}Not running as root, skipping system logrotate installation.${NC}"
    echo -e "${YELLOW}To setup system-wide log rotation, please run:${NC}"
    echo -e "${YELLOW}sudo $0${NC}"
    
    # Create a user-level logrotate config as an alternative
    USER_LOGROTATE_CONF="$CORKY_DIR/logrotate.conf"
    echo -e "${YELLOW}Creating user-level logrotate configuration at: $USER_LOGROTATE_CONF${NC}"
    cat > "$USER_LOGROTATE_CONF" << EOF
$CORKY_LOG_DIR/*.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
    create 644 $LOG_USER $LOG_GROUP
    dateext
    dateformat -%Y%m%d
}
EOF
    echo -e "${YELLOW}You can manually run logrotate with:${NC}"
    echo -e "${YELLOW}logrotate -v $USER_LOGROTATE_CONF${NC}"
    echo -e "${YELLOW}Consider adding this to your crontab:${NC}"
    echo -e "${YELLOW}0 0 * * * /usr/sbin/logrotate -v $USER_LOGROTATE_CONF${NC}"
fi

# Step 7: Create appropriate service file based on installation type
echo -e "\n${BLUE}Step 7: Setting up service...${NC}"

if [ $IS_ROOT -eq 1 ]; then
    # Create a system-wide service
    SYSTEMD_SERVICE_FILE="/etc/systemd/system/corky-zmq.service"
    echo -e "${YELLOW}Creating system-wide systemd service at: $SYSTEMD_SERVICE_FILE${NC}"
    
    cat > "$SYSTEMD_SERVICE_FILE" << EOF
[Unit]
Description=Corky ZMQ Service
After=network.target

[Service]
ExecStart=$EXECUTABLE_PATH
Restart=on-failure
RestartSec=5s
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF
    chmod 644 "$SYSTEMD_SERVICE_FILE"
    
    echo -e "${YELLOW}You can now start and enable the service with:${NC}"
    echo -e "${YELLOW}systemctl daemon-reload${NC}"
    echo -e "${YELLOW}systemctl enable --now corky-zmq.service${NC}"
    echo -e "${YELLOW}To check the status: systemctl status corky-zmq.service${NC}"
else
    # Create a user service
    SYSTEMD_USER_DIR="$HOME/.config/systemd/user"
    create_dir_if_not_exists "$SYSTEMD_USER_DIR"
    
    SYSTEMD_SERVICE_FILE="$SYSTEMD_USER_DIR/corky-zmq.service"
    echo -e "${YELLOW}Creating systemd user service at: $SYSTEMD_SERVICE_FILE${NC}"
    
    cat > "$SYSTEMD_SERVICE_FILE" << EOF
[Unit]
Description=Corky ZMQ Service
After=network.target

[Service]
ExecStart=$EXECUTABLE_PATH
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=default.target
EOF
    
    echo -e "${YELLOW}You can now start and enable the service with:${NC}"
    echo -e "${YELLOW}systemctl --user daemon-reload${NC}"
    echo -e "${YELLOW}systemctl --user enable --now corky-zmq.service${NC}"
    echo -e "${YELLOW}To check the status: systemctl --user status corky-zmq.service${NC}"
fi

echo -e "\n${GREEN}Installation completed successfully!${NC}"
echo -e "${GREEN}Executable: $EXECUTABLE_PATH${NC}"
echo -e "${GREEN}Config: $CONFIG_DEST${NC}"
echo -e "${GREEN}Logs: $CORKY_LOG_DIR${NC}"
echo -e "${BLUE}=======================================================${NC}"
