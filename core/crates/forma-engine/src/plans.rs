//! Approval plans: what a destructive or massive call would do, kept by the
//! engine until its caller commits or discards it, and the signed,
//! single-use tokens that name them.
//!
//! A plan is created by the approval gate (see `tools::plans`) when a call
//! needs approval, or when it is made with `dry_run: true`. The engine keeps
//! the call itself (actor, tool, document, arguments, base revision): the
//! caller only ever holds the token,
//!
//! ```text
//! plan_token = base64url_no_pad(plan_id ‖ HMAC-SHA256(key, plan_id ‖ actor ‖ 0x00 ‖ tool ‖ 0x00
//!                               ‖ SHA-256(canonical args) ‖ base_rev (u64 BE) ‖ expires (i64 BE)))
//! ```
//!
//! where `key` is 32 random bytes drawn when the engine is created (so tokens
//! never survive the process), `plan_id` 16 random bytes, and the canonical
//! arguments the call's arguments without `dry_run`, as compact JSON with
//! the keys of every object sorted. A plan expires [`PLAN_TTL_SECS`] seconds
//! after it is made, is used up by its first commit (even a failed one) or
//! discard, belongs to the actor that made the call, and is dropped when its
//! document closes. At most [`MAX_PENDING_PLANS`] plans, holding at most
//! [`MAX_PENDING_PLAN_BYTES`] of arguments in all, are kept; the oldest go
//! first.
//!
//! A plan shows at most [`MAX_AFFECTED_NAMES`] glyph names and, in its
//! [`BoundedDiff`], at most [`MAX_DIFF_ENTRIES`] entries per list, with the
//! full counts, so that it fits the response budget of agents whatever the
//! size of the change.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use forma_edit::{FontDiff, GlyphDiff};
use forma_model::{Actor, Rev};
use hmac::{Hmac, KeyInit, Mac};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::engine::DocHandle;
use crate::errors::ToolError;
use crate::limits::PLAN_TTL_SECS;
use crate::tools::limits::limit_exceeded;

/// The most plans kept at once; making another drops the oldest.
pub const MAX_PENDING_PLANS: usize = 1000;
/// The most argument bytes (canonical JSON) the pending plans keep in all;
/// making another plan drops the oldest ones until it fits.
pub const MAX_PENDING_PLAN_BYTES: usize = 64 * 1024 * 1024;
/// The most glyph names a [`Plan`] lists in `affected`.
pub const MAX_AFFECTED_NAMES: usize = 100;
/// The most entries each list of a [`BoundedDiff`] holds.
pub const MAX_DIFF_ENTRIES: usize = 100;

/// The length of a plan id, in bytes.
const PLAN_ID_LEN: usize = 16;
/// The length of the signature, in bytes.
const MAC_LEN: usize = 32;
/// The length of a decoded token, in bytes.
const TOKEN_LEN: usize = PLAN_ID_LEN + MAC_LEN;

type PlanId = [u8; PLAN_ID_LEN];
type HmacSha256 = Hmac<Sha256>;

/// The source of the current time, for the expiry of plans.
pub trait Clock {
    /// The current time.
    fn now(&self) -> SystemTime;
}

/// The system clock: the [`Clock`] of [`Engine::new`](crate::Engine::new).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A plan, as shown to the caller: in the `details` of an
/// `APPROVAL_REQUIRED` error, or in the result of a call made with
/// `dry_run: true`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Plan {
    /// The single-use token to pass to `plan_commit` (or `plan_discard`)
    /// once the user has confirmed the plan.
    pub plan_token: String,
    /// When the token expires (RFC 3339, UTC).
    pub expires_at: String,
    /// The tool the plan runs.
    pub tool: String,
    /// The revision the planned call is based on; the commit fails with
    /// REVISION_CONFLICT if an entity it touches changed after it.
    pub base_rev: u64,
    /// What the call does, for the user.
    pub summary: String,
    /// The names of the glyphs the call touches (at most 100).
    pub affected: Vec<String>,
    /// How many glyphs the call touches.
    pub affected_count: usize,
    /// How the font changes; empty for calls that write a file or discard
    /// unsaved changes.
    pub diff: BoundedDiff,
}

