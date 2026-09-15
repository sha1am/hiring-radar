# Hiring Radar — dashboard

React + Vite + Tailwind. The Rust process serves the built bundle from
`dist/`; there is no separate web server and no CORS, because the UI and its
API are the same origin.

## Working on it

```sh
# terminal 1 — the API, with whatever data it has crawled
cargo run

# terminal 2 — the UI, with hot reload
cd frontend && npm install && npm run dev
```

Vite proxies `/api`, `/events` and the resume upload to `127.0.0.1:8080`, so
the dev server shows real crawled data rather than fixtures. Open
<http://localhost:5173>.

## Building

```sh
npm run build     # -> dist/, which the Rust binary serves
```

`docker compose up --build` does this for you in its own stage; you only need
to run it by hand when running the binary directly. If you do run the binary
without building, the dashboard says so and tells you the command instead of
serving a blank 404.

Set `RADAR_UI_DIR` to serve the bundle from somewhere other than
`frontend/dist`.

## Shape

- `api.ts` — every call to the server. One error shape, one place to change.
- `types.ts` — hand-written mirrors of the DTOs in `src/api.rs`. A field rename
  on the server shows up here as a compile error rather than an `undefined` at
  runtime.
- `hooks.ts` — the SSE subscription (debounced), and the view/filter state that
  lives in the URL hash so Back and bookmarking work.
- `components/` — one file per region of the page.

Anything displayed that depends on server time or settings — age strings,
source labels, rounded scores — is computed server-side and sent ready to
render. A client that recomputes those drifts from the server that decides
them.
