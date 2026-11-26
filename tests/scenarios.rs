//! Scenario tests: the acceptance criteria against real TOML, JSONC and YAML files.
#![cfg(test)]

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;

use meros::{
    AppliedTarget, ApplyError, Cause, Conflict, Datetime, Declaration, Entry, Force, Format,
    FormatError, Identity, Outcome, Path, Plan, Policy, ReadError, Record, Schema, Target, UnitKey,
    Value, ValueCause, YamlFeature,
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("meros-scenarios-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn path(text: &str) -> Path {
    Path::parse(text).unwrap()
}

fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

fn list<const N: usize>(values: [Value; N]) -> Value {
    Value::List(values.into())
}

fn map<const N: usize>(members: [(&str, Value); N]) -> Value {
    Value::Map(
        members
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

fn schema<const N: usize>(entries: [(&str, Identity); N]) -> Schema {
    entries
        .into_iter()
        .map(|(at, identity)| (path(at), identity))
        .collect()
}

fn declare<const N: usize>(entries: [(&str, Value, Policy); N]) -> Declaration {
    entries
        .into_iter()
        .map(|(at, value, policy)| (path(at), value, policy))
        .collect()
}

fn enforce<const N: usize>(entries: [(&str, Value); N]) -> Declaration {
    entries
        .into_iter()
        .map(|(at, value)| (path(at), value, Policy::Enforce))
        .collect()
}

fn key(text: &str) -> UnitKey {
    serde_json::from_value(json!(text)).unwrap()
}

/// Hex SHA-256 of a unit's canonical JSON text, written out by hand per TR-1.
fn hash(canonical: &str) -> String {
    Sha256::digest(canonical.as_bytes())
        .iter()
        .fold(String::new(), |mut text, byte| {
            write!(text, "{byte:02x}").unwrap();
            text
        })
}

fn record(entries: &serde_json::Value) -> Record {
    serde_json::from_value(json!({ "scheme": "meros-1", "entries": entries })).unwrap()
}

fn serialized(record: &Record) -> serde_json::Value {
    serde_json::to_value(record).unwrap()
}

fn target(
    file: &std::path::Path,
    format: Format,
    schema: Schema,
    declaration: Declaration,
    record: Record,
) -> Target {
    Target {
        file: file.to_path_buf(),
        format,
        schema,
        declaration,
        record,
    }
}

fn toml(file: &std::path::Path, declaration: Declaration, record: Record) -> Target {
    target(file, Format::Toml, Schema::default(), declaration, record)
}

fn planned(targets: Vec<Target>, force: Force) -> Plan {
    meros::plan(&meros::read(targets).unwrap(), force).unwrap()
}

fn outcomes(plan: &Plan) -> BTreeMap<UnitKey, Outcome> {
    plan.targets().next().unwrap().outcomes().clone()
}

fn applied(target: Target) -> AppliedTarget {
    meros::apply(planned(vec![target], Force::Off))
        .unwrap()
        .targets
        .remove(0)
}

fn bytes(file: &std::path::Path) -> Vec<u8> {
    fs::read(file).unwrap()
}

#[test]
fn ac1_equal_user_value_stays_unrecorded_and_survives_undeclaring() {
    let file = scratch("ac1").join("a.toml");
    fs::write(&file, "name = \"x\"\n").unwrap();

    let plan = planned(
        vec![toml(
            &file,
            enforce([("name", text("x"))]),
            Record::default(),
        )],
        Force::Off,
    );
    assert_eq!(plan.documents().count(), 0, "nothing is written");
    let first = meros::apply(plan).unwrap().targets.remove(0);
    assert_eq!(first.outcomes.get(&key("name")), Some(&Outcome::Unchanged));
    assert!(!first.record.entries.contains_key(&key("name")));

    let second = applied(toml(&file, Declaration::default(), first.record));
    assert_ne!(second.outcomes.get(&key("name")), Some(&Outcome::Removed));
    assert_eq!(bytes(&file), b"name = \"x\"\n");
}

#[test]
fn ac2_owned_value_deleted_by_user_conflicts_and_writes_nothing() {
    let file = scratch("ac2").join("a.toml");
    fs::write(&file, "other = 1\n").unwrap();
    let owned = record(&json!({ "name": { "owned": hash("\"x\"") } }));

    let plan = planned(
        vec![toml(&file, enforce([("name", text("x"))]), owned)],
        Force::Off,
    );
    assert_eq!(
        plan.conflicts().cloned().collect::<Vec<_>>(),
        [Conflict {
            unit: key("name"),
            cause: Cause::Value(ValueCause::ForeignDeletion),
        }]
    );
    assert!(matches!(
        meros::apply(plan),
        Err(ApplyError::Conflicted { count: 1 })
    ));
    assert_eq!(bytes(&file), b"other = 1\n");
}

#[test]
fn ac3_undeclared_owned_value_edited_by_user_is_released_untouched() {
    let file = scratch("ac3").join("a.toml");
    fs::write(&file, "name = \"y\"\n").unwrap();
    let owned = record(&json!({ "name": { "owned": hash("\"x\"") } }));

    let plan = planned(vec![toml(&file, Declaration::default(), owned)], Force::Off);
    assert_eq!(plan.documents().count(), 0);
    let result = meros::apply(plan).unwrap().targets.remove(0);
    assert_eq!(result.outcomes.get(&key("name")), Some(&Outcome::Released));
    assert!(!result.record.entries.contains_key(&key("name")));
    assert_eq!(bytes(&file), b"name = \"y\"\n");
}

#[test]
fn ac4_set_element_edited_by_user_conflicts_instead_of_being_re_added() {
    let file = scratch("ac4").join("a.toml");
    let tools = || schema([("tools", Identity::Set)]);
    let declaration = || enforce([("tools", list([text("a"), text("b")]))]);
    let created = applied(target(
        &file,
        Format::Toml,
        tools(),
        declaration(),
        Record::default(),
    ));
    let edited = String::from_utf8(bytes(&file))
        .unwrap()
        .replace("\"b\"", "\"c\"");
    fs::write(&file, &edited).unwrap();

    let plan = planned(
        vec![target(
            &file,
            Format::Toml,
            tools(),
            declaration(),
            created.record,
        )],
        Force::Off,
    );
    assert_eq!(
        plan.conflicts().cloned().collect::<Vec<_>>(),
        [Conflict {
            unit: key("tools[=\"b\"]"),
            cause: Cause::Value(ValueCause::ForeignDeletion),
        }]
    );
    assert!(meros::apply(plan).is_err());
    assert_eq!(bytes(&file), edited.as_bytes());
}

#[test]
fn ac5_duplicated_set_element_with_one_recorded_keeps_both_when_undeclared() {
    let file = scratch("ac5").join("a.toml");
    let tools = || schema([("tools", Identity::Set)]);
    let created = applied(target(
        &file,
        Format::Toml,
        tools(),
        enforce([("tools", list([text("a")]))]),
        Record::default(),
    ));
    assert!(created.record.entries.contains_key(&key("tools[=\"a\"]")));
    fs::write(&file, "tools = [\"a\", \"a\"]\n").unwrap();

    let plan = planned(
        vec![target(
            &file,
            Format::Toml,
            tools(),
            Declaration::default(),
            created.record,
        )],
        Force::Off,
    );
    assert_eq!(plan.documents().count(), 0, "neither copy is removed");
    assert_ne!(
        outcomes(&plan).get(&key("tools[=\"a\"]")),
        Some(&Outcome::Removed)
    );
    let _ = meros::apply(plan);
    assert_eq!(bytes(&file), b"tools = [\"a\", \"a\"]\n");
}

#[test]
fn ac6_json_number_rewritten_from_float_to_integer_stays_mine() {
    let file = scratch("ac6").join("a.json");
    fs::write(&file, "{}\n").unwrap();
    let json = |record| {
        target(
            &file,
            Format::Json,
            Schema::default(),
            enforce([("n", Value::Float(1.0))]),
            record,
        )
    };
    let created = applied(json(Record::default()));
    let entry = json!({ "owned": hash("1") });
    assert_eq!(serialized(&created.record)["entries"]["n"], entry);
    fs::write(&file, "{ \"n\": 1 }\n").unwrap();

    let plan = planned(vec![json(created.record)], Force::Off);
    assert_eq!(plan.documents().count(), 0);
    let result = meros::apply(plan).unwrap().targets.remove(0);
    assert_eq!(result.outcomes.get(&key("n")), Some(&Outcome::Unchanged));
    assert_eq!(serialized(&result.record)["entries"]["n"], entry);
    assert_eq!(bytes(&file), b"{ \"n\": 1 }\n");
}

#[test]
fn ac7_toml_replace_and_remove_keep_every_comment_outside_the_changed_values() {
    let file = scratch("ac7").join("a.toml");
    fs::write(
        &file,
        "# header\n[tool]\n# above a\na = \"one\" # trailing a\n# above b\nb = \"two\"\n# above c\nc = \"keep\"\n# footer\n",
    )
    .unwrap();
    let owned = record(&json!({
        "tool.a": { "owned": hash("\"one\"") },
        "tool.b": { "owned": hash("\"two\"") },
    }));

    let result = applied(toml(&file, enforce([("tool.a", text("uno"))]), owned));
    assert_eq!(
        result.outcomes.get(&key("tool.a")),
        Some(&Outcome::Replaced)
    );
    assert_eq!(result.outcomes.get(&key("tool.b")), Some(&Outcome::Removed));
    assert_eq!(
        String::from_utf8(bytes(&file)).unwrap(),
        "# header\n[tool]\n# above a\na = \"uno\" # trailing a\n# above b\n# above c\nc = \"keep\"\n# footer\n"
    );
}

#[test]
fn ac8_entry_of_a_path_switched_from_atomic_to_set_is_released() {
    let file = scratch("ac8").join("a.toml");
    let declaration = || enforce([("tools", list([text("a")]))]);
    let atomic = applied(toml(&file, declaration(), Record::default()));
    assert!(atomic.record.entries.contains_key(&key("tools")));
    let written = bytes(&file);

    let plan = planned(
        vec![target(
            &file,
            Format::Toml,
            schema([("tools", Identity::Set)]),
            declaration(),
            atomic.record,
        )],
        Force::Off,
    );
    assert_eq!(plan.documents().count(), 0);
    let result = meros::apply(plan).unwrap().targets.remove(0);
    assert_eq!(result.outcomes.get(&key("tools")), Some(&Outcome::Released));
    assert!(!result.record.entries.contains_key(&key("tools")));
    assert_eq!(bytes(&file), written);
}

#[test]
fn ac9_conflict_in_one_target_leaves_both_targets_byte_identical() {
    let dir = scratch("ac9");
    let (clean, conflicted) = (dir.join("a.toml"), dir.join("b.toml"));
    fs::write(&clean, "x = 1\n").unwrap();
    fs::write(&conflicted, "x = \"theirs\"\n").unwrap();

    let plan = planned(
        vec![
            toml(&clean, enforce([("y", text("v"))]), Record::default()),
            toml(
                &conflicted,
                enforce([("x", text("mine"))]),
                Record::default(),
            ),
        ],
        Force::Off,
    );
    assert!(matches!(
        meros::apply(plan),
        Err(ApplyError::Conflicted { count: 1 })
    ));
    assert_eq!(bytes(&clean), b"x = 1\n");
    assert_eq!(bytes(&conflicted), b"x = \"theirs\"\n");
}

#[test]
fn ac10_target_changed_between_plan_and_write_is_refused() {
    let file = scratch("ac10").join("a.toml");
    fs::write(&file, "x = 1\n").unwrap();
    let plan = planned(
        vec![toml(&file, enforce([("y", text("v"))]), Record::default())],
        Force::Off,
    );
    fs::write(&file, "x = 2\n").unwrap();

    let Err(ApplyError::Changed { file: changed, .. }) = meros::apply(plan) else {
        panic!("a target changed since read stops the apply");
    };
    assert_eq!(changed, file);
    assert_eq!(bytes(&file), b"x = 2\n");
}

#[test]
fn ac11_create_only_unit_deleted_after_seeding_stays_absent() {
    let file = scratch("ac11").join("a.toml");
    fs::write(&file, "other = 1\n").unwrap();
    let seed = || declare([("seed", text("s"), Policy::CreateOnly)]);
    let mut record = applied(toml(&file, seed(), Record::default())).record;
    assert!(matches!(
        record.entries.get(&key("seed")),
        Some(Entry::Seeded { .. })
    ));
    fs::write(&file, "other = 1\n").unwrap();

    for _ in 0..3 {
        let plan = planned(vec![toml(&file, seed(), record)], Force::Off);
        assert_eq!(plan.documents().count(), 0, "the seed is not re-created");
        assert_eq!(plan.conflicts().count(), 0);
        let result = meros::apply(plan).unwrap().targets.remove(0);
        assert!(matches!(
            result.record.entries.get(&key("seed")),
            Some(Entry::Seeded { .. })
        ));
        assert_eq!(bytes(&file), b"other = 1\n");
        record = result.record;
    }
}

#[test]
fn ac12_changed_keyed_element_replaces_only_that_element() {
    let file = scratch("ac12").join("a.toml");
    let black = "[[repos]]\nid = \"black\"\nrev = \"23\"\n";
    fs::write(
        &file,
        format!("[[repos]]\nid = \"ruff\"\nrev = \"v1\"\n\n{black}"),
    )
    .unwrap();
    let owned = record(&json!({
        "repos[id=\"ruff\"]": { "owned": hash("{\"id\":\"ruff\",\"rev\":\"v1\"}") },
    }));

    let result = applied(target(
        &file,
        Format::Toml,
        schema([("repos", Identity::Keyed("id".to_owned()))]),
        enforce([(
            "repos",
            list([map([("id", text("ruff")), ("rev", text("v2"))])]),
        )]),
        owned,
    ));
    let written: Vec<_> = result
        .outcomes
        .iter()
        .filter(|(_, outcome)| **outcome != Outcome::Unchanged)
        .collect();
    assert_eq!(written, [(&key("repos[id=\"ruff\"]"), &Outcome::Replaced)]);
    let after = String::from_utf8(bytes(&file)).unwrap();
    assert!(after.contains("rev = \"v2\""), "{after}");
    assert!(!after.contains("rev = \"v1\""), "{after}");
    assert!(after.ends_with(black), "{after}");
}

#[test]
fn ac13_target_failing_to_parse_fails_read_naming_it() {
    let dir = scratch("ac13");
    let (valid, broken) = (dir.join("a.toml"), dir.join("b.toml"));
    fs::write(&broken, "x = \n").unwrap();

    let error = meros::read(vec![
        toml(&valid, enforce([("y", text("v"))]), Record::default()),
        toml(&broken, enforce([("y", text("v"))]), Record::default()),
    ])
    .unwrap_err();
    let ReadError::Parse { file, .. } = &error else {
        panic!("expected a parse failure, got {error:?}");
    };
    assert_eq!(file, &broken);
    assert!(error.to_string().contains(&broken.display().to_string()));
    assert_eq!(bytes(&broken), b"x = \n");
    assert!(!valid.exists());
}

#[test]
fn ac14_forced_conflict_writes_the_declared_value_naming_the_cause() {
    let file = scratch("ac14").join("a.toml");
    fs::write(&file, "x = \"theirs\"\n").unwrap();

    let plan = planned(
        vec![toml(
            &file,
            enforce([("x", text("mine"))]),
            Record::default(),
        )],
        Force::On,
    );
    assert_eq!(plan.conflicts().count(), 0);
    let result = meros::apply(plan).unwrap().targets.remove(0);
    assert_eq!(
        result.outcomes.get(&key("x")),
        Some(&Outcome::Forced(ValueCause::ForeignEdit))
    );
    assert_eq!(bytes(&file), b"x = \"mine\"\n");
    assert_eq!(
        serialized(&result.record)["entries"]["x"],
        json!({ "owned": hash("\"mine\"") })
    );
}

#[test]
fn ac15_missing_target_with_nothing_to_create_is_not_created() {
    let file = scratch("ac15").join("new").join("a.toml");

    let result = applied(toml(&file, Declaration::default(), Record::default()));
    assert!(result.record.entries.is_empty());
    assert!(!file.exists());
}

#[test]
fn ac16_undeclared_owned_value_unchanged_since_written_is_removed() {
    let file = scratch("ac16").join("a.toml");
    fs::write(&file, "other = 1\n").unwrap();
    let created = applied(toml(
        &file,
        enforce([("name", text("x"))]),
        Record::default(),
    ));
    assert!(created.record.entries.contains_key(&key("name")));

    let result = applied(toml(&file, Declaration::default(), created.record));
    assert_eq!(result.outcomes.get(&key("name")), Some(&Outcome::Removed));
    assert!(!result.record.entries.contains_key(&key("name")));
    assert_eq!(bytes(&file), b"other = 1\n");
}

#[test]
fn ac17_record_of_created_units_serializes_sorted_and_hashes_the_written_values() {
    let file = scratch("ac17").join("a.toml");
    let declaration = || {
        enforce([
            ("zeta", text("z")),
            ("alpha", Value::Integer(1)),
            ("mid.inner", Value::Boolean(true)),
        ])
    };
    let created = applied(toml(&file, declaration(), Record::default()));
    assert_eq!(
        serialized(&created.record),
        json!({
            "scheme": "meros-1",
            "entries": {
                "alpha": { "owned": hash("1") },
                "mid.inner": { "owned": hash("true") },
                "zeta": { "owned": hash("\"z\"") },
            },
        })
    );
    let stored = serde_json::to_string(&created.record).unwrap();
    let position = |name: &str| stored.find(&format!("\"{name}\":{{")).unwrap();
    assert!(position("alpha") < position("mid.inner"), "{stored}");
    assert!(position("mid.inner") < position("zeta"), "{stored}");

    let reread = applied(toml(&file, declaration(), created.record.clone()));
    assert!(
        reread
            .outcomes
            .values()
            .all(|outcome| *outcome == Outcome::Unchanged),
        "{:?}",
        reread.outcomes
    );
    assert_eq!(reread.record, created.record);
}

#[test]
fn ac18_declared_key_under_a_scalar_parent_conflicts_even_under_force() {
    let file = scratch("ac18").join("a.toml");
    fs::write(&file, "a = \"x\"\n").unwrap();

    let plan = planned(
        vec![toml(
            &file,
            enforce([("a.b", text("v"))]),
            Record::default(),
        )],
        Force::On,
    );
    assert_eq!(
        plan.conflicts().cloned().collect::<Vec<_>>(),
        [Conflict {
            unit: key("a.b"),
            cause: Cause::ScalarParent,
        }]
    );
    assert!(matches!(
        meros::apply(plan),
        Err(ApplyError::Conflicted { count: 1 })
    ));
    assert_eq!(bytes(&file), b"a = \"x\"\n");
}

#[test]
fn ac19_planning_the_same_inputs_twice_is_byte_identical() {
    let file = scratch("ac19").join("a.toml");
    fs::write(
        &file,
        "tools = [\"c\", \"a\"]\n\n[servers.user]\ncommand = \"u\"\n",
    )
    .unwrap();
    let inputs = || {
        target(
            &file,
            Format::Toml,
            schema([("tools", Identity::Set), ("servers", Identity::Members)]),
            enforce([
                ("tools", list([text("b"), text("d"), text("e")])),
                ("servers.zed", map([("command", text("z"))])),
                ("servers.mine", map([("command", text("m"))])),
                ("plain", text("p")),
            ]),
            record(&json!({ "gone": { "owned": hash("\"g\"") } })),
        )
    };
    let snapshot = meros::read(vec![inputs()]).unwrap();
    let first = meros::plan(&snapshot, Force::Off).unwrap();
    let second = meros::plan(&snapshot, Force::Off).unwrap();
    let reread = meros::plan(&meros::read(vec![inputs()]).unwrap(), Force::Off).unwrap();

    let documents = |plan: &Plan| -> Vec<Vec<u8>> {
        plan.documents().map(|(_, bytes)| bytes.to_vec()).collect()
    };
    assert_eq!(documents(&first), documents(&second));
    assert_eq!(documents(&first), documents(&reread));
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    assert_eq!(format!("{first:?}"), format!("{reread:?}"));
}

#[test]
fn ac20_declared_member_is_written_beside_an_untouched_user_member() {
    let file = scratch("ac20").join("settings.json");
    let user = "\"user\": { \"command\": \"u\" }";
    fs::write(
        &file,
        format!("{{\n  // user servers\n  \"mcpServers\": {{\n    {user}\n  }}\n}}\n"),
    )
    .unwrap();

    let result = applied(target(
        &file,
        Format::Json,
        schema([("mcpServers", Identity::Members)]),
        enforce([("mcpServers", map([("mine", map([("command", text("m"))]))]))]),
        Record::default(),
    ));
    assert_eq!(
        result.outcomes.get(&key("mcpServers.mine")),
        Some(&Outcome::Created)
    );
    assert!(!result.record.entries.contains_key(&key("mcpServers.user")));
    let after = String::from_utf8(bytes(&file)).unwrap();
    assert!(after.contains("// user servers"), "{after}");
    assert!(after.contains(user), "{after}");
    assert!(after.contains("\"mine\""), "{after}");
    assert!(after.contains("\"command\": \"m\""), "{after}");
}

#[test]
fn ac21_yaml_target_with_a_comment_fails_read_naming_the_file() {
    let file = scratch("ac21").join("a.yaml");
    fs::write(&file, "# note\nkey: value\n").unwrap();

    let error = meros::read(vec![target(
        &file,
        Format::Yaml,
        Schema::default(),
        enforce([("other", text("v"))]),
        Record::default(),
    )])
    .unwrap_err();
    let ReadError::Parse {
        file: named,
        source,
    } = &error
    else {
        panic!("expected a parse failure, got {error:?}");
    };
    assert_eq!(named, &file);
    assert!(matches!(
        source,
        FormatError::UnsupportedYaml(YamlFeature::Comment)
    ));
    assert!(error.to_string().contains(&file.display().to_string()));
    assert_eq!(bytes(&file), b"# note\nkey: value\n");
}

#[test]
fn ac22_second_target_changed_after_reading_stops_the_apply_before_any_write() {
    let dir = scratch("ac22");
    let (first, second) = (dir.join("a.toml"), dir.join("b.toml"));
    fs::write(&first, "x = 1\n").unwrap();
    fs::write(&second, "x = 1\n").unwrap();
    let plan = planned(
        vec![
            toml(&first, enforce([("y", text("v"))]), Record::default()),
            toml(&second, enforce([("y", text("v"))]), Record::default()),
        ],
        Force::Off,
    );
    fs::write(&second, "x = 2\n").unwrap();

    let Err(ApplyError::Changed { file, .. }) = meros::apply(plan) else {
        panic!("a target changed since read stops the apply");
    };
    assert_eq!(file, second);
    assert_eq!(bytes(&first), b"x = 1\n");
    assert_eq!(bytes(&second), b"x = 2\n");
}

#[test]
fn ac23_retired_target_removes_owned_units_and_releases_edited_ones() {
    let file = scratch("ac23").join("a.toml");
    fs::write(&file, "mine = \"m\"\nedited = \"user\"\n").unwrap();
    let owned = record(&json!({
        "mine": { "owned": hash("\"m\"") },
        "edited": { "owned": hash("\"orig\"") },
    }));

    let result = applied(toml(&file, Declaration::default(), owned));
    assert_eq!(result.outcomes.get(&key("mine")), Some(&Outcome::Removed));
    assert_eq!(
        result.outcomes.get(&key("edited")),
        Some(&Outcome::Released)
    );
    assert!(result.record.entries.is_empty());
    assert_eq!(bytes(&file), b"edited = \"user\"\n");
}

#[test]
fn ac24_symlinked_target_edits_the_resolved_file_keeping_link_and_mode() {
    let dir = scratch("ac24");
    fs::create_dir_all(dir.join("real")).unwrap();
    let (real, link) = (dir.join("real").join("a.toml"), dir.join("a.toml"));
    fs::write(&real, "x = 1\n").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&real, &link).unwrap();

    let result = applied(toml(&link, enforce([("y", text("v"))]), Record::default()));
    assert_eq!(result.outcomes.get(&key("y")), Some(&Outcome::Created));
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&link).unwrap(), real);
    assert_eq!(bytes(&real), b"x = 1\ny = \"v\"\n");
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn ac25_declared_unit_holding_a_foreign_value_lists_a_foreign_edit_conflict() {
    let file = scratch("ac25").join("a.toml");
    fs::write(&file, "x = \"theirs\"\n").unwrap();

    let plan = planned(
        vec![toml(
            &file,
            enforce([("x", text("mine"))]),
            Record::default(),
        )],
        Force::Off,
    );
    let conflict = Conflict {
        unit: key("x"),
        cause: Cause::Value(ValueCause::ForeignEdit),
    };
    assert_eq!(plan.conflicts().collect::<Vec<_>>(), [&conflict]);
    assert_eq!(
        outcomes(&plan).get(&key("x")),
        Some(&Outcome::Conflict(conflict))
    );
    assert!(matches!(
        meros::apply(plan),
        Err(ApplyError::Conflicted { count: 1 })
    ));
    assert_eq!(bytes(&file), b"x = \"theirs\"\n");
}

#[test]
fn ac26_release_only_target_writes_nothing_and_drops_the_entry() {
    let file = scratch("ac26").join("a.toml");
    fs::write(&file, "other = 1\n").unwrap();
    let owned = record(&json!({ "gone": { "owned": hash("\"g\"") } }));

    let plan = planned(vec![toml(&file, Declaration::default(), owned)], Force::Off);
    assert_eq!(plan.documents().count(), 0);
    let result = meros::apply(plan).unwrap().targets.remove(0);
    assert_eq!(
        result.outcomes.values().collect::<Vec<_>>(),
        [&Outcome::Released]
    );
    assert!(result.record.entries.is_empty());
    assert_eq!(bytes(&file), b"other = 1\n");
}

#[test]
fn ac27_refined_keyed_element_left_with_only_its_identity_is_removed() {
    let file = scratch("ac27").join("a.toml");
    let black = "[[repos]]\nid = \"black\"\n";
    fs::write(
        &file,
        format!("[[repos]]\nid = \"ruff\"\nargs = [\"--fix\"]\n\n{black}"),
    )
    .unwrap();
    let owned = record(&json!({
        "repos[id=\"ruff\"].args[=\"--fix\"]": { "owned": hash("\"--fix\"") },
    }));

    let result = applied(target(
        &file,
        Format::Toml,
        schema([
            ("repos", Identity::Keyed("id".to_owned())),
            ("repos[].args", Identity::Set),
        ]),
        Declaration::default(),
        owned,
    ));
    assert_eq!(
        result
            .outcomes
            .get(&key("repos[id=\"ruff\"].args[=\"--fix\"]")),
        Some(&Outcome::Removed)
    );
    let after = String::from_utf8(bytes(&file)).unwrap();
    assert!(!after.contains("ruff"), "{after}");
    assert!(!after.contains("args"), "{after}");
    assert!(after.ends_with(black), "{after}");
}

#[test]
fn ac28_toml_datetime_kinds_and_integer_versus_float_hash_apart() {
    let file = scratch("ac28").join("a.toml");
    let datetime = |text: &str| Value::Datetime(Datetime::parse(text).unwrap());

    let result = applied(target(
        &file,
        Format::Toml,
        schema([("ports", Identity::Set)]),
        enforce([
            ("date", datetime("1979-05-27")),
            ("local", datetime("1979-05-27T07:32:00")),
            ("offset", datetime("1979-05-27T07:32:00Z")),
            ("ports", list([Value::Integer(1), Value::Float(1.0)])),
        ]),
        Record::default(),
    ));
    let entries = &serialized(&result.record)["entries"];
    assert_eq!(
        entries["local"],
        json!({ "owned": hash("{\"$datetime\":\"local-datetime:1979-05-27T07:32:00\"}") })
    );
    assert_eq!(
        entries["offset"],
        json!({ "owned": hash("{\"$datetime\":\"offset:1979-05-27T07:32:00Z\"}") })
    );
    assert_eq!(
        entries["date"],
        json!({ "owned": hash("{\"$datetime\":\"local-date:1979-05-27\"}") })
    );
    assert_ne!(entries["local"], entries["offset"]);
    assert_ne!(entries["date"], entries["offset"]);
    let ports: Vec<_> = result
        .record
        .entries
        .keys()
        .filter(|unit| unit.as_str().starts_with("ports"))
        .map(UnitKey::as_str)
        .collect();
    assert_eq!(ports, ["ports[=1]", "ports[={\"$float\":\"1\"}]"]);
}

#[test]
fn ac29_seeded_unit_at_its_seed_value_is_released_when_undeclared() {
    let file = scratch("ac29").join("a.toml");
    fs::write(&file, "other = 1\n").unwrap();
    let seeded = applied(toml(
        &file,
        declare([("seed", text("s"), Policy::CreateOnly)]),
        Record::default(),
    ));
    let written = bytes(&file);
    assert!(
        String::from_utf8(written.clone())
            .unwrap()
            .contains("seed = \"s\"")
    );

    let result = applied(toml(&file, Declaration::default(), seeded.record));
    assert_eq!(result.outcomes.get(&key("seed")), Some(&Outcome::Released));
    assert!(!result.record.entries.contains_key(&key("seed")));
    assert_eq!(bytes(&file), written);
}

#[test]
fn ac30_refined_keyed_element_writes_rev_as_its_own_unit_that_conflicts_when_edited() {
    let file = scratch("ac30").join("a.toml");
    let repos = || {
        schema([
            ("repos", Identity::Keyed("id".to_owned())),
            ("repos[].args", Identity::Set),
        ])
    };
    let declaration = || {
        enforce([(
            "repos",
            list([map([
                ("id", text("ruff")),
                ("rev", text("v1")),
                ("args", list([text("--fix")])),
            ])]),
        )])
    };
    let rev = key("repos[id=\"ruff\"].rev");
    let created = applied(target(
        &file,
        Format::Toml,
        repos(),
        declaration(),
        Record::default(),
    ));
    assert_eq!(created.outcomes.get(&rev), Some(&Outcome::Created));
    assert_eq!(
        serialized(&created.record)["entries"][rev.as_str()],
        json!({ "owned": hash("\"v1\"") })
    );
    let edited = String::from_utf8(bytes(&file))
        .unwrap()
        .replace("\"v1\"", "\"v9\"");
    fs::write(&file, &edited).unwrap();

    let plan = planned(
        vec![target(
            &file,
            Format::Toml,
            repos(),
            declaration(),
            created.record,
        )],
        Force::Off,
    );
    assert_eq!(
        plan.conflicts().cloned().collect::<Vec<_>>(),
        [Conflict {
            unit: rev,
            cause: Cause::Value(ValueCause::ForeignEdit),
        }]
    );
    assert!(meros::apply(plan).is_err());
    assert_eq!(bytes(&file), edited.as_bytes());
}

#[test]
fn ac31_target_created_by_another_writer_after_read_stops_the_apply() {
    let file = scratch("ac31").join("a.toml");
    let plan = planned(
        vec![toml(&file, enforce([("y", text("v"))]), Record::default())],
        Force::Off,
    );
    fs::write(&file, "").unwrap();

    let Err(ApplyError::Changed { file: changed, .. }) = meros::apply(plan) else {
        panic!("a target created since read stops the apply");
    };
    assert_eq!(changed, file);
    assert_eq!(bytes(&file), b"");
}

#[test]
fn ac32_enforce_unit_switched_to_create_only_is_relabelled_seeded_then_released() {
    let file = scratch("ac32").join("a.toml");
    fs::write(&file, "other = 1\n").unwrap();
    let owned = applied(toml(
        &file,
        enforce([("name", text("v"))]),
        Record::default(),
    ));
    let hash = hash("\"v\"");
    assert_eq!(
        serialized(&owned.record)["entries"]["name"],
        json!({ "owned": hash })
    );
    let written = bytes(&file);

    let seeded = applied(toml(
        &file,
        declare([("name", text("v"), Policy::CreateOnly)]),
        owned.record,
    ));
    assert_eq!(
        serialized(&seeded.record)["entries"]["name"],
        json!({ "seeded": hash })
    );
    assert_eq!(bytes(&file), written);

    let released = applied(toml(&file, Declaration::default(), seeded.record));
    assert_eq!(
        released.outcomes.get(&key("name")),
        Some(&Outcome::Released)
    );
    assert!(!released.record.entries.contains_key(&key("name")));
    assert_eq!(bytes(&file), written);
}