/// How a font changes between two states, with each list cut to its first
/// 100 entries; the counts are always complete.
///
/// Glyphs are matched by their stable id, so a renamed glyph is modified,
/// not removed and added again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoundedDiff {
    /// Names of the glyphs added, in glyph order (the first 100).
    pub added: Vec<String>,
    /// How many glyphs were added.
    pub added_count: usize,
    /// Names of the glyphs removed, in their former glyph order (the first
    /// 100).
    pub removed: Vec<String>,
    /// How many glyphs were removed.
    pub removed_count: usize,
    /// The glyphs that changed, in glyph order (the first 100).
    pub modified: Vec<GlyphDiff>,
    /// How many glyphs changed.
    pub modified_count: usize,
    /// Whether the font info or the units per em changed.
    pub info_changed: bool,
    /// Whether the vertical metrics of any master changed.
    pub metrics_changed: bool,
    /// Whether any list was cut to its first 100 entries.
    pub truncated: bool,
}

impl BoundedDiff {
    /// A diff without any change.
    #[must_use]
    pub fn empty() -> BoundedDiff {
        BoundedDiff::from(FontDiff {
            added: Vec::new(),
            removed: Vec::new(),
            modified: Vec::new(),
            info_changed: false,
            metrics_changed: false,
        })
    }
}

impl From<FontDiff> for BoundedDiff {
    fn from(diff: FontDiff) -> BoundedDiff {
        let FontDiff {
            mut added,
            mut removed,
            mut modified,
            info_changed,
            metrics_changed,
        } = diff;
        let (added_count, removed_count, modified_count) =
            (added.len(), removed.len(), modified.len());
        let truncated = [added_count, removed_count, modified_count]
            .into_iter()
            .any(|count| count > MAX_DIFF_ENTRIES);
        added.truncate(MAX_DIFF_ENTRIES);
        removed.truncate(MAX_DIFF_ENTRIES);
        modified.truncate(MAX_DIFF_ENTRIES);
        BoundedDiff {
            added,
            added_count,
            removed,
            removed_count,
            modified,
            modified_count,
            info_changed,
            metrics_changed,
            truncated,
        }
    }
}

/// A call kept by the engine, waiting to be committed.
#[derive(Clone, Debug)]
pub(crate) struct StoredPlan {
    /// Who made the call.
    pub actor: Actor,
    /// The tool to run.
    pub tool: &'static str,
    /// The document the call is about.
    pub doc: DocHandle,
    /// The arguments of the call, without `dry_run`.
    pub args: Value,
    /// The revision the call is based on.
    pub base_rev: Rev,
    /// When the plan expires, in seconds since the Unix epoch.
    pub expires: i64,
    /// Whether the plan was made for a destructive call.
    pub destructive: bool,
    /// Whether the plan was made for a massive call.
    pub mass: bool,
    /// The SHA-256 of the commands the call translated into, for calls that
    /// edit the font with commands.
    pub fingerprint: Option<[u8; 32]>,
    /// For a call that writes the document to a file or discards its unsaved
    /// changes: the revision it was planned at, which the commit requires.
    pub pinned_rev: Option<Rev>,
    /// The order in which plans were made, for eviction.
    seq: u64,
    /// The size of the canonical arguments, for the byte budget.
    bytes: usize,
}

/// A call to keep as a plan: [`PlanStore::create`].
#[derive(Clone, Debug)]
pub(crate) struct NewPlan {
    /// Who makes the call.
    pub actor: Actor,
    /// The tool called.
    pub tool: &'static str,
    /// The document the call is about.
    pub doc: DocHandle,
    /// The arguments of the call (`dry_run` is dropped).
    pub args: Value,
    /// The revision the call is based on.
    pub base_rev: Rev,
    /// Whether the call is destructive.
    pub destructive: bool,
    /// Whether the call is massive.
    pub mass: bool,
    /// The SHA-256 of the call's commands, if it has any.
    pub fingerprint: Option<[u8; 32]>,
    /// The revision the commit requires, if any.
    pub pinned_rev: Option<Rev>,
}

