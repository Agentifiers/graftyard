//! The workflow engine's parser: a Rust port of the runner's `GitHub.Actions.WorkflowParser` with `MatrixBuilder`
//! (SHARED, "Actions", Workflow engine), differentially tested against `@actions/workflow-parser` 0.3.61
//! (`crates/actions/tests/workflows.rs`).
//!
//! - `template` reads YAML into template tokens and validates them against the bundled `workflow-v1.0` schema.
//! - This module converts the tokens as the oracle's `convertWorkflowTemplate` does: events with their filters, jobs
//!   with `needs` checked for unknown jobs and cycles, `if` conditions given a status function, and step ids.
//!   `Workflow::oracle_json` writes the oracle's JSON, which the differential test compares.
//! - `matrix` expands `strategy` as the runner's C# `ConvertToStrategy` and `MatrixBuilder` do; the npm oracle has
//!   no matrix builder.
//! - `filter` decides whether an event's ref, paths, and activity type match `on:`.
//!
//! Errors and step ids follow the oracle, which is what the differential test checks. Where the oracle and the C#
//! parser differ in a message (the C# says "and must be" where the npm says "and and must be"), the oracle's text is
//! kept [measured: both sources, 2026-11-14].

pub mod expr;
pub mod filter;
pub mod matrix;
pub mod template;

use serde_json::{Map, Value, json};

use template::{Errors, Node, T};

/// A job's step as the converter keeps it. The oracle writes only `id`, `name`, `if`, `continue-on-error`, `env`,
/// and `run` or `uses`; the rest is read for the job message.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub id: String,
    pub name: Option<Node>,
    pub cond: String,
    pub continue_on_error: Option<Node>,
    pub env: Option<Node>,
    pub run: Option<Node>,
    pub uses: Option<String>,
    pub with: Option<Node>,
    pub shell: Option<Node>,
    pub working_directory: Option<Node>,
    pub timeout_minutes: Option<Node>,
}

/// A reusable-workflow call (`jobs.<id>.uses`). Parsed and kept; running one is not built yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub uses: String,
    pub with: Option<Node>,
    pub secrets: Option<Node>,
    pub inherit_secrets: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    /// The job's `name`, or its id: a string or an expression.
    pub name: Node,
    pub needs: Option<Vec<String>>,
    pub cond: String,
    pub env: Option<Node>,
    pub concurrency: Option<Node>,
    pub environment: Option<Node>,
    pub strategy: Option<Node>,
    pub runs_on: Option<Node>,
    pub container: Option<Node>,
    pub services: Option<Node>,
    pub outputs: Option<Node>,
    pub steps: Vec<Step>,
    pub timeout_minutes: Option<Node>,
    pub continue_on_error: Option<Node>,
    pub defaults: Option<Node>,
    pub call: Option<Call>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workflow {
    pub name: Option<Node>,
    /// The oracle's `events` object: event name to its filters (`branches`, `paths-ignore`, `types`, and so on).
    pub events: Option<Map<String, Value>>,
    pub env: Option<Node>,
    pub concurrency: Option<Node>,
    pub defaults: Option<Node>,
    pub jobs: Vec<Job>,
}

/// A parsed workflow file. `errors` are the oracle's messages; a workflow with any is not run.
pub struct Parsed {
    pub workflow: Workflow,
    pub errors: Vec<String>,
    /// Errors came from reading, before conversion: the oracle then answers only its errors.
    read_failed: bool,
}

impl Parsed {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The oracle's `convertWorkflowTemplate` result as JSON.
    pub fn oracle_json(&self) -> Value {
        let mut out = Map::new();
        if !self.read_failed {
            let w = &self.workflow;
            if let Some(e) = &w.events {
                out.insert("events".into(), Value::Object(e.clone()));
            }
            if let Some(c) = &w.concurrency {
                out.insert("concurrency".into(), c.json());
            }
            if let Some(e) = &w.env {
                out.insert("env".into(), e.json());
            }
            if !w.jobs.is_empty() || w.events.is_some() {
                out.insert("jobs".into(), Value::Array(w.jobs.iter().map(job_json).collect()));
            }
        }
        if !self.errors.is_empty() {
            let errs = self.errors.iter().map(|m| json!({"Message": m})).collect();
            out.insert("errors".into(), Value::Array(errs));
        }
        Value::Object(out)
    }
}

