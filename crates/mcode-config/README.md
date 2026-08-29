# mcode-config

`mcode-config` owns MCode's strict home layout and configuration authorities. It
does not discover project configuration or load provider, plugin, session, or
CLI state.

## Canonical layout

```text
~/.mcode/
├─ config.json
├─ plugins.json
└─ plugins/
   ├─ .host/auth.json
   ├─ .staging/<transaction-id>/
   └─ <family>/
      ├─ manager/{config.json,installation.json,data/,versions/<semver>/}
      └─ packs/<pack-id>/{installation.json,data/,versions/<pack-version>/}
```

`HomeLayout` constructs this hierarchy for the 12 closed `PluginFamily` values:
`providers`, `session`, `compaction`, `resources`, `ask`, `todo`, `web`, `mcp`,
`usage`, `subagents`, `workspace`, and `ui`. Pack IDs use a portable lowercase
ASCII grammar; `.host` and `.staging` are reserved.

`HOME` is the normal user-home source. On Windows only, an absent or empty
`HOME` falls back to `USERPROFILE`. A nonempty `MCODE_HOME` replaces both.
Invalid higher-priority values fail closed rather than trying another source.
This platform home resolution is not a product data compatibility path.

## Bootstrap and owned files

`ensure_home_layout` eagerly creates exactly the owned root and `plugins/`.
Everything else is lazy. Existing owned directories must have current ownership
and private access: Unix `0700`, or a protected Windows DACL containing only the
current user and `SYSTEM`. Owned paths use handle-relative no-follow operations,
reject wrong-case aliases and foreign owners, and receive native durability
barriers.

Owned-file transactions create only the ancestors required by a requested
canonical path. Reads create nothing. Mutations retain a sibling lock across
read, validation, and replacement; use bounded zeroizing read buffers; write a
private same-directory temporary file; atomically rename it; verify the
published identity and access control; and sync the parent directory.

## Authorities

All authority documents are bounded strict UTF-8 JSON. Parsing rejects duplicate
keys, malformed or partial JSON, trailing content, excessive depth or nodes, and
non-UTF-8 input. Mutations validate the current document before revision
compare-and-swap and durable replacement.

- Root `plugins.json` is the exact-12 Manager registry. It owns enablement,
  source binding, active artifact, and trust high-water state.
- Root `config.json` is the complete Host composition. It owns explicit
  provider/model defaults, ordered provider and usage Pack sets, UI selection,
  themes, and singleton Pack slots.
- `plugins/<family>/manager/installation.json` is a Host-generated receipt; it
  does not control registry authority or activation.
- `plugins/<family>/packs/<pack-id>/installation.json` owns that Pack's source,
  selected artifact, trust high-water state, and sorted inventory.
- `plugins/.host/auth.json` is the only credential authority. It is created only
  by `initialize_empty_host_vault`; status reads expose only absence or revision.

These APIs provide storage mechanics, strict schemas, and revision state. They
do not verify bundle signatures or payload completeness, activate artifacts,
create credential injection leases, or infer composition defaults.

## Frozen old-path policy

There is no executable recognition, migration, compatibility read, dual read,
alias, or fallback for old `.MCode`, top-level `settings.json`, `models.json`,
auth/credential/auth-state, `plugins.lock*`, global session, sibling Pack-root,
profile/provider, Fake, M1, TOML, or Tier layouts. Existing obsolete artifacts
are outside the product and are never read, migrated, or deleted. MCode does not
recursively clean an old root; negative tests freeze rejection and non-creation
without touching legacy secrets, unknown user data, or current Plugin state.
