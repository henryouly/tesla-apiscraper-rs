#!/usr/bin/env node
/**
 * Verification harness for tesla-apiscraper-rs.
 *
 * Subcommands:
 *   launch [--state <path>] [--evidence-dir <dir>]
 *   doctor [--state <path>]
 *   drive <auth-and-car-index|settings|geofences|charge-cost|dashboards|all> [--state <path>]
 *   cleanup [--state <path>]
 *
 * Internal:
 *   --mock-tesla-server <port> <vin>
 */

import { execFileSync, spawn } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import http from 'node:http'
import { createRequire } from 'node:module'
import net from 'node:net'
import { tmpdir } from 'node:os'
import path from 'node:path'

const REPO_ROOT = path.resolve(import.meta.dirname, '..', '..', '..', '..')
const DEFAULT_STATE_PATH = path.join(tmpdir(), 'verify-tesla-apiscraper-state.json')
const DEFAULT_VIN = '5YJ3E1EA1NF000001'
const BIN_PATH = process.env.RUST_BIN || path.join(REPO_ROOT, 'target', 'debug', 'tesla-apiscraper-rs')
const WEB_DIST = path.join(REPO_ROOT, 'web', 'dist')

function parseFlag(args, flag, fallback = null) {
  const idx = args.indexOf(flag)
  if (idx !== -1 && idx + 1 < args.length) return args[idx + 1]
  return fallback
}

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

async function runMockTeslaServer(port, vin) {
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, `http://127.0.0.1:${port}`)
    res.setHeader('content-type', 'application/json')

    if (req.method === 'GET' && url.pathname === '/_health') {
      res.writeHead(200)
      res.end(JSON.stringify({ status: 'ok' }))
      return
    }

    if (req.method === 'POST' && url.pathname === '/oauth2/v3/token') {
      res.writeHead(200)
      res.end(
        JSON.stringify({
          access_token: makeFakeJwt(`http://127.0.0.1:${port}`),
          refresh_token: 'verify-refresh-token-rotated',
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
              vin,
              display_name: 'Verify Model 3',
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

  await new Promise((resolve, reject) => {
    server.listen(port, '127.0.0.1', resolve)
    server.on('error', reject)
  })
}

async function influxQuery(influxUrl, q, db = null) {
  const params = new URLSearchParams({ q })
  if (db) params.set('db', db)
  const resp = await fetch(`${influxUrl}/query`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: params,
  })
  const text = await resp.text()
  if (!resp.ok) throw new Error(`InfluxDB query failed (${resp.status}): ${text}`)
  return JSON.parse(text)
}

async function waitForHttpOk(url, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const resp = await fetch(url)
      if (resp.ok) return
    } catch {
      // still starting
    }
    await new Promise((r) => setTimeout(r, 250))
  }
  throw new Error(`Timed out waiting for ${url}`)
}

function isPidAlive(pid) {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

function killPid(pid) {
  if (!pid || !isPidAlive(pid)) return
  try {
    process.kill(pid, 'SIGTERM')
  } catch {
    return
  }
  const deadline = Date.now() + 2000
  while (Date.now() < deadline && isPidAlive(pid)) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 100)
  }
  if (isPidAlive(pid)) {
    try {
      process.kill(pid, 'SIGKILL')
    } catch {
      // ignore
    }
  }
}