fn put(m: &mut Map<String, Value>, k: &str, v: Option<&Node>) {
    if let Some(v) = v {
        m.insert(k.into(), v.json());
    }
}

fn cond_json(c: &str) -> Value {
    json!({"type": 3, "expr": c})
}

fn job_json(j: &Job) -> Value {
    let mut m = Map::new();
    let reusable = j.call.is_some();
    m.insert("type".into(), Value::from(if reusable { "reusableWorkflowJob" } else { "job" }));
    m.insert("id".into(), Value::from(j.id.clone()));
    m.insert("name".into(), j.name.json());
    match (&j.needs, reusable) {
        (Some(n), _) => _ = m.insert("needs".into(), json!(n)),
        (None, true) => _ = m.insert("needs".into(), json!([])),
        (None, false) => {}
    }
    m.insert("if".into(), cond_json(&j.cond));
    if let Some(c) = &j.call {
        m.insert("ref".into(), Value::from(c.uses.clone()));
        put(&mut m, "input-values", c.with.as_ref());
        put(&mut m, "secret-values", c.secrets.as_ref());
        if c.inherit_secrets {
            m.insert("inherit-secrets".into(), Value::Bool(true));
        }
        put(&mut m, "concurrency", j.concurrency.as_ref());
        put(&mut m, "strategy", j.strategy.as_ref());
        return Value::Object(m);
    }
    put(&mut m, "env", j.env.as_ref());
    put(&mut m, "concurrency", j.concurrency.as_ref());
    put(&mut m, "environment", j.environment.as_ref());
    put(&mut m, "strategy", j.strategy.as_ref());
    put(&mut m, "runs-on", j.runs_on.as_ref());
    put(&mut m, "container", j.container.as_ref());
    put(&mut m, "services", j.services.as_ref());
    put(&mut m, "outputs", j.outputs.as_ref());
    let steps = j
        .steps
        .iter()
        .map(|s| {
            let mut o = Map::new();
            o.insert("id".into(), Value::from(s.id.clone()));
            put(&mut o, "name", s.name.as_ref());
            o.insert("if".into(), cond_json(&s.cond));
            if let Some(c) = &s.continue_on_error {
                o.insert("continue-on-error".into(), c.json());
            }
            put(&mut o, "env", s.env.as_ref());
            put(&mut o, "run", s.run.as_ref());
            if let Some(u) = &s.uses {
                o.insert("uses".into(), Value::from(u.clone()));
            }
            Value::Object(o)
        })
        .collect();
    m.insert("steps".into(), Value::Array(steps));
    Value::Object(m)
}

/// The oracle's `TemplateTokenError` for a token of the wrong type.
fn wrong(n: &Node, what: &str, want: &str) -> String {
    format!("Unexpected type '{}' encountered while reading '{what}'. The type '{want}' was expected.", n.type_name())
}

struct Cx {
    errs: Errors,
}

impl Cx {
    fn string<'n>(&mut self, n: &'n Node, what: &str) -> Option<&'n str> {
        let s = n.str();
        if s.is_none() {
            self.errs.at(n, wrong(n, what, "StringToken"));
        }
        s
    }
    fn mapping<'n>(&mut self, n: &'n Node, what: &str) -> Option<&'n [(Node, Node)]> {
        let m = n.map();
        if m.is_none() {
            self.errs.at(n, wrong(n, what, "MappingToken"));
        }
        m
    }
    fn sequence<'n>(&mut self, n: &'n Node, what: &str) -> Option<&'n [Node]> {
        let s = n.seq();
        if s.is_none() {
            self.errs.at(n, wrong(n, what, "SequenceToken"));
        }
        s
    }
    fn boolean(&mut self, n: &Node, what: &str) -> Option<bool> {
        match n.t {
            T::Bool(b) => Some(b),
            _ => {
                self.errs.at(n, wrong(n, what, "BooleanToken"));
                None
            }
        }
    }
    fn scalar<'n>(&mut self, n: &'n Node, what: &str) -> Option<&'n Node> {
        if n.is_literal() || n.is_expr() {
            return Some(n);
        }
        self.errs.at(n, wrong(n, what, "ScalarToken"));
        None
    }
    fn strings(&mut self, n: &Node, what: &str) -> Option<Vec<String>> {
        if let Some(s) = n.str() {
            return Some(vec![s.into()]);
        }
        let seq = self.sequence(n, &format!("{what} list"))?;
        seq.iter().map(|i| self.string(i, &format!("{what} item")).map(String::from)).collect()
    }
}