/// The pending plans of an engine, with the key that signs their tokens.
pub(crate) struct PlanStore {
    /// The signing key, or why no secure random bytes were available (then
    /// no plan can be made, so calls that need approval fail).
    key: Result<[u8; 32], String>,
    clock: Box<dyn Clock + Send>,
    plans: BTreeMap<PlanId, StoredPlan>,
    next_seq: u64,
    /// The argument bytes of the pending plans.
    bytes: usize,
    /// [`MAX_PENDING_PLANS`] (smaller in tests).
    max_plans: usize,
    /// [`MAX_PENDING_PLAN_BYTES`] (smaller in tests).
    max_bytes: usize,
}

impl fmt::Debug for PlanStore {
    /// Never shows the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlanStore")
            .field("pending", &self.plans.len())
            .field("bytes", &self.bytes)
            .field("key_available", &self.key.is_ok())
            .finish_non_exhaustive()
    }
}

impl PlanStore {
    /// An empty store with a fresh random key, reading the time from
    /// `clock`. If the system has no secure random source, the store is
    /// created anyway (with a warning) and every attempt to make a plan
    /// fails.
    pub(crate) fn new(clock: Box<dyn Clock + Send>) -> PlanStore {
        let mut key = [0_u8; 32];
        let key = match getrandom::fill(&mut key) {
            Ok(()) => Ok(key),
            Err(error) => {
                tracing::warn!(%error, "no secure random source: approval plans are unavailable");
                Err(error.to_string())
            }
        };
        PlanStore {
            key,
            clock,
            plans: BTreeMap::new(),
            next_seq: 0,
            bytes: 0,
            max_plans: MAX_PENDING_PLANS,
            max_bytes: MAX_PENDING_PLAN_BYTES,
        }
    }

    /// The current time, in seconds since the Unix epoch.
    pub(crate) fn now_unix(&self) -> i64 {
        unix_seconds(self.clock.now())
    }