async function ensureInflux(runId) {
  const envUrl = (process.env.INFLUXDB_TEST_URL || process.env.INFLUXDB_URL || '').replace(/\/$/, '')
  if (envUrl) {
    const resp = await fetch(`${envUrl}/ping`)
    if (!resp.ok) throw new Error(`Configured InfluxDB at ${envUrl} is not healthy`)
    return { influxUrl: envUrl, dockerContainerId: null }
  }

  try {
    const resp = await fetch('http://127.0.0.1:8086/ping')
    if (resp.ok) {
      return { influxUrl: 'http://127.0.0.1:8086', dockerContainerId: null }
    }
  } catch {
    // Fall through to ephemeral Docker container
  }

  const influxPort = await getFreePort()
  const containerName = `verify-tesla-influx-${runId}`
  const containerId = execFileSync(
    'docker',
    ['run', '-d', '--name', containerName, '-p', `127.0.0.1:${influxPort}:8086`, 'influxdb:1.8'],
    { encoding: 'utf8' },
  ).trim()
  const influxUrl = `http://127.0.0.1:${influxPort}`
  try {
    await waitForHttpOk(`${influxUrl}/ping`, 30_000)
  } catch (err) {
    try {
      execFileSync('docker', ['rm', '-f', containerId], { stdio: 'ignore' })
    } catch {
      // ignore
    }
    throw err
  }
  return { influxUrl, dockerContainerId: containerId }
}

async function seedDatabase(influxUrl, dbName, vin) {
  await influxQuery(influxUrl, `CREATE DATABASE "${dbName}"`)
  const nowSec = Math.floor(Date.now() / 1000)
  const lp = [
    `positions,vin=${vin},car_id=1 battery_level=77i,usable_battery_level=75i,ideal_battery_range_km=210.0,rated_battery_range_km=205.0,odometer=12345.0,latitude=37.7749,longitude=-122.4194,outside_temp=18.5,inside_temp=21.0 ${nowSec - 300}`,
    `positions,vin=${vin},car_id=1 battery_level=77i,usable_battery_level=75i,ideal_battery_range_km=210.0,rated_battery_range_km=205.0,odometer=12345.0,latitude=37.7749,longitude=-122.4194,outside_temp=19.0,inside_temp=21.5 ${nowSec - 60}`,
    `charging_sessions,vin=${vin},charge_id=verify-chg-1 energy_added_wh=40000.0,duration_seconds=3600i,cost=10.0,start_battery_level=30i,end_battery_level=77i,geofence_name="Home" ${nowSec - 7200}`,
    `states,vin=${vin} state="asleep",battery_level=77i,locked=true,sentry_mode=false ${nowSec - 60}`,
  ].join('\n')

  const resp = await fetch(`${influxUrl}/write?db=${encodeURIComponent(dbName)}&precision=s`, {
    method: 'POST',
    headers: { 'content-type': 'text/plain; charset=utf-8' },
    body: lp,
  })
  if (!resp.ok) {
    throw new Error(`InfluxDB seed write failed (${resp.status}): ${await resp.text()}`)
  }

  // Verify the seed is immediately queryable
  const check = await influxQuery(influxUrl, 'SELECT COUNT(battery_level) FROM positions', dbName)
  const count = check?.results?.[0]?.series?.[0]?.values?.[0]?.[1] ?? 0
  if (count < 2) {
    throw new Error(`InfluxDB seed verification failed: expected >=2 positions, got ${count}`)
  }
}