/// Parses one workflow file. `file` names it in errors.
pub fn parse(file: &str, src: &str) -> Parsed {
    let mut errs = Errors::new(file);
    let root = match template::load(src) {
        Ok(r) => r,
        Err(e) => {
            errs.list.push(format!("{file}: {e}"));
            return Parsed { workflow: Workflow::default(), errors: errs.list, read_failed: true };
        }
    };
    let root = template::read(&root, "workflow-root-strict", &mut errs);
    if !errs.list.is_empty() {
        return Parsed { workflow: Workflow::default(), errors: errs.list, read_failed: true };
    }
    let mut cx = Cx { errs };
    let mut w = Workflow::default();
    if let Some(pairs) = cx.mapping(&root, "root") {
        for (k, v) in pairs {
            match k.str() {
                Some("on") => w.events = Some(convert_on(&mut cx, v)),
                Some("jobs") => w.jobs = convert_jobs(&mut cx, v),
                Some("concurrency") => w.concurrency = Some(v.clone()),
                Some("env") => w.env = Some(v.clone()),
                Some("name") => w.name = Some(v.clone()),
                Some("defaults") => w.defaults = Some(v.clone()),
                _ => {}
            }
        }
    }
    Parsed { workflow: w, errors: cx.errs.list, read_failed: false }
}

// ---- Events

fn convert_on(cx: &mut Cx, n: &Node) -> Map<String, Value> {
    let mut out = Map::new();
    match &n.t {
        T::Seq(items) => {
            for i in items {
                if let Some(e) = cx.string(i, "on") {
                    out.insert(e.into(), json!({}));
                }
            }
        }
        T::Map(pairs) => {
            for (k, v) in pairs {
                let Some(name) = cx.string(k, "event name") else { continue };
                if matches!(v.t, T::Null) {
                    out.insert(name.into(), json!({}));
                    continue;
                }
                if name == "schedule" {
                    if let Some(seq) = cx.sequence(v, "event schedule") {
                        out.insert(name.into(), Value::Array(convert_schedule(cx, seq)));
                    }
                    continue;
                }
                let Some(m) = cx.mapping(v, &format!("event {name}")) else { continue };
                let ev = match name {
                    "workflow_call" => convert_call_event(cx, m),
                    "workflow_dispatch" => convert_dispatch(cx, m),
                    _ => {
                        let mut f = Map::new();
                        for key in ["branches", "tags", "paths"] {
                            for k2 in [key.to_string(), format!("{key}-ignore")] {
                                if let Some(x) = node_get(m, &k2)
                                    && let Some(l) = cx.strings(x, &k2)
                                {
                                    f.insert(k2, json!(l));
                                }
                            }
                        }
                        for key in ["types", "versions", "names", "workflows"] {
                            if let Some(x) = node_get(m, key)
                                && let Some(l) = cx.strings(x, key)
                            {
                                f.insert(key.into(), json!(l));
                            }
                        }
                        f
                    }
                };
                out.insert(name.into(), Value::Object(ev));
            }
        }
        _ => {
            if let Some(e) = cx.string(n, "on") {
                out.insert(e.into(), json!({}));
            }
        }
    }
    out
}

fn node_get<'n>(m: &'n [(Node, Node)], k: &str) -> Option<&'n Node> {
    m.iter().rev().find(|(key, _)| key.str() == Some(k)).map(|(_, v)| v)
}

