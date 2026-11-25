use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::Permissions;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use crate::error::ApplyError;
use crate::fs::{Fs, Real};
use crate::plan::{Notice, Outcome, Plan, TargetPlan, Verification};
use crate::record::Record;
use crate::unit::UnitKey;
use crate::{FileState, TargetSnapshot};

/// The result of an apply.
#[derive(Debug)]
pub struct Applied {
    /// Every target in plan order.
    pub targets: Vec<AppliedTarget>,
}

/// A settled target: its new record, also when its record changed without a write.
#[derive(Debug)]
pub struct AppliedTarget {
    /// The target's file as declared.
    pub file: PathBuf,
    /// The record for the caller to store.
    pub record: Record,
    /// Every unit's outcome.
    pub outcomes: BTreeMap<UnitKey, Outcome>,
    /// The target's non-blocking reports.
    pub notices: Vec<Notice>,
}

/// Writes every changed target atomically after rechecking that none changed since read.
///
/// # Errors
///
/// Refuses a plan with an unforced conflict or a failed verification before any write;
/// stops at the first target found changed or failing to write, carrying the records of
/// the targets already replaced and of every target without a write.
pub fn apply(plan: Plan) -> Result<Applied, ApplyError> {
    apply_with(plan, &mut Real)
}

pub(crate) fn apply_with(plan: Plan, fs: &mut impl Fs) -> Result<Applied, ApplyError> {
    let count = plan.conflicts().count();
    if count > 0 {
        return Err(ApplyError::Conflicted { count });
    }
    if let Some(failed) = plan
        .targets
        .iter()
        .find(|target| target.verification == Verification::Failed)
    {
        return Err(ApplyError::Unverified {
            file: failed.read.target.file.clone(),
        });
    }
    let stale = plan.targets.iter().find_map(|target| {
        let stop = recheck(fs, &target.read).err()?;
        Some((stop, target.read.target.file.clone()))
    });
    if let Some((stop, file)) = stale {
        return Err(stop.into_error(file, unwritten(plan.targets).collect()));
    }

    let mut settled = Vec::with_capacity(plan.targets.len());
    let mut targets = plan.targets.into_iter();
    while let Some(target) = targets.next() {
        if let Some(bytes) = &target.write
            && let Err(stop) = write(fs, &target.read, bytes)
        {
            settled.extend(unwritten(targets));
            return Err(stop.into_error(target.read.target.file, settled));
        }
        settled.push(settle(target));
    }
    Ok(Applied { targets: settled })
}

fn unwritten(targets: impl IntoIterator<Item = TargetPlan>) -> impl Iterator<Item = AppliedTarget> {
    targets
        .into_iter()
        .filter(|target| target.write.is_none())
        .map(settle)
}

fn settle(target: TargetPlan) -> AppliedTarget {
    AppliedTarget {
        file: target.read.target.file,
        record: target.record,
        outcomes: target.outcomes,
        notices: target.notices,
    }
}

enum Stop {
    Changed,
    Io(io::Error),
}

impl From<io::Error> for Stop {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl Stop {
    fn into_error(self, file: PathBuf, settled: Vec<AppliedTarget>) -> ApplyError {
        match self {
            Self::Changed => ApplyError::Changed { file, settled },
            Self::Io(source) => ApplyError::Io {
                file,
                source,
                settled,
            },
        }
    }
}

fn recheck(fs: &mut impl Fs, snapshot: &TargetSnapshot) -> Result<(), Stop> {
    let fresh = match (&snapshot.file, fs.read(&snapshot.real)?) {
        (FileState::Absent, None) => true,
        (FileState::Present { bytes, .. }, Some(current)) => *bytes == current,
        _ => false,
    };
    if fresh { Ok(()) } else { Err(Stop::Changed) }
}

fn write(fs: &mut impl Fs, snapshot: &TargetSnapshot, bytes: &[u8]) -> Result<(), Stop> {
    let real = snapshot.real.as_path();
    let dir = real.parent().unwrap_or(Path::new(""));
    if matches!(snapshot.file, FileState::Absent) {
        fs.create_dir_all(dir)?;
    }
    let (temp, permissions) = temporary(fs, real, dir, |fs, temp| {
        let permissions = match &snapshot.file {
            FileState::Present { permissions, .. } => permissions.clone(),
            FileState::Absent => fs.default_permissions(temp)?,
        };
        fs.write_new(temp, bytes).map(|()| permissions)
    })?;
    let placed = place(fs, snapshot, &temp, permissions);
    if placed.is_err() || matches!(snapshot.file, FileState::Absent) {
        let _ = fs.remove_file(&temp);
    }
    placed
}

fn place(
    fs: &mut impl Fs,
    snapshot: &TargetSnapshot,
    temp: &Path,
    permissions: Permissions,
) -> Result<(), Stop> {
    fs.set_permissions(temp, permissions)?;
    recheck(fs, snapshot)?;
    match &snapshot.file {
        FileState::Present { .. } => fs.rename(temp, &snapshot.real)?,
        FileState::Absent => match fs.hard_link(temp, &snapshot.real) {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return Err(Stop::Changed);
            }
            linked => linked?,
        },
    }
    Ok(())
}

