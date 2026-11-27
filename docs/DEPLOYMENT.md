# Server Deployment

This guide explains how to deploy the Resonance PHP/MySQL service, configure invitations, and publish desktop installers.

## Overview

The relay exposes a single PHP endpoint backed by MySQL, with its implementation split into focused PHP modules.

The Resonance server provides accounts, friend relationships, group delivery authorization, presence, and temporary queues for encrypted messages and explicitly authorized device-history packages. It cannot read message contents or history packages.

## Prerequisites

- A PHP-enabled website and a MySQL database
- PHP `pdo_mysql`, Argon2id, and sodium support
- HTTPS for any hosted API endpoint

The database user needs permission to create and drop tables, create indexes and foreign keys, and select, insert, update, and delete records.

## Deploy the PHP endpoint

### 1. Prepare MySQL

Create an empty database and a dedicated database user in your hosting control panel. Keep these values available:

- Database host and port
- Database name
- Database username and password
- TLS CA certificate path if MySQL is remote

### 2. Configure and upload the endpoint

Upload the complete `api/` directory, preserving the `public/` and `src/` layout. Configure the web root to `api/public/` so `api/src/` is not publicly served. Expose [`api/public/index.php`](../api/public/index.php) as `resonance_api.php` (or keep the name `index.php`), for example:

```text
https://example.com/resonance_api.php
```

Edit the connection settings in [`api/src/Config.php`](../api/src/Config.php):

```php
const DB_HOST = 'localhost';
const DB_PORT = 3306;
const DB_NAME = 'your_database';
const DB_USER = 'your_database_user';
const DB_PASSWORD = 'your_database_password';
const DB_SSL_CA = '';
const ATTACHMENT_STORAGE_PATH = '';
```

For MySQL on the same host, `DB_SSL_CA` can be empty. For a remote MySQL server, set it to the CA certificate path on the web server; the API verifies the server's TLS certificate. In PHP single-quoted strings, escape literal apostrophes and backslashes.

Keep the configured `api/src/Config.php` private and preserve its deployment-specific values when updating the API. Do not upload local backup files such as `api/public/index_old.php`. The web server must execute PHP rather than return its source. No rewrite rules, schema import, or PHP CLI server is required.

### 3. Legacy attachment API storage (optional)

The current desktop client transfers attachment ciphertext directly between peers and does not upload or download file contents through the API. The PHP endpoint still includes legacy attachment routes for older clients. If those clients are in use, provide a writable directory outside the public web root using `ATTACHMENT_STORAGE_PATH`; otherwise, no attachment storage or request-body configuration is needed for desktop transfers.

The legacy routes enforce their existing size and quota limits. Do not treat legacy API storage as the current desktop transfer path.

## Verify the endpoint

Open the health URL in a browser:

```text
https://example.com/resonance_api.php?route=%2Fhealth
```

It should return the plain text `ok`. On its first connection, the endpoint creates the Resonance tables automatically. Schema upgrades are serialized with a MySQL advisory lock so concurrent first requests do not race migrations. The first device is registered with an account-identity signature; additional installations require an enrollment request signed by their device key and approval signed by an already trusted device. Migration 15 preserves queued envelopes and assigns them to registered devices when they register; each device then has an independent leased delivery and acknowledgement. Migration 17 adds device public keys, expiring enrollment requests, and a temporary opaque history-package queue. If it returns an error, check the database settings, PHP extensions, database privileges, and TLS CA path.

## Manage invitation codes

Registration accepts only configured, unused invitation codes. Generate one 48-character hexadecimal code per person on a trusted machine with PHP:

```sh
php -r "echo bin2hex(random_bytes(24)), PHP_EOL;"
```

Add the generated values to `REGISTRATION_INVITE_CODES` in `api/src/Config.php`, deploy the updated API while preserving the database settings, and send each code privately to one person. The API stores a hash of each used code and rejects reuse. Keep invitation codes and the configured API files private.

## Configure desktop clients

For local runs, set the exact URL of the PHP endpoint in `RESONANCE_API_URL`.

**PowerShell**

```powershell
$env:RESONANCE_API_URL = "https://example.com/resonance_api.php"
cargo run --bin resonance
```

For release packages, set `RESONANCE_DEFAULT_API_URL` to the hosted HTTPS endpoint before building. The app allows HTTP only for local loopback endpoints.

## Peer and voice networking