fn convert_schedule(cx: &mut Cx, seq: &[Node]) -> Vec<Value> {
    let mut out = Vec::new();
    for item in seq {
        let Some(m) = cx.mapping(item, "event schedule") else { continue };
        let mut cfg = Map::new();
        let mut valid = true;
        for (k, v) in m {
            match k.str() {
                Some("cron") => {
                    if let Some(c) = cx.string(v, "schedule cron") {
                        if !valid_cron(c) {
                            cx.errs.at(
                                v,
                                "Invalid cron expression. Expected format: '* * * * *' (minute hour day month weekday)",
                            );
                        }
                        cfg.insert("cron".into(), Value::from(c));
                    }
                }
                Some("timezone") => {
                    if let Some(t) = cx.string(v, "schedule timezone") {
                        cfg.insert("timezone".into(), Value::from(t));
                    }
                }
                _ => {
                    cx.errs.at(k, "Invalid schedule key");
                    valid = false;
                }
            }
        }
        let has_cron = cfg.get("cron").and_then(Value::as_str).is_some_and(|c| !c.is_empty());
        if valid && has_cron {
            out.push(Value::Object(cfg));
        } else if valid {
            cx.errs.at(item, "Missing required key 'cron' in schedule entry");
        }
    }
    out
}

/// The oracle's `isValidCron`.
pub fn valid_cron(c: &str) -> bool {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
    fn number(v: &str, names: Option<(&[&str], i64)>) -> f64 {
        if let Some((ns, base)) = names
            && let Some(i) = ns.iter().position(|n| n.eq_ignore_ascii_case(v))
        {
            return (i as i64 + base) as f64;
        }
        expr::js_number(v)
    }
    fn part(v: &str, lo: f64, hi: f64, names: Option<(&[&str], i64)>, seps: bool) -> bool {
        if names.is_some_and(|(ns, _)| ns.iter().any(|n| n.eq_ignore_ascii_case(v))) || v == "*" {
            return true;
        }
        if v.contains(',') {
            return seps && v.split(',').all(|x| !x.is_empty() && part(x, lo, hi, names, true));
        }
        if v.contains('/') {
            let ps: Vec<&str> = v.split('/').collect();
            let step = expr::js_number(ps.get(1).copied().unwrap_or(""));
            return seps
                && ps.len() == 2
                && !step.is_nan()
                && step > 0.0
                && !ps[0].is_empty()
                && part(ps[0], lo, hi, names, true)
                && part(ps[1], lo, hi, names, false);
        }
        if v.contains('-') {
            let ps: Vec<&str> = v.split('-').collect();
            return seps
                && ps.len() == 2
                && !ps[0].is_empty()
                && !ps[1].is_empty()
                && part(ps[0], lo, hi, names, false)
                && part(ps[1], lo, hi, names, false)
                && number(ps[1], names) >= number(ps[0], names);
        }
        let x = expr::js_number(v);
        !x.is_nan() && x >= lo && x <= hi
    }
    let ps: Vec<&str> = c.split(' ').filter(|p| !p.is_empty()).collect();
    if c.split(' ').filter(|p| !p.is_empty()).count() != 5 || c.starts_with(' ') && c.trim().is_empty() {
        return false;
    }
    part(ps[0], 0.0, 59.0, None, true)
        && part(ps[1], 0.0, 23.0, None, true)
        && part(ps[2], 1.0, 31.0, None, true)
        && part(ps[3], 1.0, 12.0, Some((&MONTHS, 1)), true)
        && part(ps[4], 0.0, 6.0, Some((&DAYS, 0)), true)
}