async function cmdLaunch(args) {
  const statePath = parseFlag(args, '--state', DEFAULT_STATE_PATH)
  if (existsSync(statePath)) {
    throw new Error(`State file already exists at ${statePath}. Run cleanup first or pass a distinct --state path.`)
  }
  if (!existsSync(BIN_PATH)) {
    throw new Error(`Rust binary not found at ${BIN_PATH}. Run 'cargo build --locked' first.`)
  }
  if (!existsSync(path.join(WEB_DIST, 'index.html'))) {
    throw new Error(`Built SPA not found at ${WEB_DIST}. Run 'npm --prefix web run build' first.`)
  }

  const runId = `${process.pid}_${Date.now()}`
  const evidenceDir = parseFlag(
    args,
    '--evidence-dir',
    path.join(tmpdir(), `verify-tesla-apiscraper-evidence-${runId}`),
  )
  mkdirSync(evidenceDir, { recursive: true })

  const configDir = mkdtempSync(path.join(tmpdir(), `verify-tesla-config-${runId}-`))
  const logDir = mkdtempSync(path.join(tmpdir(), `verify-tesla-logs-${runId}-`))
  const dbName = `verify_tesla_${runId}`
  const vin = DEFAULT_VIN

  let dockerContainerId = null
  let mockPid = null
  let rustPid = null
  let influxUrl = null

  try {
    const influx = await ensureInflux(runId)
    influxUrl = influx.influxUrl
    dockerContainerId = influx.dockerContainerId

    await seedDatabase(influxUrl, dbName, vin)

    const mockPort = await getFreePort()
    const appPort = await getFreePort()
    const baseUrl = `http://127.0.0.1:${appPort}`

    const mockLogFd = openSync(path.join(logDir, 'mock-tesla.log'), 'a')
    const mockChild = spawn(
      process.execPath,
      [import.meta.filename, '--mock-tesla-server', String(mockPort), vin],
      {
        cwd: REPO_ROOT,
        detached: true,
        stdio: ['ignore', mockLogFd, mockLogFd],
      },
    )
    mockChild.unref()
    mockPid = mockChild.pid
    await waitForHttpOk(`http://127.0.0.1:${mockPort}/_health`, 10_000)

    const rustLogFd = openSync(path.join(logDir, 'rust-server.log'), 'a')
    const rustChild = spawn(BIN_PATH, [], {
      cwd: REPO_ROOT,
      detached: true,
      stdio: ['ignore', rustLogFd, rustLogFd],
      env: {
        ...process.env,
        HOST: '127.0.0.1',
        PORT: String(appPort),
        CONFIG_DIR: configDir,
        INFLUXDB_URL: influxUrl,
        INFLUXDB_DATABASE: dbName,
        TESLA_AUTH_URL: `http://127.0.0.1:${mockPort}`,
        TESLA_API_URL: `http://127.0.0.1:${mockPort}`,
        DATA_ENCRYPTION_KEY: '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
        WEB_DIST_DIR: WEB_DIST,
        POLL_INTERVAL_SECONDS: '3600',
        RUST_LOG: process.env.RUST_LOG || 'info',
      },
    })
    rustChild.unref()
    rustPid = rustChild.pid
    await waitForHttpOk(`${baseUrl}/health/ready`, 15_000)

    const state = {
      runId,
      baseUrl,
      appPort,
      mockPort,
      mockPid,
      rustPid,
      influxUrl,
      dbName,
      dockerContainerId,
      configDir,
      logDir,
      evidenceDir,
      vin,
      binPath: BIN_PATH,
    }
    writeFileSync(statePath, JSON.stringify(state, null, 2))
    console.log(JSON.stringify({ status: 'launched', statePath, ...state }, null, 2))
  } catch (err) {
    killPid(rustPid)
    killPid(mockPid)
    if (influxUrl) await influxQuery(influxUrl, `DROP DATABASE "${dbName}"`).catch(() => {})
    if (dockerContainerId) {
      try {
        execFileSync('docker', ['rm', '-f', dockerContainerId], { stdio: 'ignore' })
      } catch {
        // ignore
      }
    }
    rmSync(configDir, { recursive: true, force: true })
    rmSync(logDir, { recursive: true, force: true })
    throw err
  }
}

function loadState(statePath) {
  if (!existsSync(statePath)) {
    throw new Error(`No verification state at ${statePath}. Run 'launch' first.`)
  }
  return JSON.parse(readFileSync(statePath, 'utf8'))
}

