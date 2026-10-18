/**
 * Backfill `drives.energy_used_wh` for records written before the kW→Wh fix.
 *
 * Run with tsx:
 *   npx tsx scripts/backfill-drive-energy.ts --before 2026-10-18T00:00:00Z
 *
 * ── Why ────────────────────────────────────────────────────────────────────
 * `drive_state.power` is kilowatts. Until the fix (d731e44, PR #112) the drive
 * accumulator integrated it as watts, so `energy_used_wh` stored kWh. Every
 * read path divides by 1000 again, which is why historical drives render as
 * "0.0 kWh". This multiplies the stored value by 1000 so those rows hold real
 * watt-hours and the existing `/1000` reads become correct.
 *
 * ── Scope ───────────────────────────────────────────────────────────────────
 * ONLY `drives`. `charging_sessions.charge_energy_used` was already correct
 * (the charge path multiplies watts by 1000) and must not be touched.
 *
 * ── Safety ──────────────────────────────────────────────────────────────────
 *  1. Dry run by default. Nothing is written without `--apply`.
 *  2. `--before` is REQUIRED. Data written by the fixed code is already
 *     correct, so there is no safe default cutoff — scaling it would corrupt
 *     it. See "Choosing --before" below.
 *  3. Every point is copied to `drives_energy_pre93_backup` before it is
 *     overwritten. To undo, re-read that measurement.
 *  4. A completed run is recorded in `backfill_runs`; re-running the same
 *     range+factor is refused unless `--force`, because a second pass would
 *     double-scale.
 *  5. Full read-modify-write: every field read is re-emitted, so no field is
 *     lost and float/integer typing is preserved (an integral float rewritten
 *     with an `i` suffix is a field-type conflict in InfluxDB).
 *
 * ── Choosing --before ───────────────────────────────────────────────────────
 * A drive point is timestamped with the drive's START time but its energy is
 * written when the drive CLOSES. A drive that started before your cutoff but
 * closed after the deploy would have correct energy (new code) and would be
 * scaled a second time. Avoid that by setting --before comfortably BEFORE the
 * deploy time (e.g. deploy − 24h); the cost is leaving a few already-old
 * drives unscaled. The run reports how many points fall in the edge zone.
 *
 * ── Usage ───────────────────────────────────────────────────────────────────
 *   npx tsx scripts/backfill-drive-energy.ts --before 2026-10-18T00:00:00Z
 *   npx tsx scripts/backfill-drive-energy.ts --before 2026-10-18T00:00:00Z --apply
 *
 * Reads INFLUXDB_URL / INFLUXDB_USERNAME / INFLUXDB_PASSWORD /
 * INFLUXDB_DATABASE from the environment, falling back to ./.env.
 */

import { createInterface } from "node:readline/promises";

// ── Config ───────────────────────────────────────────────────────────────────

const MEASUREMENT = "drives";
const BACKUP_MEASUREMENT = "drives_energy_pre93_backup";
const RUN_LOG_MEASUREMENT = "backfill_runs";

const DEFAULT_FACTOR = 1000;
const DEFAULT_CHUNK_DAYS = 30;
const EDGE_ZONE_HOURS = 6; // report points this close to the cutoff
const WRITE_BATCH = 500;
const DEFAULT_FROM = "1970-01-01T00:00:00Z";

/**
 * Field types of `drives`, from the `Drive` struct in src/influxdb.rs.
 * InfluxDB v1 JSON drops the `.0` from whole floats, so a value read back as
 * `11000` may be a float or an int. These tables pin the wire type: writing a
 * stored float with an `i` suffix is a field-type conflict.
 */
const FLOAT_FIELDS = new Set([
  "start_lat",
  "start_lng",
  "end_lat",
  "end_lng",
  "distance_meters",
  "energy_used_wh",
  "max_speed",
  "average_speed",
  "outside_temp_avg",
  "inside_temp_avg",
]);

const INT_FIELDS = new Set(["duration_seconds"]);
const TAG_FIELDS = new Set(["vin", "drive_id"]);

// ── Argument parsing ─────────────────────────────────────────────────────────

