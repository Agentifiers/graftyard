//! The API renderers every surface shares: accounts, author associations, and label colors. The cross-cutting wire
//! rules (headers, pagination, errors, hosts, ids) are `graftyard_wire`'s, re-exported here.

use graftyard_types::{AccountId, DbId};
pub use graftyard_wire::{
    FieldError, Host, NodeId, NodeType, Page, Reply, cursor, iso, parse_cursor, validation_error,
};
use serde_json::{Value, json};

use crate::view::{Directory, Repo};

/// GitHub's `simple-user` for an account, or `None` for a host bot with no account.
pub fn simple_user(h: &Host, dir: &dyn Directory, id: Option<AccountId>) -> Value {
    let Some(id) = id else { return Value::Null };
    if let Some(app) = graftyard_identity::AppId::of_bot(id) {
        return bot_user(h, dir, id, app);
    }
    let login = dir.login(id).unwrap_or_else(|| "ghost".into());
    let (api, web) = (h.api(), h.web());
    json!({
        "login": login, "id": id.0.0, "node_id": account_node(dir, id),
        "avatar_url": format!("{web}/identicons/{login}.png"), "gravatar_id": "",
        "url": format!("{api}/users/{login}"), "html_url": format!("{web}/{login}"),
        "followers_url": format!("{api}/users/{login}/followers"),
        "following_url": format!("{api}/users/{login}/following{{/other_user}}"),
        "gists_url": format!("{api}/users/{login}/gists{{/gist_id}}"),
        "starred_url": format!("{api}/users/{login}/starred{{/owner}}{{/repo}}"),
        "subscriptions_url": format!("{api}/users/{login}/subscriptions"),
        "organizations_url": format!("{api}/users/{login}/orgs"),
        "repos_url": format!("{api}/users/{login}/repos"),
        "events_url": format!("{api}/users/{login}/events{{/privacy}}"),
        "received_events_url": format!("{api}/users/{login}/received_events"),
        "type": if dir.is_org(id) { "Organization" } else { "User" }, "site_admin": false,
    })
}

/// GitHub's `simple-user` for an App's bot account (`AppId::bot`), as GitHub renders `github-actions[bot]`: `type`
/// `Bot`, a `BOT_` node id, `html_url` the App's page, and the login percent-encoded in the API URLs. `id` is the
/// App's id, positive as every GHES id is. The login is the one the directory holds (`{slug}[bot]`, which the edge
/// reads from the App's record); the install's Actions App, which has no record, is `github-actions[bot]` on every
/// surface, and any other App whose login was not read renders as `ghost[bot]` (`decisions/DECISIONS.md`, "`app-bot`").
fn bot_user(h: &Host, dir: &dyn Directory, id: AccountId, app: graftyard_identity::AppId) -> Value {
    let login = dir.login(id).unwrap_or_else(|| {
        if app == graftyard_identity::stateless::ACTIONS_APP { "github-actions[bot]" } else { "ghost[bot]" }.into()
    });
    let slug = login.strip_suffix("[bot]").unwrap_or(&login).to_owned();
    let path = login.replace('[', "%5B").replace(']', "%5D");
    let (api, web) = (h.api(), h.web());
    json!({
        "login": login, "id": app.0.0, "node_id": NodeId::top(NodeType::Bot, app.0.0).encode(),
        "avatar_url": format!("{web}/identicons/app/{slug}.png"), "gravatar_id": "",
        "url": format!("{api}/users/{path}"), "html_url": format!("{web}/apps/{slug}"),
        "followers_url": format!("{api}/users/{path}/followers"),
        "following_url": format!("{api}/users/{path}/following{{/other_user}}"),
        "gists_url": format!("{api}/users/{path}/gists{{/gist_id}}"),
        "starred_url": format!("{api}/users/{path}/starred{{/owner}}{{/repo}}"),
        "subscriptions_url": format!("{api}/users/{path}/subscriptions"),
        "organizations_url": format!("{api}/users/{path}/orgs"),
        "repos_url": format!("{api}/users/{path}/repos"),
        "events_url": format!("{api}/users/{path}/events{{/privacy}}"),
        "received_events_url": format!("{api}/users/{path}/received_events"),
        "type": "Bot", "site_admin": false,
    })
}

/// An account's node id: `O_` for an organization, `U_` for a user.
pub fn account_node(dir: &dyn Directory, id: AccountId) -> String {
    NodeId::top(if dir.is_org(id) { NodeType::Organization } else { NodeType::User }, id.0.0).encode()
}

/// A repository-scoped object's node id.
pub fn scoped(ty: NodeType, r: &Repo<'_>, id: i64) -> String {
    NodeId::scoped(ty, r.at.id, id).encode()
}

/// GitHub's `author_association` and GraphQL's `CommentAuthorAssociation` for an account on this repository.
pub fn association(r: &Repo<'_>, who: Option<AccountId>) -> &'static str {
    use graftyard_identity::RepoPermission as P;
    match who {
        Some(a) if a == r.at.owner_id => "OWNER",
        Some(a) => match r.dir.permission(a) {
            Some(P::Write | P::Maintain | P::Admin) => "COLLABORATOR",
            _ => "NONE",
        },
        None => "NONE",
    }
}

/// A label's hex color.
pub fn color(c: [u8; 3]) -> String {
    format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub fn db(id: DbId) -> i64 {
    id.0
}