    /// The number of pending plans.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.plans.len()
    }

    /// Keeps `plan` and returns its token and expiry (seconds since the Unix
    /// epoch). Expired plans are dropped first, then the oldest ones while
    /// the store is full (by count or by argument bytes).
    ///
    /// # Errors
    ///
    /// * `LIMIT_EXCEEDED` with `details = {limit, actual}` if the arguments
    ///   alone exceed the byte budget;
    /// * `IO_ERROR` if the system has no secure random source.
    pub(crate) fn create(&mut self, plan: NewPlan) -> Result<(String, i64), ToolError> {
        let key = self.key()?;
        let args = without_dry_run(plan.args);
        let bytes = canonical_json(&args).len();
        if bytes > self.max_bytes {
            return Err(limit_exceeded(
                format!(
                    "the arguments of the call ({bytes} bytes) are too large to keep as a plan; \
                     the limit is {} bytes: split the work into several calls",
                    self.max_bytes
                ),
                self.max_bytes,
                bytes,
            ));
        }
        let now = self.now_unix();
        self.purge_expired(now);
        while self.plans.len() >= self.max_plans
            || self.bytes.saturating_add(bytes) > self.max_bytes
        {
            let oldest = self
                .plans
                .iter()
                .min_by_key(|(_, stored)| stored.seq)
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => self.remove(&id),
                None => break,
            };
        }
        let mut id: PlanId = [0; PLAN_ID_LEN];
        loop {
            getrandom::fill(&mut id).map_err(|error| no_randomness(&error.to_string()))?;
            if !self.plans.contains_key(&id) {
                break;
            }
        }
        let expires = now.saturating_add(i64::try_from(PLAN_TTL_SECS).unwrap_or(i64::MAX));
        let stored = StoredPlan {
            args,
            actor: plan.actor,
            tool: plan.tool,
            doc: plan.doc,
            base_rev: plan.base_rev,
            expires,
            destructive: plan.destructive,
            mass: plan.mass,
            fingerprint: plan.fingerprint,
            pinned_rev: plan.pinned_rev,
            seq: self.next_seq,
            bytes,
        };
        self.next_seq += 1;
        self.bytes += bytes;
        let signature = sign(&key, &id, &stored)?.finalize().into_bytes();
        let mut token = Vec::with_capacity(TOKEN_LEN);
        token.extend_from_slice(&id);
        token.extend_from_slice(&signature);
        self.plans.insert(id, stored);
        Ok((URL_SAFE_NO_PAD.encode(token), expires))
    }

    /// Takes the plan of `token` out of the store for `actor` to commit: the
    /// plan is used up from here on, whatever happens to the call.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT`, checked in this order: "malformed plan token",
    /// "unknown or already used plan", "plan token is not valid" (the
    /// signature does not match the stored plan), "plan belongs to another
    /// actor" (the plan stays), "plan expired" (the plan is dropped).
    pub(crate) fn take(&mut self, token: &str, actor: &Actor) -> Result<StoredPlan, ToolError> {
        let now = self.now_unix();
        let result = self.verify(token, actor).and_then(|id| {
            let plan = self
                .remove(&id)
                .ok_or_else(|| ToolError::invalid_argument(UNKNOWN_PLAN))?;
            if now > plan.expires {
                return Err(ToolError::invalid_argument("plan expired"));
            }
            Ok(plan)
        });
        self.purge_expired(now);
        result
    }

    /// Drops the plan of `token` for `actor`, expired or not.
    ///
    /// # Errors
    ///
    /// Those of [`take`](Self::take), save for the expiry.
    pub(crate) fn discard(&mut self, token: &str, actor: &Actor) -> Result<(), ToolError> {
        let now = self.now_unix();
        let result = self.verify(token, actor).map(|id| {
            self.remove(&id);
        });
        self.purge_expired(now);
        result
    }

    /// The document of the pending plan of `token`, if the token is valid for
    /// `actor` (expired or not); the plan stays in the store. For the audit
    /// log of `plan_commit`.
    pub(crate) fn doc_of(&self, token: &str, actor: &Actor) -> Option<DocHandle> {
        let id = self.verify(token, actor).ok()?;
        self.plans.get(&id).map(|plan| plan.doc.clone())
    }

    /// Drops every plan of the document `doc`.
    pub(crate) fn remove_doc(&mut self, doc: &DocHandle) {
        self.plans.retain(|_, plan| plan.doc != *doc);
        self.recount_bytes();
    }

    /// Takes the plan `id` out of the store.
    fn remove(&mut self, id: &PlanId) -> Option<StoredPlan> {
        let plan = self.plans.remove(id)?;
        self.bytes = self.bytes.saturating_sub(plan.bytes);
        Some(plan)
    }

    fn recount_bytes(&mut self) {
        self.bytes = self.plans.values().map(|plan| plan.bytes).sum();
    }

    /// Checks `token` against the stored plan it names, in constant time, and
    /// that the plan belongs to `actor`.
    fn verify(&self, token: &str, actor: &Actor) -> Result<PlanId, ToolError> {
        let malformed = || ToolError::invalid_argument("malformed plan token");
        let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| malformed())?;
        if bytes.len() != TOKEN_LEN {
            return Err(malformed());
        }
        let (id, signature) = bytes.split_at(PLAN_ID_LEN);
        let id: PlanId = id.try_into().map_err(|_| malformed())?;
        let plan = self
            .plans
            .get(&id)
            .ok_or_else(|| ToolError::invalid_argument(UNKNOWN_PLAN))?;
        sign(&self.key()?, &id, plan)?
            .verify_slice(signature)
            .map_err(|_| ToolError::invalid_argument("plan token is not valid"))?;
        if plan.actor != *actor {
            return Err(ToolError::invalid_argument("plan belongs to another actor"));
        }
        Ok(id)
    }

    fn key(&self) -> Result<[u8; 32], ToolError> {
        self.key.clone().map_err(|error| no_randomness(&error))
    }

    /// Drops the plans that expired before `now`.
    fn purge_expired(&mut self, now: i64) {
        self.plans.retain(|_, plan| now <= plan.expires);
        self.recount_bytes();
    }
}

const UNKNOWN_PLAN: &str = "unknown or already used plan";

fn no_randomness(error: &str) -> ToolError {
    ToolError::io(format!(
        "cannot make approval plans: no secure random source ({error})"
    ))
}