async function cmdDoctor(args) {
  const statePath = parseFlag(args, '--state', DEFAULT_STATE_PATH)
  const state = loadState(statePath)

  const checks = {
    rustPidAlive: isPidAlive(state.rustPid),
    mockPidAlive: isPidAlive(state.mockPid),
    isolatedConfigDir: state.configDir.startsWith(tmpdir()) && existsSync(state.configDir),
    isolatedDbName: state.dbName.startsWith('verify_tesla_'),
    healthOk: false,
    readyOk: false,
    seededPositions: 0,
  }

  try {
    const h = await fetch(`${state.baseUrl}/health`)
    checks.healthOk = h.ok
  } catch {
    checks.healthOk = false
  }

  try {
    const r = await fetch(`${state.baseUrl}/health/ready`)
    checks.readyOk = r.ok
  } catch {
    checks.readyOk = false
  }

  try {
    const q = await influxQuery(state.influxUrl, 'SELECT COUNT(battery_level) FROM positions', state.dbName)
    checks.seededPositions = q?.results?.[0]?.series?.[0]?.values?.[0]?.[1] ?? 0
  } catch {
    checks.seededPositions = 0
  }

  const healthy =
    checks.rustPidAlive &&
    checks.mockPidAlive &&
    checks.isolatedConfigDir &&
    checks.isolatedDbName &&
    checks.healthOk &&
    checks.readyOk &&
    checks.seededPositions >= 2

  console.log(JSON.stringify({ healthy, baseUrl: state.baseUrl, dbName: state.dbName, configDir: state.configDir, evidenceDir: state.evidenceDir, checks }, null, 2))
  if (!healthy) process.exit(1)
}

async function openBrowser(state) {
  const req = createRequire(path.join(REPO_ROOT, 'web', 'package.json'))
  const { chromium } = req('@playwright/test')
  const browser = await chromium.launch({ headless: true })
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } })
  const page = await context.newPage()

  const consoleErrors = []
  const pageErrors = []
  page.on('console', (msg) => {
    if (msg.type() === 'error') {
      const text = msg.text()
      if (!/status of 401/.test(text)) consoleErrors.push(text)
    }
  })
  page.on('pageerror', (err) => pageErrors.push(String(err)))

  const transparentPng = Buffer.from(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=',
    'base64',
  )
  await page.route('https://*.tile.openstreetmap.org/**', (route) =>
    route.fulfill({ status: 200, contentType: 'image/png', body: transparentPng }),
  )
  await page.route('https://tile.openstreetmap.org/**', (route) =>
    route.fulfill({ status: 200, contentType: 'image/png', body: transparentPng }),
  )

  return { browser, page, consoleErrors, pageErrors }
}

async function ensureSignedIn(page, state) {
  await page.goto(state.baseUrl)
  await page.waitForLoadState('networkidle')
  if (page.url().endsWith('/signin')) {
    await page.getByPlaceholder('Paste refresh token').fill('verify-refresh-token')
    await page.getByRole('button', { name: 'Sign in' }).click()
    await page.waitForURL((u) => u.pathname === '/', { timeout: 10_000 })
  }
}

async function saveProof(page, state, featureId, details) {
  const dir = path.join(state.evidenceDir, featureId)
  mkdirSync(dir, { recursive: true })
  const screenshotPath = path.join(dir, 'proof.png')
  const ariaPath = path.join(dir, 'proof.aria.txt')
  const jsonPath = path.join(dir, 'proof.json')

  await page.screenshot({ path: screenshotPath, fullPage: true })
  const ariaSnapshot = await page.locator('body').ariaSnapshot()
  writeFileSync(ariaPath, ariaSnapshot)
  const record = {
    featureId,
    timestamp: new Date().toISOString(),
    url: page.url(),
    screenshotPath,
    ariaPath,
    ...details,
  }
  writeFileSync(jsonPath, JSON.stringify(record, null, 2))
  return record
}

async function driveAuthAndCarIndex(page, state) {
  await page.goto(state.baseUrl)
  await page.waitForURL(/\/signin$/, { timeout: 10_000 })
  await page.getByRole('heading', { name: 'Sign in' }).waitFor({ state: 'visible' })
  await page.getByPlaceholder('Paste refresh token').fill('verify-refresh-token')
  await page.getByRole('button', { name: 'Sign in' }).click()
  await page.waitForURL((u) => u.pathname === '/', { timeout: 10_000 })
  await page.getByRole('heading', { name: 'Verify Model 3' }).waitFor({ state: 'visible' })
  await page.getByText('77%').waitFor({ state: 'visible' })

  const tokensPath = path.join(state.configDir, 'tokens.yml')
  const tokensPersisted = existsSync(tokensPath)
  if (!tokensPersisted) throw new Error(`Expected encrypted tokens file at ${tokensPath}`)

  return saveProof(page, state, 'auth-and-car-index', {
    entryPoint: 'GET / -> redirect /signin -> Sign in form -> /',
    sideEffects: { tokensFileExists: tokensPersisted },
  })
}

