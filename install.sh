#!/bin/bash
set -e

# FakeNotify installer
# Run as: sudo ./install.sh

INSTALL_DIR="/usr/local"
SYSTEMD_DIR="/etc/systemd/system"
CONFIG_DIR="/etc/fakenotify"
OLD_PRELOAD_PATH="/run/fakenotify/libfakenotify_preload.so"
NEW_PRELOAD_PATH="/usr/local/lib/libfakenotify_preload.so"

# Migration function to detect and warn about old paths
migrate_old_paths() {
    echo ""
    echo "Checking for old FakeNotify paths..."

    local found_old=0

    # Check environment files
    for envfile in /etc/environment ~/.bashrc ~/.zshrc ~/.profile; do
        if [ -f "$envfile" ] && grep -q "$OLD_PRELOAD_PATH" "$envfile" 2>/dev/null; then
            echo "  WARNING: Found old path in $envfile"
            echo "           Please update LD_PRELOAD to: $NEW_PRELOAD_PATH"
            found_old=1
        fi
    done

    # Search for docker-compose files with old path
    if command -v find &>/dev/null; then
        while IFS= read -r f; do
            if [ -n "$f" ] && grep -q "$OLD_PRELOAD_PATH" "$f" 2>/dev/null; then
                echo "  WARNING: Found old path in $f"
                found_old=1
            fi
        done < <(find /home -maxdepth 4 \( -name "docker-compose*.yml" -o -name "compose*.yml" \) 2>/dev/null | head -20)
    fi

    # Check systemd override files
    if [ -d /etc/systemd/system ]; then
        for f in /etc/systemd/system/*.service.d/*.conf /etc/systemd/system/*.service 2>/dev/null; do
            if [ -f "$f" ] && grep -q "$OLD_PRELOAD_PATH" "$f" 2>/dev/null; then
                echo "  WARNING: Found old path in $f"
                found_old=1
            fi
        done
    fi

    if [ "$found_old" -eq 1 ]; then
        echo ""
        echo "  To fix, update LD_PRELOAD from:"
        echo "    $OLD_PRELOAD_PATH"
        echo "  to:"
        echo "    $NEW_PRELOAD_PATH"
        echo ""
        echo "  And update volume mounts to:"
        echo "    - /run/fakenotify:/run/fakenotify:ro"
        echo "    - /usr/local/lib/libfakenotify_preload.so:/usr/local/lib/libfakenotify_preload.so:ro"
    else
        echo "  No old paths found."
    fi
}

echo "Installing FakeNotify..."

# Check if built
if [ ! -f "target/release/fakenotifyd" ] || [ ! -f "target/release/libfakenotify_preload.so" ]; then
    echo "Building first..."
    cargo build --release
fi

# Install binaries
echo "Installing binaries to $INSTALL_DIR..."
install -Dm755 target/release/fakenotifyd "$INSTALL_DIR/bin/fakenotifyd"
install -Dm755 target/release/libfakenotify_preload.so "$INSTALL_DIR/lib/libfakenotify_preload.so"

# Install systemd service
echo "Installing systemd service..."
install -Dm644 fakenotify.service "$SYSTEMD_DIR/fakenotify.service"

# Create config directory
mkdir -p "$CONFIG_DIR"

# Create default config if not exists
if [ ! -f "$CONFIG_DIR/config.toml" ]; then
    echo "Creating default config..."
    cat > "$CONFIG_DIR/config.toml" << 'EOF'
[daemon]
socket = "/run/fakenotify/fakenotify.sock"
log_level = "info"

# Add your NFS paths here:
# [[watch]]
# path = "/mnt/media"
# poll_interval = 5
# recursive = true
EOF
fi

# Reload systemd
systemctl daemon-reload

echo ""
echo "Installation complete!"
echo ""
echo "Next steps:"
echo "  1. Edit /etc/fakenotify/config.toml to add your NFS paths"
echo "  2. Start the daemon: sudo systemctl start fakenotify"
echo "  3. Enable on boot: sudo systemctl enable fakenotify"
echo ""
echo "For Docker containers, add to your compose:"
echo "  environment:"
echo "    - LD_PRELOAD=/usr/local/lib/libfakenotify_preload.so"
echo "  volumes:"
echo "    - /run/fakenotify:/run/fakenotify:ro"
echo "    - /usr/local/lib/libfakenotify_preload.so:/usr/local/lib/libfakenotify_preload.so:ro"
echo ""
echo "For LinuxServer.io containers, use the DockerMod instead:"
echo "  environment:"
echo "    - DOCKER_MODS=ghcr.io/zachhandley/fakenotify-mod:latest"
echo "  volumes:"
echo "    - /run/fakenotify:/run/fakenotify:ro"
echo "    - /usr/local/lib/libfakenotify_preload.so:/usr/local/lib/libfakenotify_preload.so:ro"

# Check for old paths that need updating
migrate_old_paths