/// The HMAC of a plan, before finalization: over the plan id, the actor, the
/// tool, the digest of the canonical arguments, the base revision and the
/// expiry.
fn sign(key: &[u8; 32], id: &PlanId, plan: &StoredPlan) -> Result<HmacSha256, ToolError> {
    // HMAC takes keys of any length: this cannot fail.
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|_| ToolError::invalid_argument("internal error: cannot sign approval plans"))?;
    mac.update(id);
    mac.update(plan.actor.to_string().as_bytes());
    mac.update(&[0]);
    mac.update(plan.tool.as_bytes());
    mac.update(&[0]);
    mac.update(&args_digest(&plan.args));
    mac.update(&plan.base_rev.0.to_be_bytes());
    mac.update(&plan.expires.to_be_bytes());
    Ok(mac)
}

/// `args` without its top-level `dry_run`.
fn without_dry_run(mut args: Value) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.remove("dry_run");
    }
    args
}

/// The SHA-256 of the canonical JSON of `args`.
fn args_digest(args: &Value) -> [u8; 32] {
    Sha256::digest(canonical_json(args).as_bytes()).into()
}

/// The SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// `value` as compact JSON with the keys of every object sorted, whatever
/// order the map keeps them in.
pub(crate) fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(value, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// `time` in whole seconds since the Unix epoch (negative before it).
fn unix_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_secs()).map_or(i64::MIN, |secs| -secs),
    }
}

/// `seconds` since the Unix epoch as an RFC 3339 UTC timestamp, such as
/// `2027-01-15T08:05:00Z`.
pub(crate) fn rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

