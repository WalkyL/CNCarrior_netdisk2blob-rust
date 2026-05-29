# ADR-001: Add Pluggable Streaming Object Encryption

## Status
Proposed

## Date
2026-05-23

## Context

The gateway currently stores raw object bytes in the selected home provider and returns raw
provider bytes back to S3 clients.

Current data-plane shape:

- S3 `PUT /{bucket}/{key}` streams the request body directly into the selected home provider
  backend in [`put_object`](../../crates/gatewayd/src/main.rs#L30682).
- S3 `GET /{bucket}/{key}` streams the selected provider body directly back to the client in
  [`get_object`](../../crates/gatewayd/src/main.rs#L30587).
- Replication workers copy provider bytes by doing
  `source_backend.get_object(...) -> target_backend.put_object(...)` in
  [`process_replication_job`](../../crates/gatewayd/src/main.rs#L28080).

Current storage/runtime constraints:

- `BlobBackend` is a small transport-oriented trait with `head/get/put/delete`, but it does not
  expose a standard range-read primitive today
  ([`BlobBackend`](../../crates/blob-core/src/lib.rs#L481)).
- Provider `ObjectInfo` values carry provider-native `size`, `etag`, and `content_type`. They do
  not carry gateway-owned logical metadata.
- Persistent metadata currently covers replication jobs and object placements in SQLite, not
  logical object manifests
  ([`metadata-store` schema](../../crates/metadata-store/src/lib.rs#L727)).
- Control-plane state is persisted in `./data/control-plane.json`, credentials in
  `./data/provider-credentials/*.json`, and replication/placement metadata in `./data/ccbg.db`
  ([`AppConfig`](../../crates/gatewayd/src/main.rs#L3763),
  [`persist_control_plane_state`](../../crates/gatewayd/src/main.rs#L28534)).

New product requirements:

- Support optional encrypted-at-rest object storage across carrier providers and OneDrive.
- Keep the capability pluggable instead of hard-coding it into a single provider.
- Keep memory usage low enough for `N100`-class routers and ARM64 soft routers.
- Avoid turning replication into a forced decrypt-then-re-encrypt slow path.
- Preserve a path for desktop mounts and possible future SMB plugins.

## Decision

Add **optional gateway-managed client-side object encryption** at the **S3 data-plane boundary**,
not inside individual provider adapters.

The first implementation should:

- Encrypt object bodies before they are uploaded from the S3 ingress path.
- Decrypt object bodies before they are returned to S3 readers.
- Keep provider adapters as raw cloud transports.
- Prefer ciphertext-to-ciphertext replication between providers.
- Persist logical object metadata in SQLite so S3 `HEAD`/`LIST` can expose plaintext-facing size
  and content type without depending on provider-native object metadata.
- Keep enough envelope metadata in the object header to allow offline recovery and later reindex.

This should be implemented as a new pluggable crate, for example `crates/object-crypto`, plus a
gateway integration layer in `gatewayd`.

## Why This Boundary

### Rejected: provider-wrapper encryption

Wrapping each `BlobBackend` with a transparent `encrypt on put / decrypt on get` decorator looks
simple, but it fights the current replication path.

Today replication reads through `source_backend.get_object(...)` and writes through
`target_backend.put_object(...)`
([`process_replication_job`](../../crates/gatewayd/src/main.rs#L28148),
[`process_replication_job`](../../crates/gatewayd/src/main.rs#L28195)).

If `get_object` always returns plaintext, replication is forced into:

`ciphertext at source provider -> decrypt -> re-encrypt -> ciphertext at target provider`

That:

- burns CPU on every replicated byte,
- complicates retries and content-length handling,
- breaks the goal of keeping encryption overhead low on routers.

### Accepted: S3 boundary encryption

By encrypting at S3 ingress/egress instead:

- providers store opaque ciphertext objects,
- provider adapters remain transport-focused,
- replication can copy stored ciphertext bytes unchanged when the destination uses the same
  encryption profile,
- the encryption layer stays fully pluggable and provider-agnostic.

## High-Level Architecture

### New control-plane objects

Add `encryption_profiles` to persisted control-plane state.

Each profile should include:

- `id`
- `enabled`
- `algorithm_preference`: `auto | aes_256_gcm | chacha20_poly1305`
- `chunk_plaintext_bytes`
- `key_id`
- `key_source`
- `notes`

First-version policy binding:

- add optional `encryption_profile_id` to `DataPlaneApplication`,
- leave content-policy overrides for a later phase.

This keeps the first public contract simple: an application either writes plaintext objects or
writes encrypted objects under one named profile.

### New runtime module boundary

Introduce a pluggable object-crypto surface such as:

```rust
trait ObjectCryptoEngine {
    fn runtime_choice(&self, profile: &EncryptionProfile) -> RuntimeCryptoChoice;
    fn encrypt_upload(
        &self,
        profile: &EncryptionProfile,
        input: EncryptUploadInput,
    ) -> Result<EncryptedUploadPlan, CryptoError>;
    fn decrypt_download(
        &self,
        profile: &EncryptionProfile,
        input: DecryptDownloadInput,
    ) -> Result<DecryptedDownloadPlan, CryptoError>;
}
```

The engine should operate on `ObjectBody` streams, not on fully collected buffers.

### New metadata persistence

Add a new SQLite table, separate from `object_placements`, for logical object metadata.

Suggested table shape:

- `bucket`
- `key`
- `encryption_profile_id`
- `encrypted`
- `algorithm`
- `key_id`
- `chunk_plaintext_bytes`
- `plaintext_size`
- `stored_size`
- `logical_content_type`
- `provider_etag`
- `header_bytes`
- `updated_at_unix_ms`

Rationale:

- provider-native `size` becomes ciphertext size,
- S3 `HEAD` and `LIST` still need plaintext-facing size,
- fallback reads need a logical manifest independent of provider quirks,
- future recovery tooling needs a stable place to rehydrate logical metadata.

## Object Format

Version `ccbgenc1` should be a self-describing streaming envelope.

### Envelope goals

- streamable upload without buffering the full object,
- deterministic total stored size when plaintext size is known,
- enough header data to recover an object if SQLite metadata is lost,
- chunk-local authentication,
- room for key rotation.

### Header fields

Suggested fixed/variable fields:

- magic: `CCBGENC1`
- format version
- flags
- algorithm id
- `key_id`
- `profile_id`
- plaintext size
- chunk plaintext size
- wrapped DEK length
- header length
- base nonce / nonce seed
- optional logical content type
- wrapped DEK bytes

The object header is not secret, but it must be authenticated as associated data.

### Chunk format

Each object is split into fixed-size plaintext chunks except the tail chunk.

Suggested chunk record:

- chunk index
- ciphertext length
- ciphertext bytes with AEAD tag appended

Nonce handling:

- derive each chunk nonce from `base_nonce + chunk_index`,
- do not store a full nonce per chunk unless a later algorithm requires it.

### Key hierarchy

Use envelope encryption:

- one long-lived KEK per `key_id`, loaded from env/file/KMS later,
- one random DEK per object,
- store the DEK wrapped in the object header.

Do **not** store master keys in SQLite or control-plane JSON.

## Runtime Algorithm Selection

Profile setting `algorithm_preference=auto` should select based on CPU capabilities:

- `x86_64` with `AES-NI` and `PCLMULQDQ`: prefer `AES-256-GCM`
- `ARM64` with `aes` and `pmull`: prefer `AES-256-GCM`
- otherwise: prefer `ChaCha20-Poly1305`

Why:

- `N100`-class and newer x86 routers normally have strong AES acceleration.
- Many ARM64 SoCs also expose usable crypto extensions.
- Low-end ARM64 without crypto extensions is the case where ChaCha is safer as a default.

The engine must fail closed if a profile explicitly requests an unsupported algorithm.

## Data-Plane Flow

### S3 PUT

1. Authorize request and resolve `DataPlaneApplication`.
2. Resolve `encryption_profile_id`, if any.
3. Select home provider as today.
4. If no encryption profile is set, keep the current flow.
5. If an encryption profile is set:
   - build an encrypted streaming body,
   - compute stored size from plaintext size + header + chunk tags,
   - upload ciphertext to the home provider,
   - persist logical object metadata,
   - persist home placement,
   - enqueue replication using both logical and stored sizes.

### S3 GET

1. Resolve object read backend as today.
2. Load logical object metadata.
3. Fetch ciphertext object from provider.
4. Validate and decrypt stream chunk-by-chunk.
5. Return plaintext bytes with logical content type and logical content length.

### S3 HEAD / LIST

Use persisted logical metadata instead of provider-native ciphertext size whenever an object is
known to be encrypted.

If metadata is missing but the placement exists, report the object as degraded and force explicit
reindex/recovery instead of silently returning provider ciphertext size as logical size.

## Replication and Fallback

### Primary rule

Replication should prefer copying **stored ciphertext bytes** unchanged.

That keeps CPU cost low and makes retries simpler.

### Same-profile replication

If source and target use the same encryption profile:

- read stored ciphertext bytes from the source provider,
- write the same ciphertext bytes to the target provider,
- persist target replication state normally.

### Cross-profile replication

If the target profile differs:

- do not silently decrypt and re-encrypt in the hot path in v1,
- either reject that topology combination or route it through an explicit maintenance/migration
  workflow.

This keeps the first implementation predictable and avoids accidental CPU explosions on weak
routers.

### Fallback reads

Fallback should use the same logical metadata table and the same encryption profile id as the home
copy. A readable fallback object is still a ciphertext object; decryption happens only at the
gateway S3 egress.

## Performance Guidance

## CPU

The performance risk is platform-dependent.

- On x86 with hardware AES, object encryption should usually be well below carrier-cloud I/O as a
  bottleneck.
- On `N100`-class routers, encryption should be a second-order cost compared with upstream
  provider limits, HTTP overhead, and spool I/O.
- On ARM64 with crypto extensions, the expectation is similar.
- On ARM64 without crypto extensions, encryption can become the dominant CPU consumer for large
  sequential transfers and multi-stream workloads.

Observed reference point:

- On the current `.43` x86_64 test machine with `aes` support, `openssl speed` measured roughly
  `AES-128-GCM ~= 2.6-2.7 GiB/s` and
  `ChaCha20-Poly1305 ~= 1.5-1.6 GiB/s`.

Those are not end-to-end gateway numbers, but they show that on hardware-accelerated x86, crypto
throughput is comfortably above current carrier-cloud transfer rates.

## Memory

The implementation must remain streaming.

Target memory model:

- `O(chunk_plaintext_bytes * in_flight_streams)`
- no full-object buffering for normal reads or writes
- default chunk size should stay small enough for routers, for example `1 MiB`

This matches the existing gateway bias toward low-memory streaming and spool-based writes.

## Desktop Mounts and Future SMB

Encryption is compatible with desktop mounts and a future SMB plugin, but it changes where the
cost shows up.

- `rclone mount` or other desktop clients will read plaintext through the gateway S3 API.
- The gateway decrypts on every read.
- If a future SMB server sits on top of a mount or another S3 client, the aggregate stack becomes:

`provider HTTP + gateway crypto + S3 client/FUSE + SMB`

That means:

- encryption is usually not the only overhead,
- SMB should stay a plugin or separate service,
- the weakest routers should not be expected to run
  `gatewayd + heavy desktop mount traffic + SMB + encryption` as a default profile.

## Recovery and Backup Consequences

Backups must include:

- `control-plane.json`
- provider credential files
- `instance-id`
- SQLite metadata
- encryption key material referenced by `key_id`

Key material must be backed up separately and more carefully than normal config.

The object header should carry enough crypto metadata to support a future reindex tool:

- scan providers,
- identify `CCBGENC1` objects,
- parse headers,
- rebuild logical metadata rows.

Because provider range-read is not standardized today, that recovery path may initially require a
full-object download or provider-specific prefix fetch helper. That is acceptable for recovery,
but not for the hot path.

## Rollout Plan

### Phase 1

- add encryption profiles to control-plane state
- add crypto engine crate
- add logical object metadata table
- support encrypted S3 `PUT/GET/HEAD`
- keep `LIST` consistent for encrypted home objects
- disallow cross-profile replication

### Phase 2

- support ciphertext-preserving replication across providers
- integrate fallback reads with encrypted manifests
- add admin export/import for encryption profile config

### Phase 3

- add object reindex/recovery tooling
- add range-read support and partial decrypt
- consider content-policy-level encryption overrides

## Alternatives Considered

### 1. Per-provider transparent encryption wrappers

Rejected because the current replication path would force decrypt-then-re-encrypt on every copied
object.

### 2. Provider-native encryption only

Rejected because provider behavior is inconsistent, not portable across carriers, and not under
gateway policy control.

### 3. Client-only encryption outside the gateway

Rejected because it prevents the gateway from enforcing or observing storage guarantees and makes
desktop mounts / admin UX inconsistent.

## Consequences

Positive:

- encryption stays pluggable and provider-agnostic
- replication can stay efficient
- the design works on x86, `N100`, and ARM64 with a runtime algorithm switch
- backups and recovery can be made explicit

Negative:

- schema work is required in SQLite
- S3 `HEAD`/`LIST` can no longer trust provider-native object metadata for encrypted objects
- first-version range reads should be treated as unsupported or degraded
- key management becomes a first-class operational concern

## Open Questions

- whether the first accepted AES profile should be `AES-128-GCM` or `AES-256-GCM`
- whether logical metadata should live in a dedicated `object_manifests` table or a broader
  `object_metadata` table
- whether cross-profile replication should be blocked outright or surfaced as an explicit queued
  migration job type
