use meros::{
    Applied, ApplyError, Conflict, Declaration, Force, Format, Identity, Notice, Outcome, Path,
    PathError, Plan, PlanError, Policy, ReadError, Record, Schema, Snapshot, Target, Unit, UnitKey,
    Value,
};

type Document<'plan> = (&'plan std::path::Path, &'plan [u8]);

#[test]
fn domain_enums_have_exactly_the_contract_variants() {
    fn identity_name(identity: &Identity) -> &'static str {
        match identity {
            Identity::Atomic => "atomic",
            Identity::Set => "set",
            Identity::Keyed(_) => "keyed",
            Identity::Members => "members",
        }
    }

    fn policy_name(policy: &Policy) -> &'static str {
        match policy {
            Policy::Enforce => "enforce",
            Policy::CreateOnly => "create-only",
        }
    }

    fn outcome_name(outcome: &Outcome) -> &'static str {
        match outcome {
            Outcome::Unchanged => "unchanged",
            Outcome::Created => "created",
            Outcome::Replaced => "replaced",
            Outcome::Removed => "removed",
            Outcome::Released => "released",
            Outcome::Forced(_) => "forced",
            Outcome::Conflict(_) => "conflict",
        }
    }

    fn notice_name(notice: &Notice) -> &'static str {
        match notice {
            Notice::RecordSchemeUnknown => "record-scheme-unknown",
        }
    }

    let _: fn(&Identity) -> &'static str = identity_name;
    let _: fn(&Policy) -> &'static str = policy_name;
    let _: fn(&Outcome) -> &'static str = outcome_name;
    let _: fn(&Notice) -> &'static str = notice_name;
}

#[test]
fn boundary_functions_have_the_contract_signatures() {
    fn target_fields(target: &Target) {
        let Target {
            file,
            format,
            schema,
            declaration,
            record: _,
        } = target;
        let _: &std::path::Path = file.as_ref();
        let _: &Format = format;
        let _: &Schema = schema;
        let _: &Declaration = declaration;
    }

    fn plan_documents(plan: &Plan) -> Vec<Document<'_>> {
        plan.documents().collect()
    }

    fn plan_conflicts(plan: &Plan) -> Vec<&Conflict> {
        plan.conflicts().collect()
    }

    fn boundary_error<E: std::error::Error>() {}

    let _: fn(&Target) = target_fields;
    let _: fn(&Plan) -> Vec<Document<'_>> = plan_documents;
    let _: fn(&Plan) -> Vec<&Conflict> = plan_conflicts;

    let _: fn(&str) -> Result<Path, PathError> = Path::parse;
    let _: fn(Vec<Target>) -> Result<Snapshot, ReadError> = meros::read;
    let _: fn(&Snapshot, Force) -> Result<Plan, PlanError> = meros::plan;
    let _: fn(Plan) -> Result<Applied, ApplyError> = meros::apply;

    boundary_error::<PathError>();
    boundary_error::<ReadError>();
    boundary_error::<PlanError>();
    boundary_error::<ApplyError>();

    let _: Option<(&UnitKey, &Unit, &Value, &Record)> = None;
}
