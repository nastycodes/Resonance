<h1 align="left">
  <img src="assets/images/resonance-logo.png" alt="Resonance logo" width="22" height="22">
  Resonance
</h1>

**A direct-first, end-to-end encrypted messenger for small, invite-only groups.**

[Documentation](docs/) · [Getting started](docs/FRIENDS.md) · [Report an issue](https://github.com/nastycodes-Software/Resonance/issues) · [Releases](https://github.com/nastycodes-Software/Resonance/releases)

[![Rust 2024](https://img.shields.io/badge/rust-2024%20edition-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Windows-blue?style=flat-square)](#platform-support)
[![Status](https://img.shields.io/badge/status-alpha-yellow?style=flat-square)](#platform-support)

Resonance is a native messenger for small, trusted groups. It pairs a Rust desktop client built with [GPUI](https://github.com/gpui-ce/gpui-ce) with a lightweight, self-hosted PHP/MySQL coordination service. When a peer endpoint is reachable, text and group messages are sent directly; otherwise, encrypted message envelopes are queued by the service for offline delivery. Conversation history is encrypted on each device; an explicitly authorized device can transfer an encrypted history package through the service.

There is no phone number or email sign-up: accounts are created with a username, a password, and a one-time invitation code issued by the administrator of your instance.

> [!WARNING]
> **Early development.** Resonance is under active development, and its Windows installer is currently unsigned. Only install builds from a source you trust. See [Platform support](#platform-support) and [Security and privacy](#security-and-privacy) before installing.

## Highlights

- **End-to-end encryption** — Messages are sealed with X25519 key agreement, HKDF-SHA256 key derivation, and AES-256-GCM authenticated encryption.
- **Your history stays yours** — Conversations are encrypted at rest. Explicit history sync sends only a target-device-encrypted package that the server cannot read.
- **Self-hosted relay** — Run your own PHP/MySQL endpoint and keep control of accounts and delivery.
- **Invite-only access** — Registration requires a single-use invitation code from the server administrator.
- **Direct-first messaging** — Online peers receive text and group messages over an authenticated direct connection; encrypted queueing is the offline/unreachable-peer fallback.
- **Rich conversations** — Files, voice notes, image previews, audio playback, and delivery/read status.
- **Live voice calls** — Encrypted 1:1 calls and group calls for up to 12 members, with mute and call controls.
- **Small groups** — Group chats for up to 12 members, with messages encrypted separately for each member.
- **Windows desktop app** — built for Windows PCs

## How it works

```text
Resonance client  ── encrypted envelopes ──▶  PHP + MySQL relay
      │                                        (accounts, queue, opaque
      │                                         encrypted history packages)
      ├── encrypted chat history on device
      └── encrypted file bytes ── direct libp2p/QUIC ──▶ peer

Resonance peers  ◀── direct DTLS-SRTP voice ──▶  Resonance peers
```

When you send a message, your client encrypts a copy for every recipient and hands the resulting envelope to the relay. The relay authenticates the sender, authorizes delivery, and queues the ciphertext until each recipient comes online. Recipients decrypt the message locally with their own keys. The server never receives plaintext, and local history is encrypted with a key derived from your identity key.

Text and group messages use MLS over libp2p or encrypted offline envelopes through the account API. Attachment metadata and its per-file key are carried inside the MLS-protected message; ciphertext is fetched directly from an authenticated conversation participant over libp2p/QUIC and is never sent to the account API by the desktop client. Voice signaling uses the existing encrypted API queue; audio uses WebRTC and Opus over direct ICE with public STUN servers. SDP carries the gathered ICE candidates; no TURN server or media relay is configured, so restrictive NAT or firewall rules can prevent calls. Group calls use a participant-to-participant mesh.

Voice media is encrypted in transit by WebRTC DTLS-SRTP between call participants. The SDP is sent through the existing authenticated encrypted signaling flow; signaling metadata remains visible to the account service. Direct ICE and peer-to-peer file connections are not guaranteed on every network. File downloads resume from authenticated partial ciphertext, verify AES-256-GCM before completion, and reject existing save paths rather than overwrite them. `RESONANCE_MAX_ATTACHMENT_BYTES` can lower the 25 MiB protocol maximum.

## Security and privacy

- **End-to-end encryption.** Relay messages use ephemeral X25519 key agreement with HKDF-SHA256 and AES-256-GCM, and every envelope is signed with the sender's Ed25519 identity key. Group messages are encrypted separately for each member. This scheme is not a double-ratchet: a captured queued envelope can be decrypted if its recipient's long-term identity key is later compromised, and group membership changes do not currently use MLS epoch rotation.
- **Encrypted local storage.** Chat history is encrypted with AES-256-GCM under a key derived from your identity key.
- **Password-protected identity.** Your identity key is encrypted with a key derived from your password using PBKDF2-HMAC-SHA256 (310,000 iterations). Losing your password can make local history unreadable, so keep a backup.
- **Peer-to-peer attachments.** Files and voice notes are encrypted on-device with per-file AES-256-GCM keys and transferred directly in resumable chunks. The MLS-protected message binds the file key, conversation, sender, and participant set. No file bytes are sent to the account API by this desktop client.
- **Peer-to-peer voice calls.** WebRTC sends Opus media directly between participants and protects it with DTLS-SRTP. Offer/answer SDP, including gathered ICE candidates, uses the existing encrypted signaling flow. STUN is used for candidate gathering; TURN and media relays are deliberately not configured, so some NATs, carrier-grade NAT, and firewalls prevent direct connections.
- **Minimal server knowledge.** The service stores account profiles, Argon2id password hashes, sessions, password-encrypted identity keys, preferences, friend and group metadata, short-lived presence, delivery/read timestamps, and queued encrypted message envelopes. It cannot read message contents or direct-transfer file bytes, but does observe account, recipient, timing, and routing metadata. Legacy attachment API routes may remain enabled for older clients.
- **No plaintext backups on the server.** History packages are encrypted for one authorized device before temporary server storage; the server cannot read them. History transfer is limited to 3 MiB and may report attachments unavailable on the source device.

For account safety guidance, see [Getting Started](docs/FRIENDS.md#privacy-and-account-safety). For operator guidance, see [Server Deployment](docs/DEPLOYMENT.md#security-and-distribution).

## Platform support

| Platform | Status | Distribution |
| --- | --- | --- |
| Windows (x64) | Available | Installer — currently unsigned |

Windows is currently the only supported platform. Build the installer on Windows.

## Getting started

### Install the app

Resonance is distributed by instance administrators rather than through public app stores. Ask your administrator for an installer and a one-time invitation code, then follow the [Getting Started guide](docs/FRIENDS.md).

### Build from source

You need a stable Rust toolchain and a reachable Resonance API endpoint.

- **Windows:** Visual Studio Build Tools with the **Desktop development with C++** workload.

```powershell
git clone https://github.com/nastycodes-Software/Resonance.git
cd Resonance
$env:RESONANCE_API_URL = "https://example.com/resonance_api.php"
cargo run --bin resonance
```

HTTP endpoints are accepted only for local loopback addresses; hosted endpoints must use HTTPS. Run the test suite with `cargo test`.
`RESONANCE_API_URL` overrides the build-time API endpoint in debug and release builds.

The client selects an available UDP port for libp2p/QUIC by default. Set `RESONANCE_P2P_PORT` only when you need a fixed firewall rule; a port already used by another app instance will prevent peer networking from starting.

## Self-hosting the relay

The relay exposes a single PHP endpoint backed by MySQL, with its implementation split into focused PHP modules. It requires PHP with `pdo_mysql` and Argon2id support and a MySQL database. The current desktop client does not use the legacy attachment upload/download routes; those routes and their private storage configuration are only relevant if older clients still use them.

Voice calls require no additional server service or configuration; they reuse encrypted API signaling and direct WebRTC ICE with public STUN. No TURN or media relay is used, so restrictive NAT or firewall rules may prevent a call.

For the implemented behavior, protocol evaluation, security limitations, and the target transport design, see [Networking architecture and status](docs/NETWORKING.md).

1. Copy the `api/` directory to a PHP-enabled web server, preserving its `public/` and `src/` layout. Set the web root to `api/public/` so the implementation files remain outside the public directory.
2. Configure the database connection and invitation codes in [`api/src/Config.php`](api/src/Config.php). Configure the legacy attachment storage path only if required by older clients.
3. Expose `api/public/index.php` as `resonance_api.php` (or use `index.php`) and open `https://example.com/resonance_api.php?route=%2Fhealth`; it should return `ok`.
4. Share one-time invitation codes privately. Do not deploy local backup files such as `api/public/index_old.php`.

The endpoint creates its schema automatically on first connection. See [Server Deployment](docs/DEPLOYMENT.md) for invitations, updates, and installer publishing.

Use the current [`api/public/index.php`](api/public/index.php) with the desktop client’s MLS features. An MLS package rejection now identifies the package number and failed validation constraint. A generic “Account service is temporarily unavailable” response includes a reference ID; match it to the PHP error log to diagnose the server-side failure. If you do not have log access, provide that reference ID to the server administrator.

## Project structure

```text
src/                 GPUI desktop application
api/public/          PHP API entrypoint
api/src/             PHP API configuration, services, and route handlers
assets/              Logo, icons, and bundled fonts
installer/           Windows packaging
docs/                User, development, and deployment guides
vendor/              Vendored sqlly-textbox with local GPUI-CE adaptations
```

## Documentation

| Guide | Audience |
| --- | --- |
| [Getting Started](docs/FRIENDS.md) | Installing the app, creating an account, and chatting |
| [Local Development](docs/DEVELOPMENT.md) | Building, running, testing, and packaging the app |
| [Server Deployment](docs/DEPLOYMENT.md) | Hosting the relay, managing invitations, and publishing installers |
| [Networking architecture and status](docs/NETWORKING.md) | Transport research, current implementation limits, and target design |

## Contributing

Bug reports and feature ideas are welcome through [GitHub Issues](https://github.com/nastycodes-Software/Resonance/issues). Pull requests should stay focused, follow the existing code style, and pass `cargo test`.

Please do not report security vulnerabilities in public issues. Use GitHub's private vulnerability reporting for this repository instead.

## License

No open-source license has been published for this repository yet. Until a license is added, the source is © the Resonance authors and all rights are reserved.

Third-party components keep their own licenses:

- [`sqlly-textbox`](vendor/sqlly-textbox) is dual-licensed under MIT or Apache-2.0.
- The bundled Bricolage Grotesque and Instrument Sans fonts are distributed under the SIL Open Font License; license texts are included in [`assets/fonts`](assets/fonts).

## Acknowledgements

Resonance builds on [GPUI-CE](https://github.com/gpui-ce/gpui-ce) for its native UI and on [sqlly-textbox](vendor/sqlly-textbox) for text input. Thanks to the maintainers of both projects and to the creators of the bundled open-source fonts.
