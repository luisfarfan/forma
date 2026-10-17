//! Approval: the gate every call that changes a document, writes a file or
//! discards unsaved changes passes (`approval_gate`), and the `plan_commit`
//! and `plan_discard` tools.
//!
//! # The gate
//!
//! A call **needs approval** when it is destructive or massive (it touches
//! more than [`MASS_THRESHOLD`] distinct glyphs, kerning pairs and kerning
//! groups, counted together; see `is_affected`). Destructive are: deleting
//! glyphs; replacing a drawn layer of a glyph that existed before the call;
//! removing the last contour of a layer that was drawn when the call started;
//! a singular contour transform; a revert that deletes a glyph or replaces or
//! empties a drawn layer; restoring an earlier revision; overwriting an
//! existing file other than the document's own; closing a document with
//! unsaved changes, discarding them. Then, by [`Policy`]:
//!
//! * `prompted` (the default): `APPROVAL_REQUIRED`, with a plan in
//!   `details.plan`;
//! * `auto`: a massive call that destroys nothing runs; a destructive one
//!   needs a plan as with `prompted`;
//! * `strict`: `UNSUPPORTED` (the FORMA app approves them, Milestone 3).
//!
//! A call made with `dry_run: true` never changes anything: it answers with
//! `{rev, plan}`, a committable plan, whether it needs approval or not
//! (under `strict`, a call that needs approval is refused instead).
//!
//! A call that would fail with `REVISION_CONFLICT` gets that error, never a
//! plan: the document tools check conflicts before they ask the gate (see
//! `approval_gate` for the order).
//!
//! # Committing
//!
//! `plan_commit` takes the plan out of the store (single use), then runs the
//! stored tool with the stored arguments and base revision through the
//! registry handler, exactly like the original call. The only difference is
//! one pass through the gate for that very call (same tool, same document,
//! not a dry run). Every other check (limits, validation, revision conflicts)
//! runs again. The pass is refused, with `REVISION_CONFLICT`, if the call now
//! does something the plan did not show: other commands than the planned
//! ones, or a reason for approval the plan did not have (a save that would
//! now overwrite a file, say). A plan of a call that writes the document to
//! a file or discards its unsaved changes is pinned to the revision it was
//! made at: the commit is refused the same way if the document changed
//! since, so what is written or discarded is what the user approved.
//!
//! # For tools that write files
//!
//! A tool that may overwrite a file (`document_save`, `export_font`) builds a
//! `Proposal` with `Change::Snapshot`, `destructive` set when the target
//! exists and is not the document's own file, and a `label` that is the whole
//! summary (`"overwrites <path>"`); see `document_save`.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::marker::PhantomData;

use forma_edit::Command;
use forma_model::{Actor, EntityKey, Font, Rev};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::engine::{DocHandle, Engine, Policy};
use crate::errors::{ErrorCode, ToolError};
use crate::limits::MASS_THRESHOLD;
use crate::locks::read_only_handle;
use crate::plans::{BoundedDiff, MAX_AFFECTED_NAMES, NewPlan, Plan, rfc3339, sha256};
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, find, schema_of};
use crate::tools::json_output;
use crate::tools::limits::parse_checked;

/// `plan_commit`.
pub(crate) const COMMIT: CommandSpec = CommandSpec {
    name: "plan_commit",
    title: "Commit plan",
    description: "Run the plan identified by `plan_token`, as returned with APPROVAL_REQUIRED or by \
                  a call made with `dry_run: true`. Only commit a plan after showing it to the \
                  user and getting their explicit confirmation. A token is single-use (even if \
                  the call then fails), expires 300 s after the plan was made and only works for \
                  the caller that made the plan. The planned call runs exactly as planned, \
                  against the revision it was based on: if what it touches changed since, it \
                  fails with REVISION_CONFLICT and a new plan is needed. The result is the \
                  planned tool's result plus `plan_committed: true`.",
    toolset: Toolset::Core,
    effects: Effects::DESTRUCTIVE,
    output: OutputKind::Json,
    input_schema: schema_of::<PlanCommitArgs>,
    output_schema: Some(plan_commit_output_schema),
    handler: plan_commit,
};