Text and group messages use the libp2p peer transport; encrypted offline envelopes and coordination use the existing account API. Attachment keys and conversation/participant bindings are carried inside MLS-protected messages, while encrypted file bytes move directly over authenticated libp2p/QUIC connections in resumable chunks. The desktop client does not send attachment bytes to the API. Configure existing relay addresses with `RESONANCE_RELAY_ADDRESSES` if peers need the already-supported Circuit Relay path; no new relay service is provisioned by this feature.

Voice uses WebRTC media with Opus and direct ICE connectivity. The SDP offer/answer carries gathered ICE candidates through the existing encrypted signaling channel. STUN servers help discover public-facing candidates; they do not carry voice media. Resonance configures no TURN server or media relay, and the API does not relay media or create NAT mappings. The client selects an available UDP port by default; set `RESONANCE_P2P_PORT` to a fixed port when firewall rules require one. Restrictive or symmetric NAT, carrier-grade NAT, and inbound-blocking firewalls can prevent direct ICE connectivity; calls fail clearly rather than falling back to a relay.

The deployment requires no additional voice, TURN, or media-server component. See [Networking architecture and status](NETWORKING.md) for current implementation limits, including group-encryption and attachment-transfer gaps.

## Publish desktop installers

Set the GitHub repository variable `RESONANCE_DEFAULT_API_URL` to the hosted HTTPS endpoint. Then manually run the Windows installer workflow and provide an existing release tag, such as `v0.1.0`.

The workflow builds the Windows installer.

The workflow builds the installer and attaches it to the selected GitHub release. For local build prerequisites, see [Local Development](DEVELOPMENT.md).

## Security and distribution

The Windows installer is currently unsigned. Windows Authenticode signing requires a publisher certificate before broad distribution. Until then, distribute installers only from trusted sources.

The server stores account profiles and profile pictures, password hashes, sessions, password-encrypted private identity keys, preferences, friend requests and links, group membership metadata, short-lived presence, queued end-to-end-encrypted message envelopes, and temporarily queued history ciphertext. Profile pictures are limited to 1 MB. Changing an account password also replaces the encrypted identity key and signs out other active sessions. History packages are encrypted to the target device's registered X25519 public key and signed by the source device; the service cannot decrypt them. Full plaintext conversation history remains on clients. The server observes account, device, recipient, timing, and routing metadata. Legacy attachment endpoints remain in the PHP API for older clients.

The current offline-message encryption is not a double-ratchet and does not provide forward secrecy against later compromise of a recipient's long-term identity key. Group messages are encrypted separately to each member; membership changes do not use MLS epoch rotation or revocation. The custom direct peer handshake and TCP audio framing have not been independently audited. Do not treat this alpha implementation as a security-reviewed production messenger.

The current desktop client stores encrypted attachment ciphertext locally and on the sending peer; offline message delivery does not queue file bytes. Legacy API attachment blobs remain subject to the PHP endpoint's existing expiry and cleanup behavior.

## Updates and data migrations

Existing deployments must upload the updated `api/src/` modules and `api/public/index.php`, preserving deployment-specific values in `api/src/Config.php`. Schema migrations run automatically and are serialized across concurrent API requests. Migration 15 adds device registrations and per-device delivery leases without deleting queued messages; existing queued messages are assigned to a device when it registers. Migration 16 adds account-level group delivery state and bounded per-device retry timing. A group account is considered delivered after any active device acknowledges; revoked devices are excluded. It remains pending while an active device is pending or no active device is registered, and otherwise reflects terminal per-device outcomes. Migration 17 adds device names and signing/encryption public keys, enrollment requests, and seven-day opaque history packages. Existing installations are upgraded in place with a default device name and nullable public keys; each installation must sign in once to publish its device keys. Additional installations cannot self-register and require approval from an active device. Revoking a device invalidates its queue access, marks its pending deliveries revoked, and deletes history packages involving it; the API prevents revoking the current or last active trusted device. The device-specific message routes require an updated client, so older client binaries cannot poll or acknowledge messages after migration 15. Existing server-side conversation, message, and community tables are deleted by the API schema migration.

The current endpoint reports the package number and validation constraint when rejecting an MLS key package. Generic 500 responses include a reference ID that is also written to the PHP error log; use it to locate the corresponding server-side exception. Share the reference ID with the server administrator if you do not have log access.

## Related documentation

- [Resonance overview](../README.md)
- [Getting Started](FRIENDS.md)
- [Local Development](DEVELOPMENT.md)