interface Options {
  before: string; // RFC3339, required
  from: string; // RFC3339, defaults to the earliest drive point
  fromGiven: boolean;
  apply: boolean;
  force: boolean;
  yes: boolean;
  factor: number;
  chunkDays: number;
  vin?: string;
  timeoutMs: number;
}

function parseArgs(argv: string[]): Options {
  const o = {
    before: "",
    from: DEFAULT_FROM,
    fromGiven: false,
    apply: false,
    force: false,
    yes: false,
    factor: DEFAULT_FACTOR,
    chunkDays: DEFAULT_CHUNK_DAYS,
    timeoutMs: 300_000,
  } as Options;
  let vin: string | undefined;

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v === undefined) fail(`missing value for ${arg}`);
      return v;
    };
    switch (arg) {
      case "--before": o.before = next(); break;
      case "--from": o.from = next(); o.fromGiven = true; break;
      case "--vin": vin = next(); break;
      case "--apply": o.apply = true; break;
      case "--force": o.force = true; break;
      case "--yes": o.yes = true; break;
      case "--help": case "-h": usage(0);
      case "--factor": o.factor = Number(next()); break;
      case "--chunk-days": o.chunkDays = Number(next()); break;
      case "--timeout-ms": o.timeoutMs = Number(next()); break;
      default: fail(`unknown argument: ${arg}`);
    }
  }

  o.vin = vin;
  if (!o.before) usage(1, "--before is required (see 'Choosing --before' in the header)");
  if (!Number.isFinite(o.factor) || o.factor === 0) fail("--factor must be a non-zero number");
  if (!Number.isFinite(o.chunkDays) || o.chunkDays <= 0) fail("--chunk-days must be > 0");
  if (!Number.isFinite(o.timeoutMs) || o.timeoutMs <= 0) fail("--timeout-ms must be > 0");

  // Validate timestamps up front so a typo fails before any I/O.
  for (const [name, val] of [["--before", o.before], ["--from", o.from]] as const) {
    if (Number.isNaN(Date.parse(val))) fail(`${name} is not a valid RFC3339 timestamp: ${val}`);
  }
  return o;
}

function usage(code: number, message?: string): never {
  if (message) console.error(`error: ${message}\n`);
  console.error(`usage: npx tsx scripts/backfill-drive-energy.ts --before <RFC3339> [options]

  --before <RFC3339>  required; only points strictly before this are touched
  --from <RFC3339>    lower bound (default: the earliest point in the measurement)
  --vin <VIN>         restrict to one vehicle
  --factor <N>        multiply by N (default: ${DEFAULT_FACTOR})
  --chunk-days <N>    time window per query (default: ${DEFAULT_CHUNK_DAYS})
  --apply             actually write (default: dry run)
  --force             allow re-running a range already recorded in ${RUN_LOG_MEASUREMENT}
  --yes               skip the interactive confirmation on --apply
  --timeout-ms <N>    overall per-pass wall-clock budget (default: 300000)
  -h, --help`);
  process.exit(code);
}

function fail(msg: string): never {
  console.error(`error: ${msg}`);
  process.exit(1);
}

// ── InfluxDB v1 client ───────────────────────────────────────────────────────

interface Point {
  timeNs: number;
  tags: [string, string][];
  fields: [string, unknown][];
}

class Influx {
  readonly baseUrl: string;
  readonly database: string;
  #auth?: string;

  constructor() {
    // Same variables the Rust app reads; fall back to ./.env.
    try {
      process.loadEnvFile(".env");
    } catch {
      /* no .env, or unreadable — rely on the ambient environment */
    }

    const url = process.env.INFLUXDB_URL;
    if (!url) fail("INFLUXDB_URL is not set (and not found in ./.env)");
    this.baseUrl = url.replace(/\/+$/, "");
    this.database = process.env.INFLUXDB_DATABASE || "tesla";

    const user = process.env.INFLUXDB_USERNAME || "";
    const pass = process.env.INFLUXDB_PASSWORD || "";
    if (user || pass) {
      this.#auth = "Basic " + Buffer.from(`${user}:${pass}`).toString("base64");
    }
  }

