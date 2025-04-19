#!/bin/bash
# Removed the 'set -e' to prevent premature exits

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=======================================================${NC}"
echo -e "${RED}Corky ZMQ Service Uninstaller${NC}"
echo -e "${BLUE}=======================================================${NC}"

# Check if running as root
IS_ROOT=0
if [ $(id -u) -eq 0 ]; then
    IS_ROOT=1
fi

# Variables for installed locations
CORKY_DIR="$HOME/.corky"
CORKY_BIN_DIR="$CORKY_DIR/bin"
CORKY_CONFIG_DIR="$CORKY_DIR"
CORKY_LOG_DIR="$CORKY_DIR/logs"
SYSTEM_LOG_DIR="/var/log/corky"
EXECUTABLE_NAME="corky-service-zmq"
EXECUTABLE_PATH="$CORKY_BIN_DIR/$EXECUTABLE_NAME"
CONFIG_FILE="$CORKY_CONFIG_DIR/config.toml"
USER_LOGROTATE_CONF="$CORKY_DIR/logrotate.conf"
SYSTEM_LOGROTATE_CONF="/etc/logrotate.d/corky"
USER_SERVICE_FILE="$HOME/.config/systemd/user/corky-zmq.service"
SYSTEM_SERVICE_FILE="/etc/systemd/system/corky-zmq.service"

# Default to keeping logs
KEEP_LOGS_FLAG=1

# Function to display files that will be removed
show_removal_info() {
    echo -e "${BLUE}The following items will be removed:${NC}"
    
    # Always show user files
    echo -e "${YELLOW}User-level files:${NC}"
    [ -f "$EXECUTABLE_PATH" ] && echo "  - $EXECUTABLE_PATH (the executable only)"
    [ -f "$USER_LOGROTATE_CONF" ] && echo "  - $USER_LOGROTATE_CONF"
    [ -f "$USER_SERVICE_FILE" ] && echo "  - $USER_SERVICE_FILE"
    
    # Only show system files if root
    if [ $IS_ROOT -eq 1 ]; then
        echo -e "${YELLOW}System-level files:${NC}"
        [ -f "$SYSTEM_LOGROTATE_CONF" ] && echo "  - $SYSTEM_LOGROTATE_CONF"
        [ -f "$SYSTEM_SERVICE_FILE" ] && echo "  - $SYSTEM_SERVICE_FILE"
    else
        echo -e "${YELLOW}System-level files (requires sudo to remove):${NC}"
        [ -f "$SYSTEM_LOGROTATE_CONF" ] && echo "  - $SYSTEM_LOGROTATE_CONF"
        [ -f "$SYSTEM_SERVICE_FILE" ] && echo "  - $SYSTEM_SERVICE_FILE"
    fi
    
    # Check for PATH modifications
    if grep -q "CORKY_BIN_DIR" "$HOME/.bashrc" 2>/dev/null; then
        echo "  - PATH modification in $HOME/.bashrc"
    fi
    if grep -q "CORKY_BIN_DIR" "$HOME/.zshrc" 2>/dev/null; then
        echo "  - PATH modification in $HOME/.zshrc"
    fi
    if grep -q "CORKY_BIN_DIR" "$HOME/.profile" 2>/dev/null; then
        echo "  - PATH modification in $HOME/.profile"
    fi
    
    # Show what will NOT be removed
    echo -e "\n${GREEN}The following shared resources will be preserved:${NC}"
    echo "  - $CORKY_BIN_DIR (directory)"
    echo "  - $CONFIG_FILE (configuration file)"
    echo "  - $CORKY_DIR (main Corky directory)"
    
    if [ $KEEP_LOGS_FLAG -eq 1 ]; then
        echo "  - $CORKY_LOG_DIR (log files)"
        [ $IS_ROOT -eq 1 ] && [ -d "$SYSTEM_LOG_DIR" ] && echo "  - $SYSTEM_LOG_DIR (system log files)"
    fi
}

# Show what will be removed
show_removal_info

