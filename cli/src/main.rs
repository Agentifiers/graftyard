//! `graftyard`, the operator CLI (`design/DESIGN.md`, "Usage (caller's view)", the operator block; "Install
//! operations"). Milestone 1 builds `init`, `sites add`, `sites add-spare`, `placement set`, `genesis`, `sign`, and
//! `install --genesis`. The deploy token comes from `CLOUDFLARE_API_TOKEN`.

mod cf;
mod config;
mod genesis;
mod placement;
mod sites;

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
    /// Signs a genesis offline with the customer's private key. Needs no network and no install.toml.
    Sign {
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// The customer's Ed25519 private key: a 64-hex-character seed or a PEM PRIVATE KEY.
        #[arg(long)]
        key: PathBuf,
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
        Cmd::Sign { input, out, key } => genesis::sign(&input, &out, &key),
        Cmd::Install { genesis, edge } => genesis::install(config, &genesis, edge.as_deref()),
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