fn convert_inputs(cx: &mut Cx, m: &[(Node, Node)], call: bool) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in m {
        let Some(name) = cx.string(k, "input name") else { continue };
        let Some(cfg) = cx.mapping(v, "input configuration") else { continue };
        let mut r = Map::new();
        r.insert("type".into(), Value::from("string"));
        let mut default = None;
        for (ik, iv) in cfg {
            let Some(key) = cx.string(ik, "workflow dispatch input key") else { continue };
            match key {
                "description" => {
                    if let Some(s) = cx.string(iv, "input description") {
                        r.insert("description".into(), Value::from(s));
                    }
                }
                "required" => {
                    if let Some(b) = cx.boolean(iv, "input required") {
                        r.insert("required".into(), Value::Bool(b));
                    }
                }
                "default" => default = cx.scalar(iv, "input default"),
                "type" => {
                    if let Some(t) = cx.string(iv, "input type") {
                        if ["string", "choice", "boolean", "environment"].contains(&t) {
                            r.insert("type".into(), Value::from(t));
                        } else {
                            r.remove("type");
                        }
                    }
                }
                "options" => {
                    if let Some(seq) = cx.sequence(iv, "input options") {
                        let l: Option<Vec<String>> =
                            seq.iter().map(|i| cx.string(i, "input options item").map(String::from)).collect();
                        if let Some(l) = l {
                            r.insert("options".into(), json!(l));
                        }
                    }
                }
                _ => cx.errs.at(ik, format!("Invalid key '{key}'")),
            }
        }
        let ty = r.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(d) = default
            && !(call && d.is_expr())
        {
            match ty.as_str() {
                "boolean" => {
                    if let Some(b) = cx.boolean(d, "input default") {
                        r.insert("default".into(), Value::Bool(b));
                    }
                }
                "string" | "choice" | "environment" => {
                    if let Some(s) = cx.string(d, "input default") {
                        r.insert("default".into(), Value::from(s));
                    }
                }
                _ => {}
            }
        }
        let has_options = r.get("options").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
        if ty == "choice" {
            if !has_options {
                cx.errs.at(v, "Missing 'options' for choice input");
            }
        } else if r.contains_key("options") {
            cx.errs.at(v, "Input type is not 'choice', but 'options' is defined");
        }
        out.insert(name.into(), Value::Object(r));
    }
    out
}

fn convert_dispatch(cx: &mut Cx, m: &[(Node, Node)]) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in m {
        if cx.string(k, "workflow dispatch input key") == Some("inputs")
            && let Some(im) = cx.mapping(v, "workflow dispatch inputs")
        {
            out.insert("inputs".into(), Value::Object(convert_inputs(cx, im, false)));
        }
    }
    out
}

fn convert_call_event(cx: &mut Cx, m: &[(Node, Node)]) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in m {
        match cx.string(k, "workflow dispatch input key") {
            Some("inputs") => {
                if let Some(im) = cx.mapping(v, "workflow dispatch inputs") {
                    out.insert("inputs".into(), Value::Object(convert_inputs(cx, im, true)));
                }
            }
            Some("secrets") => {
                if let Some(sm) = cx.mapping(v, "workflow dispatch inputs") {
                    let mut secrets = Map::new();
                    for (sk, sv) in sm {
                        let Some(name) = cx.string(sk, "secret name") else { continue };
                        let mut r = Map::new();
                        for (pk, pv) in sv.map().unwrap_or_default() {
                            match cx.string(pk, "workflow call secret key") {
                                Some("description") => {
                                    if let Some(s) = cx.string(pv, "secret description") {
                                        r.insert("description".into(), Value::from(s));
                                    }
                                }
                                Some("required") => {
                                    if let Some(b) = cx.boolean(pv, "secret required") {
                                        r.insert("required".into(), Value::Bool(b));
                                    }
                                }
                                _ => {}
                            }
                        }
                        secrets.insert(name.into(), Value::Object(r));
                    }
                    out.insert("secrets".into(), Value::Object(secrets));
                }
            }
            _ => {}
        }
    }
    out
}

// ---- Jobs

/// The oracle's `IdBuilder`: identifiers for steps, with `_2` suffixes for repeats.
#[derive(Default)]
pub struct IdBuilder {
    name: String,
    seen: std::collections::HashSet<String>,
}

pub const ID_MAX: usize = 100;

