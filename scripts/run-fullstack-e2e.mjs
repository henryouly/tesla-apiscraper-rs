#!/usr/bin/env node
/**
 * Full-stack browser E2E runner:
 *   1. Starts a lightweight local mock for Tesla OAuth + Owner API (products/vehicle_data)
 *   2. Seeds a fresh InfluxDB 1.8 test database with a position + closed charging session
 *   3. Starts the real Rust binary serving the built SPA (`web/dist`)
 *   4. Runs Playwright `web/e2e/fullstack.spec.ts` against the live Rust server
 *   5. Tears down the Rust server, mock Tesla server, temp config dir, and InfluxDB test DB
 */

import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import http from 'node:http'
import net from 'node:net'
import { tmpdir } from 'node:os'
import path from 'node:path'

const REPO_ROOT = path.resolve(import.meta.dirname, '..')
const INFLUX_URL = (process.env.INFLUXDB_TEST_URL || process.env.INFLUXDB_URL || 'http://127.0.0.1:8086').replace(/\/$/, '')
const DB_NAME = `e2e_fullstack_${process.pid}_${Date.now()}`
const VIN = '5YJ3E1EA1NF000001'
const BIN_PATH = process.env.RUST_BIN || path.join(REPO_ROOT, 'target', 'debug', 'tesla-apiscraper-rs')

function getFreePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer()
    srv.listen(0, '127.0.0.1', () => {
      const addr = srv.address()
      srv.close(() => resolve(addr.port))
    })
    srv.on('error', reject)
  })
}

function makeFakeJwt(aud) {
  const header = Buffer.from(JSON.stringify({ alg: 'none', typ: 'JWT' })).toString('base64url')
  const payload = Buffer.from(JSON.stringify({ aud, exp: Math.floor(Date.now() / 1000) + 28800 })).toString('base64url')
  return `${header}.${payload}.sig`
}

function startMockTeslaServer(port) {
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, `http://127.0.0.1:${port}`)
    res.setHeader('content-type', 'application/json')

    if (req.method === 'POST' && url.pathname === '/oauth2/v3/token') {
      res.writeHead(200)
      res.end(
        JSON.stringify({
          access_token: makeFakeJwt(`http://127.0.0.1:${port}`),
          refresh_token: 'fullstack-refresh-token-2',
          expires_in: 28800,
          token_type: 'Bearer',
        }),
      )
      return
    }

    if (req.method === 'GET' && url.pathname === '/api/1/products') {
      res.writeHead(200)
      res.end(
        JSON.stringify({
          response: [
            {
              id: 1001,
              vehicle_id: 2002,
              vin: VIN,
              display_name: 'Fullstack Model 3',
              state: 'asleep',
              api_version: 18,
              in_service: false,
            },
          ],
        }),
      )
      return
    }

    if (req.method === 'GET' && url.pathname.includes('/vehicle_data')) {
      res.writeHead(408)
      res.end(JSON.stringify({ error: 'vehicle unavailable: vehicle is asleep' }))
      return
    }

    res.writeHead(404)
    res.end(JSON.stringify({ error: `unexpected mock tesla request: ${req.method} ${url.pathname}` }))
  })

  return new Promise((resolve, reject) => {
    server.listen(port, '127.0.0.1', () => resolve(server))
    server.on('error', reject)
  })
}

async function influxQuery(q) {
  const resp = await fetch(`${INFLUX_URL}/query`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ q }),
  })
  const text = await resp.text()
  if (!resp.ok) throw new Error(`InfluxDB query failed (${resp.status}): ${text}`)
}

