#!/bin/bash
set -e

# FakeNotify release installer
# Downloads pre-built binaries from GitHub releases
#
# Usage: curl -sSL https://raw.githubusercontent.com/zachhandley/FakeNotify/main/install-release.sh | sudo bash

VERSION="${FAKENOTIFY_VERSION:-latest}"
INSTALL_DIR="/usr/local"
SYSTEMD_DIR="/etc/systemd/system"
CONFIG_DIR="/etc/fakenotify"
REPO="zachhandley/FakeNotify"

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
    aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    *)       echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

echo "Installing FakeNotify ($TARGET)..."

# Get download URL
if [ "$VERSION" = "latest" ]; then
    RELEASE_URL="https://github.com/$REPO/releases/latest/download"
else
    RELEASE_URL="https://github.com/$REPO/releases/download/v$VERSION"
fi

# Download binaries
echo "Downloading from $RELEASE_URL..."
TMP_DIR=$(mktemp -d)
trap "rm -rf $TMP_DIR" EXIT

curl -sSL "$RELEASE_URL/fakenotifyd-$TARGET" -o "$TMP_DIR/fakenotifyd"
curl -sSL "$RELEASE_URL/libfakenotify_preload-$TARGET.so" -o "$TMP_DIR/libfakenotify_preload.so"

# Install binaries
echo "Installing binaries..."
install -Dm755 "$TMP_DIR/fakenotifyd" "$INSTALL_DIR/bin/fakenotifyd"
install -Dm755 "$TMP_DIR/libfakenotify_preload.so" "$INSTALL_DIR/lib/libfakenotify_preload.so"

# Download and install systemd service
echo "Installing systemd service..."
curl -sSL "https://raw.githubusercontent.com/$REPO/main/fakenotify.service" -o "$TMP_DIR/fakenotify.service"
install -Dm644 "$TMP_DIR/fakenotify.service" "$SYSTEMD_DIR/fakenotify.service"

# Create config directory and default config
mkdir -p "$CONFIG_DIR"
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
echo "For Docker containers:"
echo "  environment:"
echo "    - LD_PRELOAD=/run/fakenotify/libfakenotify_preload.so"
echo "  volumes:"
echo "    - /run/fakenotify:/run/fakenotify"