# Ask for confirmation
echo ""
echo -e "${RED}WARNING: This will remove all Corky ZMQ Service files and settings.${NC}"
echo -e "${YELLOW}Note: Log files will be preserved by default.${NC}"
read -p "Do you want to proceed with uninstallation? (y/n): " CONFIRM
if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
    echo -e "${GREEN}Uninstallation aborted.${NC}"
    exit 0
fi

# Ask about log files
read -p "Do you want to remove log files? (y/n, default: n): " REMOVE_LOGS
if [[ "$REMOVE_LOGS" =~ ^[Yy]$ ]]; then
    KEEP_LOGS_FLAG=0
    echo -e "${RED}Log files will be removed.${NC}"
else
    echo -e "${YELLOW}Log files will be preserved.${NC}"
fi

# 1. Stop and disable services if they exist
echo -e "\n${BLUE}Step 1: Stopping services...${NC}"

# User-level service
if [ -f "$USER_SERVICE_FILE" ]; then
    echo -e "${YELLOW}Stopping and disabling user service...${NC}"
    systemctl --user stop corky-zmq.service 2>/dev/null || true
    systemctl --user disable corky-zmq.service 2>/dev/null || true
    systemctl --user daemon-reload 2>/dev/null || true
fi

# System-level service (if root)
if [ $IS_ROOT -eq 1 ] && [ -f "$SYSTEM_SERVICE_FILE" ]; then
    echo -e "${YELLOW}Stopping and disabling system service...${NC}"
    systemctl stop corky-zmq.service 2>/dev/null || true
    systemctl disable corky-zmq.service 2>/dev/null || true
    systemctl daemon-reload 2>/dev/null || true
elif [ -f "$SYSTEM_SERVICE_FILE" ]; then
    echo -e "${YELLOW}System service exists but requires root to stop.${NC}"
    echo -e "${YELLOW}Consider running: sudo systemctl stop corky-zmq.service${NC}"
    echo -e "${YELLOW}Consider running: sudo systemctl disable corky-zmq.service${NC}"
fi

# 2. Remove binaries and configuration 
echo -e "\n${BLUE}Step 2: Removing executable...${NC}"

# Remove executable
if [ -f "$EXECUTABLE_PATH" ]; then
    echo -e "${YELLOW}Removing executable...${NC}"
    rm -f "$EXECUTABLE_PATH"
    echo -e "${GREEN}Removed: $EXECUTABLE_PATH${NC}"
else
    echo -e "${YELLOW}Executable not found: $EXECUTABLE_PATH${NC}"
fi

# Note about config preservation
echo -e "${GREEN}Preserving shared config file: $CONFIG_FILE${NC}"
echo -e "${YELLOW}This file may be used by other Corky services.${NC}"

# 3. Remove log files if requested
echo -e "\n${BLUE}Step 3: Handling log files...${NC}"
if [ $KEEP_LOGS_FLAG -eq 0 ]; then
    # Remove user logs
    if [ -d "$CORKY_LOG_DIR" ]; then
        echo -e "${YELLOW}Removing user log files...${NC}"
        rm -rf "$CORKY_LOG_DIR"
    fi
    
    # Remove system logs if root
    if [ $IS_ROOT -eq 1 ] && [ -d "$SYSTEM_LOG_DIR" ]; then
        echo -e "${YELLOW}Removing system log files...${NC}"
        rm -rf "$SYSTEM_LOG_DIR"
    elif [ -d "$SYSTEM_LOG_DIR" ]; then
        echo -e "${YELLOW}System log directory exists but requires root to remove.${NC}"
        echo -e "${YELLOW}Consider running: sudo rm -rf $SYSTEM_LOG_DIR${NC}"
    fi
else
    echo -e "${GREEN}Keeping log files as requested.${NC}"
    if [ -d "$CORKY_LOG_DIR" ]; then
        echo -e "${GREEN}User logs preserved at: $CORKY_LOG_DIR${NC}"
    fi
    if [ -d "$SYSTEM_LOG_DIR" ]; then
        echo -e "${GREEN}System logs preserved at: $SYSTEM_LOG_DIR${NC}"
    fi
