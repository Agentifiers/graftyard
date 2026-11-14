//! The parser's differential test: every `tests/m4/wp3/workflows/*.yml` parsed by the Rust port must give the JSON
//! `@actions/workflow-parser` 0.3.61 gave (`*.oracle.json`, written by `tests/m4/wp3/oracle.mjs`), errors included.
//! The `corpus-*` files are public workflows, the others cases written for the port.
//!
//! One difference is allowed: a YAML syntax error's text comes from the YAML library (`yaml` in the oracle,
//! `yaml-rust2` here), so for a file that does not parse as YAML only the error's position must match.

use std::path::Path;

use graftyard_actions::workflow;

#[test]
fn the_port_matches_the_oracle_on_every_workflow() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/m4/wp3/workflows");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml"))
        .collect();
    files.sort();
    assert!(files.len() >= 20, "the corpus is missing: {}", dir.display());
    let mut failed = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_str().unwrap();
        let src = std::fs::read_to_string(f).unwrap();
        let want: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(f.with_extension("oracle.json")).unwrap()).unwrap();
        let got = workflow::parse(name, &src).oracle_json();
        if got != want && !(yaml_error_at(&want).is_some() && yaml_error_at(&want) == yaml_error_at(&got)) {
            failed.push(format!(
                "{name}\n  want {}\n  got  {}",
                serde_json::to_string(&want).unwrap(),
                serde_json::to_string(&got).unwrap()
            ));
        }
    }
    assert!(failed.is_empty(), "{} of {} differ:\n{}", failed.len(), files.len(), failed.join("\n"));
}

/// The position of a lone YAML syntax error, `file: ... at line L, column C`.
fn yaml_error_at(v: &serde_json::Value) -> Option<(u64, u64)> {
    let errs = v.get("errors")?.as_array()?;
    if errs.len() != 1 || v.get("jobs").is_some() {
        return None;
    }
    let m = errs[0].get("Message")?.as_str()?;
    let (_, rest) = m.split_once(" at line ")?;
    let (line, rest) = rest.split_once(", column ")?;
    let col: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((line.parse().ok()?, col.parse().ok()?))
}
