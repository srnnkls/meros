# Study: owning values in files other writers share

Decided 2026-11-25.

## Summary

meros lets a tool own some values of a structured file that the user and other programs also write, such as `~/.claude/settings.json`, `.codex/config.toml` or `pyproject.toml`. The tool declares desired values for the units it owns; meros plans writes, removals, conflicts and releases from the file's current content and a record of what the tool last wrote, edits the file in place, and returns the new record. A unit is the tool's own only while its value is exactly what the tool last wrote. Everything else is reported, never changed.

## Question

How should a declarative tool manage the values it owns inside a file it shares with other writers, so that it never silently loses their changes, converges on reruns and fresh machines, keeps the file's formatting, and stays small enough to reuse across tools?

## Prior art

Kubernetes moved from client-side apply, which stored the whole last-applied document and silently overwrote other writers' changes (its KEP calls it "buggy and hard to fix"), to server-side apply. Server-side apply records field ownership per manager, conflicts only when a value owned elsewhere would change, releases a field on omission without deleting it when another manager owns it, and identifies list elements by value for scalar sets, by key fields for object lists, or treats the list as one unit.

Infrastructure tools share the same small set of policies: enforce, create-only (Terraform `ignore_changes`, Crossplane `initProvider`, Flux `IfNotPresent`), additive merge, hands off, and release without delete (Terraform `removed`, Pulumi `retainOnDelete`, Argo CD `Delete=false`). Mixing exclusive and additive ownership of one collection fails (AWS IAM policy attachments, security group rules). Comparing representations instead of values produces false drift (CloudFormation "1024 MB" vs "1GB").

Config-file editors (Augeas, Elektra, Ansible, Puppet, Salt, chezmoi) keep no record of what they wrote and silently revert other writers' edits; home-manager makes files read-only, which breaks apps that write their settings. Format fidelity comes from editing in place, not re-serializing.

