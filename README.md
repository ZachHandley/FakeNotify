# FakeNotify

**Transparent inotify injection for NFS filesystems**

FakeNotify solves the fundamental problem that NFS mounts don't emit inotify events, breaking applications like Jellyfin, Plex, Sonarr, Radarr, and qBittorrent that rely on filesystem change notifications.

## How It Works

```
┌─────────────────────────────────────────────────┐
│ Application (Jellyfin, Sonarr, etc.)            │
│                                                  │
│ inotify_init() ──→ [HOOK] ──→ returns pipe fd   │
│ inotify_add_watch() ──→ [HOOK] ──→ returns wd   │
│ read(fd) ←── receives inotify_event structs ────┤
└─────────────────────────────────────────────────┘
                    ↑
          LD_PRELOAD=libfakenotify.so
                    ↑
              Unix Socket IPC
                    ↓
┌─────────────────────────────────────────────────┐
│ fakenotifyd (daemon)                            │
│                                                  │
│ • Polls NFS mounts for changes                  │
│ • Tracks watch descriptors ↔ paths              │
│ • Writes synthetic inotify_event to pipes       │
│ • CLI for runtime configuration                 │
└─────────────────────────────────────────────────┘
```

**Two components:**

1. **`libfakenotify.so`** - LD_PRELOAD library that intercepts `inotify_init`, `inotify_add_watch`, `inotify_rm_watch`, returns pipe fds instead, and connects to the daemon
2. **`fakenotifyd`** - Background daemon that polls NFS paths, detects changes, and writes synthetic `inotify_event` structs to connected applications

## Features

- **Transparent** - No application modifications needed, just `LD_PRELOAD`
- **Docker-friendly** - Works with containerized apps via volume-mounted socket
- **Configurable polling** - Adjust intervals per-path based on your needs
- **Runtime CLI** - Add/remove watched paths without restart

## Installation

### One-Liner (Recommended)

```bash
curl -sSL https://raw.githubusercontent.com/zachhandley/FakeNotify/main/install-release.sh | sudo bash
```

Then configure and start:
```bash
sudo nano /etc/fakenotify/config.toml  # Add your NFS paths
sudo systemctl enable --now fakenotify
```

### From Source

```bash
git clone https://github.com/zachhandley/FakeNotify.git
cd FakeNotify
cargo build --release
sudo ./install.sh
```

## Usage

### Start the daemon

```bash
# Start with default config
fakenotifyd start

# Or specify config file
fakenotifyd start --config /etc/fakenotify/config.toml
```

### Configure watched paths

```bash
# Add an NFS path to monitor
fakenotifyd add /mnt/media --poll-interval 5s

# Remove a path
fakenotifyd remove /mnt/media

# List watched paths
fakenotifyd list

# Check status
fakenotifyd status
```

### Run applications with injection

```bash
# Single application
LD_PRELOAD=/usr/lib/libfakenotify.so jellyfin

# Docker container
docker run -e LD_PRELOAD=/fakenotify/libfakenotify.so \
           -v /usr/lib/libfakenotify.so:/fakenotify/libfakenotify.so:ro \
           -v /run/fakenotify.sock:/run/fakenotify.sock \
           jellyfin/jellyfin
```

### Docker Integration

**The daemon runs on the host**, containers just need the library and socket mounted.

```yaml
# docker-compose.yml
services:
  jellyfin:
    image: jellyfin/jellyfin
    environment:
      - LD_PRELOAD=/usr/local/lib/libfakenotify_preload.so
    volumes:
      # Mount the socket directory and library
      - /run/fakenotify:/run/fakenotify:ro
      - /usr/local/lib/libfakenotify_preload.so:/usr/local/lib/libfakenotify_preload.so:ro
      # Your NFS media
      - /mnt/media:/media
```

Two mounts: the socket directory and the library file.

### LinuxServer.io Containers (DockerMod)

For LSIO containers (Sonarr, Radarr, etc.), use the DockerMod - zero config needed:

```yaml
services:
  sonarr:
    image: linuxserver/sonarr
    environment:
      - DOCKER_MODS=ghcr.io/zachhandley/fakenotify-mod:latest
    volumes:
      - /run/fakenotify:/run/fakenotify:ro
      - /usr/local/lib/libfakenotify_preload.so:/usr/local/lib/libfakenotify_preload.so:ro
      - /mnt/media:/media
```

The mod automatically configures `LD_PRELOAD`.

## Configuration

`/etc/fakenotify/config.toml`:

```toml
[daemon]
socket = "/run/fakenotify.sock"
log_level = "info"

[[watch]]
path = "/mnt/media"
poll_interval = "5s"
recursive = true

[[watch]]
path = "/mnt/downloads"
poll_interval = "2s"
recursive = true
```

## How NFS + inotify Breaks

Linux's `inotify` monitors filesystem changes at the kernel VFS layer. When files change on an NFS server (or from another NFS client), the local kernel never sees the operation - it happens remotely. Therefore, `inotify` watches on NFS mounts are silent.

This affects:
- **Jellyfin/Plex/Emby** - Library not updating when new media added
- **Sonarr/Radarr** - Downloads not detected (when using folder watching)
- **qBittorrent** - Watched folders not triggering
- **Any app** using `inotify`, `fswatch`, `watchdog`, etc.

## Technical Details

### LD_PRELOAD Library

Uses the `redhook` crate to intercept:
- `inotify_init()` / `inotify_init1()` - Returns a pipe fd instead
- `inotify_add_watch()` - Registers path with daemon, returns synthetic wd
- `inotify_rm_watch()` - Unregisters path with daemon

The pipe fd is indistinguishable from a real inotify fd to the application - it works with `poll()`, `epoll()`, `select()`, and blocking `read()`.

### Daemon Polling

Uses the `notify` crate with `PollWatcher` backend:
- Periodic `stat()` calls to detect mtime/ctime changes
- Directory listing comparison for create/delete detection
- Debouncing via `notify-debouncer-full` to coalesce rapid changes

### Event Format

Writes standard `inotify_event` structs to pipes:
```c
struct inotify_event {
    int      wd;       // Watch descriptor
    uint32_t mask;     // Event mask (IN_CREATE, IN_MODIFY, etc.)
    uint32_t cookie;   // Cookie for rename pairing
    uint32_t len;      // Length of name field
    char     name[];   // Filename (variable length)
};
```

## Limitations

- **Only affects dynamically linked binaries** - Static binaries bypass LD_PRELOAD
- **Polling latency** - Changes detected on poll interval, not instantly
- **NFS attribute caching** - May need `actimeo=0` mount option for immediate visibility
- **No rename cookie pairing** - `IN_MOVED_FROM`/`IN_MOVED_TO` won't have matching cookies across polls

## Requirements

- Linux (uses Linux-specific inotify API)
- Rust 1.75+ (for building)
- NFS mounts accessible to the daemon

## License

MIT
