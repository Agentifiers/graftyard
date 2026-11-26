//! `graftyard`, the operator CLI (`design/DESIGN.md`, "Usage (caller's view)", the operator block; "Install
//! operations"). Milestone 1 builds `init`, `sites add`, `sites add-spare`, `placement set`, `genesis`, `sign`, and
//! `install --genesis`; Milestone 3 `admin bootstrap` and `mint-key rotate | step`; AM2 `apps init`; AM3 `agents init`.
//! The deploy token comes from `CLOUDFLARE_API_TOKEN`.

mod admin;
mod agents;
mod apps;
mod cf;
mod config;
mod genesis;
mod lifeboat;
mod oidc;
mod placement;
mod release;
mod runners;
mod sites;
mod ssh;
mod vault;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use graftyard_types::{Jurisdiction, Region};

use crate::config::{InstallToml, Res, mint_install, read_public_key};

#[derive(Parser)]
#[command(name = "graftyard", about = "Graftyard operator CLI")]
struct Cli {
    /// The install file every command reads and `init` writes.
    #[arg(long, global = true, default_value = "install.toml")]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Mints the install id and writes install.toml. Creates nothing in Cloudflare.
    Init {
        #[arg(long)]
        account: String,
        #[arg(long)]
        zone: String,
        /// The customer's Ed25519 public key: 64 hex characters or a PEM PUBLIC KEY.
        #[arg(long)]
        customer_key: PathBuf,
        /// The signed evaluation flag: lets `Solo` placements through, with no region-loss guarantee and no SLO.
        #[arg(long)]
        evaluation: bool,
    },
    Sites {
        #[command(subcommand)]
        cmd: SitesCmd,
    },
    Placement {
        #[command(subcommand)]
        cmd: PlacementCmd,
    },
    /// Builds the unsigned genesis and makes the drain and executor keys.
    Genesis {
        #[arg(long)]
        out: PathBuf,
        /// The vendor's release-signing public key.
        #[arg(long)]
        release_key: PathBuf,
        /// The vendor intent public key; leave it out for self-hosted (`vendor: None`).
        #[arg(long)]
        vendor_key: Option<PathBuf>,
        /// Sites for the system chains, by letter, id, or script (default: the first three active sites).
        #[arg(long, value_delimiter = ',')]
        sites: Option<Vec<String>>,
        /// Do not write the drain and executor private keys anywhere (tests only: the keys are then lost).
        #[arg(long)]
        offline_keys: bool,
    },
    /// Signs a genesis or any other `Release` entry offline. Needs no network and no install.toml.
    Sign {
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// The signer's Ed25519 private key: a 64-hex-character seed or a PEM PRIVATE KEY.
        #[arg(long)]
        key: PathBuf,
        /// Rebind the entry to this digest first (the `prev` a 409 named), dropping its earlier signatures.
        #[arg(long)]
        prev: Option<String>,
    },
    Admin {
        #[command(subcommand)]
        cmd: AdminCmd,
    },
    /// The install's first-party Apps, `Graftyard MCP` and `graftyard-insights` (`cli/src/apps.rs`).
    Apps {
        #[command(subcommand)]
        cmd: AppsCmd,
    },
    /// The agents account's first-party Apps, `graftyard-agent` and `graftyard-workspace` (`cli/src/agents.rs`).
    Agents {
        #[command(subcommand)]
        cmd: AgentsCmd,
    },
    /// The installation-token signing key in `MintWrapKey`.
    MintKey {
        #[command(subcommand)]
        cmd: MintKeyCmd,
    },
    /// Builds, posts, and reads `Release` entries.
    Release {
        #[command(subcommand)]
        cmd: ReleaseCmd,
    },
    /// `graft-runners` in the runner account, with `CLOUDFLARE_RUNNER_API_TOKEN` (`cli/src/runners.rs`).
    Runners {
        #[command(subcommand)]
        cmd: RunnersCmd,
    },
    /// The Actions OIDC issuer as relying parties reach it (`cli/src/oidc.rs`).
    Oidc {
        #[command(subcommand)]
        cmd: OidcCmd,
    },
    /// Native git over SSH (`cli/src/ssh.rs`).
    Ssh {
        #[command(subcommand)]
        cmd: SshCmd,
    },
    /// The vault Worker and export tokens for the lifeboat feed (`cli/src/vault.rs`).
    Vault {
        #[command(subcommand)]
        cmd: VaultCmd,
    },
    /// The lifeboat: a read-only copy of the install off Cloudflare (`cli/src/lifeboat.rs`).
    Lifeboat {
        #[command(subcommand)]
        cmd: LifeboatCmd,
    },
    /// Sends the signed genesis to the edge, which writes it to the empty Release group.
    Install {
        #[arg(long)]
        genesis: PathBuf,
        /// The edge's base URL (default: https://<zone>).
        #[arg(long)]
        edge: Option<String>,
    },
}