fi

# 4. Remove systemd service files
echo -e "\n${BLUE}Step 4: Removing service files...${NC}"
if [ -f "$USER_SERVICE_FILE" ]; then
    echo -e "${YELLOW}Removing user service file...${NC}"
    rm -f "$USER_SERVICE_FILE"
fi

if [ $IS_ROOT -eq 1 ] && [ -f "$SYSTEM_SERVICE_FILE" ]; then
    echo -e "${YELLOW}Removing system service file...${NC}"
    rm -f "$SYSTEM_SERVICE_FILE"
elif [ -f "$SYSTEM_SERVICE_FILE" ]; then
    echo -e "${YELLOW}System service file exists but requires root to remove.${NC}"
    echo -e "${YELLOW}Consider running: sudo rm -f $SYSTEM_SERVICE_FILE${NC}"
fi

# 5. Remove logrotate configuration
echo -e "\n${BLUE}Step 5: Removing logrotate configuration...${NC}"
if [ -f "$USER_LOGROTATE_CONF" ]; then
    echo -e "${YELLOW}Removing user logrotate configuration...${NC}"
    rm -f "$USER_LOGROTATE_CONF"
fi

if [ $IS_ROOT -eq 1 ] && [ -f "$SYSTEM_LOGROTATE_CONF" ]; then
    echo -e "${YELLOW}Removing system logrotate configuration...${NC}"
    rm -f "$SYSTEM_LOGROTATE_CONF"
elif [ -f "$SYSTEM_LOGROTATE_CONF" ]; then
    echo -e "${YELLOW}System logrotate file exists but requires root to remove.${NC}"
    echo -e "${YELLOW}Consider running: sudo rm -f $SYSTEM_LOGROTATE_CONF${NC}"
fi

# 6. Remove PATH modifications (comment out added lines rather than removing)
echo -e "\n${BLUE}Step 6: Cleaning up PATH modifications...${NC}"
for RC_FILE in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
    if [ -f "$RC_FILE" ] && grep -q "Added by Corky ZMQ Service Installer" "$RC_FILE"; then
        echo -e "${YELLOW}Commenting out PATH modifications in $RC_FILE...${NC}"
        sed -i '/Added by Corky ZMQ Service Installer/s/^/# REMOVED: /' "$RC_FILE"
        sed -i '/export PATH=.*CORKY_BIN_DIR/s/^/# REMOVED: /' "$RC_FILE"
    fi
done

# 7. Clean up remaining directories if empty
echo -e "\n${BLUE}Step 7: Shared resources information...${NC}"
echo -e "${GREEN}The following shared resources were preserved:${NC}"
echo -e "${YELLOW}  - $CORKY_BIN_DIR (bin directory)${NC}"
echo -e "${YELLOW}  - $CONFIG_FILE (configuration file)${NC}"
echo -e "${YELLOW}  - $CORKY_DIR (main Corky directory)${NC}"

if [ $KEEP_LOGS_FLAG -eq 1 ]; then
    echo -e "${YELLOW}  - Log files in $CORKY_LOG_DIR${NC}"
    [ $IS_ROOT -eq 1 ] && [ -d "$SYSTEM_LOG_DIR" ] && echo -e "${YELLOW}  - System logs in $SYSTEM_LOG_DIR${NC}"
fi

# 8. Final summary
echo -e "\n${GREEN}Uninstallation completed!${NC}"
echo -e "${GREEN}Corky ZMQ Service has been removed from your system.${NC}"
echo -e "${GREEN}Shared resources were preserved for other Corky services.${NC}"

echo -e "${BLUE}=======================================================${NC}"
echo -e "${YELLOW}You may need to restart your terminal or run 'source ~/.bashrc'${NC}"
echo -e "${YELLOW}to complete the uninstallation process.${NC}"
echo -e "${BLUE}=======================================================${NC}"
