# Networking architecture and status

**Status: OpenMLS message encryption and membership synchronization, libp2p/QUIC peer messaging and file transfer, direct WebRTC voice, per-device delivery, trusted-device enrollment, and encrypted history transfer are integrated.** Real-network NAT traversal and concurrent multi-device MLS state recovery remain incomplete. A successful LAN connection is not evidence that a path works across the public internet.

## Current behavior

- The PHP/MySQL service manages accounts, relationships, group membership, endpoint presence, public MLS KeyPackages, encrypted message queueing, and signaling. It is not required for a direct message after MLS state exists and both peers can reach one another; it is required for account coordination and offline message delivery.
- Message delivery tries the authenticated libp2p/QUIC peer path first. If that path is unavailable, the client stores the already MLS-encrypted message inside a signed, recipient-encrypted envelope in the existing API queue. API acceptance means pending, not delivered; the sender marks delivery only after a peer has durably saved the message and acknowledged it. Pending envelopes expire after 30 days, and receipt records are retained for 90 days.
- Incoming API envelopes are decrypted and authenticated on-device, then deduplicated and durably persisted before the recipient submits a delivery receipt and removes the envelope from the API queue. The client polls on sign-in and retries persisted outgoing messages after restart; unresolved relay submissions retry on a persisted 60-second cadence, and permanent API rejections appear as failed with a manual retry action. Duplicate direct/API arrivals share the same stable message ID.
- Direct and group application messages are encrypted with OpenMLS (RFC 9420) before either libp2p delivery or per-recipient encrypted store-and-forward. MLS signing credentials are bound to the persistent account Ed25519 key. Direct-chat MLS group IDs are deterministic; the lower public-key account initiates the first session to avoid competing initial groups.
- New MLS KeyPackages are published to the existing account API and claimed once by mutual friends. The backend stores only public packages and opaque relay envelopes. Initial group membership is carried in MLS Welcome messages; received application messages and commits are authenticated by OpenMLS.
- Attachments are encrypted on-device with a fresh AES-256-GCM key and nonce, stored locally as ciphertext, and transferred in bounded 256 KiB chunks over authenticated libp2p connections. The MLS-protected metadata binds ciphertext authentication to the stable conversation identifier, sender identity, and participant set. Requests are authorized against the authenticated peer and the exact local conversation; group membership changes do not silently grant access to older attachments. Partial downloads resume from the local offset, cancellation preserves resumable ciphertext, duplicate chunks must match, and AEAD verification succeeds before a download is finalized. Save operations refuse to overwrite existing files. The desktop client sends no attachment bytes to the account API.
- Offline attachment bytes are not queued. The encrypted message and metadata can be stored by the API, but the sender must be reachable later for the recipient to fetch the attachment directly. The recipient displays download progress, throughput, completion, failure and cancellation; the sender displays bytes served, throughput, pending peer confirmations, and completion after all current recipients report success. The normal client sends an authenticated completion report after authenticating the full ciphertext. This is an assertion by the authenticated peer, not independent proof that a human opened the file. Relay addresses must already be configured for relay fallback; no new relay infrastructure is introduced.
- Voice signaling uses authenticated encrypted API envelopes. Offer/answer SDP carries the gathered ICE candidates; WebRTC sends Opus audio directly and protects it with DTLS-SRTP. Public STUN is used for candidate gathering, with no TURN or media-relay configuration. Restrictive NATs and firewalls can prevent calls.
- WebRTC connection state is reflected in the call UI. On ICE failure, the offerer attempts up to two ICE restarts; if direct ICE remains unavailable, the call reports failure rather than relaying media.
- One-to-one conversations also use MLS rather than a Double Ratchet. Compromise of a current MLS epoch's member state may expose messages in that epoch; later commits rotate epoch secrets. Owner-controlled membership updates are synchronized and commits are checked against the roster; removed members are denied future API queue access and cannot decrypt subsequent MLS epochs. Multi-device state recovery remains incomplete. Contacts can compare fingerprints out of band and persist mutual verification locally. The account API still has no independent key-transparency log or external witnesses: unverified keys remain vulnerable to server substitution, while a previously verified key change is detected and blocked locally.
- Each installation has a stable device ID plus distinct locally encrypted Ed25519 signing and X25519 history-encryption keys; these are not account identities or MLS credentials. The first device bootstraps with the account identity. Later installations create an expiring signed request, and an active trusted device must verify the request signature, compare the pairing code on both devices, and sign an approval bound to the target ID and public keys. Migration 17 stores device names/public keys, enrollment state, and encrypted history packages; per-device queue operations continue to use separate authenticated credentials. A revoked device can no longer poll or acknowledge its queue, and history packages involving it are deleted.
- History transfer is user-initiated from Settings > Devices. The source reads a local history snapshot, encrypts it to the target's X25519 public key with an ephemeral key and AES-256-GCM, then signs the ciphertext. The API can retain that opaque package for up to seven days; only the target device decrypts it, imports idempotently, and acknowledges after successful persistence. Packages are capped at 3 MiB. Attachments absent from the source are reported rather than fabricated. This transfer copies the current MLS storage to the new device; simultaneous use of cloned MLS state can diverge, and the application does not yet maintain independent MLS group state or synchronize commits per device. Keep a trusted source device available for recovery.

