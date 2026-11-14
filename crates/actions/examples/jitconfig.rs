//! A runner key pair and its JIT config for a hosted install (`tests/m4/wp3/hosted-run.mjs`).
//!
//! - `jitconfig keygen <dir>` writes `<dir>/key.der` (the private key, PKCS#8) and prints the public half, SPKI DER in
//!   base64, which `POST /api/graftyard/v1/repos/{o}/{r}/actions/runners` takes as `key`.
//! - `jitconfig config <dir> <answer.json> <name>` prints the `--jitconfig` value from that route's answer: its
//!   `server_url_v2`, `authorization_url`, `client_id`, and `github_url`, with the private key from `<dir>`.

use base64::Engine as _;
use graftyard_actions::runsvc::{self, Urls};
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = &args[2];
    match args[1].as_str() {
        "keygen" => {
            let key = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
            std::fs::write(format!("{dir}/key.der"), key.to_pkcs8_der().unwrap().as_bytes()).unwrap();
            let public = key.to_public_key().to_public_key_der().unwrap();
            println!("{}", base64::engine::general_purpose::STANDARD.encode(public.as_bytes()));
        }
        "config" => {
            let key = rsa::RsaPrivateKey::from_pkcs8_der(&std::fs::read(format!("{dir}/key.der")).unwrap()).unwrap();
            let a: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&args[3]).unwrap()).unwrap();
            let server = a["server_url_v2"].as_str().unwrap().trim_end_matches("/_services/broker/");
            let urls = Urls::new(server);
            let at = runsvc::runner_of_client(a["client_id"].as_str().unwrap()).unwrap();
            print!("{}", runsvc::jit_config(&urls, at, &args[4], a["github_url"].as_str().unwrap(), &key));
        }
        other => panic!("unknown command {other}"),
    }
}
