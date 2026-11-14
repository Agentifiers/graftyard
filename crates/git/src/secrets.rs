//! A repository's Actions secrets and variables, and its environments' (`design/SHARED.md`, "Actions", Secrets and
//! variables; `build/BUILD-PLAN.md`, M4-WP3, BF-59).
//!
//! - **Rows.** `RepoEvent::ActionsSecret` (tag 44) and `ActionsVariable` (tag 45) write the core `actions_secret` and
//!   `actions_variable` rows, keyed by scope (`""` for the repository, `env:<name>` for an environment) and name.
//!   A secret's row holds only the `crypto_box_seal` bytes the client sent and the owner's `key_id`, so no entry and
//!   no row holds a plaintext; only `graft-ops` opens one, at `acquirejob`.
//! - **Commands.** `IssueOp::PutActionsSecret`, `DeleteActionsSecret`, `PutActionsVariable`, and
//!   `DeleteActionsVariable`, admin only (an App needs `secrets: write` or `actions_variables: write`). A variable's
//!   create refuses an existing name, as GHES answers 409, so `gh variable set` falls back to its PATCH.
//! - **Format 32.23** (`REPO_FORMAT_SECRETS`): the four commands and the two events. 32.22 is main's
//!   `REPO_FORMAT_REVIEWS`. A member reads it only after a site day onto a release that lists it, and the edge sends
//!   none before `FormatsEnable` enabled it.

use graftyard_fold::{Read, Rows, Val};
use graftyard_identity::sealed::{ActionsName, EnvName, SecretRecord, SecretScope, VariableRecord};
use graftyard_raft::Format;

use crate::repo::RepoEvent;
use crate::tiers::{get_rec, put_rec};

pub const REPO_FORMAT_SECRETS: Format = Format { family: 0x20, version: 23 };

/// GitHub's limits: 100 secrets and 500 variables a repository, 48 KB a secret, 48 KB a variable
/// (docs.github.com, "Limits for secrets" and "Limits for configuration variables") [doc].
pub const SECRETS_PER_SCOPE_MAX: usize = 100;
pub const VARIABLES_PER_SCOPE_MAX: usize = 500;
pub const VALUE_MAX_BYTES: usize = 48 * 1024;

fn scope_key(s: &SecretScope) -> Option<Val> {
    match s {
        SecretScope::Repo => Some(Val::Text("".into())),
        SecretScope::Env(e) => Some(Val::Text(format!("env:{}", e.0).into())),
        SecretScope::Org { .. } => None,
    }
}

fn key(s: &SecretScope, n: &ActionsName) -> Option<Vec<Val>> {
    Some(vec![scope_key(s)?, Val::Text(n.as_str().into())])
}

fn scope_of(v: &Val) -> Option<SecretScope> {
    match v {
        Val::Text(t) if t.is_empty() => Some(SecretScope::Repo),
        Val::Text(t) => t.strip_prefix("env:").map(|e| SecretScope::Env(EnvName(e.into()))),
        _ => None,
    }
}

/// One scope's rows of `table`, by name.
fn rows<T: serde::de::DeserializeOwned>(
    core: &(impl Read + ?Sized),
    table: &str,
    s: &SecretScope,
) -> Vec<(ActionsName, T)> {
    let Some(sk) = scope_key(s) else { return Vec::new() };
    core.prefix(table, std::slice::from_ref(&sk))
        .into_iter()
        .filter_map(|(k, _)| match k.get(1) {
            Some(Val::Text(n)) => {
                let name = ActionsName::parse(n)?;
                Some((name, get_rec(core, table, &k)?))
            }
            _ => None,
        })
        .collect()
}

pub fn secret(core: &(impl Read + ?Sized), s: &SecretScope, n: &ActionsName) -> Option<SecretRecord> {
    get_rec(core, "actions_secret", &key(s, n)?)
}

pub fn variable(core: &(impl Read + ?Sized), s: &SecretScope, n: &ActionsName) -> Option<VariableRecord> {
    get_rec(core, "actions_variable", &key(s, n)?)
}

pub fn secrets(core: &(impl Read + ?Sized), s: &SecretScope) -> Vec<(ActionsName, SecretRecord)> {
    rows(core, "actions_secret", s)
}

pub fn variables(core: &(impl Read + ?Sized), s: &SecretScope) -> Vec<(ActionsName, VariableRecord)> {
    rows(core, "actions_variable", s)
}

/// Every secret of the repository and its environments, for the rotation job's reseal.
pub fn all_secrets(core: &(impl Read + ?Sized)) -> Vec<(SecretScope, ActionsName, SecretRecord)> {
    core.prefix("actions_secret", &[])
        .into_iter()
        .filter_map(|(k, _)| {
            let s = scope_of(k.first()?)?;
            let Val::Text(n) = k.get(1)? else { return None };
            Some((s, ActionsName::parse(n)?, get_rec(core, "actions_secret", &k)?))
        })
        .collect()
}

pub(crate) fn write_core(w: &mut dyn Rows, ev: &RepoEvent) {
    match ev {
        RepoEvent::ActionsSecret { scope, name, record, .. } => {
            let Some(k) = key(scope, name) else { return };
            match record {
                Some(r) => put_rec(w, "actions_secret", k, r),
                None => w.delete("actions_secret", &k),
            }
        }
        RepoEvent::ActionsVariable { scope, name, record, .. } => {
            let Some(k) = key(scope, name) else { return };
            match record {
                Some(r) => put_rec(w, "actions_variable", k, r),
                None => w.delete("actions_variable", &k),
            }
        }
        _ => {}
    }
}
