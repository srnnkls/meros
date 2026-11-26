# meros

*μέρος • a part, share*

> From μείρομαι (meíromai, "to receive as one's portion")
>
> Pronunciation: /ˈme.ros/

## About

meros is a Rust library that lets a tool own declared values inside structured files that the user and other programs also write: `~/.claude/settings.json`, `.codex/config.toml`, `pyproject.toml`, a CI workflow. The tool declares the values it wants; meros compares them with the file's current content and a record of what the tool last wrote, edits the file in place, and returns the new record. It never silently changes a value it does not own.

meros handles TOML, JSON with comments (JSONC) and YAML. It runs on Unix only.

## Concepts

- *Unit* — one position in a document the tool may own: a key's whole value, one element of a list, or one member of a map. Its address is its path, plus `[field=literal]` for a keyed list element or `[=<typed canonical JSON>]` for a set element.
- *Path* — taxis's grammar: TOML bare and quoted keys joined by `.`, and `[field=literal]` selectors with a TOML string, integer or boolean literal. There are no indexes. `[]`, meaning any element or member, appears only in schema paths.
- *Schema* — the identity of each container path in one file. A path without an entry is one atomic unit.
- *Declaration* — the desired value and policy per path for one file, expanded into units through the schema.
- *Record* — what the tool last wrote in one file: a hash scheme and, per unit key, an `Owned` or `Seeded` entry holding the SHA-256 of the written value. It is a plain serde value that the caller stores.
- *Mine* — a unit is mine iff the record has an entry for it and the unit's current value hashes to that entry.
- *Released* — the record entry is dropped and the file keeps the value.

## The ownership rule

A unit is mine only while its value is exactly what the tool last wrote. Values compare by a versioned canonical hash of the parsed value, not by text: SHA-256 over RFC 8785 JSON of a typed projection. JSON `1.0` and `1` are equal; TOML and YAML integers and floats differ, and so do TOML datetimes of different kinds. A value that has no projection (NaN, infinities, integers beyond ±2^53) makes that one unit unhashable.

An absent recorded unit is not mine, so a value another writer deleted is never silently re-added. An unrecorded unit is not mine, even when it equals the declared value, so a value the user set is never adopted and never removed on release. When several elements or members share one identity, none of them is mine, and meros never removes one copy of several.

Units come from the current schema only. A record entry that no current unit matches, such as one left behind when a path switches from atomic to set, is dropped and reported released without touching the file.

## Identity

The schema gives each container path one of four identities:

| Identity       | Units                                                              | Declared as      |
| -------------- | ------------------------------------------------------------------ | ---------------- |
| `Atomic`       | the whole value at the path (the default)                          | any value        |
| `Set`          | each scalar element of a list, by value                            | a list           |
| `Keyed(field)` | each map element of a list, addressed `path[field=literal]`        | a list of maps   |
| `Members`      | each member of a map, addressed `path.name`                        | a map, or `path.name` directly |

A schema entry under `path[]` refines the elements of a keyed list or the members of a map. A refined element is no longer a unit itself: its units are the refined paths plus one atomic unit per other declared field, and its identity field belongs to its address. With `repos` as `Keyed(id)` and `repos[].args` as `Set`, each argument of `repos[id="ruff"]` is its own unit, `repos[id="ruff"].rev` is another, and the element is created with its `id` when its first unit is created and removed when its last unit is removed and only `id` remains.

Content the schema cannot address stays untouched and is never a unit: a keyed element missing its field or holding a non-literal one, a non-map element in a keyed list, a compound or unprojectable element in a set. A schema entry nested inside an atomic unit, or a declaration with overlapping paths, is an error.

## Policies and outcomes

Each declared path carries a policy, and a unit's record entry, when it has one, follows it:

- `Enforce` keeps the declared value while the unit is mine. Its entry is `Owned`.
- `CreateOnly` creates the unit once, when it is absent and unrecorded, and never changes, conflicts on or removes it afterwards. Its entry is `Seeded` and keeps the hash it was recorded with while the unit stays declared, whatever the unit's current value. Switching an `Enforce` unit to `CreateOnly` relabels its entry `Seeded` with the hash kept.

A value the user set stays unrecorded under either policy: one equal to an `Enforce` unit's declared value, and one already present where a `CreateOnly` unit would be created.

Planning gives every unit one outcome:

| Outcome           | When                                                                                                                  |
| ----------------- | --------------------------------------------------------------------------------------------------------------------- |
| `Unchanged`       | an `Enforce` unit already holds the declared value, recorded only while mine; a `CreateOnly` unit is recorded or present |
| `Created`         | a declared unit is absent and unrecorded                                                                              |
| `Replaced`        | an `Enforce` unit is mine and differs from the declared value                                                         |
| `Removed`         | an undeclared unit with an `Owned` entry is still mine                                                                |
| `Released`        | an undeclared unit is recorded but not mine or has a `Seeded` entry, or no current unit matches an entry; the file keeps the value |
| `Conflict(cause)` | the unit cannot be written                                                                                            |
| `Forced(cause)`   | a value conflict written under force                                                                                  |

Conflict causes split in two. Value conflicts are a foreign edit (the unit holds a value that is not mine and differs from the declared one), a foreign deletion (another writer deleted a recorded value), and an inner comment (an owned compound value to replace or remove holds a comment). Structural conflicts are a duplicated identity, a parent holding a non-table value where a table is needed, a container of the wrong kind, and an unhashable current value.

