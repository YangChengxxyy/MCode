# mcode-config

`mcode-config` is MCode's independent configuration foundation. It has no
dependency on provider, plugin, MCP, resource, session, or CLI crates, and it
does not discover a project root.

## Format

Product settings are JSON only. There is no TOML/YAML parser, migration, or
compatibility path; `Cargo.toml` remains build metadata only. Every source uses
this exact versioned envelope:

```json
{
  "formatVersion": 1,
  "config": {
    "model": "example"
  }
}
```

Only the crate's current `FORMAT_VERSION` is accepted. The envelope has exactly
the `formatVersion` and `config` members. Domain keys under `config` remain
opaque until the caller's typed serde/schema validation hook runs.

Parsing rejects:

- duplicate object keys at every nesting level;
- comments, trailing commas, trailing non-whitespace, and partial/torn JSON;
- non-UTF-8 input;
- unsupported versions or malformed envelopes;
- configured byte, source-count, depth, and node-count limit violations.

## Sources and precedence

The caller supplies `ConfigLayer` values and typed
`ConfigSource { scope, path, trust }` metadata. Paths stay as native `PathBuf`
values, so non-UTF-8 paths do not require a lossy conversion. Layers are stably
ordered into this precedence chain (later layers win):

1. `CompiledDefaults`;
2. `Global` — normally `$MCODE_HOME/settings.json`;
3. `Project` — normally `<trusted-project>/.mcode/settings.json`;
4. `Explicit` — ephemeral invocation overrides.

Input order is retained within one scope. At least one compiled-defaults source
must participate. A project source marked `Untrusted` is not opened or parsed,
does not merge, and emits a bounded `UntrustedProjectSkipped` diagnostic.
Untrusted non-project sources are errors because they indicate invalid caller
wiring.

File-backed layers may be required or optional. In-memory layers copy bytes and
redact them from `Debug`; they are suitable for compiled and explicit
inputs.

## JSON Merge Patch

Payloads use RFC 7396 JSON Merge Patch:

- object members merge recursively;
- arrays replace as a whole (never by index);
- scalars replace as a whole;
- `null` as an object member deletes that member.

Given these two payloads:

```json
{"formatVersion":1,"config":{"ui":{"theme":"dark","dense":true},"models":["a","b"]}}
```

```json
{"formatVersion":1,"config":{"ui":{"dense":null},"models":["c"]}}
```

the result is:

```json
{"ui":{"theme":"dark"},"models":["c"]}
```

`ConfigSnapshot::provenance()` has an entry for every final RFC 6901 JSON
Pointer, including the root, containers, and array elements. Object member
names escape `~` to `~0` and `/` to `~1`. A composed object's own pointer keeps
the source that created or replaced the object; each changed descendant records
its winning source.

## Credential boundary

Credential-like keys are detected recursively and case-insensitively, including
`token`, `key`, `secret`, `password`, `passphrase`, `cookie`,
`authorization`, `credential`, and common singular/plural compounds such as
`apiKey`, `apiKeys`, and `accessKeys`. Markers are matched at snake-case,
kebab-case, or camel-case term boundaries, with fail-closed recognition for
unambiguous concatenated/all-uppercase suffixes and trailing numeric version
labels rather than arbitrary internal substrings. Token quantity fields such as
`maxTokens`, `max_tokens`, and `tokenBudget` are ordinary domain settings. A
material credential field accepts only this exact shape:

```json
{"apiKey":{"secretRef":"provider/openai/default"}}
```

Extra fields, inline scalars, arrays, and other objects fail closed. `null` is
allowed only for a member of an RFC 7396 patch object, where it is a deletion
marker. Objects inside array replacements are material values, so credential
fields there cannot be `null`. Checks run on every source patch and again on the
merged value, so a later override cannot hide an unsafe earlier source.

The crate does not resolve secret references, implement a secret store, or
perform `${ENV}`/environment/string interpolation. Snapshot/runtime/layer/error
`Debug`, errors, and diagnostics never render JSON values or validator details.

## Reload publication

`ConfigRuntime` publishes immutable `ConfigSnapshot` values. Reload is
watcher-independent and accepts a cooperative `ReloadCancellation` token:

1. read all participating sources;
2. parse and merge with provenance;
3. run foundation security/resource checks;
4. invoke the caller's `ConfigValidator` typed serde/schema hook;
5. compute a canonical BLAKE3 digest;
6. swap the complete snapshot under one publication lock.

Any failure leaves the previous snapshot untouched. Equal value digests do not
advance `generation`, although refreshed provenance/diagnostics are published.
Concurrent readers therefore observe a complete old or new snapshot, never a
partially updated value.

## Owned-home directory bootstrap

`HomeLayout` lexically constructs a relocatable hierarchy for the 12 closed
MCode-owned `PluginFamily` values. Each family has a stable `com.mcode.<family>` Plugin identity and a
`plugins/<family>/` container; its Manager is at `manager/`, and work Packs are
nested at `packs/<pack-id>/`. Host-only credentials are reserved at
`plugins/.host/auth.json`, and global Host staging is at
`plugins/.staging/<transaction-id>`. The families are `providers`, `session`,
`compaction`, `resources`, `ask`, `todo`, `web`, `mcp`, `usage`, `subagents`,
`workspace`, and `ui`. Pack IDs retain the portable lowercase ASCII validator;
`.host` and `.staging` are reserved and cannot be Pack IDs. Constructors never
inspect or modify the filesystem.