  async #req(path: string, init: RequestInit = {}): Promise<Response> {
    const headers: Record<string, string> = { ...(init.headers as Record<string, string>) };
    if (this.#auth) headers["authorization"] = this.#auth;
    return fetch(`${this.baseUrl}${path}`, { ...init, headers });
  }

  async ping(): Promise<void> {
    const res = await this.#req("/ping").catch((e) => {
      fail(`cannot reach InfluxDB at ${this.baseUrl}: ${e instanceof Error ? e.message : e}`);
    });
    if (!res.ok) fail(`InfluxDB ping failed (HTTP ${res.status}) — is ${this.baseUrl} reachable?`);
  }

  async query(q: string, epoch = "ns"): Promise<any> {
    const url = new URL(`${this.baseUrl}/query`);
    url.searchParams.set("db", this.database);
    // InfluxDB v1 defaults to nanoseconds in JSON responses; always be explicit.
    url.searchParams.set("epoch", epoch);
    url.searchParams.set("q", q);
    const res = await this.#req(url.pathname + url.search).catch((e) => {
      fail(`query request failed: ${e instanceof Error ? e.message : e}`);
    });
    if (!res.ok) fail(`query failed (HTTP ${res.status}): ${(await res.text()).slice(0, 400)}`);
    const json = await res.json();
    // InfluxDB v1 reports statement failures as HTTP 200 with a result error.
    const err = json?.results?.[0]?.error;
    if (err) fail(`InfluxDB query error: ${err}`);
    return json;
  }

  async write(lines: string[]): Promise<void> {
    if (lines.length === 0) return;
    const url = new URL(`${this.baseUrl}/write`);
    url.searchParams.set("db", this.database);
    url.searchParams.set("precision", "ns");
    const res = await this.#req(url.pathname + url.search, {
      method: "POST",
      headers: { "content-type": "text/plain; charset=utf-8" },
      body: lines.join("\n"),
    });
    if (!res.ok) {
      fail(`write failed (HTTP ${res.status}): ${(await res.text()).slice(0, 400)}`);
    }
  }
}

// ── Line protocol ────────────────────────────────────────────────────────────

function escapeTag(s: string): string {
  // Line protocol has no escape for newlines inside a tag — one splits the
  // body into extra points. Strip them, as the Rust path does.
  return s.replace(/[\n\r]/g, " ").replace(/\\/g, "\\\\").replace(/ /g, "\\ ")
    .replace(/,/g, "\\,").replace(/=/g, "\\=");
}

