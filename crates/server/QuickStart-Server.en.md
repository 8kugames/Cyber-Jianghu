# Server Quick Start Guide

> **中文版本**: [QuickStart-Server.md](./QuickStart-Server.md)

This guide helps developers quickly deploy and run the Cyber-Jianghu game server.

## Requirements

- Rust (Cargo)
- Docker and Docker Compose
- PostgreSQL (can be run via Docker container)

## Starting the Service

### Using `install.sh` in the project root (recommended)

Use the provided install script from the **project root** to manage services:

```bash
# Start the dev environment (starts both Server and Agent)
./install.sh all start

# Start the production environment
./install.sh all start --prod

# Check service status
./install.sh all status

# View logs
./install.sh all logs

# Stop services
./install.sh all stop

# Warning: resets all database and volume data
./install.sh all reset
```

### Pure local Cargo startup (for development debugging)

Make sure the database is already running (e.g. via `docker compose up -d db`) and the connection is configured in `crates/server/.env`.

```bash
cd crates/server

# Copy the env template and adjust the database password / connection string
cp .env.example .env

# Run the server
cargo run

# Or run in release mode
cargo run --release
```

## Environment Variables

The server loads `crates/server/.env` on startup. Key variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `DATABASE_URL` | PostgreSQL connection string | None |
| `SERVER_HOST` | Server bind IP | `0.0.0.0` |
| `SERVER_PORT` | Server listening port | `23333` |
| `ADMIN_READ_TOKEN` | Dashboard read-only token | Auto-generated if not configured |
| `ADMIN_WRITE_TOKEN` | Dashboard read/write token | Auto-generated if not configured |
| `RUST_LOG` | Log level | `info` |

> **Note**: The tick period is no longer configured via an environment variable. Modify the `tick.real_seconds_per_tick` field in `config/game_rules.yaml` instead.

## Service Access

- **API base URL**: `http://localhost:23333`
- **Admin Dashboard**: `http://localhost:23333/admin/`
  - If tokens are not configured via environment variables, the server auto-generates them on startup and writes them to `crates/server/logs/cyber_jianghu_admin.tmp`.
- **Health check**: `http://localhost:23333/health`
- **WebSocket endpoint**: `ws://localhost:23333/ws?token=YOUR_AUTH_TOKEN`

## Administration and Configuration

1. **Hot-reload config**: Trigger a hot-reload from the Dashboard, or via `POST /api/admin/reload-config`.
2. **Game data config**: Located at `crates/server/config/*.yaml`. You can edit the files directly or use the editor in the Dashboard.