#[derive(Subcommand)]
enum VaultCmd {
    /// Deploys the vault Worker, its bucket and lock rules, under CLOUDFLARE_VAULT_API_TOKEN.
    Init {
        #[arg(long)]
        account: String,
        /// The customer's Ed25519 private key: a 64-hex-character seed or a PEM PRIVATE KEY.
        #[arg(long)]
        key: PathBuf,
        /// The release folder: release.json, release.sig, and the vault/ modules release.json lists.
        #[arg(long)]
        release: PathBuf,
        /// The vendor's release public key (the genesis's Keys.release): 64 hex characters or a PEM PUBLIC KEY.
        #[arg(long)]
        release_key: PathBuf,
        #[arg(long, default_value = "graft-vault")]
        bucket: String,
        /// The R2 location hint used only when the bucket is created.
        #[arg(long, default_value = "enam")]
        region: String,
        #[arg(long, default_value = "graft-vault")]
        script: String,
        /// The edge's base URL (default: https://<zone>).
        #[arg(long)]
        edge: Option<String>,
        /// The export token's life in days, at most 400.
        #[arg(long, default_value_t = 365)]
        days: u64,
    },
    /// Deploys a later release's vault Worker at 100%, keeping its bindings, under CLOUDFLARE_VAULT_API_TOKEN.
    Upgrade {
        #[arg(long)]
        account: String,
        /// The release folder: release.json, release.sig, and the vault/ modules release.json lists.
        #[arg(long)]
        release: PathBuf,
        /// The vendor's release public key (the genesis's Keys.release): 64 hex characters or a PEM PUBLIC KEY.
        #[arg(long)]
        release_key: PathBuf,
        #[arg(long, default_value = "graft-vault")]
        script: String,
    },
    /// Signs an export token offline and writes it to --out (mode 0600). Never prints the token or the key.
    Token {
        /// The customer's Ed25519 private key: a 64-hex-character seed or a PEM PRIVATE KEY.
        #[arg(long)]
        key: PathBuf,
        /// install, owner:<id>, or repo:<id>.
        #[arg(long, default_value = "install")]
        scope: String,
        #[arg(long, default_value = "vault")]
        holder: String,
        /// The token's life in days, at most 400.
        #[arg(long, default_value_t = 365)]
        days: u64,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum LifeboatCmd {
    /// Makes the host's own key, registers it with a `LifeboatAdd` the customer key signs, and writes the bundle's
    /// lifeboat.json (the install's trust anchor) and lifeboat.key (0600) into --dir.
    Init {
        #[arg(long)]
        host: String,
        /// The customer's Ed25519 private key: a 64-hex-character seed or a PEM PRIVATE KEY.
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        dir: PathBuf,
        /// The edge's base URL (default: https://<zone>).
        #[arg(long)]
        edge: Option<String>,
        /// The signed genesis the install committed (install.toml's `genesis` digest).
        #[arg(long)]
        genesis: PathBuf,
    },
    /// Mints the caller's `lifeboat:read` token from the main host with their own token, and writes it for gh and git.
    Enroll {
        #[arg(long)]
        host: String,
        /// The main host's base URL.
        #[arg(long)]
        edge: String,
        /// A file holding the developer's own token; never passed on the command line.
        #[arg(long)]
        token_file: PathBuf,
        /// gh's config directory (its hosts.yml gets the lifeboat host).
        #[arg(long)]
        gh_config: PathBuf,
        /// Where the lifeboat token is written (0600), for a git credential helper.
        #[arg(long)]
        out: PathBuf,
    },
    /// Writes the bundle's workerd config (0600) from --dir's lifeboat.json, lifeboat.key, and feed.json.
    Bundle {
        #[arg(long)]
        dir: PathBuf,
        /// The release's site/, edge/, and lifeboat/ builds.
        #[arg(long)]
        builds: PathBuf,
        /// The release the decoder reports: the SHA-256 of its release.json, hex.
        #[arg(long)]
        release: String,
        #[arg(long, default_value = "*:443")]
        listen: String,
        /// The front's TLS private key and certificate chain (PEM); without them it serves plain HTTP.
        #[arg(long, requires = "tls_chain")]
        tls_key: Option<PathBuf>,
        #[arg(long)]
        tls_chain: Option<PathBuf>,
        /// graft-blob's base URL.
        #[arg(long, default_value = "http://127.0.0.1:8790")]
        blob: String,
        /// A localhost socket for the puller's /status and /pass, such as 127.0.0.1:8791.
        #[arg(long)]
        admin: Option<String>,
        /// The copy's directory, fresh for each workerd version.
        #[arg(long)]
        state: PathBuf,
        /// The release's pinned workerd binary: the bundle records its version in --state, and start.sh runs only it.
        #[arg(long, default_value = "workerd")]
        workerd: String,
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        git: Box<BundleGit>,
    },
    /// Drops a dead or retired lifeboat with a `LifeboatRemove` the customer key signs.
    Remove {
        #[arg(long)]
        host: String,
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        edge: Option<String>,
    },
}

/// `lifeboat bundle`'s git flags: with `--git`, the bundle serves clones and fetches from graft-git.
#[derive(clap::Args)]
struct BundleGit {
    /// The release's lifeboat-git build (usr/local and shard-server): the bundle then serves clones and fetches
    /// from graft-git, which start.sh runs beside workerd.
    #[arg(long, requires_all = ["git_data", "blob_root"])]
    git: Option<PathBuf>,
    /// graft-git's repositories, fresh for each git version.
    #[arg(long)]
    git_data: Option<PathBuf>,
    /// graft-git's address: loopback only.
    #[arg(long, default_value = "127.0.0.1:8792")]
    git_listen: String,
    /// graft-blob's --root, whose packs the git directory's free-disk floor counts.
    #[arg(long)]
    blob_root: Option<PathBuf>,
}

#[derive(Subcommand)]
enum OidcCmd {
    /// Puts (or keeps) the zone's configuration rule that turns Browser Integrity Check off for the issuer's path
    /// `/_services/token/*` on the install's host only, and reads it back. Needs a token with zone Config Rules edit.
    BicRule {
        #[arg(long)]
        zone: String,
        /// The install's host, such as git.example.com.
        #[arg(long)]
        host: String,
    },
    /// Fetches the issuer's JWKS as a stock Python client (`Python-urllib`); fails on anything but 200.
    Check {
        /// The edge's base URL, such as https://git.example.com.
        #[arg(long)]
        edge: String,
    },
}

#[derive(Subcommand)]
enum SshCmd {
    /// Creates (or rewrites, or with --delete removes) the zone's Spectrum TCP app on port 22 whose Worker origin is
    /// the edge. Needs a token with zone Spectrum edit and account Workers Scripts read.
    Spectrum {
        #[arg(long)]
        account: String,
        #[arg(long)]
        zone: String,
        /// The SSH hostname clients use, such as ssh.example.com.
        #[arg(long)]
        hostname: String,
        /// The edge script.
        #[arg(long)]
        edge: String,
        #[arg(long)]
        delete: bool,
    },
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// Builds one unsigned entry from a JSON spec (`cli/src/release.rs`).
    Entry {
        spec: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// The digest of the group's last signed entry (a 409 names it).
        #[arg(long)]
        prev: Option<String>,
        /// For a `Keys` spec: the latest committed signed genesis or `Keys` entry.
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    /// Posts one signed entry and prints the answer.
    Post {
        input: PathBuf,
        #[arg(long)]
        edge: Option<String>,
    },
    /// Signs and submits release R's next intent in the roll order (`cli/src/release.rs`, `roll`).
    Roll {
        /// R: the release id, the SHA-256 of its `release.json`.
        release: String,
        /// The group's entries: a member's `/view` answers from position 0.
        #[arg(long)]
        entries: PathBuf,
        /// The intent key (vendor or customer), which also pre-signs the undos.
        #[arg(long)]
        key: PathBuf,
        /// How long the intent stays valid (`not_after`).
        #[arg(long, default_value_t = 86_400 * 7)]
        ttl_s: u64,
        /// Write the signed intent here instead of posting it.
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        edge: Option<String>,
        /// The vault account: once the code intents are done, upgrade its vault Worker to R under
        /// CLOUDFLARE_VAULT_API_TOKEN (never an install token). Without it the step is printed as owed.
        #[arg(long, requires_all = ["vault_release", "release_key"])]
        vault_account: Option<String>,
        /// R's release folder (release.json, release.sig, vault/).
        #[arg(long)]
        vault_release: Option<PathBuf>,
        /// The vendor's release public key (the genesis's Keys.release).
        #[arg(long)]
        release_key: Option<PathBuf>,
        #[arg(long, default_value = "graft-vault")]
        vault_script: String,
    },
}

#[derive(Subcommand)]
enum RunnersCmd {
    /// Writes `HostedRunnersAppKey`, deploys `graft-runners` with its sweep, and creates `RunnerBox`'s application. On
    /// an account at `v1` or `v2` it is release M3, which deletes the Sandbox classes and their applications.
    Init {
        #[arg(long)]
        account: String,
        /// `workers/runners/deploy.mjs`'s `runners.json`.
        #[arg(long)]
        spec: PathBuf,
        /// The folder `wrangler deploy --dry-run --outdir` wrote.
        #[arg(long)]
        bundle: PathBuf,
        /// The runner image in the runner account's registry, pinned by digest.
        #[arg(long)]
        image: String,
        /// The `graftyard-hosted-runners` App's private key (PEM), written to `HostedRunnersAppKey` only once the
        /// App accepts a JWT signed with it. Without it, the stored key is kept, and an account with none refuses.
        #[arg(long)]
        app_key: Option<PathBuf>,
        /// A file holding the App's webhook secret.
        #[arg(long)]
        webhook_secret: PathBuf,
    },
    /// Deploys a signed release: 10% first, then 100% and the new image on a run a day later.
    Upgrade {
        #[arg(long)]
        account: String,
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        image: String,
        /// The release's `release.json`, which lists every bundle file's SHA-256 and the image digest.
        #[arg(long)]
        record: PathBuf,
        /// Its Ed25519 signature: 64 bytes, or 128 hex characters.
        #[arg(long)]
        sig: PathBuf,
        /// The vendor's release-signing public key.
        #[arg(long)]
        release_key: PathBuf,
    },
}

#[derive(Subcommand)]
enum SitesCmd {
    /// Creates a site script, its bucket, and its bindings to every site, and checks its latency.
    Add {
        #[arg(long)]
        script: String,
        #[arg(long)]
        region: String,
        #[arg(long)]
        jurisdiction: Option<String>,
        /// `binding` (default) or `https` (the account-separated option, not built in Milestone 1).
        #[arg(long, default_value = "binding")]
        link: String,
        /// The site's own account, with `--link https`.
        #[arg(long)]
        account: Option<String>,
        /// The site bundle `worker-build --release` wrote.
        #[arg(long, default_value = "workers/site/build")]
        bundle: PathBuf,
        /// The git cache shard's container image in the account's registry, pinned by digest.
        #[arg(long)]
        image: String,
    },
    /// Creates one spare site per region, with no members and no warm containers.
    AddSpare {
        #[arg(long, value_delimiter = ',')]
        regions: Vec<String>,
        /// The spare scripts' names are `<prefix>-<region>`.
        #[arg(long, default_value = "graft-spare")]
        prefix: String,
        #[arg(long, default_value = "workers/site/build")]
        bundle: PathBuf,
        #[arg(long)]
        image: String,
    },
}

#[derive(Subcommand)]
enum AdminCmd {
    /// Draws a one-time sign-in link for the first site admin and writes its `PutCode` request.
    Bootstrap {
        /// The admin's account id.
        #[arg(long)]
        account: i64,
        /// A new file (mode 0600) for the link's secret.
        #[arg(long)]
        link_out: PathBuf,
        #[arg(long, default_value = "bootstrap.request.json")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum AppsCmd {
    /// Without --app-ids: draws the Apps' keys, the MCP client secret, and graft-mcp's sealing key into the install's
    /// Secrets Store, and writes apps.json and the slug claims. With --app-ids: writes the registration requests.
    Init {
        /// The install's host; `Graftyard MCP`'s callback is https://mcp.<host>/callback.
        #[arg(long, required_unless_present = "app_ids", conflicts_with = "app_ids")]
        host: Option<String>,
        /// The slug claims' answers: graftyard-mcp's App id, then graftyard-insights'.
        #[arg(long, value_delimiter = ',')]
        app_ids: Option<Vec<i64>>,
        /// The Secrets Store id to write to, instead of finding or creating graft-ops-<install>. The account has one
        /// store, which installs in it share.
        #[arg(long, conflicts_with = "app_ids")]
        store: Option<String>,
        /// Put this before each secret's name, so installs sharing a store do not collide.
        #[arg(long, default_value = "", conflicts_with = "app_ids")]
        secret_prefix: String,
        /// Where graft-mcp is served, when not at https://mcp.<host>; the callback is <origin>/callback.
        #[arg(long, conflicts_with = "app_ids")]
        mcp_origin: Option<String>,
        /// The folder for apps.json and the request files.
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum AgentsCmd {
    /// Without --app-ids: draws the Apps' keys, graftyard-agent's webhook secret, and the agents executor key into the
    /// agents account's Secrets Store, writes agents.json and the slug claims, and prints the executor public key for
    /// `Keys.agents_executor`. With --app-ids: writes the registration requests.
    Init {
        /// The install's host.
        #[arg(long, required_unless_present = "app_ids", conflicts_with = "app_ids")]
        host: Option<String>,
        /// The slug claims' answers: graftyard-agent's App id, then graftyard-workspace's.
        #[arg(long, value_delimiter = ',')]
        app_ids: Option<Vec<i64>>,
        /// The agents account, whose token CLOUDFLARE_API_TOKEN must be (default: install.toml's account).
        #[arg(long, conflicts_with = "app_ids")]
        account: Option<String>,
        /// The Secrets Store id to write to, instead of finding or creating graft-agents-<install>.
        #[arg(long, conflicts_with = "app_ids")]
        store: Option<String>,
        /// Put this before each secret's name, so installs sharing a store do not collide.
        #[arg(long, default_value = "", conflicts_with = "app_ids")]
        secret_prefix: String,
        /// graftyard-agent's webhook URL, the controller's hostname; without it the App has no webhook.
        #[arg(long, conflicts_with = "app_ids")]
        webhook_url: Option<String>,
        /// The folder for agents.json and the request files.
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum MintKeyCmd {
    /// Draws the next signing key: writes its `MintWrapKey` text and its `PutVerifyingKey` request.
    Rotate {
        /// The new key's `kid`, one above the current key's.
        #[arg(long)]
        kid: u32,
        /// A new file (mode 0600) for the `MintWrapKey` secret's text.
        #[arg(long)]
        secret_out: PathBuf,
        #[arg(long, default_value = "mint-key.request.json")]
        out: PathBuf,
    },
    /// Says what the rotation does next, and writes the old key's retire request when it is due.
    Step {
        #[arg(long)]
        old_kid: u32,
        /// The old key's public half: 64 hex characters or a PEM PUBLIC KEY.
        #[arg(long)]
        old_public: String,
        #[arg(long)]
        new_kid: u32,
        /// When the new key's `PutVerifyingKey` committed, in ms since the epoch.
        #[arg(long)]
        new_published: u64,
        /// When the secret switched to the new key, in ms since the epoch.
        #[arg(long)]
        switched: Option<u64>,
        #[arg(long, default_value = "mint-key-retire.request.json")]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum PlacementCmd {
    /// Checks a record with `Placement::new`'s rules and writes the request for the Names leader.
    Set {
        /// `default` for the install account's record, or an account id.
        owner: String,
        /// tri, na3, us2w, eu2w, apac2w, or solo.
        name: String,
        #[arg(long, alias = "site", value_delimiter = ',', required = true)]
        sites: Vec<String>,
        #[arg(long)]
        home: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn parse_region(s: &str) -> Res<Region> {
    serde_json::from_value(serde_json::Value::String(s.to_ascii_lowercase()))
        .map_err(|_| format!("{s:?} is not a region: enam, wnam, weur, eeur, apac, or oc"))
}

fn parse_jurisdiction(s: &str) -> Res<Jurisdiction> {
    serde_json::from_value(serde_json::Value::String(s.to_ascii_lowercase()))
        .map_err(|_| format!("{s:?} is not a jurisdiction: eu or us"))
}

fn init(config: &Path, account: String, zone: String, key: &Path, evaluation: bool) -> Res<()> {
    if config.exists() {
        return Err(format!(
            "{} exists: the install id is minted once, and the anchor never changes",
            config.display()
        ));
    }
    let customer = read_public_key(key)?;
    let id = mint_install();
    let t = InstallToml {
        install: id.to_string(),
        account,
        zone,
        customer_key: customer.hex(),
        evaluation,
        genesis: None,
        sites: Vec::new(),
        rtt: Vec::new(),
        default_placement: None,
    };
    t.anchor()?;
    t.save(config)?;
    println!("install {id}: wrote {} (customer key {}); created nothing", config.display(), customer.hex());
    Ok(())
}

fn run(cli: Cli) -> Res<()> {
    let config = &cli.config;
    match cli.cmd {
        Cmd::Init { account, zone, customer_key, evaluation } => init(config, account, zone, &customer_key, evaluation),
        Cmd::Sites { cmd: SitesCmd::Add { script, region, jurisdiction, link, account, bundle, image } } => {
            if link != "binding" || account.is_some() {
                return Err("--link https and --account are the account-separated option: its signed-HTTPS ring \
                            transport is not built in Milestone 1, so only --link binding in the install's account is"
                    .into());
            }
            let spec = sites::SiteSpec {
                script,
                region: parse_region(&region)?,
                jurisdiction: jurisdiction.as_deref().map(parse_jurisdiction).transpose()?,
                spare: false,
            };
            sites::add(config, &bundle, &image, vec![spec])
        }
        Cmd::Sites { cmd: SitesCmd::AddSpare { regions, prefix, bundle, image } } => {
            let specs = regions
                .iter()
                .map(|r| {
                    let region = parse_region(r)?;
                    Ok(sites::SiteSpec {
                        script: format!("{prefix}-{}", region.hint()),
                        region,
                        jurisdiction: None,
                        spare: true,
                    })
                })
                .collect::<Res<Vec<_>>>()?;
            sites::add(config, &bundle, &image, specs)
        }
        Cmd::Placement { cmd: PlacementCmd::Set { owner, name, sites, home, out } } => {
            let out = out.unwrap_or_else(|| PathBuf::from(format!("placement-{owner}.request.json")));
            placement::set(config, &owner, &name, &sites, home.as_deref(), &out)
        }
        Cmd::Genesis { out, release_key, vendor_key, sites, offline_keys } => {
            let t = InstallToml::load(config)?;
            let sites =
                sites.map(|s| s.iter().map(|x| placement::parse_site(&t, x)).collect::<Res<Vec<_>>>()).transpose()?;
            genesis::genesis(
                config,
                genesis::GenesisArgs {
                    out: &out,
                    release_key: &release_key,
                    vendor_key: vendor_key.as_deref(),
                    sites,
                    offline_keys,
                },
            )
        }
        Cmd::Sign { input, out, key, prev } => genesis::sign(&input, &out, &key, prev.as_deref()),
        Cmd::Release { cmd: ReleaseCmd::Entry { spec, out, prev, keys } } => {
            release::entry(config, &spec, &out, prev.as_deref(), keys.as_deref())
        }
        Cmd::Release { cmd: ReleaseCmd::Post { input, edge } } => release::post(config, &input, edge.as_deref()),
        Cmd::Release {
            cmd:
                ReleaseCmd::Roll {
                    release,
                    entries,
                    key,
                    ttl_s,
                    out,
                    edge,
                    vault_account,
                    vault_release,
                    release_key,
                    vault_script,
                },
        } => {
            let vault = match (&vault_account, &vault_release, &release_key) {
                (Some(account), Some(dir), Some(key)) => {
                    Some(release::VaultStep { account, script: &vault_script, release: dir, release_key: key })
                }
                _ => None,
            };
            release::roll(config, &release, &entries, &key, ttl_s, out.as_deref(), edge.as_deref(), vault.as_ref())
        }
        Cmd::Install { genesis, edge } => genesis::install(config, &genesis, edge.as_deref()),
        Cmd::Vault {
            cmd: VaultCmd::Init { account, key, release, release_key, bucket, region, script, edge, days },
        } => vault::init(
            config,
            &vault::InitArgs {
                account: &account,
                key: &key,
                release: vault::Release { dir: &release, release_key: &release_key },
                bucket: &bucket,
                region: &region,
                script: &script,
                edge: edge.as_deref(),
                days,
            },
        ),
        Cmd::Vault { cmd: VaultCmd::Upgrade { account, release, release_key, script } } => {
            vault::upgrade(&vault::UpgradeArgs {
                account: &account,
                script: &script,
                release: vault::Release { dir: &release, release_key: &release_key },
            })
        }
        Cmd::Lifeboat { cmd: LifeboatCmd::Init { host, key, dir, edge, genesis } } => lifeboat::init(
            config,
            lifeboat::InitArgs { host: &host, key: &key, dir: &dir, edge: edge.as_deref(), genesis: &genesis },
        ),
        Cmd::Lifeboat { cmd: LifeboatCmd::Enroll { host, edge, token_file, gh_config, out } } => {
            lifeboat::enroll(lifeboat::EnrollArgs {
                host: &host,
                edge: &edge,
                token_file: &token_file,
                gh_config: &gh_config,
                out: &out,
            })
        }
        Cmd::Lifeboat {
            cmd:
                LifeboatCmd::Bundle {
                    dir,
                    builds,
                    release,
                    listen,
                    tls_key,
                    tls_chain,
                    blob,
                    admin,
                    state,
                    workerd,
                    out,
                    git,
                },
        } => lifeboat::bundle(lifeboat::BundleArgs {
            dir: &dir,
            builds: &builds,
            release: &release,
            listen: &listen,
            tls: tls_key.as_deref().zip(tls_chain.as_deref()),
            blob: &blob,
            admin: admin.as_deref(),
            state: &state,
            workerd: &workerd,
            out: &out,
            git: git.git.as_deref().zip(git.git_data.as_deref()).zip(git.blob_root.as_deref()).map(
                |((build, data), blob_root)| lifeboat::GitArgs { build, data, listen: &git.git_listen, blob_root },
            ),
        }),
        Cmd::Lifeboat { cmd: LifeboatCmd::Remove { host, key, edge } } => {
            lifeboat::remove(config, &host, &key, edge.as_deref())
        }
        Cmd::Vault { cmd: VaultCmd::Token { key, scope, holder, days, out } } => {
            vault::token(config, &vault::TokenArgs { key: &key, scope: &scope, holder: &holder, days, out: &out })
        }
        Cmd::Oidc { cmd: OidcCmd::BicRule { zone, host } } => {
            let done = oidc::bic_rule(&cf::Cf::from_env("")?, &zone, &host)?;
            println!("{done:?}");
            Ok(())
        }
        Cmd::Oidc { cmd: OidcCmd::Check { edge } } => {
            let keys = oidc::check(&edge)?;
            println!("JWKS as {}: 200, {keys} keys", oidc::CHECK_AGENT);
            Ok(())
        }
        Cmd::Ssh { cmd: SshCmd::Spectrum { account, zone, hostname, edge, delete } } => {
            let done = ssh::spectrum(&cf::Cf::from_env(&account)?, &zone, &hostname, &edge, delete)?;
            println!("{done:?}");
            Ok(())
        }
        Cmd::Runners { cmd: RunnersCmd::Init { account, spec, bundle, image, app_key, webhook_secret } } => {
            runners::init(runners::InitArgs {
                account: &account,
                spec: &spec,
                bundle: &bundle,
                image: &image,
                app_key: app_key.as_deref(),
                webhook_secret: &webhook_secret,
            })
        }
        Cmd::Runners { cmd: RunnersCmd::Upgrade { account, spec, bundle, image, record, sig, release_key } } => {
            runners::upgrade(runners::UpgradeArgs {
                account: &account,
                spec: &spec,
                bundle: &bundle,
                image: &image,
                record: &record,
                sig: &sig,
                release_key: &release_key,
            })
        }
        Cmd::Admin { cmd: AdminCmd::Bootstrap { account, link_out, out } } => {
            admin::bootstrap(account, &link_out, &out)
        }
        Cmd::Apps { cmd: AppsCmd::Init { host, app_ids, store, secret_prefix, mcp_origin, out } } => {
            match (host, app_ids) {
                (_, Some(ids)) => apps::register(&out, &ids),
                (Some(host), None) => {
                    apps::init(config, &host, &apps::InitOpts { store, secret_prefix, mcp_origin }, &out)
                }
                (None, None) => Err("apps init takes --host or --app-ids".into()),
            }
        }
        Cmd::Agents { cmd: AgentsCmd::Init { host, app_ids, account, store, secret_prefix, webhook_url, out } } => {
            match (host, app_ids) {
                (_, Some(ids)) => agents::register(&out, &ids),
                (Some(host), None) => {
                    let opts = agents::InitOpts { store, secret_prefix, webhook_url };
                    agents::init(config, account.as_deref(), &host, &opts, &out)
                }
                (None, None) => Err("agents init takes --host or --app-ids".into()),
            }
        }
        Cmd::MintKey { cmd: MintKeyCmd::Rotate { kid, secret_out, out } } => admin::rotate(kid, &secret_out, &out),
        Cmd::MintKey { cmd: MintKeyCmd::Step { old_kid, old_public, new_kid, new_published, switched, out } } => {
            admin::step(&admin::StepArgs {
                old_kid,
                old_public: &old_public,
                new_kid,
                new_published,
                switched,
                out: &out,
            })
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("graftyard: {e}");
            ExitCode::FAILURE
        }
    }
}