function escapeString(s: string): string {
  return `"${s.replace(/[\n\r]/g, " ").replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/** Plain-decimal float. InfluxQL rejects exponent notation. */
function fmtFloat(n: number): string {
  const s = String(n);
  if (/e/i.test(s)) {
    fail(`value ${n} would serialise in exponent notation, which InfluxDB line ` +
      `protocol may reject; refusing to write`);
  }
  return s;
}

function fieldValue(name: string, v: unknown): string {
  if (FLOAT_FIELDS.has(name)) {
    const n = typeof v === "number" ? v : Number(v);
    if (!Number.isFinite(n)) fail(`field ${name} is not a finite number: ${v}`);
    return fmtFloat(n);
  }
  if (INT_FIELDS.has(name)) {
    const n = typeof v === "number" ? v : Number(v);
    if (!Number.isInteger(n)) fail(`field ${name} is not an integer: ${v}`);
    return `${n}i`;
  }
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "string") return escapeString(v);
  if (typeof v === "number") return String(v); // unknown column, arrival shape
  fail(`field ${name} has unsupported type ${typeof v}`);
}

/**
 * One line-protocol line. Fields are joined with a bare comma: InfluxDB 1.x
 * rejects a space after the comma between fields.
 */
function toLine(measurement: string, p: Point, energy?: number): string {
  const tags = p.tags.map(([k, v]) => `${k}=${escapeTag(v)}`).join(",");
  const fields = p.fields
    .map(([k, v]) => (k === "energy_used_wh" ? `${k}=${fmtFloat(energy!)}` : `${k}=${fieldValue(k, v)}`))
    .join(",");
  if (!fields) fail("refusing to write a point with no fields");
  return `${measurement},${tags} ${fields} ${p.timeNs}`;
}

// ── Reading ──────────────────────────────────────────────────────────────────

/**
 * Read a window of `drives`. NO `GROUP BY` — a grouped query moves tag
 * columns into a separate `tags` object, and the rewrite needs `vin` and
 * `drive_id` as columns to hit the same series.
 */
async function readWindow(db: Influx, fromIso: string, toIso: string, vin?: string): Promise<Point[]> {
  // No `IS NOT NULL` filter: InfluxQL 1.x rejects it in WHERE. Points without
  // an energy field (still-open drives) are filtered out by the caller.
  const clauses = [`time >= '${fromIso}'`, `time < '${toIso}'`];
  if (vin) clauses.push(`"vin" = '${escapeTag(vin)}'`);
  const q = `SELECT * FROM "${MEASUREMENT}" WHERE ${clauses.join(" AND ")}`;
  const json = await db.query(q);

  const series = json?.results?.[0]?.series ?? [];
  const points: Point[] = [];
  for (const s of series) {
    const columns: string[] = s.columns ?? [];
    const timeIdx = columns.indexOf("time");
    if (timeIdx < 0) fail("series has no time column — unexpected response shape");
    for (const row of s.values ?? []) {
      // Series are rectangular; a ragged row is corrupt. Skipping a missing
      // cell would silently drop that field from the rewrite.
      if (row.length !== columns.length) {
        fail(`ragged row from InfluxDB (${row.length} cells, ${columns.length} columns) — ` +
          `refusing to rewrite a point whose fields may be lost`);
      }
      const tags: [string, string][] = [];
      const fields: [string, unknown][] = [];
      for (let i = 0; i < columns.length; i++) {
        const name = columns[i];
        const val = row[i];
        if (name === "time") continue;
        if (val === null || val === undefined) continue;
        if (TAG_FIELDS.has(name)) {
          if (typeof val !== "string") fail(`tag ${name} is not a string: ${val}`);
          tags.push([name, val]);
        } else {
          fields.push([name, val]);
        }
      }
      const has = (k: string) => tags.some(([key]) => key === k);
      if (!(has("vin") && has("drive_id"))) {
        fail("point is missing vin/drive_id — refusing to rewrite without both tags");
      }
      points.push({ timeNs: Number(row[timeIdx]), tags, fields });
    }
  }
  return points;
}

/** Earliest point in the measurement, so a default run doesn't sweep from the epoch. */
async function earliestPointTime(db: Influx, vin?: string): Promise<number | null> {
  const filter = vin ? ` WHERE "vin" = '${escapeTag(vin)}'` : "";
  // Milliseconds here — the caller feeds this straight into `new Date()`.
  const json = await db
    .query(`SELECT * FROM "${MEASUREMENT}"${filter} ORDER BY time ASC LIMIT 1`, "ms")
    .catch(() => null); // empty measurement / missing field → treat as "no data"
  return json?.results?.[0]?.series?.[0]?.values?.[0]?.[0] ?? null;
}

// ── Run log (idempotency guard) ──────────────────────────────────────────────

async function findPriorRun(db: Influx, key: string): Promise<boolean> {
  const q = `SELECT * FROM "${RUN_LOG_MEASUREMENT}" WHERE "run_key" = '${escapeTag(key)}' LIMIT 1`;
  const json = await db.query(q).catch(() => null); // measurement may not exist yet
  return (json?.results?.[0]?.series?.length ?? 0) > 0;
}

async function recordRun(db: Influx, key: string, o: Options, count: number, beforeIso: string): Promise<void> {
  const line = `${RUN_LOG_MEASUREMENT},run=backfill_drive_energy ` +
    [
      `run_key=${escapeString(key)}`,
      `from=${escapeString(o.from)}`,
      `before=${escapeString(beforeIso)}`,
      `factor=${o.factor}i`,
      `points=${count}i`,
      `applied=${o.apply}`,
    ].join(",") +
    ` ${Date.now() * 1e6}`;
  await db.write([line]);
}

// ── Planning / applying ──────────────────────────────────────────────────────

/**
 * Walk the time range in windows, handing each window's points to `visit`.
 * The range is bounded by `--before` in the past, so no new data can enter
 * between two passes — a plan pass and an apply pass see the same points.
 */
async function forEachWindow(
  db: Influx,
  o: Options,
  beforeMs: number,
  visit: (points: Point[], startMs: number, endMs: number) => Promise<void>,
): Promise<void> {
  const chunkMs = o.chunkDays * 86_400_000;
  const deadline = Date.now() + o.timeoutMs;
  for (let start = Date.parse(o.from); start < beforeMs; start += chunkMs) {
    if (Date.now() > deadline) {
      fail(`exceeded --timeout-ms; rerun to resume from where this stopped`);
    }
    const end = Math.min(start + chunkMs, beforeMs);
    const window = await readWindow(
      db,
      new Date(start).toISOString(),
      new Date(end).toISOString(),
      o.vin,
    );
    if (window.length > 0) await visit(window, start, end);
  }
}

/** Split a window into points to back up, points to rewrite, and stats. */
function planWindow(window: Point[], factor: number, beforeNs: number, edgeNs: number) {
  const backup: [Point, number][] = [];
  const rewrite: [Point, number][] = [];
  let skipped = 0;
  let scored = 0;
  let energyBefore = 0;
  let energyAfter = 0;
  let edgeZone = 0;
  for (const p of window) {
    const raw = p.fields.find(([k]) => k === "energy_used_wh")?.[1];
    // An open drive has no energy field yet (it is written on close).
    if (raw === undefined) {
      skipped++;
      continue;
    }
    const current = typeof raw === "number" ? raw : Number(raw);
    if (!Number.isFinite(current)) {
      fail(`energy_used_wh is not numeric on a point in range: ${raw}`);
    }
    const scaled = current * factor;
    energyBefore += current;
    energyAfter += scaled;
    scored++;
    if (beforeNs - p.timeNs < edgeNs) edgeZone++;
    if (current === scaled) continue; // factor of 1 — nothing to do
    backup.push([p, current]);
    rewrite.push([p, scaled]);
  }
  return { backup, rewrite, skipped, scored, energyBefore, energyAfter, edgeZone };
}

// ── Main ─────────────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const o = parseArgs(process.argv.slice(2));
  const db = new Influx();
  await db.ping();

  if (!o.fromGiven) {
    const earliest = await earliestPointTime(db, o.vin);
    if (earliest === null) {
      console.log(`no points found in "${MEASUREMENT}"${o.vin ? ` for vin ${o.vin}` : ""} — nothing to do`);
      return;
    }
    o.from = new Date(Number(earliest)).toISOString();
    console.log(`auto --from   ${o.from} (earliest point; pass --from to override)`);
  }

  const beforeMs = Date.parse(o.before);
  const beforeIso = new Date(beforeMs).toISOString();
  const fromIso = new Date(Date.parse(o.from)).toISOString();
  if (Date.parse(o.from) >= beforeMs) {
    if (o.fromGiven) {
      fail(`--from (${fromIso}) must be earlier than --before (${beforeIso})`);
    }
    fail(`earliest point ${fromIso} is not before --before ${beforeIso} — ` +
      `everything in range was written by the fixed code and is already correct`);
  }

  console.log(`database ${db.database} @ ${db.baseUrl}`);
  console.log(`window  ${fromIso} .. ${beforeIso} (exclusive)`);
  console.log(`factor  ${o.factor}`);
  console.log(`mode    ${o.apply ? "APPLY (writes)" : "DRY RUN (no writes)"}`);
  if (o.vin) console.log(`vin     ${o.vin}`);

  const runKey = `${fromIso}|${beforeIso}|${o.factor}`;
  if (o.apply && !o.force && (await findPriorRun(db, runKey))) {
    fail(`a run for exactly this range+factor is already recorded in ${RUN_LOG_MEASUREMENT}. ` +
      `Re-running would double-scale. Use --force only if you are certain.`);
  }

  // Point times come back in nanoseconds; the cutoff edge zone is compared
  // against the same unit.
  const beforeNs = beforeMs * 1_000_000;
  const edgeNs = EDGE_ZONE_HOURS * 3_600_000_000_000;

  // ── Pass 1: plan (read-only) ──
  let points = 0;
  let skipped = 0;
  let energyBefore = 0;
  let energyAfter = 0;
  let edgeZone = 0;
  await forEachWindow(db, o, beforeMs, async (window) => {
    const plan = planWindow(window, o.factor, beforeNs, edgeNs);
    points += plan.scored;
    skipped += plan.skipped;
    energyBefore += plan.energyBefore;
    energyAfter += plan.energyAfter;
    edgeZone += plan.edgeZone;
  });

  console.log("");
  console.log(`points scanned : ${points}`);
  console.log(`points skipped : ${skipped} (no energy field — still-open drives)`);
  console.log(`energy before  : ${energyBefore.toFixed(2)} (kWh-scale, 1000x low)`);
  console.log(`energy after   : ${energyAfter.toFixed(2)} (Wh, correct)`);
  console.log(`in edge zone   : ${edgeZone} within ${EDGE_ZONE_HOURS}h of --before`);
  if (edgeZone > 0) {
    console.log(`  ^ these started close to the cutoff and may have been written by the FIXED`);
    console.log(`    code. Inspect them; re-scaling correct data would double it.`);
  }

  if (!o.apply) {
    console.log("");
    console.log("DRY RUN — nothing was written. Re-run with --apply to make these changes.");
    return;
  }

  // Confirm only now that the plan is known, and still BEFORE any write.
  if (points > 0 && !o.yes && !(await confirm())) {
    console.log("aborted — nothing was written");
    return;
  }

  // ── Pass 2: apply ──
  // The range ends at a past `--before`, so no new points can have appeared
  // since the plan pass; the two passes see the same rows.
  let applied = 0;
  await forEachWindow(db, o, beforeMs, async (window, start, end) => {
    const plan = planWindow(window, o.factor, beforeNs, edgeNs);
    if (plan.rewrite.length === 0) return;
    const backupLines = plan.backup.map(([p, v]) => toLine(BACKUP_MEASUREMENT, p, v));
    const rewriteLines = plan.rewrite.map(([p, v]) => toLine(MEASUREMENT, p, v));
    // Backup first: if the rewrite half-succeeds, the original values are
    // still recoverable from BACKUP_MEASUREMENT.
    for (let i = 0; i < backupLines.length; i += WRITE_BATCH) {
      await db.write(backupLines.slice(i, i + WRITE_BATCH));
    }
    for (let i = 0; i < rewriteLines.length; i += WRITE_BATCH) {
      await db.write(rewriteLines.slice(i, i + WRITE_BATCH));
    }
    applied += plan.rewrite.length;
    console.log(
      `  ${new Date(start).toISOString().slice(0, 10)}..${new Date(end).toISOString().slice(0, 10)}  ` +
      `${plan.rewrite.length} points  (backed up + scaled)`,
    );
  });

  await recordRun(db, runKey, o, applied, beforeIso);
  console.log("");
  console.log(`DONE. ${applied} points scaled in "${MEASUREMENT}".`);
  console.log(`Originals preserved in "${BACKUP_MEASUREMENT}". To undo, rewrite from that measurement.`);
}

async function confirm(): Promise<boolean> {
  if (!process.stdin.isTTY) {
    fail("refusing to write without confirmation (stdin is not a TTY); pass --yes");
  }
  // node:readline handles the TTY line editing; a hand-rolled async iterator
  // over stdin can see EOF before the keystroke arrives.
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    const answer = await rl.question("\nApply these changes? [y/N] ");
    return /^y(es)?$/i.test(answer.trim());
  } finally {
    rl.close();
  }
}

main().catch((e) => {
  console.error(`\nerror: ${e instanceof Error ? e.message : e}`);
  process.exit(1);
});