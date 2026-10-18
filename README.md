# tolkmeet

Open-source video meetings with context-aware multilingual captions powered by Google Gemini.

Tolkmeet uses **Google Gemini as its only AI provider**, with one API key configured by the server operator. Video, audio and screen sharing use peer-to-peer WebRTC.

## Screenshots

| Video meeting with live captions (Korean ⇄ Japanese) | Live captions screen for in-person meetings |
|---|---|
| ![Meeting room: two participants, caption panel with the original and the translation](docs/screenshots/meeting.jpg) | ![Live captions screen on a tablet: each sentence's original in small text and the translation in large text](docs/screenshots/live-captions.jpg) |
| **Home** | **New meeting: pick a kind and see what it looks like** |
| ![Home with the live caption demo](docs/screenshots/home.jpg) | ![New meeting dialog with a preview of the selected kind](docs/screenshots/new-meeting.jpg) |

## Current status

- **Implemented:** host login, meeting links, guest admission, video/audio calls, screen sharing and a multilingual caption panel.
- **Implemented:** browser microphone → client VAD → Gemini Live transcription (`gemini-3.5-transcribe-live`) with a one-time token issued by the server, so the API key never reaches the browser. Only speech is sent; sessions are opened ahead of time, kept alive and rotated before they expire.
- **Implemented:** in-person mode — two people at one device each speak their own language and read the other's words translated (host only, `New meeting → In-person conversation`).
- **Implemented:** live captions screen — one tablet on the table listens and automatically tells which of two languages each sentence is in (by writing system), showing the original and the translation in large stacked text (host only, `New meeting → Live captions screen`).
- **Implemented:** server-side STT text → sentence buffering → Gemini streaming translation → captions, with recent sentence context and one translation per target language. Translated text appears from the first chunk; failed or incomplete translations fall back to the original.

The Gemini key belongs in the server environment or the repository root `.env`. Participants do not supply individual keys.

## Structure

| Path | Contents |
|---|---|
| `server/` | Rust (axum) — rooms, WebRTC signaling, translation pipeline |
| `web/` | React + Vite + TypeScript — home, lobby, meeting room, caption panel |
| `site/` | Astro — the landing page (tolkmeet.com), static HTML in English, Korean and Japanese, for GitHub Pages |
| `design/` | Design tokens (`tokens.css`), rules, static preview |
| `docs/ARCHITECTURE.md` | Architecture and deployment (EC2) plan |

## Local development

Requires Rust (stable) and Node 20+ / pnpm. The quickest way:

```bash
scripts/dev.sh
```

Starts the server in `dev` mode and the web app together (http://localhost:5173). In `dev` mode the server creates a test host account `dev@tolkmeet.test` on first run, with its password saved in `server/.dev-host-password` (excluded from git). It only runs when bound to localhost.

To capture screens headlessly (for cloud sessions or AI checks):

```bash
pnpm --dir web shot --login / @meeting
```

The landing page is a separate static site:

```bash
pnpm --dir site install && pnpm --dir site dev
```

Run the server and web separately:

```bash
cargo run --manifest-path server/Cargo.toml -- dev
```

```bash
pnpm --dir web install && pnpm --dir web dev
```

Only **hosts** can create meetings. Guests open the meeting link and **ask to join**, and get in once the host admits them.

### Registering host accounts

Accounts are registered by the server operator via an environment variable (no DB). Run the command below, type the password and press Enter, and it prints the hash (the password isn't left in your shell history):

```bash
cargo run -q --manifest-path server/Cargo.toml -- hash-password
```

Put the output into `HOST_ACCOUNTS` as `email:hash` (separate multiple accounts with spaces or newlines). Sessions live in server memory, so a server restart means logging in again.

Open http://localhost:5173, log in via **Host login**, click **New meeting**, and open the generated link in a different browser (or a private window).
The Vite dev server proxies `/api` and `/ws` to the Rust server on `127.0.0.1:8080`.

For local secrets, put `GEMINI_API_KEY=your-key` in the repository root `.env` (see `.env.example`).
The Rust server loads this file before starting its runtime; existing environment variables take precedence.
The Gemini benchmark also reads the root file, with `bench/gemini/.env` as a legacy fallback.
`.env` is ignored by Git. Keep this key server-side; never give it a `VITE_` prefix.
The same key issues the browsers' one-time speech recognition tokens and translates the recognized text. Without it, nobody's speech is captioned.

Server environment variables:

| Variable | Default | Description |
|---|---|---|
| `BIND_ADDR` | `127.0.0.1:8080` | Listen address |
| `ICE_SERVERS` | Google public STUN | JSON array of STUN servers (`[{"urls": "stun:…"}]`), sent to each admitted participant in `welcome`. Only `stun:`/`stuns:` URLs without `username`/`credential` are accepted; use `TURN_*` for TURN |
| `TURN_URLS` | (none) | Space-separated `turn:`/`turns:` URLs of your coturn server. Set together with `TURN_SECRET` |
| `TURN_SECRET` | (none) | coturn `static-auth-secret` (32+ characters). Each admitted participant gets their own 12-hour TURN credential; without TURN, some participants behind strict NATs can't connect |
| `RUST_LOG` | `info` | Log filter |
| `HOST_ACCOUNTS` | (none) | Host accounts: `email:argon2-hash`, space-separated (emails are case-insensitive). If empty, nobody can create meetings |
| `COOKIE_SECURE` | `true` | Session cookie `Secure` flag. Use `false` only for local http development |
| `GEMINI_API_KEY` | (none) | Translation API key (server only, never sent to browsers). If empty, captions show the original only |
| `GEMINI_TRANSLATE_MODEL` | `gemini-3.5-flash-lite` | Translation model |
| `GEMINI_STT_MODEL` | `gemini-3.5-transcribe-live` | Speech recognition model, locked into each browser token |
| `TRUSTED_PROXY` | (unset) | Set to `1` only behind a reverse proxy (Caddy) that sets `X-Forwarded-For`; login throttling then uses its last hop as the client IP. Otherwise the socket peer address is used |
| `APP_ORIGIN` | `http://localhost:5173` in dev, `https://meet.tolkmeet.com` otherwise | Public address of the app (e.g. `https://meet.tolkmeet.com`). Used for links in emails, passkeys and the Google redirect URI |
| `DATABASE_PATH` | `server/data/dev.db` in dev, `server/data/tolkmeet.db` otherwise (build-time path; set it explicitly in production) | SQLite file (accounts, sessions, meetings, usage) |
| `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` | (none) | Google sign-in (OAuth "Web application" client). Register the redirect URI `{APP_ORIGIN}/api/auth/google/callback` (dev: `http://localhost:5173/api/auth/google/callback`). If empty, the Google button is hidden. Failures are logged with a reason (`google sign-in failed`) |
| `CLOUDFLARE_ACCOUNT_ID` / `CLOUDFLARE_EMAIL_TOKEN` | (none) | Sends account emails through Cloudflare Email Sending (REST API). The token needs the **Email Sending: Edit** permission on the account that onboarded the sending domain (Compute › Email Service › Email Sending › Onboard Domain, which adds the SPF/DKIM/DMARC records). Arbitrary recipients require the Workers Paid plan. If either is empty, dev mode writes mail to `server/data/mail/` (text and HTML) |
| `MAIL_FROM` | `Tolkmeet <no-reply@tolkmeet.com>` | Sender for account emails; must be on the onboarded domain |
| `PADDLE_API_KEY`, `PADDLE_CLIENT_TOKEN`, `PADDLE_WEBHOOK_SECRET`, `PADDLE_PRICE_BASIC`, `PADDLE_PRICE_PRO`, `PADDLE_PRICE_HOUR` | (none) | Paddle billing (all required together). `PADDLE_ENV=production` for live, otherwise sandbox. If missing, checkout is disabled (dev mode activates plans without payment) |

Per-step translation latency (first token, total, STT → translation) is logged at debug level: `RUST_LOG=info,tolkmeet_server::pipeline=debug`. Caption text is never logged.

Protocol types are defined in `server/src/protocol.rs`; `cargo test --manifest-path server/Cargo.toml` regenerates `web/src/protocol/*.ts`.

### Verification

Run `cargo test` and `cargo clippy --all-targets` in `server/`, and `pnpm test`, `pnpm build`, and `pnpm lint` in `web/`.
The browser regression script uses an optional existing Playwright installation and a running Vite server:

```bash
PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs node web/tests/browser-regressions.mjs
PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs node web/tests/browser-mesh.mjs
```

To check the real speech path (a WAV played as the microphone → Gemini Live → translated captions, with per-step timings), run the app with a Gemini key and:

```bash
node web/tests/browser-stt.mjs speech.wav --lang ko --partner ja
```

It calls Gemini for real, so it is not part of CI. `CHROMIUM_PATH` picks a different Chromium build.

These scripts check waiting-room disconnects, failed admissions, mobile caption controls, mute preservation and capture cleanup, plus real WebRTC connections with and without local devices. Signaling and the screen picker are mocked. `TEST_ORIGIN` overrides the default `http://127.0.0.1:5173`.

## License

Copyright (C) 2026 Yoo Seung Jae

Tolkmeet is free software under the [GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`).
If you run a modified version as a network service, you must offer its source code to the users of that service.
Third-party material is listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