/// The proleptic Gregorian date of a day number (days since 1970-01-01), by
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::errors::ErrorCode;

    #[derive(Clone)]
    struct FixedClock(Arc<Mutex<SystemTime>>);

    impl FixedClock {
        fn at(seconds: u64) -> FixedClock {
            FixedClock(Arc::new(Mutex::new(
                UNIX_EPOCH + Duration::from_secs(seconds),
            )))
        }

        fn advance(&self, seconds: u64) {
            *self.0.lock().expect("clock") += Duration::from_secs(seconds);
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            *self.0.lock().expect("clock")
        }
    }

    fn agent(name: &str) -> Actor {
        Actor::Agent(name.into())
    }

    fn new_plan(actor: &Actor, doc: &str) -> NewPlan {
        NewPlan {
            actor: actor.clone(),
            tool: "glyph_delete",
            doc: DocHandle::from(doc),
            args: json!({ "doc": doc, "base_rev": 3, "glyphs": ["a"], "dry_run": true }),
            base_rev: Rev(3),
            destructive: true,
            mass: false,
            fingerprint: Some([7; 32]),
            pinned_rev: None,
        }
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_800_000_300), "2027-01-15T08:05:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(-86_400), "1969-12-31T00:00:00Z");
        assert_eq!(rfc3339(253_402_300_799), "9999-12-31T23:59:59Z");
        assert_eq!(unix_seconds(UNIX_EPOCH + Duration::from_secs(5)), 5);
        assert_eq!(unix_seconds(UNIX_EPOCH - Duration::from_secs(5)), -5);
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let mut a = serde_json::Map::new();
        a.insert("z".into(), json!(1));
        a.insert("a".into(), json!({ "y": [true, null, "x\"y"], "b": 2.5 }));
        assert_eq!(
            canonical_json(&Value::Object(a)),
            r#"{"a":{"b":2.5,"y":[true,null,"x\"y"]},"z":1}"#
        );
        assert_eq!(
            args_digest(&json!({ "b": 1, "a": 2 })),
            args_digest(&json!({ "a": 2, "b": 1 }))
        );
        assert_ne!(
            args_digest(&json!({ "a": 1 })),
            args_digest(&json!({ "a": 2 }))
        );
    }

    #[test]
    fn a_token_names_one_plan_of_one_actor_once() {
        let clock = FixedClock::at(1_000);
        let mut store = PlanStore::new(Box::new(clock.clone()));
        let alice = agent("alice");
        let (token, expires) = store.create(new_plan(&alice, "doc_1")).expect("create");
        assert_eq!(expires, 1_300);
        assert_eq!(token.len(), 64);

        let error = store.take(&token, &agent("bob")).expect_err("other actor");
        assert_eq!(error.message, "plan belongs to another actor");
        let plan = store.take(&token, &alice).expect("take");
        assert_eq!(plan.tool, "glyph_delete");
        assert_eq!(plan.doc.as_str(), "doc_1");
        assert_eq!(plan.base_rev, Rev(3));
        assert!(plan.destructive && !plan.mass);
        assert_eq!(plan.fingerprint, Some([7; 32]));
        assert_eq!(
            plan.args,
            json!({ "doc": "doc_1", "base_rev": 3, "glyphs": ["a"] }),
            "dry_run is not kept"
        );
        let error = store.take(&token, &alice).expect_err("used");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(error.message, UNKNOWN_PLAN);
    }

    #[test]
    fn the_signature_covers_every_stored_field() {
        let mut store = PlanStore::new(Box::new(FixedClock::at(1_000)));
        let alice = agent("alice");
        let (token, _) = store.create(new_plan(&alice, "doc_1")).expect("create");
        let tamperings: [fn(&mut StoredPlan); 5] = [
            |plan| plan.actor = Actor::Agent("mallory".into()),
            |plan| plan.tool = "glyph_update",
            |plan| plan.args["doc"] = json!("doc_2"),
            |plan| plan.base_rev = Rev(4),
            |plan| plan.expires += 1,
        ];
        for tamper in tamperings {
            let original = store.plans.values().next().cloned().expect("plan");
            if let Some(plan) = store.plans.values_mut().next() {
                tamper(plan);
            }
            let error = store.verify(&token, &alice).expect_err("tampered");
            assert_eq!(error.message, "plan token is not valid");
            if let Some(plan) = store.plans.values_mut().next() {
                *plan = original;
            }
        }
        store.verify(&token, &alice).expect("intact");

        // Another store (another process) has another key.
        let mut other = PlanStore::new(Box::new(FixedClock::at(1_000)));
        let id: PlanId = URL_SAFE_NO_PAD.decode(&token).expect("token")[..PLAN_ID_LEN]
            .try_into()
            .expect("id");
        let plan = store.plans.get(&id).cloned().expect("plan");
        other.plans.insert(id, plan);
        let error = other.verify(&token, &alice).expect_err("other key");
        assert_eq!(error.message, "plan token is not valid");
    }

    #[test]
    fn expiry_is_strict_after_the_ttl_and_ignored_by_discard() {
        let clock = FixedClock::at(1_000);
        let mut store = PlanStore::new(Box::new(clock.clone()));
        let alice = agent("alice");
        let (first, _) = store.create(new_plan(&alice, "doc_1")).expect("create");
        let (second, _) = store.create(new_plan(&alice, "doc_1")).expect("create");
        let (third, _) = store.create(new_plan(&alice, "doc_1")).expect("create");
        clock.advance(300);
        store.take(&first, &alice).expect("valid at the TTL");
        clock.advance(1);
        let error = store.take(&second, &alice).expect_err("expired");
        assert_eq!(error.message, "plan expired");
        // The lookup of `second` purged the other expired plans.
        assert_eq!(store.len(), 0);
        let error = store.discard(&third, &alice).expect_err("purged");
        assert_eq!(error.message, UNKNOWN_PLAN);

        // Discard ignores the expiry of a plan that is still stored.
        let (fourth, _) = store.create(new_plan(&alice, "doc_1")).expect("create");
        clock.advance(301);
        store.discard(&fourth, &alice).expect("discard");
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn the_oldest_plan_is_evicted_when_full_and_documents_take_theirs() {
        let mut store = PlanStore::new(Box::new(FixedClock::at(1_000)));
        let alice = agent("alice");
        let mut tokens = Vec::new();
        for index in 0..=MAX_PENDING_PLANS {
            let doc = if index % 2 == 0 {
                "doc_even"
            } else {
                "doc_odd"
            };
            tokens.push(store.create(new_plan(&alice, doc)).expect("create").0);
        }
        assert_eq!(store.len(), MAX_PENDING_PLANS);
        let error = store.verify(&tokens[0], &alice).expect_err("evicted");
        assert_eq!(error.message, UNKNOWN_PLAN);
        store.verify(&tokens[1], &alice).expect("kept");

        store.remove_doc(&DocHandle::from("doc_odd"));
        assert_eq!(store.len(), MAX_PENDING_PLANS / 2);
        store.verify(&tokens[2], &alice).expect("other document");
        let error = store.verify(&tokens[1], &alice).expect_err("closed");
        assert_eq!(error.message, UNKNOWN_PLAN);
    }

    #[test]
    fn the_oldest_plans_are_evicted_past_the_byte_budget() {
        let mut store = PlanStore::new(Box::new(FixedClock::at(1_000)));
        let alice = agent("alice");
        let plan_bytes = canonical_json(&without_dry_run(new_plan(&alice, "doc_1").args)).len();
        store.max_bytes = 3 * plan_bytes;
        let tokens: Vec<String> = (0..4)
            .map(|_| store.create(new_plan(&alice, "doc_1")).expect("create").0)
            .collect();
        assert_eq!(store.len(), 3);
        assert_eq!(store.bytes, 3 * plan_bytes);
        let error = store.verify(&tokens[0], &alice).expect_err("evicted");
        assert_eq!(error.message, UNKNOWN_PLAN);
        store.take(&tokens[1], &alice).expect("kept");
        assert_eq!(store.bytes, 2 * plan_bytes);
        store.remove_doc(&DocHandle::from("doc_1"));
        assert_eq!((store.len(), store.bytes), (0, 0));

        // Arguments larger than the whole budget are refused.
        store.max_bytes = plan_bytes - 1;
        let error = store
            .create(new_plan(&alice, "doc_1"))
            .expect_err("too large");
        assert_eq!(error.code, ErrorCode::LimitExceeded);
        assert_eq!(error.details["limit"], json!(plan_bytes - 1));
        assert_eq!(error.details["actual"], json!(plan_bytes));
        assert_eq!(store.len(), 0);
        assert_eq!(MAX_PENDING_PLAN_BYTES, 64 * 1024 * 1024);
    }

    #[test]
    fn bounded_diffs_keep_the_first_entries_and_full_counts() {
        let names = |prefix: &str, count: usize| -> Vec<String> {
            (0..count).map(|index| format!("{prefix}{index}")).collect()
        };
        let modified = |count: usize| -> Vec<GlyphDiff> {
            names("m", count)
                .into_iter()
                .map(|name| GlyphDiff {
                    name,
                    outline_changed: true,
                    advance_changed: false,
                    unicodes_changed: false,
                })
                .collect()
        };
        let diff = BoundedDiff::from(FontDiff {
            added: names("a", 250),
            removed: names("r", 100),
            modified: modified(101),
            info_changed: true,
            metrics_changed: false,
        });
        assert_eq!(
            (diff.added.len(), diff.added_count),
            (MAX_DIFF_ENTRIES, 250)
        );
        assert_eq!(diff.added[99], "a99");
        assert_eq!((diff.removed.len(), diff.removed_count), (100, 100));
        assert_eq!((diff.modified.len(), diff.modified_count), (100, 101));
        assert!(diff.truncated && diff.info_changed && !diff.metrics_changed);

        let small = BoundedDiff::from(FontDiff {
            added: names("a", 100),
            removed: Vec::new(),
            modified: modified(3),
            info_changed: false,
            metrics_changed: true,
        });
        assert!(!small.truncated);
        assert_eq!(small.modified_count, 3);
        let empty = BoundedDiff::empty();
        assert_eq!(
            (empty.added_count, empty.removed_count, empty.modified_count),
            (0, 0, 0)
        );
        assert!(!empty.truncated && !empty.info_changed);
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let store = PlanStore::new(Box::new(SystemClock));
        let key = store.key().expect("key");
        let text = format!("{store:?}");
        assert!(text.contains("pending: 0"), "{text}");
        assert!(!text.contains(&format!("{:?}", &key[..4])), "{text}");
        assert_ne!(key, [0; 32]);
        assert_ne!(
            PlanStore::new(Box::new(SystemClock)).key().expect("key"),
            key
        );
    }
}
