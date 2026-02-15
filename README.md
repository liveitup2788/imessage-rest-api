# imessage-rest-api

Send iMessages from any Linux server via a simple HTTP API. Built on top of [OpenBubbles](https://github.com/OpenBubbles/openbubbles-app) and [rustpush](https://github.com/OpenBubbles/rustpush).

```
curl -X POST http://localhost:8787/api/send \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"to": "+15551234567", "message": "Hello from Linux!"}'
```

## How It Works

```
┌──────────────────────────────────────────────────┐
│  Your app / n8n / curl / anything                 │
│  Makes HTTP requests to send messages             │
└───────────────────┬──────────────────────────────┘
                    │  POST http://localhost:8787/api/send
                    v
┌──────────────────────────────────────────────────┐
│  imessage-rest-api                                │
│  Reads an OpenBubbles session from disk           │
│  Connects to Apple's servers as your device       │
└───────────────────┬──────────────────────────────┘
                    │  Apple Push Service (APS)
                    v
┌──────────────────────────────────────────────────┐
│  Apple iMessage servers                           │
│  Delivers your message as a real iMessage         │
└──────────────────────────────────────────────────┘
```

OpenBubbles handles the hard part: registering with Apple, verifying your phone number, and creating the cryptographic session. This project is a thin HTTP wrapper (~200 lines of Rust) that reuses that session to expose three endpoints.

## Prerequisites

- A **Mac** (one-time only, for initial registration)
- An **Apple ID** with a phone number registered for iMessage
- A **Linux server** (tested on Oracle Linux 9 / Oracle Cloud free tier)
- **Rust** toolchain (`rustup`)
- **Flatpak** (for OpenBubbles)

## Setup

### 1. Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

### 2. Install OpenBubbles via Flatpak

```bash
sudo dnf install -y flatpak  # or apt install flatpak
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install flathub app.openbubbles.OpenBubbles
```

### 3. Register Your iMessage Identity

This is the one-time step that requires a GUI (remote desktop / VNC) and a Mac.

1. On your Mac, set up the OpenBubbles registration relay (see [OpenBubbles docs](https://openbubbles.app/))
2. On your Linux server, open OpenBubbles via remote desktop
3. In OpenBubbles: enter your Mac's relay code, log in with your Apple ID, verify your phone number via SMS
4. Once you see your conversations, registration is complete

The session files are saved to:
```
~/.var/app/app.openbubbles.OpenBubbles/data/bluebubbles/
```

After this step, your Mac is no longer needed.

### 4. Build imessage-rest-api

```bash
# Clone the OpenBubbles app (includes rustpush as a submodule)
git clone --recurse-submodules https://github.com/OpenBubbles/openbubbles-app.git
cd openbubbles-app
git checkout rustpush

# Clone this project into the repo (it depends on ../rustpush)
git clone https://github.com/YOUR_USERNAME/imessage-rest-api.git
cd imessage-rest-api
cargo build --release
```

The project must live inside the `openbubbles-app/` directory because it depends on `../rustpush` (which contains private Apple certificates that can't be distributed via a standalone git dependency).

The first build takes a few minutes (compiling rustpush and all its dependencies).

### 5. Run

**Important:** Stop OpenBubbles first. Only one connection to Apple can be active at a time.

```bash
flatpak kill app.openbubbles.OpenBubbles

RUST_LOG=info \
IMESSAGE_DATA_DIR=~/.var/app/app.openbubbles.OpenBubbles/data/bluebubbles \
IMESSAGE_API_KEY=your-secret-key \
./target/release/imessage-api
```

### 6. Test

```bash
# Health check
curl -H "Authorization: Bearer your-secret-key" http://localhost:8787/api/health

# List registered handles
curl -H "Authorization: Bearer your-secret-key" http://localhost:8787/api/handles

# Send a message
curl -X POST http://localhost:8787/api/send \
  -H "Authorization: Bearer your-secret-key" \
  -H "Content-Type: application/json" \
  -d '{"to": "+15551234567", "message": "Hello!"}'
```

## API Reference

### `POST /api/send`

Send an iMessage.

**Request:**
```json
{
  "to": "+15551234567",
  "message": "Hello from the API!"
}
```

**Response:**
```json
{
  "success": true,
  "message_id": "40872D59-9FE8-44D5-82DE-A570C8B15F3A"
}
```

Phone numbers are automatically formatted. All of these work:
- `+15551234567`
- `15551234567`
- `5551234567` (assumes US +1)
- `tel:+15551234567`

### `GET /api/handles`

List your registered iMessage handles.

**Response:**
```json
{
  "handles": [
    "tel:+15551234567",
    "mailto:you@icloud.com"
  ]
}
```

### `GET /api/health`

Check if the server is connected and has registered handles.

**Response:**
```json
{
  "status": "ok"
}
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `IMESSAGE_DATA_DIR` | (required) | Path to OpenBubbles data directory |
| `IMESSAGE_API_KEY` | (empty = no auth) | API key for Bearer token authentication |
| `IMESSAGE_API_PORT` | `8787` | Port to listen on |
| `RUST_LOG` | (none) | Log level (`info`, `debug`, `warn`) |

## Running as a systemd Service

```bash
sudo cp imessage-api.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable imessage-api
sudo systemctl start imessage-api
```

Edit the service file to set your `IMESSAGE_DATA_DIR` and `IMESSAGE_API_KEY`.

Check status:
```bash
sudo systemctl status imessage-api
sudo journalctl -u imessage-api -f
```

## n8n Integration

Add an **HTTP Request** node to any n8n workflow:

| Setting | Value |
|---------|-------|
| Method | `POST` |
| URL | `http://localhost:8787/api/send` |
| Authentication | Header Auth |
| Header Name | `Authorization` |
| Header Value | `Bearer your-secret-key` |
| Body Content Type | JSON |

**Body:**
```json
{
  "to": "{{ $json.phone }}",
  "message": "{{ $json.message }}"
}
```

If n8n runs in a container (Podman/Docker) on the same server, use the container gateway IP instead of `localhost`. Find it with:
```bash
# Podman
podman exec n8n ip route | grep default  # usually 10.88.0.1

# Docker
docker exec n8n ip route | grep default  # usually 172.17.0.1
```

Then use `http://10.88.0.1:8787/api/send` as the URL.

You may also need to allow container-to-host traffic in your firewall:
```bash
sudo firewall-cmd --zone=trusted --add-interface=podman0 --permanent
sudo firewall-cmd --reload
sudo podman network reload --all  # IMPORTANT: always run this after firewall reload
```

## Multiple Phone Numbers

Each phone number needs its own OpenBubbles session and its own API server instance.

1. Register a second Apple ID + phone number through OpenBubbles
2. Copy the session files to a separate directory
3. Run a second instance on a different port:

```bash
IMESSAGE_DATA_DIR=/path/to/account2 \
IMESSAGE_API_PORT=8788 \
IMESSAGE_API_KEY=key2 \
./target/release/imessage-api
```

## Security Notes

- The session files contain your Apple ID credentials and encryption keys. **Treat them like passwords.**
- By default the server binds to `0.0.0.0`. If you only need local access, consider binding behind a reverse proxy.
- Always set `IMESSAGE_API_KEY` in production.
- The API key is compared in constant-time is NOT implemented yet — for production use, put this behind nginx with HTTPS.

## Architecture

This project is a thin wrapper around [rustpush](https://github.com/OpenBubbles/rustpush), the Rust library that implements Apple's iMessage protocol. It:

1. Reads the session files that OpenBubbles created during registration
2. Initializes the cryptographic keystore
3. Opens an APS (Apple Push Service) connection
4. Creates an IMClient for sending messages
5. Runs a background task to keep the APS connection alive
6. Serves three HTTP endpoints via axum

The session auto-renews every ~45 days without any user interaction.

## License

SSPL-1.0 (Server Side Public License), same as [rustpush](https://github.com/OpenBubbles/rustpush).

## Credits

- [OpenBubbles](https://github.com/OpenBubbles/openbubbles-app) — the app that makes iMessage on non-Apple platforms possible
- [rustpush](https://github.com/OpenBubbles/rustpush) — the Rust library implementing Apple's messaging protocols