`ensure_home_layout` eagerly creates exactly the owned root and `plugins/`.
Pre-existing prefix links outside the owned boundary may be followed, but the
owned root and every owned child are opened or created handle-relative without
following links. A `HOME`/`USERPROFILE`-derived root rejects wrong-case `.mcode`
aliases during bootstrap; every layout rejects wrong-case `plugins` aliases.
Existing owned directories must be owned by the current user (Windows also
accepts `SYSTEM` before repair), then are tightened to Unix mode `0700` or a
protected Windows DACL containing exact full-control ACEs for only the current
user and `SYSTEM`. Newly created directories and their parents receive native
durability barriers.

All Plugin containers, Managers, Packs, `.host/`, `auth.json`, `.staging`,
`config.json`, `plugins.json`, data, versions, sessions, and auth-state paths
remain absent and lazy. This bootstrap performs no regular-file read or write,
lock, temporary-file, replacement, or migration operation.

## Owned-file transaction substrate

Crate-private owned-file operations validate relative paths through
`HomeLayout`. Missing reads create nothing. Mutations create only the owned
ancestors required by the requested path, retain a persistent sibling lock
across read/callback/replace, and use bounded reads plus handle-relative
no-follow opens. Directories and files must have current ownership and exact
private access (`0700`/`0600` on Unix; protected current-user-and-`SYSTEM` DACLs
on Windows). Replacement writes a private same-directory temporary file,
flushes it, atomically renames it relative to the opened parent, verifies the
published identity and access control, and executes the parent durability
barrier. Links/reparse points, wrong types, wrong-case aliases, and foreign
owners fail closed. The root `plugins.json` authority builds directly on this
substrate. Its exact newline-terminated document is
`{formatVersion:1,kind:"mcode-manager-registry",revision,managers}` with exactly
the 12 short family keys. Each family is either fully absent (`enabled:false`
and all nullable fields `null`) or fully installed with a strict opaque source
binding, canonical SemVer plus SHA-256 active artifact, and signed-manifest
trust high-water mark. `read_manager_registry` creates nothing when missing;
`replace_manager_registry` performs the lock-held bounded read, strict parse,
revision compare, full serialization, increment, and durable atomic replacement
as one transaction. Missing has logical revision zero. Stale, malformed, and
exhausted revisions never replace the target. This registry does not accept
Pack IDs, project merges, null patches, legacy ABI data, receipts, signatures,
or production source writes.

Per-family Manager installation receipts use the same substrate at
`plugins/<family>/manager/installation.json`. Their exact newline-terminated,
16 KiB-bounded document is
`{formatVersion:1,kind:"mcode-manager-installation-receipt",revision,family,active:{version,digest}}`.
The short `family` must match the path, and the active artifact uses the same
canonical SemVer and lowercase SHA-256 grammar as the registry.
`read_manager_receipt` creates nothing when missing; `replace_manager_receipt`
strictly validates the current receipt before a lock-held revision CAS and
durable atomic replacement. Missing has logical revision zero, while stale,
malformed, and exhausted revisions never replace the target. Receipts are
Host-generated, non-authoritative bookkeeping: they never control enablement,
source binding, trust high-water, artifact selection, loading, routing, or
activation, and these APIs never read or update `plugins.json`. A missing or
corrupt receipt may be rebuilt only by a future verified Host install
transaction.

Nested Pack installation authority uses the same substrate at
`plugins/<family>/packs/<pack-id>/installation.json`. Its exact 4 MiB-bounded,
newline-terminated document is
`{formatVersion:1,kind:"mcode-pack-installation",revision,family,packId,source,selected,trustHighWater,inventory}`.
The short family and Pack ID must match the path. Inventory contains 1 through
4096 entries strictly sorted by canonical forward-slash `BundlePath`, each with
a lowercase SHA-256 digest. `BundlePath` rejects traversal, platform aliases,
reserved `data` and `installation.json` components, and nonportable spellings.
Reads create nothing; replacement strictly parses the current document and
performs a lock-held revision CAS before durable atomic replacement. This is
mechanical authority only: it does not scan payloads, prove inventory
completeness, verify signatures or source/trust/artifact relationships, install
or activate anything, update `plugins.json`, or control a Manager receipt. A
future verified same-volume staging transaction must prove that every selected
version regular file exactly matches the inventory, with no extra file, link,
reparse point, or device, before writing this authority.

The independent root `config.json` authority uses the same transaction and
revision CAS substrate. Its exact flat document kind is
`mcode-root-composition`; it contains an explicitly nullable opaque default
provider/model route, ordered unique provider and usage Pack lists, a nullable
UI runtime with strictly sorted unique themes, and exactly nine nullable
singleton-family Pack slots. Pack IDs use the same frozen portable grammar as
`HomeLayout`. Empty composition is valid, and no default, registry metadata,
activation, source collision, or product-cardinality rule is inferred.
`read_root_composition` creates nothing when missing; successful replacement
writes one deterministic newline-terminated complete document. The 64 KiB
bound, strict JSON parsing, malformed-current preservation, revision conflict,
and revision exhaustion behavior match the standalone authority contract.

The substrate and authorities define no settings merge, auth, session,
migration, or credential entry. `write_config_file` below remains the separate
arbitrary-path configuration writer and does not use this owned authority
boundary.

## Atomic writes

`write_config_file` serializes the current envelope as JSON. Explicit node
limits count the same complete envelope as the reader, while the depth budget
still starts at the `config` root. Writes use:

- a persistent same-directory advisory lock;
- a random same-directory temporary file opened with `create_new`;
- mode `0600` on Unix or inherited directory ACLs on Windows;
- `write_all`, `flush`, and `sync_data` before replacement;
- atomic rename replacement on Unix and Unicode `ReplaceFileW` replacement on
  Windows;
- best-effort temporary cleanup on every pre-replacement failure.

It does not write session files, plugin manifests, or CLI state.
