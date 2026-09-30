# easytier-web Architecture

## Layout

| Path | Role |
|------|------|
| `src/` | Rust config server + REST API (`client_manager`, `db`, `restful`, optional `web` embed) |
| `frontend/` | Web dashboard SPA (auth, device/user lists, routing) |
| `frontend-lib/` | Shared Vue UI (Config / Status / RemoteManagement / theme / tooltip / i18n) |
| `config-generator/` | WASM config editor; CI copies its `dist/` into `frontend/dist/config-generator/` |

Also consumes this lib: `easytier-gui` (Tauri).

## Ownership

- **frontend-lib**: shared product UI + `EasyTierPreset` + floating-vue tooltip. Apps must not call `app.use(PrimeVue)` again.
- **frontend**: web-only REST client (`src/modules/api.ts`), pages, router.
- **config-generator / easytier-gui**: thin shells; GUI adapts `Api.RemoteClient` via Tauri IPC.

## Build order

1. `pnpm --dir easytier-web/frontend-lib build` (proto codegen + Vite lib)
2. `pnpm --dir easytier-web/frontend build` (and/or config-generator)
3. `cargo build -p easytier-web --features embed` → serves `frontend/dist/`

Windows helper: `script/build-easytier-web.ps1`.

## Theme

Single source: `frontend-lib/src/modules/theme.ts` (`EasyTierPreset`, sky primary) + `--et-*` tokens in `frontend-lib/src/style.css`.