Unit tests cover MLS Welcome/application round-trips, account-key binding, device enrollment signatures and substitution resistance, encrypted history package target binding and tampering, restart-persisted enrollment identity, history-import MLS conflict handling, exact-conversation authorization, duplicate chunks, cancellation/resumption, receipt persistence, signaling validation, and Opus frame encoding/decoding. They do not demonstrate MySQL concurrency, a live API migration, cross-device delivery, internet-wide NAT traversal, real audio-device behavior, or delivery through arbitrary firewalls.

## Technology evaluation

### Peer data transport: rust-libp2p

The current rust-libp2p release documents native QUIC and TCP transports, Noise, AutoNAT, DCUtR hole punching, and Circuit Relay v2. This is a better foundation than hand-written TCP discovery and handshake code for peer discovery, authenticated streams, reconnection, and traversal. QUIC can multiplex independent streams and avoids TCP's head-of-line blocking between streams.

AutoNAT, DCUtR, and Circuit Relay v2 do not remove the need for reachable infrastructure. Peers need bootstrap/rendezvous nodes and a public relay that accepts relay reservations. Hole punching can fail with symmetric NAT, carrier-grade NAT, or restrictive firewalls. The relay can observe connection metadata and traffic timing; it must only forward authenticated, encrypted streams and must never receive application plaintext.

Use the existing Ed25519 identity as the root of peer authentication, with an explicit identity/key migration and pinning plan. The coordination service may publish rendezvous data and signed public keys, but must not be trusted to silently replace a contact's identity key.

### Real-time media: WebRTC alongside libp2p

Use WebRTC for real-time audio rather than carrying media over reliable libp2p streams. WebRTC supplies ICE candidate checks, DTLS-SRTP, RTP/RTCP congestion and jitter handling, and Opus interoperability. The current deployment uses direct host/server-reflexive candidates and public STUN only; STUN helps discover mapped addresses but does not relay media. There is deliberately no TURN or media relay, so direct ICE failure is a call failure.

The Rust `webrtc` crate provides an asynchronous, runtime-agnostic peer-connection API over its `rtc` protocol core and is the preferred starting point for GPUI desktop integration. Run it on a dedicated async runtime rather than blocking the UI. `str0m` is an actively developed Sans-I/O WebRTC implementation and is attractive when strict control of the network loop is needed, but the application must drive its sockets, timers, ICE, and output polling itself. That extra integration responsibility is not justified for the current desktop app. A single QUIC stack would require implementing media pacing, jitter buffering, loss recovery, and media congestion behavior that WebRTC already provides.

Group voice can use a small-group peer mesh initially, but it costs O(n²) peer connections and is not a scalable large-room design. It must remain explicit rather than implying server-hosted conferencing.

### End-to-end encryption and groups

OpenMLS (RFC 9420) provides the current one-to-one and group application encryption, signed account-bound credentials, Welcome handling, persistent local group state and commit processing. KeyPackage distribution reuses the authenticated account API. The application currently supports initial group creation; an owner-controlled member add/remove workflow and its API roster synchronization are not implemented. Do not treat account group membership as proof that an MLS membership change has completed.