async function seedInfluxDb() {
  await influxQuery(`CREATE DATABASE "${DB_NAME}"`)
  const nowSec = Math.floor(Date.now() / 1000)
  const updStartIso = new Date((nowSec - 3600) * 1000).toISOString()
  const updEndIso = new Date((nowSec - 1800) * 1000).toISOString()
  const lp = [
    `positions,vin=${VIN},car_id=1 battery_level=77i,usable_battery_level=75i,ideal_battery_range_km=210.0,odometer=12345.0,latitude=37.7749,longitude=-122.4194 ${nowSec - 300}`,
    `positions,vin=${VIN},car_id=1 battery_level=77i,usable_battery_level=75i,ideal_battery_range_km=210.0,odometer=12345.0,latitude=37.7749,longitude=-122.4194 ${nowSec - 60}`,
    `charging_sessions,vin=${VIN},charge_id=fs-chg-1 energy_added_wh=40000.0,duration_seconds=3600i,cost=10.0,start_battery_level=30i,end_battery_level=77i,geofence_name="Home" ${nowSec - 7200}`,
    `updates,vin=${VIN},update_id=fs-upd-1 version_before="2024.32.6",version_after="2024.38.6",install_start="${updStartIso}",install_end="${updEndIso}",status="completed",abandoned=false ${nowSec - 3600}`,
  ].join('\n')

  const resp = await fetch(`${INFLUX_URL}/write?db=${encodeURIComponent(DB_NAME)}&precision=s`, {
    method: 'POST',
    headers: { 'content-type': 'text/plain; charset=utf-8' },
    body: lp,
  })
  if (!resp.ok) {
    throw new Error(`InfluxDB seed write failed (${resp.status}): ${await resp.text()}`)
  }
}

async function waitForReady(baseUrl, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const resp = await fetch(`${baseUrl}/health/ready`)
      if (resp.ok) return
    } catch {
      // Server still booting
    }
    await new Promise((r) => setTimeout(r, 200))
  }
  throw new Error(`Timed out waiting for ${baseUrl}/health/ready`)
}

function runCommand(cmd, args, opts) {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { stdio: 'inherit', ...opts })
    child.on('exit', (code) => {
      if (code === 0) resolve()
      else reject(new Error(`${cmd} ${args.join(' ')} exited with code ${code}`))
    })
    child.on('error', reject)
  })
}

const configDir = mkdtempSync(path.join(tmpdir(), 'tesla-fullstack-e2e-'))
let mockTesla = null
let rustServer = null

try {
  const mockPort = await getFreePort()
  const appPort = await getFreePort()
  const baseUrl = `http://127.0.0.1:${appPort}`

  mockTesla = await startMockTeslaServer(mockPort)
  await seedInfluxDb()

  rustServer = spawn(BIN_PATH, [], {
    cwd: REPO_ROOT,
    stdio: 'inherit',
    env: {
      ...process.env,
      HOST: '127.0.0.1',
      PORT: String(appPort),
      CONFIG_DIR: configDir,
      INFLUXDB_URL: INFLUX_URL,
      INFLUXDB_DATABASE: DB_NAME,
      TESLA_AUTH_URL: `http://127.0.0.1:${mockPort}`,
      TESLA_API_URL: `http://127.0.0.1:${mockPort}`,
      DATA_ENCRYPTION_KEY: '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
      WEB_DIST_DIR: path.join(REPO_ROOT, 'web', 'dist'),
      POLL_INTERVAL_SECONDS: '3600',
      RUST_LOG: process.env.RUST_LOG || 'warn',
    },
  })

  await waitForReady(baseUrl)
  await runCommand('npm', ['--prefix', 'web', 'run', 'test:e2e'], {
    cwd: REPO_ROOT,
    env: { ...process.env, E2E_BASE_URL: baseUrl },
  })
} finally {
  if (rustServer && rustServer.exitCode === null) {
    rustServer.kill('SIGTERM')
    await new Promise((r) => setTimeout(r, 300))
    if (rustServer.exitCode === null) rustServer.kill('SIGKILL')
  }
  if (mockTesla) {
    mockTesla.closeAllConnections()
    mockTesla.close()
  }
  await influxQuery(`DROP DATABASE "${DB_NAME}"`).catch(() => {})
  rmSync(configDir, { recursive: true, force: true })
}