impl IdBuilder {
    pub fn append(&mut self, v: &str) {
        let Some(first) = v.chars().next() else { return };
        if self.name.is_empty() {
            if first.is_ascii_digit() || first == '-' {
                self.name.push('_');
            }
        } else {
            self.name.push('_');
        }
        for c in v.chars() {
            self.name.push(if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' });
        }
    }
    /// The name appended so far, leaving the builder empty.
    pub fn take(&mut self) -> String {
        std::mem::take(&mut self.name)
    }
    pub fn build(&mut self) -> String {
        let original = if self.name.is_empty() { "job".to_string() } else { std::mem::take(&mut self.name) };
        for attempt in 1..1000 {
            let suffix = if attempt == 1 { String::new() } else { format!("_{attempt}") };
            let keep = original.chars().count().min(ID_MAX - suffix.len());
            let candidate: String = original.chars().take(keep).collect::<String>() + &suffix;
            if self.seen.insert(candidate.clone()) {
                return candidate;
            }
        }
        original
    }
    pub fn known(&mut self, v: &str) -> Result<(), String> {
        let valid = v.chars().enumerate().all(|(i, c)| {
            if i == 0 { c.is_ascii_alphabetic() || c == '_' } else { c.is_ascii_alphanumeric() || c == '_' || c == '-' }
        });
        if v.is_empty() || !valid || v.len() >= ID_MAX {
            return Err(format!(
                "The identifier '{v}' is invalid. IDs may only contain alphanumeric characters, '_', and '-'. IDs must start with a letter or '_' and and must be less than {ID_MAX} characters."
            ));
        }
        if v.starts_with("__") {
            return Err(format!("The identifier '{v}' is invalid. IDs starting with '__' are reserved."));
        }
        if !self.seen.insert(v.into()) {
            return Err(format!("The identifier '{v}' may not be used more than once within the same scope."));
        }
        Ok(())
    }
}

fn convert_jobs(cx: &mut Cx, n: &Node) -> Vec<Job> {
    let Some(pairs) = n.map() else {
        cx.errs.at(n, "Invalid format for jobs");
        return Vec::new();
    };
    let mut jobs = Vec::new();
    for (k, v) in pairs {
        let Some(id) = cx.string(k, "job name") else { continue };
        let Some(body) = cx.mapping(v, &format!("job {id}")) else { continue };
        jobs.push(convert_job(cx, k, id, body));
    }
    validate_needs(cx, n, pairs, &jobs);
    jobs
}

fn validate_needs(cx: &mut Cx, jobs_node: &Node, pairs: &[(Node, Node)], jobs: &[Job]) {
    struct Want<'a> {
        name: &'a str,
        needs: Vec<&'a Node>,
    }
    let need_nodes = |id: &str| -> Vec<&Node> {
        let Some(body) = pairs.iter().find(|(k, _)| k.str() == Some(id)).map(|(_, v)| v) else { return Vec::new() };
        match body.get("needs") {
            Some(n) if n.str().is_some() => vec![n],
            Some(n) => n.seq().map(|s| s.iter().collect()).unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let mut ready: std::collections::VecDeque<&str> = std::collections::VecDeque::new();
    let mut waiting: Vec<Want<'_>> = Vec::new();
    for j in jobs {
        let needs = if j.needs.as_ref().is_some_and(|n| !n.is_empty()) { need_nodes(&j.id) } else { Vec::new() };
        if needs.is_empty() {
            ready.push_back(&j.id);
        } else {
            waiting.push(Want { name: &j.id, needs });
        }
    }
    if ready.is_empty() {
        cx.errs.at(jobs_node, "The workflow must contain at least one job with no dependencies.");
        return;
    }
    while let Some(cur) = ready.pop_front() {
        let mut i = waiting.len();
        while i > 0 {
            i -= 1;
            waiting[i].needs.retain(|n| n.str() != Some(cur));
            if waiting[i].needs.is_empty() {
                ready.push_back(waiting.remove(i).name);
            }
        }
    }
    for w in &waiting {
        for n in &w.needs {
            let need = n.str().unwrap_or("");
            if jobs.iter().any(|j| j.id == need) {
                cx.errs.at(
                    n,
                    format!("Job '{}' depends on job '{need}' which creates a cycle in the dependency graph.", w.name),
                );
            } else {
                cx.errs.at(n, format!("Job '{}' depends on unknown job '{need}'.", w.name));
            }
        }
    }
}

/// Gives a condition a status function unless it calls one (`ensureStatusFunction`), and checks it.
fn convert_if(cx: &mut Cx, n: &Node, def: &str) -> Option<String> {
    let cond = match &n.t {
        T::Expr(e) => e.clone(),
        _ => cx.string(n, "if condition")?.trim().to_string(),
    };
    let allowed = expr::Allowed::new(template::Schema::get().context(def));
    let fin = if cond.is_empty() {
        "success()".to_string()
    } else {
        match expr::parse(&cond, &allowed) {
            Ok(Some(a)) if expr::has_status_call(&a) => cond,
            Ok(_) => format!("success() && ({cond})"),
            Err(_) => cond,
        }
    };
    if let Err(e) = expr::parse(&fin, &allowed) {
        cx.errs.at(n, e);
        return None;
    }
    Some(fin)
}

fn convert_job(cx: &mut Cx, key: &Node, id: &str, body: &[(Node, Node)]) -> Job {
    if let Err(e) = IdBuilder::default().known(id) {
        cx.errs.at(key, e);
    }
    let mut j = Job {
        id: id.into(),
        name: key.clone(),
        needs: None,
        cond: "success()".into(),
        env: None,
        concurrency: None,
        environment: None,
        strategy: None,
        runs_on: None,
        container: None,
        services: None,
        outputs: None,
        steps: Vec::new(),
        timeout_minutes: None,
        continue_on_error: None,
        defaults: None,
        call: None,
    };
    let mut name = None;
    let mut uses = None;
    let mut with = None;
    let mut secrets = None;
    let mut inherit = false;
    for (k, v) in body {
        let Some(p) = cx.string(k, "job property name") else { continue };
        match p {
            "concurrency" => j.concurrency = Some(v.clone()),
            "container" => {
                check_container(cx, v);
                j.container = Some(v.clone());
            }
            "env" => j.env = cx.mapping(v, "job env").map(|_| v.clone()),
            "environment" => {
                check_environment(cx, v);
                j.environment = Some(v.clone());
            }
            "if" => {
                if let Some(c) = convert_if(cx, v, "job-if") {
                    j.cond = c;
                }
            }
            "name" => name = cx.scalar(v, "job name").cloned(),
            "needs" => {
                let mut needs = Vec::new();
                if let Some(s) = v.str() {
                    needs.push(s.to_string());
                }
                if let Some(seq) = v.seq() {
                    for i in seq {
                        if let Some(s) = cx.string(i, "job needs id") {
                            needs.push(s.into());
                        }
                    }
                }
                j.needs = Some(needs);
            }
            "outputs" => j.outputs = cx.mapping(v, "job outputs").map(|_| v.clone()),
            "runs-on" => {
                check_runs_on(cx, v);
                j.runs_on = Some(v.clone());
            }
            "services" => {
                if let Some(m) = cx.mapping(v, "services") {
                    for (_, c) in m {
                        check_container(cx, c);
                    }
                }
                j.services = Some(v.clone());
            }
            "steps" => j.steps = convert_steps(cx, v),
            "strategy" => j.strategy = Some(v.clone()),
            "uses" => uses = cx.string(v, "job uses value").map(String::from),
            "with" => with = cx.mapping(v, "uses-with value").map(|_| v.clone()),
            "secrets" => {
                if v.str() == Some("inherit") {
                    inherit = true;
                } else {
                    secrets = cx.mapping(v, "uses-secrets value").map(|_| v.clone());
                }
            }
            "timeout-minutes" => j.timeout_minutes = Some(v.clone()),
            "continue-on-error" => j.continue_on_error = Some(v.clone()),
            "defaults" => j.defaults = Some(v.clone()),
            _ => {}
        }
    }
    if let Some(n) = name
        && n.str() != Some("")
    {
        j.name = n;
    }
    if let Some(u) = uses {
        j.call = Some(Call { uses: u, with, secrets, inherit_secrets: inherit });
    }
    j
}

fn check_runs_on(cx: &mut Cx, n: &Node) {
    let Some(m) = n.map() else { return };
    for (k, v) in m {
        if k.str() != Some("group") || v.is_expr() {
            continue;
        }
        let Some(g) = cx.string(v, "job runs-on group name") else { continue };
        let parts: Vec<&str> = g.split('/').collect();
        let bad_prefix = format!(
            "Invalid runs-on group name '{g}. Please use 'organization/' or 'enterprise/' prefix to target a single runner group.'"
        );
        match parts.len() {
            1 => {}
            2 if !["org", "organization", "ent", "enterprise"].contains(&parts[0]) => cx.errs.at(v, bad_prefix),
            2 if parts[1].is_empty() => cx.errs.at(v, format!("Invalid runs-on group name '{g}'.")),
            2 => {}
            _ => cx.errs.at(v, bad_prefix),
        }
    }
}

fn check_environment(cx: &mut Cx, n: &Node) {
    let Some(m) = n.map() else { return };
    for (k, v) in m {
        if k.str() == Some("deployment") && !k.is_expr() && !v.is_expr() {
            cx.boolean(v, "job environment deployment");
        }
    }
}

fn check_container(cx: &mut Cx, n: &Node) {
    if n.has_expr() || n.str().is_some() {
        return;
    }
    let Some(m) = cx.mapping(n, "container item") else { return };
    if !m.iter().any(|(k, _)| k.str() == Some("image")) {
        cx.errs.at(n, "Container image cannot be empty");
    }
}

fn convert_steps(cx: &mut Cx, n: &Node) -> Vec<Step> {
    let Some(items) = n.seq() else {
        cx.errs.at(n, "Invalid format for steps");
        return Vec::new();
    };
    let mut ids = IdBuilder::default();
    let mut out: Vec<Step> = Vec::new();
    for item in items {
        let Some(m) = cx.mapping(item, "steps item") else { continue };
        let mut s = Step {
            id: String::new(),
            name: None,
            cond: "success()".into(),
            continue_on_error: None,
            env: None,
            run: None,
            uses: None,
            with: None,
            shell: None,
            working_directory: None,
            timeout_minutes: None,
        };
        for (k, v) in m {
            let Some(key) = cx.string(k, "steps item key") else { continue };
            match key {
                "id" => {
                    if let Some(id) = cx.string(v, "steps item id") {
                        if !id.is_empty()
                            && let Err(e) = ids.known(id)
                        {
                            cx.errs.at(v, e);
                        }
                        s.id = id.into();
                    }
                }
                "name" => s.name = cx.scalar(v, "steps item name").cloned(),
                "run" => s.run = cx.scalar(v, "steps item run").cloned(),
                "uses" => s.uses = cx.string(v, "steps item uses").map(String::from),
                "env" => s.env = cx.mapping(v, "step env").map(|_| v.clone()),
                "if" => {
                    if let Some(c) = convert_if(cx, v, "step-if") {
                        s.cond = c;
                    }
                }
                "continue-on-error" => {
                    s.continue_on_error = if v.is_expr() {
                        cx.scalar(v, "steps item continue-on-error").cloned()
                    } else {
                        cx.boolean(v, "steps item continue-on-error").map(|_| v.clone())
                    }
                }
                "with" => s.with = Some(v.clone()),
                "shell" => s.shell = Some(v.clone()),
                "working-directory" => s.working_directory = Some(v.clone()),
                "timeout-minutes" => s.timeout_minutes = Some(v.clone()),
                _ => {}
            }
        }
        if s.run.is_none() && s.uses.is_none() {
            cx.errs.at(item, "Expected one of uses or run to be defined");
            continue;
        }
        out.push(s);
    }
    for s in out.iter_mut().filter(|s| s.id.is_empty()) {
        let id = s.uses.as_deref().map(action_step_id).filter(|i| !i.is_empty()).unwrap_or_else(|| "run".into());
        ids.append(&format!("__{id}"));
        s.id = ids.build();
    }
    out
}

/// `createActionStepId`: the image of a docker step, `self` for a local action, `owner/repo` for a repository's.
fn action_step_id(uses: &str) -> String {
    if let Some(img) = uses.strip_prefix("docker://") {
        return img.into();
    }
    if uses.starts_with("./") || uses.starts_with(".\\") || uses.starts_with("$/") {
        return "self".into();
    }
    let segs: Vec<&str> = uses.split('@').collect();
    if segs.len() != 2 {
        return String::new();
    }
    let path: Vec<&str> = segs[0].split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if path.len() >= 2 && !segs[1].is_empty() {
        return format!("{}/{}", path[0], path[1]);
    }
    String::new()
}