`Force::On` is a planning input that turns value conflicts into writes reported as `Forced(cause)`. Structural conflicts stay conflicts under force.

## Reading, planning, applying

An apply has three steps, and only the first and last touch the filesystem:

1. `read` resolves each target, following symlinks to the real file, reads and parses it, and validates its schema and declaration. A file that fails to parse, two targets naming one real file, or an invalid declaration fails `read` naming the file. A missing file reads as absent; an empty file parses as an empty document.
2. `plan` is a pure function of the snapshot and force. It decides every outcome, conflicts included, produces each new document and re-parses it, and builds each new record. A target whose re-parsed document differs outside its planned units fails verification. `Plan::conflicts` and `Plan::documents` serve checks and diffs.
3. `apply` refuses a plan with an unforced conflict or a failed verification and writes nothing. Otherwise it rechecks every target's bytes against the snapshot before the first rename and again right before each target's own rename, and replaces each file atomically through a temporary file in its directory, keeping its mode. A symlink stays a symlink; the resolved file changes. A missing target is created with its parent directories and only the created units, and only if it is still missing; no file is created when nothing would be written. A target found changed stops the apply there, and the error carries the new records of every target already replaced and of every target whose record changed without a write.

`apply` returns a new record and the outcomes for every target. `Owned` entries are hashed from the written document, or from the read document when nothing was written; `Seeded` entries keep the hash they were recorded with.

## Formats

TOML and JSONC are edited in place: comments and formatting outside the changed units stay. A created unit is appended at the end of its parent in the style of its siblings, with missing parents created the same way; a removed unit's leading comment stays with the next key or in place; a parent emptied by removal stays.

YAML follows the 1.2 core schema, so timestamps are strings. A YAML target with comments, anchors, aliases, tags, duplicate keys or several documents fails `read`. Other YAML targets are rebuilt with only owned units changed in value; indentation and quoting of unowned content may change.

A declaration fails `read` when its format cannot hold a value: null in TOML, a datetime in JSON or YAML.

## The caller's duties

meros never stores records, takes locks or finds configuration. The caller:

- stores the record each apply returns, per target, and passes it back on the next run; a record with a missing or unknown scheme counts as no record and is reported as a `RecordSchemeUnknown` notice;
- serializes runs against the same files, for example with a lock: meros detects a file that changed since it was read, but takes no lock;
- keeps the schema it last applied for each file. Retiring a file is an apply with that schema and an empty declaration: units with an `Owned` entry that are still mine are removed, the rest are released.

Ejecting a unit is dropping its entry from the record.

## Example

A tool adds its MCP server to a JSONC settings file the user also edits, and seeds a theme once:

```rust
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use meros::{
    Declaration, Force, Format, Identity, Outcome, Path, Policy, Record, Schema, Target, Value,
};

/// Plans and applies the tool's declaration, returning every outcome and the new record.
fn sync(
    file: PathBuf,
    record: Record,
) -> Result<(Vec<Outcome>, Record), Box<dyn std::error::Error>> {
    let schema: Schema = [(Path::parse("mcpServers")?, Identity::Members)]
        .into_iter()
        .collect();
    let server =
        Value::Map([("command".to_owned(), Value::String("search-mcp".to_owned()))].into());
    let declaration: Declaration = [
        (Path::parse("mcpServers.search")?, server, Policy::Enforce),
        (
            Path::parse("theme")?,
            Value::String("dark".to_owned()),
            Policy::CreateOnly,
        ),
    ]
    .into_iter()
    .collect();
    let target = Target {
        file,
        format: Format::Json,
        schema,
        declaration,
        record,
    };

    let plan = meros::plan(&meros::read(vec![target])?, Force::Off)?;
    if let Some(conflict) = plan.conflicts().next() {
        return Err(format!("{} conflicts: {:?}", conflict.unit.as_str(), conflict.cause).into());
    }
    let applied = meros::apply(plan)?.targets.remove(0);
    Ok((applied.outcomes.into_values().collect(), applied.record))
}

/// Creates a directory no other run has created.
fn fresh_dir() -> std::io::Result<PathBuf> {
    let mut attempt = 0_u32;
    loop {
        let name = format!("meros-readme-{}-{attempt}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => attempt += 1,
            Err(error) => return Err(error),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = fresh_dir()?;
    let file = dir.join("settings.json");
    let settings = r#"{
  // the user's own server
  "mcpServers": {
    "notes": { "command": "notes-mcp" }
  }
}
"#;
    fs::write(&file, settings)?;

    let (outcomes, record) = sync(file.clone(), Record::default())?;
    assert_eq!(outcomes, [Outcome::Created, Outcome::Created]);
    let written = fs::read_to_string(&file)?;
    assert!(written.contains("// the user's own server") && written.contains("notes-mcp"));
    assert!(written.contains("search-mcp") && written.contains("dark"));

    // The caller stores the record and passes it back on the next run.
    let stored = serde_json::to_string(&record)?;
    let (outcomes, _) = sync(file, serde_json::from_str(&stored)?)?;
    assert_eq!(outcomes, [Outcome::Unchanged, Outcome::Unchanged]);

    fs::remove_dir_all(&dir)?;
    Ok(())
}
```

If the user then edits the server's command, the next plan reports a `ForeignEdit` conflict instead of overwriting it; if the user deletes the theme, it stays deleted.