async function driveSettings(page, state) {
  await ensureSignedIn(page, state)
  await page.getByRole('link', { name: 'Settings' }).first().click()
  await page.getByRole('heading', { name: 'Settings', exact: true }).waitFor({ state: 'visible' })
  await page.getByLabel('Unit of length').selectOption('km')
  await page.getByRole('button', { name: 'Save settings' }).click()
  await page.getByText('Saved.').waitFor({ state: 'visible' })

  await page.reload()
  const persistedValue = await page.getByLabel('Unit of length').inputValue()
  if (persistedValue !== 'km') throw new Error(`Expected unit_length 'km' after reload, got '${persistedValue}'`)

  const settingsPath = path.join(state.configDir, 'settings.yml')
  const yamlText = existsSync(settingsPath) ? readFileSync(settingsPath, 'utf8') : ''
  if (!yamlText.includes('unit_length: km')) {
    throw new Error(`Expected settings.yml to contain 'unit_length: km', got:\n${yamlText}`)
  }

  return saveProof(page, state, 'settings', {
    entryPoint: 'Nav link Settings -> /settings',
    sideEffects: { settingsYamlContainsKm: true },
  })
}

async function driveGeofences(page, state) {
  await ensureSignedIn(page, state)
  await page.getByRole('link', { name: 'Geofences' }).first().click()
  await page.getByRole('heading', { name: 'Geofences', exact: true }).waitFor({ state: 'visible' })
  await page.getByLabel('Name').fill('Verify Home')
  await page.getByLabel('Radius (m)').fill('150')
  await page.getByRole('button', { name: 'Create' }).click()
  await page.getByRole('heading', { name: 'Verify Home', exact: true }).waitFor({ state: 'visible' })

  await page.reload()
  await page.getByRole('heading', { name: 'Verify Home', exact: true }).waitFor({ state: 'visible' })

  const geofencesPath = path.join(state.configDir, 'geofences.yml')
  const yamlText = existsSync(geofencesPath) ? readFileSync(geofencesPath, 'utf8') : ''
  if (!yamlText.includes('Verify Home')) {
    throw new Error(`Expected geofences.yml to contain 'Verify Home', got:\n${yamlText}`)
  }

  const proof = await saveProof(page, state, 'geofences', {
    entryPoint: 'Nav link Geofences -> /geofences',
    sideEffects: { geofencesYamlContainsVerifyHome: true },
  })

  // Restore clean geofence state via UI two-step delete
  await page.getByRole('button', { name: 'Delete' }).first().click()
  await page.getByRole('button', { name: 'Confirm delete?' }).first().click()
  await page.getByRole('heading', { name: 'Verify Home', exact: true }).waitFor({ state: 'detached' })

  return proof
}

async function driveChargeCost(page, state) {
  await ensureSignedIn(page, state)
  await page.goto(`${state.baseUrl}/charge/verify-chg-1/cost`)
  await page.getByText('40.0 kWh').waitFor({ state: 'visible' })
  await page.getByLabel('Cost per kWh').fill('0.30')
  await page.getByLabel('Session fee').fill('1.50')
  await page.getByText('Preview: 13.50').waitFor({ state: 'visible' })
  await page.getByRole('button', { name: 'Save cost' }).click()
  await page.getByText('Saved cost 13.50').waitFor({ state: 'visible' })

  await page.reload()
  await page.getByText('13.5', { exact: true }).waitFor({ state: 'visible' })

  const q = await influxQuery(
    state.influxUrl,
    `SELECT cost FROM charging_sessions WHERE charge_id = 'verify-chg-1'`,
    state.dbName,
  )
  const dbCost = q?.results?.[0]?.series?.[0]?.values?.[0]?.[1]
  if (dbCost !== 13.5) throw new Error(`Expected InfluxDB cost 13.5, got ${dbCost}`)

  return saveProof(page, state, 'charge-cost', {
    entryPoint: 'GET /charge/verify-chg-1/cost',
    sideEffects: { influxDbCost: dbCost },
  })
}