Agent-config tools repeatedly lost user data by rewriting whole files or whole top-level keys (ai-rulez #185) and fixed it by merging by key. Claude Code itself loses updates in `~/.claude.json` through uncoordinated read-modify-write. Where an app reads several layers and never writes one (Claude Code project and managed settings, mise override files, devcontainer snippets), owning a whole layer file needs no field machinery.

Closest tools: mise `dot` merge entries own the keys in their source with format fidelity but keep no record, so they cannot tell another writer's edit from drift and never remove released keys. dfm keeps a per-leaf ledger with take-over and retire and writes sorted keys. microsoft/apm keeps per-file hashes and an ownership ledger. henia's config install owns units with a per-unit hash record and is the baseline this design revises.

The pattern keeps being rebuilt inside individual tools — managed-machine's key ledger, weaver's per-module state, rulesync's owned top-level keys, agent-hub's digest ledger, apm's per-entry fingerprints — and their reported failures are the same ones: no seed-once mode, a parse failure read as an empty document, an empty source reported as success, lost comments. None packages it for reuse.

In Rust, `toml_edit` and `jsonc-parser` edit TOML and JSON/JSONC in place with comments; YAML editors that keep comments are young; RFC 8785 canonicalizers exist; no crate offers ownership of values in local files (`settings_loader` and `setty` edit config without ownership).

## Design

Inputs per file: the document, the declared units (path → desired value, policy), the schema (identity per path) and the previous record. Paths follow taxis's grammar: keys and `[field=literal]` selectors, no indexes. The caller stores the record and serializes runs.

1. Units come from the current schema only; a record entry that is no longer a unit is dropped and reported released.
2. A unit is mine iff the record has an entry for it and the current value hashes to it. Absent recorded units and unrecorded units are not mine.
3. A duplicated identity makes every unit with that identity not mine; meros never removes one copy of several.
4. Declared, `enforce`: equal to desired → nothing written, an equal value someone else set stays unrecorded; mine → replace; unrecorded and absent → create; otherwise conflict.
5. Declared, `create-only`: write only when absent and unrecorded, and record it; never conflict, never remove.
6. Undeclared: mine → removed; recorded but not mine → entry dropped, file untouched, reported released.
7. Identity per path: `atomic` (default), `set` (scalars by value), `keyed(field)` (list elements by a unique field), map members by name.
8. The whole apply is planned before any write: new bytes are produced and re-parsed, and anything beyond the planned units aborts the apply, as does any conflict. Edits are in place and keep surrounding comments; a comment inside an owned compound value conflicts only on a real replace or removal. Writes refuse if the file changed since it was read; record entries are hashed from the written bytes.
9. Hashes are versioned RFC 8785 over the parsed value; TOML and YAML distinguish integer and float, TOML also its datetime kinds (YAML 1.2 timestamps are strings); JSON numbers compare by value; integers beyond ±2^53 are refused.
10. A declaration that fails to load aborts the apply.
11. Force applies to one apply's conflicts and is reported per conflict with its cause.

Formats on day one: TOML and JSON/JSONC in place; YAML refused when it has comments or anchors, otherwise rebuilt for owned changes only.

## Gains

| Scenario | henia units | meros |
|---|---|---|
| User value equal to declared, later undeclared | deleted | kept |
| Owned value deleted by user or app | silently re-added | conflict if declared, released if not |
| Undeclared value edited by someone else | stuck conflict | released, kept |
| Owned set element edited | duplicated | conflict |
| `1.0` vs `1` in JSON | conflict | equal |
| Comments outside owned values | conflict; force drops them | kept |
| Identity change | undefined | old entries released |

## Costs

- An equal value someone else set is never owned, so a later declared change conflicts once and needs force.
- An app that wipes owned values (Claude Code's `~/.claude.json`) produces conflicts that need force.
- Values an app rewrites into its own normal form conflict on every run.
- A narrow window between the final compare and the rename remains.
- About 18–20 rules, against henia's 11.

## Positioning

phora distributes immutable, verified artifacts and never owns parts of files. mise pins tools and can own keys without a record. taxis composes layered documents and writes them to targets; its `merge` target mode uses meros. henia manages agent configuration and uses meros for harness settings in its Rust rewrite.

## Decisions

1. Equal values someone else set stay unrecorded (G1). Rejected: recording them, which deletes the user's own value on a later release. taxis requirement 7 ("adopted when already equal") follows this rule.
2. Owned values deleted by another writer conflict (G1). Rejected: silent healing, indistinguishable from undoing a deliberate deletion.
3. Released values someone else edited are kept and reported (G2). Rejected: a conflict, which never converges.
4. Identity is declared per path: `atomic`, `set`, `keyed(field)`, map members, all on day one (G1, G2). Rejected: element identity by value hash for every list.
5. Values compare by canonical parsed value, not text (G2). Rejected: text hashes.
6. Files are edited in place through format adapters; the apply is planned and verified before any write (G1, G4). Rejected: re-serializing whole files.
7. The record is data the caller stores; meros owns no paths, locks or persistence (G5).
8. henia's existing `*.units.json` records are not imported; henia's Rust rewrite starts with no records after a final Go install (G1). Rejected: importing them, which turns adopted values into owned ones.

## Open questions

1. Per-path normalization for values an app rewrites into its own form.
2. Leaf-by-leaf writes of owned compound values, so comments inside them survive.
3. Manager identity in the record, so two tools declaring one path conflict.
4. Per-path force.
5. Exchange-and-verify writes to close the compare-then-rename window.
6. Identity by part of a string (pyproject `dependencies` by package name).
7. A comment-preserving YAML adapter.

## Sources

- Kubernetes: Server-Side Apply and declarative `kubectl apply` docs; KEP-555; kubernetes/kubernetes #99003, #90066, #134902; Kubernetes blog posts on server-side apply (2020, 2022)
- Terraform `lifecycle`, `import` and `removed` blocks; AWS provider `aws_iam_policy_attachment` and `aws_security_group`; Pulumi `ignoreChanges`, `retainOnDelete`; CloudFormation drift detection; Argo CD diffing and sync options; Flux; Helm 3 three-way merge; Crossplane `initProvider`
- Augeas; libelektra; Ansible `ini_file`, `lineinfile`, `blockinfile`; Puppet `file_line`, `inifile`, `resources purge`; Salt `file.serialize`, `file.keyvalue`; chezmoi; home-manager #1800
- Claude Code settings docs and anthropics/claude-code #29395; ruler; rulesync; ai-rulez #185; Dev Container spec; VS Code Settings Sync; mise `dot` merge entries (jdx/mise #14012, #15077); dfm (stevencarpenter/dotfiles-manager #61); microsoft/apm lockfile spec; qwts/managed-machine #294, #295; Web-tree/weaver #110; rulesync #3355; flyingrobots/agent-hub #38; language-operator/coding-runtime #12; mlorentedev/dotfiles #2192; `settings_loader`, `setty`
- `toml_edit`, `serde_json`, `jsonc-parser`, YAML crates (`serde_yaml`, `serde_yml`, `yaml-rust2`, `saphyr`, `yamlpatch`), `json-patch`; RFC 6902, 7386, 8785; `serde_jcs`, `serde_json_canonicalizer`; openai/codex `config/edit.rs`, #14601; astral-sh/uv `uv-project-edit`
- henia `internal/units/units.go`, `specs/draft/harness-config/spec.md`; taxis `docs/research.md`, `docs/merge.md`; phora `docs/template-merge-study.md`