/// Runs `create` on fresh hidden names beside `real` until one does not exist yet.
fn temporary<F: Fs, T>(
    fs: &mut F,
    real: &Path,
    dir: &Path,
    mut create: impl FnMut(&mut F, &Path) -> io::Result<T>,
) -> io::Result<(PathBuf, T)> {
    let name = real.file_name().unwrap_or_default();
    for attempt in 0u32.. {
        let mut temp = OsString::from(".");
        temp.push(name);
        temp.push(format!(".{}.{attempt}.meros", std::process::id()));
        let temp = dir.join(temp);
        match create(fs, &temp) {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            created => return created.map(|value| (temp, value)),
        }
    }
    Err(io::Error::from(ErrorKind::AlreadyExists))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    use super::{apply, apply_with};
    use crate::error::{ApplyError, ReadError};
    use crate::format::Format;
    use crate::fs::{Fs, Real};
    use crate::path::Path;
    use crate::plan::{Force, Outcome, Plan, plan};
    use crate::record::{Entry, Record};
    use crate::schema::Schema;
    use crate::unit::{Policy, UnitKey};
    use crate::value::{Value, canonical_hash};
    use crate::{Target, read};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meros-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key(text: &str) -> UnitKey {
        serde_json::from_value(serde_json::Value::String(text.to_owned())).unwrap()
    }

    fn owned(text: &str) -> Entry {
        Entry::Owned {
            hash: canonical_hash(&Value::String(text.to_owned()), Format::Toml).unwrap(),
        }
    }

    fn record<const N: usize>(entries: [(&str, Entry); N]) -> Record {
        Record {
            entries: entries
                .into_iter()
                .map(|(text, entry)| (key(text), entry))
                .collect(),
            ..Record::default()
        }
    }

    fn target(file: PathBuf, declared: &[(&str, &str)], record: Record) -> Target {
        Target {
            file,
            format: Format::Toml,
            schema: Schema::default(),
            declaration: declared
                .iter()
                .map(|(path, text)| {
                    let value = Value::String((*text).to_owned());
                    (Path::parse(path).unwrap(), value, Policy::Enforce)
                })
                .collect(),
            record,
        }
    }

    fn planned(targets: Vec<Target>) -> Plan {
        plan(&read(targets).unwrap(), Force::Off).unwrap()
    }

    #[test]
    fn apply_writes_nothing_for_a_conflicted_plan_or_a_target_changed_since_read() {
        let dir = scratch("refuse");
        let (first, second, absent) = (dir.join("a.toml"), dir.join("b.toml"), dir.join("c.toml"));
        fs::write(&first, "x = 1\n").unwrap();
        fs::write(&second, "k = \"user\"\n").unwrap();
        let conflicted = planned(vec![
            target(first.clone(), &[("k", "new")], Record::default()),
            target(second.clone(), &[("k", "new")], Record::default()),
        ]);
        assert!(matches!(
            apply(conflicted),
            Err(ApplyError::Conflicted { count: 1 })
        ));
        assert_eq!(fs::read(&first).unwrap(), b"x = 1\n");
        assert_eq!(fs::read(&second).unwrap(), b"k = \"user\"\n");

        let stale = planned(vec![
            target(first.clone(), &[("k", "new")], Record::default()),
            target(second.clone(), &[("other", "new")], Record::default()),
        ]);
        fs::write(&second, "k = \"later\"\n").unwrap();
        let Err(ApplyError::Changed { file, settled }) = apply(stale) else {
            panic!("a target changed after read stops the apply");
        };
        assert_eq!(file, second);
        assert!(settled.is_empty());
        assert_eq!(fs::read(&first).unwrap(), b"x = 1\n");
        assert_eq!(fs::read(&second).unwrap(), b"k = \"later\"\n");

        let raced = planned(vec![target(
            absent.clone(),
            &[("k", "new")],
            Record::default(),
        )]);
        fs::write(&absent, "").unwrap();
        let Err(ApplyError::Changed { file, .. }) = apply(raced) else {
            panic!("a target created by another writer after read stops the apply");
        };
        assert_eq!(file, absent);
        assert_eq!(fs::read(&absent).unwrap(), b"");
    }

    #[test]
    fn apply_edits_the_real_file_of_a_symlink_and_returns_a_record_per_target() {
        let dir = scratch("symlink");
        let (real, link) = (dir.join("real.toml"), dir.join("link.toml"));
        let (nested, untouched) = (dir.join("new/deeper/n.toml"), dir.join("none.toml"));
        fs::write(&real, "# keep\nk = \"old\"\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&real, &link).unwrap();
        assert!(matches!(
            read(vec![
                target(link.clone(), &[], Record::default()),
                target(real.clone(), &[], Record::default()),
            ]),
            Err(ReadError::SameFile { .. })
        ));

        let plan = planned(vec![
            target(link.clone(), &[("added", "v")], Record::default()),
            target(nested.clone(), &[("k", "v")], Record::default()),
            target(untouched.clone(), &[], record([("gone", owned("v"))])),
        ]);
        let documents: Vec<(PathBuf, Vec<u8>)> = plan
            .documents()
            .map(|(file, bytes)| (file.to_path_buf(), bytes.to_vec()))
            .collect();
        assert_eq!(documents.len(), 2);
        let applied = apply(plan).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_link(&link).unwrap(), real);
        assert_eq!(fs::read(&real).unwrap(), documents[0].1);
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read(&nested).unwrap(), documents[1].1);
        assert!(!untouched.exists());

        let settled: Vec<_> = applied
            .targets
            .iter()
            .map(|applied| (applied.file.clone(), applied.record.clone()))
            .collect();
        assert_eq!(
            settled,
            [
                (link, record([("added", owned("v"))])),
                (nested, record([("k", owned("v"))])),
                (untouched, record([])),
            ]
        );
        assert_eq!(
            applied.targets[2].outcomes.get(&key("gone")),
            Some(&Outcome::Released)
        );
    }

    struct Intruder {
        file: PathBuf,
        renamed: bool,
    }

    impl Fs for Intruder {
        fn rename(&mut self, from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
            Real.rename(from, to)?;
            if !self.renamed {
                self.renamed = true;
                fs::write(&self.file, "k = \"foreign\"\n")?;
            }
            Ok(())
        }
    }

    #[test]
    fn apply_stops_before_a_target_changed_after_an_earlier_rename() {
        let dir = scratch("seam");
        let (first, second) = (dir.join("a.toml"), dir.join("b.toml"));
        fs::write(&first, "x = 1\n").unwrap();
        fs::write(&second, "x = 1\n").unwrap();
        let plan = planned(vec![
            target(first.clone(), &[("k", "new")], Record::default()),
            target(second.clone(), &[("k", "new")], Record::default()),
        ]);
        let (_, written) = plan.documents().next().unwrap();
        let written = written.to_vec();
        let mut intruder = Intruder {
            file: second.clone(),
            renamed: false,
        };

        let Err(ApplyError::Changed { file, settled }) = apply_with(plan, &mut intruder) else {
            panic!("a target changed between renames stops the apply before it");
        };
        assert_eq!(file, second);
        assert_eq!(fs::read(&first).unwrap(), written);
        assert_eq!(fs::read(&second).unwrap(), b"k = \"foreign\"\n");
        let settled: Vec<_> = settled
            .into_iter()
            .map(|applied| (applied.file, applied.record))
            .collect();
        assert_eq!(settled, [(first, record([("k", owned("new"))]))]);
    }

    #[test]
    fn apply_stopped_mid_way_carries_the_records_of_later_targets_without_a_write() {
        let dir = scratch("remaining");
        let (first, second, third) = (dir.join("a.toml"), dir.join("b.toml"), dir.join("c.toml"));
        fs::write(&first, "x = 1\n").unwrap();
        fs::write(&second, "x = 1\n").unwrap();
        let plan = planned(vec![
            target(first.clone(), &[("k", "new")], Record::default()),
            target(second.clone(), &[("k", "new")], Record::default()),
            target(third.clone(), &[], record([("gone", owned("v"))])),
        ]);
        let mut intruder = Intruder {
            file: second.clone(),
            renamed: false,
        };

        let Err(ApplyError::Changed { file, settled }) = apply_with(plan, &mut intruder) else {
            panic!("a target changed between renames stops the apply before it");
        };
        assert_eq!(file, second);
        let settled: Vec<_> = settled
            .into_iter()
            .map(|applied| (applied.file, applied.record))
            .collect();
        assert_eq!(
            settled,
            [(first, record([("k", owned("new"))])), (third, record([])),]
        );
    }

    #[test]
    fn apply_stopped_before_any_write_carries_the_records_of_targets_without_a_write() {
        let dir = scratch("unwritten");
        let (first, released) = (dir.join("a.toml"), dir.join("b.toml"));
        fs::write(&first, "x = 1\n").unwrap();
        let plan = planned(vec![
            target(first.clone(), &[("k", "new")], Record::default()),
            target(released.clone(), &[], record([("k", owned("v"))])),
        ]);
        fs::write(&first, "x = 2\n").unwrap();

        let Err(ApplyError::Changed { file, settled }) = apply(plan) else {
            panic!("a target changed after read stops the apply");
        };
        assert_eq!(file, first);
        assert_eq!(fs::read(&first).unwrap(), b"x = 2\n");
        let settled: Vec<_> = settled
            .into_iter()
            .map(|applied| (applied.file, applied.record, applied.outcomes))
            .collect();
        assert_eq!(
            settled,
            [(
                released,
                record([]),
                [(key("k"), Outcome::Released)].into_iter().collect()
            )]
        );
    }

    #[test]
    fn read_resolves_absent_targets_through_symlinks_and_rejects_hard_links_to_one_file() {
        let dir = scratch("identity");
        fs::create_dir(dir.join("dir")).unwrap();
        let (dangling, missing) = (dir.join("dangling.toml"), dir.join("dir/missing.toml"));
        symlink("dir/missing.toml", &dangling).unwrap();
        let applied = apply(planned(vec![target(
            dangling.clone(),
            &[("k", "v")],
            Record::default(),
        )]))
        .unwrap();
        assert_eq!(applied.targets[0].record, record([("k", owned("v"))]));
        assert_eq!(fs::read(&missing).unwrap(), b"k = \"v\"\n");
        assert!(
            fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        symlink(dir.join("dir"), dir.join("alias")).unwrap();
        assert!(matches!(
            read(vec![
                target(dir.join("dir/x.toml"), &[("k", "v")], Record::default()),
                target(dir.join("alias/x.toml"), &[("k", "w")], Record::default()),
            ]),
            Err(ReadError::SameFile { .. })
        ));

        let (linked, other) = (dir.join("linked.toml"), dir.join("other.toml"));
        fs::write(&linked, "x = 1\n").unwrap();
        fs::hard_link(&linked, &other).unwrap();
        assert!(matches!(
            read(vec![
                target(linked, &[("k", "v")], Record::default()),
                target(other, &[("k", "w")], Record::default()),
            ]),
            Err(ReadError::SameFile { .. })
        ));
    }
}