/// `plan_discard`.
pub(crate) const DISCARD: CommandSpec = CommandSpec {
    name: "plan_discard",
    title: "Discard plan",
    description: "Discard the pending plan identified by `plan_token` without running it, for \
                  example when the user declines it. Only the caller that made the plan can \
                  discard it.",
    toolset: Toolset::Core,
    effects: Effects::WRITE,
    output: OutputKind::Json,
    input_schema: schema_of::<PlanDiscardArgs>,
    output_schema: Some(schema_of::<PlanDiscardResult>),
    handler: plan_discard,
};

/// The message of a call that needs approval under the `strict` policy.
const STRICT: &str = "strict policy requires the FORMA app (milestone 3)";

// ------------------------------------------------------------------- types

/// Arguments of `plan_commit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanCommitArgs {
    /// The `plan_token` of the plan the user confirmed.
    pub plan_token: String,
}

/// Arguments of `plan_discard`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDiscardArgs {
    /// The `plan_token` of the plan to discard.
    pub plan_token: String,
}

/// Result of `plan_discard`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanDiscardResult {
    /// Always `true`.
    pub discarded: bool,
}

/// The result of a call made with `dry_run: true`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedResult {
    /// The current revision (unchanged).
    pub rev: u64,
    /// The plan of the call, which was not applied.
    pub plan: Plan,
}

/// The result schema of a tool that answers either with its result `R` or,
/// with `dry_run: true`, with a [`PlannedResult`] (`{rev, plan}`): `R`'s
/// properties plus `plan`, and exactly one of the two sets of required
/// fields.
pub struct OrPlan<R>(PhantomData<R>);

impl<R: JsonSchema> JsonSchema for OrPlan<R> {
    fn schema_name() -> Cow<'static, str> {
        R::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        format!("forma_engine::OrPlan<{}>", R::schema_id()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = R::json_schema(generator);
        let mut plan = generator.subschema_for::<Plan>().to_value();
        if let Some(plan) = plan.as_object_mut() {
            plan.insert(
                "description".into(),
                "Only with `dry_run: true`: the plan of the call, which was not applied.".into(),
            );
        }
        let object = schema.ensure_object();
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .map_or_else(String::new, |text| format!("{text} "));
        object.insert(
            "description".into(),
            format!("{description}With `dry_run: true`, only `rev` and `plan`.").into(),
        );
        let applied = object
            .remove("required")
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let properties = object
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert("plan".into(), plan);
        }
        object.insert("required".into(), json!(["rev"]));
        object.insert(
            "oneOf".into(),
            json!([{ "required": applied }, { "required": ["rev", "plan"] }]),
        );
        schema
    }
}

/// The result schema of `plan_commit`: the planned tool's own result, plus
/// `plan_committed`.
fn plan_commit_output_schema() -> Value {
    json!({
        "title": "PlanCommitResult",
        "description": "The result of the planned call, as its tool returns it, plus \
                        `plan_committed: true`.",
        "type": "object",
        "properties": {
            "rev": {
                "description": "The current revision after the call.",
                "type": "integer",
                "format": "uint64",
                "minimum": 0
            },
            "plan_committed": {
                "description": "Always `true`.",
                "type": "boolean",
                "const": true
            }
        },
        "required": ["rev", "plan_committed"]
    })
}

// -------------------------------------------------------------------- gate

/// What a call that may need approval is about to do: what
/// [`approval_gate`] decides on.
#[derive(Clone, Debug)]
pub(crate) struct Proposal<'a> {
    /// Who makes the call.
    pub actor: &'a Actor,
    /// The tool called.
    pub tool: &'static str,
    /// The arguments of the call, exactly as received (a plan keeps them,
    /// without `dry_run`, to run the call again on commit).
    pub args: &'a Value,
    /// The document the call is about.
    pub doc: &'a DocHandle,
    /// The revision the call is based on.
    pub base_rev: Rev,
    /// Whether the caller only wants the plan.
    pub dry_run: bool,
    /// Whether the call destroys data (see the module documentation).
    pub destructive: bool,
    /// For a [`Change::Font`], the history label of the change (the summary
    /// adds the number of entities affected); for a [`Change::Snapshot`], the whole
    /// summary, such as `"overwrites <path>"`.
    pub label: String,
    /// What changes.
    pub change: Change<'a>,
}