async function driveDashboards(page, state) {
  await ensureSignedIn(page, state)
  await page.goto(`${state.baseUrl}/dashboards/overview`)
  await page.getByRole('heading', { name: 'Overview', level: 1, exact: true }).waitFor({ state: 'visible' })
  await page.locator('.uplot').first().waitFor({ state: 'visible', timeout: 10_000 })

  return saveProof(page, state, 'dashboards', {
    entryPoint: 'GET /dashboards/overview',
    sideEffects: { uplotRendered: true },
  })
}

const DRIVERS = {
  'auth-and-car-index': driveAuthAndCarIndex,
  settings: driveSettings,
  geofences: driveGeofences,
  'charge-cost': driveChargeCost,
  dashboards: driveDashboards,
}

async function cmdDrive(args) {
  const feature = args[0]
  if (!feature || (feature !== 'all' && !DRIVERS[feature])) {
    throw new Error(`Usage: drive <${Object.keys(DRIVERS).join('|')}|all> [--state <path>]`)
  }
  const statePath = parseFlag(args, '--state', DEFAULT_STATE_PATH)
  const state = loadState(statePath)

  const { browser, page, consoleErrors, pageErrors } = await openBrowser(state)
  const results = []
  try {
    const list = feature === 'all' ? Object.keys(DRIVERS) : [feature]
    for (const f of list) {
      results.push(await DRIVERS[f](page, state))
    }
    if (consoleErrors.length > 0 || pageErrors.length > 0) {
      throw new Error(`Browser errors detected: console=${JSON.stringify(consoleErrors)} page=${JSON.stringify(pageErrors)}`)
    }
    console.log(JSON.stringify({ status: 'passed', results }, null, 2))
  } finally {
    await browser.close()
  }
}

async function cmdCleanup(args) {
  const statePath = parseFlag(args, '--state', DEFAULT_STATE_PATH)
  if (!existsSync(statePath)) {
    console.log(JSON.stringify({ status: 'already-clean', statePath }, null, 2))
    return
  }
  const state = loadState(statePath)

  killPid(state.rustPid)
  killPid(state.mockPid)

  if (state.influxUrl && state.dbName) {
    await influxQuery(state.influxUrl, `DROP DATABASE "${state.dbName}"`).catch(() => {})
  }
  if (state.dockerContainerId) {
    try {
      execFileSync('docker', ['rm', '-f', state.dockerContainerId], { stdio: 'ignore' })
    } catch {
      // ignore
    }
  }
  if (state.configDir) rmSync(state.configDir, { recursive: true, force: true })
  if (state.logDir) rmSync(state.logDir, { recursive: true, force: true })
  rmSync(statePath, { force: true })

  // Never delete state.evidenceDir!
  const evidencePreserved = existsSync(state.evidenceDir)
  console.log(
    JSON.stringify(
      {
        status: 'cleaned',
        evidenceDir: state.evidenceDir,
        evidencePreserved,
      },
      null,
      2,
    ),
  )
}

const [, , cmd, ...rest] = process.argv
try {
  if (cmd === '--mock-tesla-server') {
    await runMockTeslaServer(Number(rest[0]), rest[1] || DEFAULT_VIN)
  } else if (cmd === 'launch') {
    await cmdLaunch(rest)
  } else if (cmd === 'doctor') {
    await cmdDoctor(rest)
  } else if (cmd === 'drive') {
    await cmdDrive(rest)
  } else if (cmd === 'cleanup') {
    await cmdCleanup(rest)
  } else {
    console.error('Usage: verify-app.mjs <launch|doctor|drive|cleanup> [options]')
    process.exit(2)
  }
} catch (err) {
  console.error(err instanceof Error ? err.stack || err.message : String(err))
  process.exit(1)
}
