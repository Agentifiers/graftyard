//! `graftyard`, the operator CLI (`design/DESIGN.md`, "Usage (caller's view)", the operator block; "Install
//! operations"). Milestone 1 builds `init`, `sites add`, `sites add-spare`, `placement set`, `genesis`, `sign`, and
//! `install --genesis`; Milestone 3 `admin bootstrap` and `mint-key rotate | step`; AM2 `apps init`. The deploy token
//! comes from `CLOUDFLARE_API_TOKEN`.

mod admin;
mod apps;
mod cf;
mod config;
mod genesis;
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
        /// The folder `wrangler deploy --dry-run --outdir` wrote for the vault Worker.
        #[arg(long)]
        bundle: PathBuf,
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
    },
}

#[derive(Subcommand)]
enum RunnersCmd {
    /// Writes `HostedRunnersAppKey`, deploys `graft-runners` with its sweep, and creates its Sandbox applications.
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
        /// The `graftyard-hosted-runners` App's private key (PEM).
        #[arg(long)]
        app_key: PathBuf,
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
        /// The folder for apps.json and the request files.
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
        Cmd::Release { cmd: ReleaseCmd::Roll { release, entries, key, ttl_s, out, edge } } => {
            release::roll(config, &release, &entries, &key, ttl_s, out.as_deref(), edge.as_deref())
        }
        Cmd::Install { genesis, edge } => genesis::install(config, &genesis, edge.as_deref()),
        Cmd::Vault { cmd: VaultCmd::Init { account, key, bundle, bucket, region, script, edge, days } } => vault::init(
            config,
            &vault::InitArgs {
                account: &account,
                key: &key,
                bundle: &bundle,
                bucket: &bucket,
                region: &region,
                script: &script,
                edge: edge.as_deref(),
                days,
            },
        ),
        Cmd::Vault { cmd: VaultCmd::Token { key, scope, holder, days, out } } => {
            vault::token(config, &vault::TokenArgs { key: &key, scope: &scope, holder: &holder, days, out: &out })
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
                app_key: &app_key,
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
        Cmd::Apps { cmd: AppsCmd::Init { host, app_ids, out } } => match (host, app_ids) {
            (_, Some(ids)) => apps::register(&out, &ids),
            (Some(host), None) => apps::init(config, &host, &out),
            (None, None) => Err("apps init takes --host or --app-ids".into()),
        },
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
