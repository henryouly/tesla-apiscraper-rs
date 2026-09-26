# TeslaApiScraper Web UI

SolidJS + TypeScript + Tailwind CSS single-page app (Vite). Live vehicle
cards over SSE, refresh-token sign-in, suspend/resume, dark mode.

## Scripts

| Script            | What it does                                      |
|-------------------|---------------------------------------------------|
| `npm run dev`     | Vite dev server (`:5173`); `/api` + `/health` proxied to `localhost:4000` |
| `npm run typecheck` | `tsc -b --noEmit`                               |
| `npm run lint`    | ESLint with SolidJS rules                         |
| `npm run build`   | `tsc -b && vite build` → `dist/`                  |

## Backend wiring

- Run the Rust backend first (`cargo run` in the repo root, port 4000), then `npm run dev` here.
- In production the backend serves `dist/` itself when `WEB_DIST_DIR` points at the build (the Docker image bakes this in); see `docs/api.md`.
- API client and event shapes live in `src/lib/api.ts` — they must match `docs/api.md`; update both together.

## Routes

`/` cars (auth-guarded) · `/signin` · `/settings`, `/settings/car/:id`, `/geofences`, `/charge/:id/cost` (Phase 7 stubs, guarded).