/// What a [`Proposal`] changes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Change<'a> {
    /// The font of the document, from `before` (its current state) to
    /// `after`.
    Font {
        /// The current state.
        before: &'a Font,
        /// The state after the call.
        after: &'a Font,
        /// The distinct entities the call touches that count toward the
        /// mass threshold: glyphs, kerning pairs and kerning groups (see
        /// [`is_affected`]).
        affected: &'a BTreeSet<EntityKey>,
        /// The commands the call runs, when it runs commands (a committed
        /// plan must run exactly these); `None` for undo-like changes whose
        /// effect a revision or transaction id fixes.
        commands: Option<&'a [Command]>,
    },
    /// The document as it is now, as a whole, without a change to the font:
    /// the call writes it to a file (save, export) or discards its unsaved
    /// changes (close). The plan pins the current revision.
    Snapshot,
}

/// Whether a touched entity is one a call *affects*: a glyph, a kerning
/// pair or a kerning group. Plans count them, and a call that affects more
/// than [`MASS_THRESHOLD`] is massive; the font info, the vertical metrics
/// and the glyph order do not count.
pub(crate) fn is_affected(key: &EntityKey) -> bool {
    // Exhaustive, so that a new kind of entity needs a decision here.
    match key {
        EntityKey::Glyph(_) | EntityKey::KerningPair(..) | EntityKey::KerningGroup(_) => true,
        EntityKey::FontInfo | EntityKey::VerticalMetrics(_) | EntityKey::GlyphOrder => false,
    }
}

/// `count` followed by the singular or plural noun.
fn counted(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

impl Proposal<'_> {
    fn affected_count(&self) -> usize {
        match self.change {
            Change::Font { affected, .. } => affected.len(),
            Change::Snapshot => 0,
        }
    }

    fn mass(&self) -> bool {
        self.affected_count() > MASS_THRESHOLD
    }

    /// The label plus what is affected: `"<label> (3 glyphs affected)"`, or
    /// `"<label> (1 glyph, 21 kerning pairs, 1 kerning group affected)"` when
    /// kerning is affected too (glyphs are left out when there are none
    /// then), with `", destructive"` before the closing parenthesis for a
    /// destructive call.
    fn summary(&self) -> String {
        match self.change {
            Change::Font { affected, .. } => {
                let count = |wanted: fn(&EntityKey) -> bool| {
                    affected.iter().filter(|key| wanted(key)).count()
                };
                let glyphs = count(|key| matches!(key, EntityKey::Glyph(_)));
                let pairs = count(|key| matches!(key, EntityKey::KerningPair(..)));
                let groups = count(|key| matches!(key, EntityKey::KerningGroup(_)));
                let mut parts = Vec::new();
                if glyphs > 0 || pairs + groups == 0 {
                    parts.push(counted(glyphs, "glyph", "glyphs"));
                }
                if pairs > 0 {
                    parts.push(counted(pairs, "kerning pair", "kerning pairs"));
                }
                if groups > 0 {
                    parts.push(counted(groups, "kerning group", "kerning groups"));
                }
                let destructive = if self.destructive {
                    ", destructive"
                } else {
                    ""
                };
                format!(
                    "{} ({} affected{destructive})",
                    self.label,
                    parts.join(", ")
                )
            }
            Change::Snapshot => self.label.clone(),
        }
    }

    /// The SHA-256 of the commands, if the call runs commands.
    fn fingerprint(&self) -> Result<Option<[u8; 32]>, ToolError> {
        let Change::Font {
            commands: Some(commands),
            ..
        } = self.change
        else {
            return Ok(None);
        };
        serde_json::to_vec(commands)
            .map(|bytes| Some(sha256(&bytes)))
            .map_err(|error| {
                ToolError::invalid_argument(format!(
                    "internal error: cannot encode the commands: {error}"
                ))
            })
    }
}

/// What the gate decided.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Gate {
    /// Go on and apply the call.
    Proceed,
    /// Answer the call with this result and apply nothing (a dry run).
    Answer(ToolOutput),
}

/// The plan `plan_commit` is running: the one call it lets through the gate,
/// and what the user approved.
#[derive(Clone, Debug)]
pub(crate) struct Committing {
    tool: &'static str,
    doc: DocHandle,
    destructive: bool,
    mass: bool,
    fingerprint: Option<[u8; 32]>,
    pinned_rev: Option<Rev>,
}

