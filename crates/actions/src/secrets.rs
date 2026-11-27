//! `secrets-wait` (format 0x15.14, F142; `design/AGENT-LAYER.md`, Shape, `AgentPolicy`: "only jobs that reference
//! secrets wait"): whether a job's steps reference a repository secret. This decides only whether an agent run's job
//! waits for `AgentGate` or starts at once. It never decides what a job gets: a job that starts before the gate runs as
//! a fork job with no secret, so a reference this scan misses runs with none (`crate::shard`,
//! `ActionsEvent::SecretsWait`; the edge's `job_config`). The rules are M0-WP14's `secretRefs`
//! (`tests/m0/src/wp14/ci.ts`).

/// Whether `steps`, a job's steps as the runner's JSON (`crate::runsvc::steps_json`, with the workflow's and the
/// job's `env` already merged into each step's), reference a secret: `secrets.X`, `secrets['X']` or `secrets["X"]`
/// for any name but `GITHUB_TOKEN`, which is the run's token, or the whole map (`toJSON(secrets)`, a bare
/// `${{ secrets }}`, `secrets: inherit`). The text is matched without regard to case, as expressions are. Expressions
/// sit in the JSON as `"expr"` strings, so the whole text is scanned and a reference inside a string literal also
/// waits: waiting is the safe side.
pub fn references(steps: &str) -> bool {
    let t = steps.to_ascii_lowercase();
    let b = t.as_bytes();
    let word = |i: usize| i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_');
    let skip = |mut i: usize| {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        i
    };
    let mut from = 0;
    while let Some(p) = t[from..].find("secrets") {
        let at = from + p;
        from = at + "secrets".len();
        if (at > 0 && word(at - 1)) || word(from) {
            continue;
        }
        let i = skip(from);
        match b.get(i) {
            // `secrets.NAME`
            Some(b'.') => {
                let s = skip(i + 1);
                let mut e = s;
                while word(e) {
                    e += 1;
                }
                if e > s && &t[s..e] != "github_token" {
                    return true;
                }
            }
            // `secrets['NAME']` or `secrets["NAME"]`: any name but the run's token. A quote inside the JSON text
            // is escaped (`\"`), so only the name between the quotes is compared.
            Some(b'[') => {
                let rest = t[i + 1..].trim_start_matches([' ', '\'', '"', '\\']);
                let name: String = rest.chars().take_while(|c| !matches!(c, '\'' | '"' | '\\' | ']')).collect();
                if name.trim() != "github_token" {
                    return true;
                }
            }
            // `toJSON(secrets)`, `secrets: inherit`, or the map passed whole anywhere else.
            Some(b')' | b'}' | b',' | b':') => return true,
            // `${{ secrets }}` alone, which the converter writes as the expression `secrets`.
            Some(b'"') if t[..at].ends_with("\"expr\":\"") => return true,
            _ => {}
        }
    }
    false
}

/// Whether a job of `run` may read its secrets: any job of a run not marked fork, and of a fork run (a fork pull
/// request's, or an agent's) only one `GateHeld` released (`ActionsFold::released`), after `AgentGate` held for its
/// head. A released job keeps the fork run's read-only `GITHUB_TOKEN` and gets no ID token (`decisions/DECISIONS.md`,
/// "`secrets-wait`").
pub fn reads_secrets(run: &crate::shard::WorkflowRun, released: bool) -> bool {
    !run.fork || released
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(expr: &str) -> String {
        serde_json::json!([{ "type": "action", "inputs": { "type": 2, "map": [
            { "Key": "script", "Value": { "type": 3, "expr": expr } }
        ] } }])
        .to_string()
    }

    /// Each form M0's `secretRefs` counts makes a job wait, the run's own token and look-alike words do not, and the
    /// job- and workflow-level `env` reach the scan through the steps' merged `environment`.
    #[test]
    fn a_job_waits_for_each_secret_form_and_not_for_the_token_or_look_alikes() {
        for e in [
            "format('{0}', secrets.NPM_TOKEN)",
            "secrets . deploy_key",
            "secrets['NPM_TOKEN']",
            "Secrets[\"npm\"]",
            "toJSON(secrets)",
            "secrets",
            "join(secrets, ',')",
        ] {
            assert!(references(&step(e)), "{e} waits");
        }
        for e in ["secrets.GITHUB_TOKEN", "secrets['github_token']", "github.token", "vars.secrets_url", "mysecrets.X"]
        {
            assert!(!references(&step(e)), "{e} does not wait");
        }
        let env = serde_json::json!([{ "type": "action", "environment": { "type": 2, "map": [
            { "Key": "TOKEN", "Value": { "type": 3, "expr": "secrets.PUBLISH" } }
        ] } }])
        .to_string();
        assert!(references(&env));
    }

    /// The scan over a parsed workflow: a job whose `env:` (or the workflow's) names a secret waits, a job that names
    /// none does not, as `job_specs` builds them.
    #[test]
    fn a_workflow_or_job_env_secret_makes_only_its_jobs_wait() {
        let src = "on: push\njobs:\n  lint:\n    runs-on: x\n    steps: [{run: make lint}]\n  publish:\n    runs-on: x\n    env:\n      TOKEN: ${{ secrets.PUBLISH }}\n    steps: [{run: make publish}]\n";
        let p = crate::workflow::parse("ci.yml", src);
        assert!(p.ok(), "{:?}", p.errors);
        let specs = crate::specs::job_specs(&p.workflow).unwrap();
        let waits: Vec<(&str, bool)> = specs.iter().map(|s| (&*s.name, references(&s.steps))).collect();
        assert_eq!(waits, [("lint", false), ("publish", true)]);
        let src = "on: push\nenv:\n  K: ${{ secrets.K }}\njobs:\n  a:\n    runs-on: x\n    steps: [{run: a}]\n";
        let p = crate::workflow::parse("ci.yml", src);
        assert!(references(&crate::specs::job_specs(&p.workflow).unwrap()[0].steps));
    }
}