MLS Welcome and application messages travel through the existing signed, recipient-encrypted store-and-forward envelope when direct delivery is unavailable. This outer envelope is coordination transport protection; OpenMLS is the application E2EE layer. The backend must never receive private MLS state or plaintext.

## Target component boundaries

1. **Identity and key storage:** persistent device identity, encrypted local key state, contact-key verification, rotation, and revocation.
2. **Coordination:** account, friends, groups, signed peer records, rendezvous, signaling, push, and bounded ciphertext store-and-forward only.
3. **Peer data plane:** rust-libp2p with QUIC/TCP, Noise-authenticated streams, direct paths first, DCUtR, and optional Circuit Relay v2 fallback; automatically reconnect or upgrade to a direct path when possible.
4. **Messaging and persistence:** OpenMLS state for direct and group conversations, replay/idempotency handling, encrypted local history, and offline queue processing.
5. **Attachments:** encrypted local storage and chunked, resumable peer streams. When peers are offline, only the encrypted message metadata is queued; attachment bytes wait for a direct connection.
6. **Voice:** WebRTC ICE with direct candidate pairs, public STUN, Opus, DTLS-SRTP, encrypted API signaling, ICE restart attempts, and no TURN or server-side media path.
7. **GPUI integration:** consume typed connection/message/call events; never synthesize an online, connected, delivered, or secure state when the underlying operation failed.

## Security assumptions and limitations

- Direct TCP or UDP reachability is not guaranteed by a LAN test or by the presence of a public IP address. NAT mapping behavior, CGNAT, firewall policy, and network changes can invalidate a previously working endpoint.
- If all direct candidates fail, messaging can fall back to encrypted store-and-forward. Calls report direct ICE failure instead of routing media through the account API.
- Relay operators can observe routing metadata and deny service. Application encryption must remain end-to-end, and authenticated transport encryption must be established before relays forward peer streams.
- There is no independent key-transparency log or witness infrastructure. Out-of-band fingerprint comparison prevents silent key substitution only after both people have verified and pinned the exact account key. A malicious API can disrupt discovery and may substitute an unverified key; it cannot silently replace a locally pinned key without an explicit user re-verification.
- Direct file availability depends on peer reachability. Restrictive or symmetric NAT, carrier-grade NAT, firewalls, missing relay configuration, or an offline sender can prevent a transfer; interrupted downloads can resume when the sender is reachable again.
- No formal security audit or live MySQL concurrency test has been performed. Device secrets and private device keys are stored in the encrypted local chat store, whose encryption key is derived from the account identity; there is no OS-backed keystore. Enrollment requires at least one active trusted device, and the API refuses self-revocation and revocation of the last active trusted device, so recovery after loss of every trusted device is not implemented. History transfer is manual, package-limited, and requires the source device; concurrent use of copied MLS state can diverge. MLS and transport dependencies do not by themselves establish that the application's identity binding, group lifecycle, relay envelope, transfer protocol, or local persistence are audited.

## Research references

- [rust-libp2p crate documentation](https://docs.rs/libp2p/latest/libp2p/) — transport and optional AutoNAT, DCUtR, and relay behaviour crates.
- [rust-libp2p repository](https://github.com/libp2p/rust-libp2p)
- [str0m crate documentation](https://docs.rs/str0m/latest/str0m/) — Sans-I/O WebRTC design and run-loop requirements.
- [webrtc crate documentation](https://docs.rs/webrtc/latest/webrtc/) — async peer connections, media, and runtime abstraction.
- [vodozemac documentation](https://docs.rs/vodozemac/latest/vodozemac/) — Olm Double Ratchet and Megolm trade-offs.
- [OpenMLS documentation](https://docs.rs/openmls/latest/openmls/) and [RFC 9420](https://datatracker.ietf.org/doc/html/rfc9420).
- [Opus Rust bindings](https://docs.rs/opus/latest/opus/).
- [QUIC RFC 9000](https://datatracker.ietf.org/doc/html/rfc9000) — standardized UDP-based multiplexed transport.

Research checked on 2026-11-26. Crate versions, maintenance status, platform support, and security advisories should be rechecked before dependency adoption.