/// The approval gate of every call that changes a document, writes a file
/// or discards unsaved changes; see the module documentation.
///
/// Each tool calls it once, after its own checks of the call (arguments,
/// limits, paths) and right before the change is applied. For changes of the
/// document, the revision-conflict check comes **before** the gate, so a
/// call that would conflict gets `REVISION_CONFLICT` and never a plan:
/// `mutate` runs `Document::check_conflicts` on the entities the measured
/// commands touch, and `history_revert`/`history_restore` run the change on
/// a copy of the document first. `Document::execute` (or the real revert or
/// restore) checks conflicts once more when it applies the change, after the
/// gate. Tools that write a file or discard changes have no conflict check;
/// their plans pin the revision instead.
///
/// # Errors
///
/// * `APPROVAL_REQUIRED` with `details = {plan}` when the call needs a plan;
/// * `UNSUPPORTED` when it needs approval under the `strict` policy;
/// * `REVISION_CONFLICT` when a plan being committed no longer describes the
///   call;
/// * `UNSUPPORTED` for a read-only document handle, unless the call only
///   writes a new file (an export);
/// * `IO_ERROR` if no secure random source is available to make a plan;
/// * `NOT_FOUND` for an unknown document handle.
pub(crate) fn approval_gate(
    engine: &mut Engine,
    proposal: &Proposal<'_>,
) -> Result<Gate, ToolError> {
    let destructive = proposal.destructive;
    let mass = proposal.mass();
    // A read-only handle (its file is open for writing in another process)
    // neither changes the document nor writes or discards it; the tools
    // refuse that earlier, this keeps any path from reaching a plan.
    let changes = matches!(proposal.change, Change::Font { .. });
    if (changes || destructive || proposal.tool == "document_save")
        && engine.doc(proposal.doc)?.read_only()
    {
        return Err(read_only_handle());
    }

    // The one pass a plan being committed grants: to the planned call.
    if let Some(commit) = engine.committing.take()
        && !proposal.dry_run
        && commit.tool == proposal.tool
        && commit.doc == *proposal.doc
    {
        let moved = match commit.pinned_rev {
            Some(pinned) => engine.doc(proposal.doc)?.rev() != pinned,
            None => false,
        };
        let outdated = moved
            || (destructive && !commit.destructive)
            || (mass && !commit.mass)
            || commit.fingerprint != proposal.fingerprint()?;
        if outdated {
            return Err(outdated_plan(engine, proposal.doc));
        }
        return Ok(Gate::Proceed);
    }

    let needs_approval = destructive || mass;
    let policy = engine.cfg.policy;
    if needs_approval && policy == Policy::Strict {
        return Err(ToolError::unsupported(STRICT));
    }
    if proposal.dry_run {
        let plan = make_plan(engine, proposal)?;
        let rev = engine.doc(proposal.doc)?.rev().0;
        return json_output(&PlannedResult { rev, plan }).map(Gate::Answer);
    }
    let required = match policy {
        Policy::Prompted | Policy::Strict => needs_approval,
        Policy::Auto => destructive,
    };
    if !required {
        tracing::debug!(
            actor = %proposal.actor,
            tool = proposal.tool,
            doc = %proposal.doc,
            affected = proposal.affected_count(),
            destructive,
            policy = policy.as_str(),
            "approval gate: no approval needed"
        );
        return Ok(Gate::Proceed);
    }
    let plan = make_plan(engine, proposal)?;
    let message = format!(
        "This operation needs approval: {}. Show the plan to the user and call plan_commit with \
         the plan_token only after they confirm.",
        plan.summary
    );
    let plan = serde_json::to_value(&plan).map_err(|error| {
        ToolError::invalid_argument(format!("internal error: cannot encode the plan: {error}"))
    })?;
    let mut details = Map::new();
    details.insert("plan".into(), plan);
    Err(ToolError::new(ErrorCode::ApprovalRequired, message).with_details(Value::Object(details)))
}

