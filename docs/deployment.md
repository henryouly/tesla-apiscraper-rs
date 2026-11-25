# Production Deployment Guide

This guide covers deploying `tesla-apiscraper-rs` with Docker Compose, InfluxDB 1.8, Mosquitto 2, and a reverse proxy (HTTPS + SSE).

## 1. Prepare `.env`

Copy `.env.example` and replace the encryption key with a freshly generated 32-byte hex key:

```bash
cp .env.example .env
echo "DATA_ENCRYPTION_KEY=$(openssl rand -hex 32)" >> .env
```

Recommended production settings in `.env`:

```dotenv
# Pin a release tag from GHCR
APP_VERSION=v0.4.8

# Keep the app bound to localhost behind a reverse proxy (set 0.0.0.0 only on a trusted LAN)
APP_BIND_ADDR=127.0.0.1

# Required: 64-character hex key (AES-256-GCM for config/tokens.yml).
# Keep this value backed up; changing it invalidates stored encrypted tokens.
DATA_ENCRYPTION_KEY=<output of openssl rand -hex 32>

# Recommended: enable InfluxDB v1 HTTP authentication
INFLUXDB_HTTP_AUTH_ENABLED=true
INFLUXDB_USERNAME=teslamate
INFLUXDB_PASSWORD=<strong-password>
INFLUXDB_DATABASE=tesla

# Optional: set MQTT_HOST="" if you do not use Home Assistant / MQTT
MQTT_HOST=mosquitto
```

> **Note:** `docker-compose.yml` binds InfluxDB (`8086`) and Mosquitto (`1883`) to `127.0.0.1` only so neither service is exposed to the public internet.

## 2. Pull & Start

```bash
docker compose pull
docker compose up -d
```

To build from local source instead of pulling from GHCR:

```bash
docker compose up -d --build
```

Check container health and readiness:

```bash
docker compose ps
curl -sf http://127.0.0.1:4000/health
curl -sf http://127.0.0.1:4000/health/ready
```

Both endpoints should return HTTP `200`.

## 3. Reverse Proxy (HTTPS + SSE)

`tesla-apiscraper-rs` serves plain HTTP on port `4000` with permissive CORS and live Server-Sent Events at `/api/events`. Always place it behind an HTTPS reverse proxy (or private Tailscale/WireGuard network) and ensure SSE responses are not buffered.

### Caddy

```caddyfile
tesla.example.com {
    reverse_proxy 127.0.0.1:4000 {
        flush_interval -1
    }
}
```

### Nginx

```nginx
server {
    listen 443 ssl http2;
    server_name tesla.example.com;

    location / {
        proxy_pass http://127.0.0.1:4000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Connection "";
        proxy_buffering off;
        proxy_read_timeout 3600s;
    }
}
```

## 4. First-Boot Sign-In

1. Open your URL in a browser — a fresh instance redirects automatically to `/signin`.
2. Paste a valid Tesla refresh token and click **Sign in**. The server validates it, stores it encrypted in `/config/tokens.yml`, and discovers linked vehicles.
3. Open **Settings** (`/settings`) to set units (`km`/`mi`, `°C`/`°F`, `bar`/`psi`) and per-car sleep / streaming options.
4. Open **Geofences** (`/geofences`) to add Home/Work locations and charging tariffs.

## 5. Backups

Two persistent volumes hold all state:

- **`influxdb-data`** — time-series telemetry (`positions`, `drives`, `charging_sessions`, `charge_readings`, `states`, `updates`):
  ```bash
  docker exec tesla-apiscraper-influxdb influxd backup -portable -db tesla /tmp/influx-backup
  docker cp tesla-apiscraper-influxdb:/tmp/influx-backup ./backup-influx-$(date +%F)
  docker exec tesla-apiscraper-influxdb rm -rf /tmp/influx-backup
  ```
- **`config-data`** (`/config`) — `settings.yml`, `geofences.yml`, and encrypted `tokens.yml` (always back up alongside `DATA_ENCRYPTION_KEY` from `.env`):
  ```bash
  docker run --rm -v tesla-apiscraper-rs_config-data:/config:ro -v "$PWD":/backup alpine \
    tar czf "/backup/backup-config-$(date +%F).tar.gz" -C /config .
  ```