/// Keeps the call of `proposal` as a plan and describes it.
fn make_plan(engine: &mut Engine, proposal: &Proposal<'_>) -> Result<Plan, ToolError> {
    let (affected, diff) = match proposal.change {
        Change::Font {
            before,
            after,
            affected,
            ..
        } => (
            affected_names(before, after, affected),
            BoundedDiff::from(forma_edit::diff(before, after)),
        ),
        Change::Snapshot => (Vec::new(), BoundedDiff::empty()),
    };
    let pinned_rev = match proposal.change {
        Change::Snapshot => Some(engine.doc(proposal.doc)?.rev()),
        Change::Font { .. } => None,
    };
    let (plan_token, expires) = engine.plans.create(NewPlan {
        actor: proposal.actor.clone(),
        tool: proposal.tool,
        doc: proposal.doc.clone(),
        args: proposal.args.clone(),
        base_rev: proposal.base_rev,
        destructive: proposal.destructive,
        mass: proposal.mass(),
        fingerprint: proposal.fingerprint()?,
        pinned_rev,
    })?;
    Ok(Plan {
        plan_token,
        expires_at: rfc3339(expires),
        tool: proposal.tool.to_owned(),
        base_rev: proposal.base_rev.0,
        summary: proposal.summary(),
        affected,
        affected_count: proposal.affected_count(),
        diff,
    })
}

/// The names of the glyphs among `affected`, at most
/// [`MAX_AFFECTED_NAMES`]: in the glyph order of `after` (with their new
/// names), then the deleted ones in the order of `before`. A glyph that
/// exists in neither (created and deleted by the same call) has no name to
/// list.
fn affected_names(before: &Font, after: &Font, affected: &BTreeSet<EntityKey>) -> Vec<String> {
    let mut named = BTreeSet::new();
    let mut names = Vec::new();
    for font in [after, before] {
        for id in &font.glyph_order {
            if names.len() == MAX_AFFECTED_NAMES {
                return names;
            }
            if affected.contains(&EntityKey::Glyph(*id))
                && let Some(glyph) = font.glyphs.get(id)
                && named.insert(*id)
            {
                names.push(glyph.name.clone());
            }
        }
    }
    names
}

/// The error of a plan being committed whose call would now do something
/// else than the plan showed.
fn outdated_plan(engine: &Engine, doc: &DocHandle) -> ToolError {
    let current_rev = engine.doc(doc).map_or(0, |open| open.rev().0);
    ToolError::new(
        ErrorCode::RevisionConflict,
        "the document or its files changed since the plan was made, so the call would no \
         longer do what the plan showed; make the call again to get a new plan",
    )
    .with_details(json!({ "current_rev": current_rev, "conflicts": [] }))
}

// ---------------------------------------------------------------- handlers

fn plan_commit(engine: &mut Engine, actor: &Actor, args: Value) -> Result<ToolOutput, ToolError> {
    let args: PlanCommitArgs = parse_checked(args)?;
    // Used up from here on, whatever happens to the call.
    let plan = engine.plans.take(&args.plan_token, actor)?;
    let spec = find(plan.tool)
        .ok_or_else(|| ToolError::invalid_argument(format!("unknown tool '{}'", plan.tool)))?;
    engine.check_available(spec)?;
    let mut call = plan.args;
    // The planned base revision (tools without `base_rev`, such as
    // document_close, keep their arguments as they are).
    if let Some(object) = call.as_object_mut()
        && object.contains_key("base_rev")
    {
        object.insert("base_rev".into(), Value::from(plan.base_rev.0));
    }
    let (doc, base_rev) = (plan.doc.clone(), plan.base_rev.0);
    engine.committing = Some(Committing {
        tool: plan.tool,
        doc: plan.doc,
        destructive: plan.destructive,
        mass: plan.mass,
        fingerprint: plan.fingerprint,
        pinned_rev: plan.pinned_rev,
    });
    let result = (spec.handler)(engine, actor, call)
        .map_err(|error| engine.stale_reference(Some(doc.as_str()), Some(base_rev), error));
    // The pass is for this call only, used or not.
    engine.committing = None;
    match result? {
        ToolOutput::Json(mut value) => {
            if let Some(object) = value.as_object_mut() {
                object.insert("plan_committed".into(), Value::Bool(true));
            }
            Ok(ToolOutput::Json(value))
        }
        image @ ToolOutput::Image { .. } => Ok(image),
    }
}

fn plan_discard(engine: &mut Engine, actor: &Actor, args: Value) -> Result<ToolOutput, ToolError> {
    let args: PlanDiscardArgs = parse_checked(args)?;
    engine.plans.discard(&args.plan_token, actor)?;
    json_output(&PlanDiscardResult { discarded: true })
}

#[cfg(test)]
mod tests {
    use forma_edit::{Document, apply};

    use super::*;
    use crate::engine::{EngineConfig, OpenDoc};
    use crate::registry::schema_of;
    use crate::tools::mutate::MutationResult;

    /// A destructive file write (as `document_save` proposes it).
    fn overwrite<'a>(
        actor: &'a Actor,
        args: &'a Value,
        tool: &'static str,
        doc: &'a DocHandle,
        dry_run: bool,
    ) -> Proposal<'a> {
        Proposal {
            actor,
            tool,
            args,
            doc,
            base_rev: Rev(0),
            dry_run,
            destructive: true,
            label: "overwrites x".into(),
            change: Change::Snapshot,
        }
    }

    #[test]
    fn the_pass_of_a_commit_is_for_the_planned_call_only() {
        let mut engine = Engine::new(EngineConfig::default());
        let doc = engine.insert_doc(OpenDoc::new(
            Document::new(Font::new("Test", 1000)),
            None,
            false,
        ));
        let other = engine.insert_doc(OpenDoc::new(
            Document::new(Font::new("Other", 1000)),
            None,
            false,
        ));
        let actor = Actor::Agent("test".into());
        let args = json!({ "doc": doc, "base_rev": 0 });
        let pass = || {
            Some(Committing {
                tool: "document_save",
                doc: doc.clone(),
                destructive: true,
                mass: false,
                fingerprint: None,
                pinned_rev: Some(Rev(0)),
            })
        };

        // Another tool or another document gets no pass, and the pass is
        // spent by the first call that reaches the gate.
        for proposal in [
            overwrite(&actor, &args, "export_font", &doc, false),
            overwrite(&actor, &args, "document_save", &other, false),
        ] {
            engine.committing = pass();
            let error = approval_gate(&mut engine, &proposal).expect_err("needs approval");
            assert_eq!(error.code, ErrorCode::ApprovalRequired);
            assert!(engine.committing.is_none());
        }
        // A dry run gets no pass either: it plans.
        engine.committing = pass();
        let dry_run = overwrite(&actor, &args, "document_save", &doc, true);
        let answer = approval_gate(&mut engine, &dry_run).expect("dry run");
        assert!(matches!(answer, Gate::Answer(_)));
        assert!(engine.committing.is_none());

        // The planned call itself passes, once.
        engine.committing = pass();
        let planned = overwrite(&actor, &args, "document_save", &doc, false);
        assert_eq!(
            approval_gate(&mut engine, &planned).expect("pass"),
            Gate::Proceed
        );
        let error = approval_gate(&mut engine, &planned).expect_err("spent");
        assert_eq!(error.code, ErrorCode::ApprovalRequired);

        // A pinned plan passes only at the revision it was made at.
        engine.committing = pass().map(|commit| Committing {
            pinned_rev: Some(Rev(7)),
            ..commit
        });
        let error = approval_gate(&mut engine, &planned).expect_err("moved");
        assert_eq!(error.code, ErrorCode::RevisionConflict);
    }

    #[test]
    fn or_plan_schemas_accept_either_answer() {
        let schema = schema_of::<OrPlan<MutationResult>>();
        assert_eq!(schema["title"], json!("MutationResult"));
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["required"], json!(["rev"]));
        assert_eq!(
            schema["oneOf"],
            json!([
                { "required": ["rev", "touched", "touched_count", "touched_truncated"] },
                { "required": ["rev", "plan"] }
            ])
        );
        assert_eq!(schema["properties"]["plan"]["$ref"], json!("#/$defs/Plan"));
        assert!(schema["$defs"]["Plan"]["properties"]["diff"].is_object());
        assert!(
            schema["$defs"]["BoundedDiff"]["properties"]["truncated"].is_object(),
            "{schema}"
        );
        // `tx` (nullable) is a property, not a required field.
        assert!(schema["properties"]["tx"].is_object());
        let described = schema["description"].as_str().expect("description");
        assert!(described.ends_with("With `dry_run: true`, only `rev` and `plan`."));
    }

    #[test]
    fn glyphs_kerning_pairs_and_groups_count_as_affected() {
        use forma_model::{GlyphId, KernRef, KerningGroupId};

        let font = Font::new("Test", 1000);
        let master = font.masters[0].id;
        let (actor, args, doc) = (Actor::User, json!({}), DocHandle::from("doc_1"));
        let group = KerningGroupId::new();
        let pair = |_| {
            EntityKey::KerningPair(
                master,
                KernRef::Glyph(GlyphId::new()),
                KernRef::Group(group),
            )
        };
        let proposal = |affected| Proposal {
            actor: &actor,
            tool: "edit_batch",
            args: &args,
            doc: &doc,
            base_rev: Rev(0),
            dry_run: false,
            destructive: false,
            label: "Kern".into(),
            change: Change::Font {
                before: &font,
                after: &font,
                affected,
                commands: None,
            },
        };

        assert!(is_affected(&EntityKey::Glyph(GlyphId::new())));
        assert!(is_affected(&EntityKey::KerningGroup(group)));
        assert!(is_affected(&pair(0)));
        for key in [
            EntityKey::FontInfo,
            EntityKey::GlyphOrder,
            EntityKey::VerticalMetrics(master),
        ] {
            assert!(!is_affected(&key), "{key}");
        }

        // 20 pairs are not massive; 20 pairs and a group are.
        let pairs: BTreeSet<EntityKey> = (0..20).map(pair).collect();
        let twenty = proposal(&pairs);
        assert!(!twenty.mass());
        assert_eq!(twenty.summary(), "Kern (20 kerning pairs affected)");
        let mut more = pairs.clone();
        more.insert(EntityKey::KerningGroup(group));
        let massive = proposal(&more);
        assert!(massive.mass());
        assert_eq!(massive.affected_count(), 21);
        assert_eq!(
            massive.summary(),
            "Kern (20 kerning pairs, 1 kerning group affected)"
        );
        let mut all = more.clone();
        all.insert(EntityKey::Glyph(GlyphId::new()));
        assert_eq!(
            proposal(&all).summary(),
            "Kern (1 glyph, 20 kerning pairs, 1 kerning group affected)"
        );
        // Without kerning, the summary reads as before.
        let none = BTreeSet::new();
        assert_eq!(proposal(&none).summary(), "Kern (0 glyphs affected)");
    }

    #[test]
    fn affected_glyphs_are_named_in_glyph_order_then_deleted_ones() {
        let create = |name: &str| Command::CreateGlyph {
            name: name.into(),
            unicodes: Vec::new(),
            advance_width: 1.0,
            paths: Vec::new(),
        };
        let mut before = Font::new("Test", 1000);
        for name in ["a", "b", "c"] {
            before = apply(&before, &create(name)).expect("create").0;
        }
        let id = |font: &Font, name: &str| font.glyph_by_name(name).expect("glyph").id;
        let (a, b, c) = (id(&before, "a"), id(&before, "b"), id(&before, "c"));
        let delete_b = Command::DeleteGlyphs {
            glyphs: vec![forma_edit::GlyphRef::Name("b".into())],
        };
        let (after, _) = apply(&before, &delete_b).expect("delete");
        let (after, _) = apply(&after, &create("d")).expect("create");
        let d = id(&after, "d");
        let ids: BTreeSet<EntityKey> = [a, b, c, d].into_iter().map(EntityKey::Glyph).collect();
        assert_eq!(affected_names(&before, &after, &ids), ["a", "c", "d", "b"]);
        assert_eq!(
            affected_names(&before, &after, &BTreeSet::from([EntityKey::Glyph(c)])),
            ["c"]
        );

        // At most 100 names, however many glyphs are affected.
        let mut many = Font::new("Test", 1000);
        for index in 0..105 {
            many = apply(&many, &create(&format!("g{index:03}")))
                .expect("create")
                .0;
        }
        let all: BTreeSet<EntityKey> = many.glyphs.keys().copied().map(EntityKey::Glyph).collect();
        let names = affected_names(&many, &many, &all);
        assert_eq!(names.len(), MAX_AFFECTED_NAMES);
        assert_eq!(names.first().map(String::as_str), Some("g000"));
        assert_eq!(names.last().map(String::as_str), Some("g099"));
    }
}
