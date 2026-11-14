//! `strategy` expansion: a port of the runner's `WorkflowTemplateConverter.ConvertToStrategy` and `MatrixBuilder`
//! (MIT, GitHub, `src/Sdk/WorkflowParser/Conversion/`, runner 2.338.2).
//!
//! - The cross product walks the vectors with the last one fastest.
//! - An `exclude` item matches a combination when every leaf it names equals the combination's (strings compare
//!   case-insensitively, as expression `==` does); a matching combination is dropped.
//! - An `include` item's keys that name vectors filter, and its other keys are extras: every combination it matches
//!   gets its extras, later includes overwriting earlier ones, and an include that matched none becomes its own
//!   combination after the cross product.
//! - The configuration's id is built from the scalar values in order by the same `IdBuilder` steps use (without a
//!   reserved `__` prefix), and its display name is `job (v1, v2)`, cut to 97 characters and `...`.
//!
//! A matrix that holds an expression is evaluated by the caller first, with the `strategy` definition's contexts
//! (`github`, `inputs`, `vars`, `needs`), and then expanded here.

use serde_json::{Value, json};

use super::expr::{self, g15};
use super::template::{Node, T};

/// Context data in its written order: a matrix's keys order its configurations' ids and names, and serde_json's
/// `Map` sorts them (its `preserve_order` feature would change every JSON body the workspace writes).
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Seq(Vec<Data>),
    Map(Vec<(String, Data)>),
}

impl Data {
    pub fn from_node(n: &Node) -> Data {
        match &n.t {
            T::Null => Data::Null,
            T::Bool(b) => Data::Bool(*b),
            T::Num(x) => Data::Num(*x),
            T::Str(s) => Data::Str(s.clone()),
            T::Seq(s) => Data::Seq(s.iter().map(Data::from_node).collect()),
            T::Map(m) => Data::Map(m.iter().map(|(k, v)| (k.text(), Data::from_node(v))).collect()),
            T::Expr(_) | T::Insert => Data::Str(n.text()),
        }
    }

    /// Context data from evaluated JSON (a `fromJSON` matrix), in serde_json's key order.
    pub fn from_json(v: &Value) -> Data {
        match v {
            Value::Null => Data::Null,
            Value::Bool(b) => Data::Bool(*b),
            Value::Number(n) => Data::Num(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => Data::Str(s.clone()),
            Value::Array(a) => Data::Seq(a.iter().map(Data::from_json).collect()),
            Value::Object(m) => Data::Map(m.iter().map(|(k, v)| (k.clone(), Data::from_json(v))).collect()),
        }
    }

    /// Plain JSON, for expressions.
    pub fn json(&self) -> Value {
        match self {
            Data::Null => Value::Null,
            Data::Bool(b) => Value::Bool(*b),
            Data::Num(x) => expr::num(*x),
            Data::Str(s) => Value::String(s.clone()),
            Data::Seq(a) => Value::Array(a.iter().map(Data::json).collect()),
            Data::Map(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), v.json())).collect()),
        }
    }

    /// The runner's context-data JSON (`PipelineContextDataJsonConverter`): `{"t": 2, "d": [{"k", "v"}]}` for a
    /// dictionary, `{"t": 1, "a": [...]}` for an array, bare scalars.
    pub fn context(&self) -> Value {
        match self {
            Data::Seq(a) => json!({"t": 1, "a": a.iter().map(Data::context).collect::<Vec<_>>()}),
            Data::Map(m) => {
                json!({"t": 2, "d": m.iter().map(|(k, v)| json!({"k": k, "v": v.context()})).collect::<Vec<_>>()})
            }
            other => other.json(),
        }
    }

    fn get(&self, k: &str) -> Option<&Data> {
        match self {
            Data::Map(m) => m.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Sets `k`, keeping an existing key's place and case (`DictionaryExpressionData`'s indexer).
    fn set(m: &mut Vec<(String, Data)>, k: &str, v: Data) {
        match m.iter_mut().find(|(key, _)| key.eq_ignore_ascii_case(k)) {
            Some(slot) => slot.1 = v,
            None => m.push((k.to_string(), v)),
        }
    }
}

/// One job a strategy expands into.
#[derive(Clone, Debug, PartialEq)]
pub struct Configuration {
    pub id: String,
    pub name: Option<String>,
    /// The `matrix` context, `Null` for a job without a matrix.
    pub matrix: Data,
    /// The `strategy` context: `fail-fast`, `job-index`, `job-total`, `max-parallel`.
    pub strategy: Data,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Strategy {
    pub fail_fast: bool,
    pub max_parallel: Option<u32>,
    pub configurations: Vec<Configuration>,
}

/// A filter's leaves: the path to each scalar, and the scalar.
fn leaves(path: Vec<Data>, v: &Data, out: &mut Vec<(Vec<Data>, Value)>) {
    match v {
        Data::Map(m) => {
            for (k, x) in m {
                let mut p = path.clone();
                p.push(Data::Str(k.clone()));
                leaves(p, x, out);
            }
        }
        Data::Seq(a) => {
            for (i, x) in a.iter().enumerate() {
                let mut p = path.clone();
                p.push(Data::Num(i as f64));
                leaves(p, x, out);
            }
        }
        _ => out.push((path, v.json())),
    }
}

fn lookup(mut v: &Data, path: &[Data]) -> Value {
    for k in path {
        v = match (v, k) {
            (Data::Map(_), Data::Str(s)) => match v.get(s) {
                Some(x) => x,
                None => return Value::Null,
            },
            (Data::Seq(a), Data::Num(i)) => match a.get(*i as usize) {
                Some(x) => x,
                None => return Value::Null,
            },
            _ => return Value::Null,
        };
    }
    v.json()
}

/// `matrix[...] == literal` for every leaf of a filter, with expression equality.
fn filter_matches(filter: &[(Vec<Data>, Value)], combo: &Data) -> bool {
    filter.iter().all(|(p, want)| expr::equals(&lookup(combo, p), want))
}

/// Scalar values in traversal order (`Traverse(omitKeys: true)`), which name the configuration.
fn segments(v: &Data, out: &mut Vec<String>) {
    match v {
        Data::Bool(b) => out.push(b.to_string()),
        Data::Num(n) => out.push(g15(*n)),
        Data::Str(s) => out.push(s.clone()),
        Data::Seq(a) => a.iter().for_each(|x| segments(x, out)),
        Data::Map(m) => m.iter().for_each(|(_, x)| segments(x, out)),
        Data::Null => {}
    }
}

/// The C# `IdBuilder.Build(allowReservedPrefix: false)`, whose set of names ignores case.
fn build_id(name: &str, seen: &mut std::collections::HashSet<String>) -> String {
    let mut original = if name.is_empty() { "job".to_string() } else { name.to_string() };
    if original.starts_with("__") {
        original = format!("_{}", original.trim_start_matches('_'));
    }
    for attempt in 1..1000 {
        let suffix = match attempt {
            1 => String::new(),
            _ if original == "_" => attempt.to_string(),
            _ => format!("_{attempt}"),
        };
        let keep = original.chars().count().min(super::ID_MAX - suffix.len());
        let candidate: String = original.chars().take(keep).collect::<String>() + &suffix;
        if seen.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    original
}

fn display_name(job: Option<&str>, segs: &[String]) -> Option<String> {
    let job = job.filter(|j| !j.is_empty())?;
    let name = if segs.is_empty() { job.to_string() } else { format!("{job} ({})", segs.join(", ")) };
    Some(if name.chars().count() > 100 { name.chars().take(97).collect::<String>() + "..." } else { name })
}

fn fail(n: &Node, msg: impl Into<String>) -> String {
    let m = msg.into();
    if n.line > 0 { format!("(Line: {}, Col: {}): {m}", n.line, n.col) } else { m }
}

fn strategy_data(fail_fast: bool, index: usize, total: usize, max: u64) -> Data {
    Data::Map(vec![
        ("fail-fast".into(), Data::Bool(fail_fast)),
        ("job-index".into(), Data::Num(index as f64)),
        ("job-total".into(), Data::Num(total as f64)),
        ("max-parallel".into(), Data::Num(max as f64)),
    ])
}

/// Expands a literal `strategy` mapping. `job_name` is the job's display name when it is a literal. Returns `Ok(None)`
/// when the strategy holds an expression; errors are the C# converter's messages.
pub fn expand(strategy: Option<&Node>, job_name: Option<&str>) -> Result<Option<Strategy>, Vec<String>> {
    let mut out = Strategy { fail_fast: true, max_parallel: None, configurations: Vec::new() };
    let mut errs = Vec::new();
    let mut vectors: Vec<(String, Vec<Data>)> = Vec::new();
    let mut include: Vec<&Node> = Vec::new();
    let mut exclude: Vec<&Node> = Vec::new();
    if let Some(s) = strategy {
        if s.has_expr() {
            return Ok(None);
        }
        let Some(pairs) = s.map() else { return Err(vec![fail(s, "Unexpected strategy")]) };
        for (k, v) in pairs {
            match k.str() {
                Some("fail-fast") => match v.t {
                    T::Bool(b) => out.fail_fast = b,
                    _ => errs.push(fail(v, "Unexpected type for strategy fail-fast")),
                },
                Some("max-parallel") => match v.t {
                    T::Num(x) => out.max_parallel = Some(x as u32).filter(|m| *m > 0),
                    _ => errs.push(fail(v, "Unexpected type for strategy max-parallel")),
                },
                Some("matrix") => {
                    let Some(mp) = v.map() else {
                        errs.push(fail(v, "Unexpected type for matrix"));
                        continue;
                    };
                    let mut any_vector = false;
                    let mut any_include = false;
                    for (mk, mv) in mp {
                        let key = mk.text();
                        match key.as_str() {
                            "include" => match mv.seq() {
                                Some(seq) => {
                                    any_include = !seq.is_empty();
                                    include = seq.iter().collect();
                                }
                                None => errs.push(fail(mv, "Unexpected type for matrix includes")),
                            },
                            "exclude" => match mv.seq() {
                                Some(seq) => exclude = seq.iter().collect(),
                                None => errs.push(fail(mv, "Unexpected type for matrix excludes")),
                            },
                            _ => {
                                any_vector = true;
                                match mv.seq() {
                                    Some([]) => errs
                                        .push(fail(mv, format!("Matrix vector '{key}' does not contain any values"))),
                                    Some(seq) => vectors.push((key, seq.iter().map(Data::from_node).collect())),
                                    None => errs.push(fail(mv, "Unexpected type for matrix vector value")),
                                }
                            }
                        }
                    }
                    if !any_vector && !any_include {
                        errs.push(fail(v, "Matrix must define at least one vector"));
                    }
                }
                _ => errs.push(fail(k, format!("Unexpected value '{}'", k.text()))),
            }
        }
    }
    // Filters and extras.
    let vector_named = |k: &str| vectors.iter().any(|(n, _)| n.eq_ignore_ascii_case(k));
    type Leaves = Vec<(Vec<Data>, Value)>;
    type Pairs = Vec<(String, Data)>;
    let mut includes: Vec<(Leaves, Pairs, Pairs)> = Vec::new();
    for item in &include {
        let Some(m) = item.map() else {
            errs.push(fail(item, "Unexpected type for matrix includes item"));
            continue;
        };
        let mut filter = Vec::new();
        let mut extra = Vec::new();
        for (k, v) in m {
            let key = k.text();
            if vector_named(&key) {
                filter.push((key, Data::from_node(v)));
            } else {
                extra.push((key, Data::from_node(v)));
            }
        }
        if filter.is_empty() && extra.is_empty() {
            errs.push(fail(item, "Matrix include mapping does not contain any values"));
            continue;
        }
        let mut leafs = Vec::new();
        leaves(Vec::new(), &Data::Map(filter.clone()), &mut leafs);
        includes.push((leafs, filter, extra));
    }
    let mut excludes: Vec<Leaves> = Vec::new();
    for item in &exclude {
        let Some(m) = item.map() else {
            errs.push(fail(item, "Unexpected type for matrix excludes item"));
            continue;
        };
        if m.is_empty() {
            errs.push(fail(item, "Matrix exclude filter must not be empty"));
            continue;
        }
        for (k, _) in m {
            if !vector_named(&k.text()) {
                errs.push(fail(
                    k,
                    format!("Matrix exclude key '{}' does not match any key within the matrix", k.text()),
                ));
            }
        }
        let mut leafs = Vec::new();
        leaves(Vec::new(), &Data::from_node(item), &mut leafs);
        excludes.push(leafs);
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let mut seen = std::collections::HashSet::new();
    let mut configs = Vec::new();
    let mut make = |combo: Vec<(String, Data)>, extra: Option<Vec<(String, Data)>>| {
        let mut segs = Vec::new();
        segments(&Data::Map(combo.clone()), &mut segs);
        let mut ids = super::IdBuilder::default();
        for s in &segs {
            ids.append(s);
        }
        let id = build_id(&ids.take(), &mut seen);
        let name = display_name(job_name, &segs);
        let mut matrix = combo;
        for (k, v) in extra.into_iter().flatten() {
            Data::set(&mut matrix, &k, v);
        }
        Configuration { id, name, matrix: Data::Map(matrix), strategy: Data::Null }
    };
    let product: usize = if vectors.is_empty() { 0 } else { vectors.iter().map(|v| v.1.len()).product() };
    let mut matched = vec![false; includes.len()];
    for i in 0..product {
        let mut combo = Vec::new();
        let mut block = product;
        for (name, vals) in &vectors {
            block /= vals.len();
            combo.push((name.clone(), vals[(i / block) % vals.len()].clone()));
        }
        let cv = Data::Map(combo.clone());
        if excludes.iter().any(|f| filter_matches(f, &cv)) {
            continue;
        }
        let mut extra: Option<Vec<(String, Data)>> = None;
        for (j, (leafs, _, e)) in includes.iter().enumerate() {
            if filter_matches(leafs, &cv) {
                matched[j] = true;
                let x = extra.get_or_insert_with(Vec::new);
                for (k, v) in e {
                    Data::set(x, k, v.clone());
                }
            }
        }
        configs.push(make(combo, extra));
    }
    for (j, (_, filter, e)) in includes.into_iter().enumerate() {
        if !matched[j] {
            let mut combo = filter;
            combo.extend(e);
            configs.push(make(combo, None));
        }
    }
    if configs.is_empty() {
        // `EvaluateStrategy`'s default: one configuration with no matrix.
        return Ok(Some(Strategy {
            configurations: vec![Configuration {
                id: "__default".into(),
                name: display_name(job_name, &[]),
                matrix: Data::Null,
                strategy: strategy_data(out.fail_fast, 0, 1, 1),
            }],
            ..out
        }));
    }
    let total = configs.len();
    for (i, c) in configs.iter_mut().enumerate() {
        let max = out.max_parallel.map_or(total as u64, u64::from);
        c.strategy = strategy_data(out.fail_fast, i, total, max);
    }
    out.configurations = configs;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strategy(yaml: &str) -> Node {
        super::super::template::load(yaml).unwrap()
    }

    fn ids(s: &Strategy) -> Vec<(&str, String)> {
        s.configurations.iter().map(|c| (c.id.as_str(), c.matrix.json().to_string())).collect()
    }

    /// A matrix in its written order, `key=value` pairs.
    fn text(d: &Data) -> String {
        match d {
            Data::Map(m) => {
                m.iter().map(|(k, v)| format!("{k}={}", v.json().as_str().unwrap_or(""))).collect::<Vec<_>>().join(" ")
            }
            _ => String::new(),
        }
    }

    /// GitHub's documented example ("Expanding or adding matrix configurations"): includes add extras to every
    /// combination they match, a later include overwrites an earlier extra, and an include that matched nothing is
    /// its own job, all after the cross product.
    #[test]
    fn include_extras_and_unmatched_includes() {
        let s = strategy(
            "matrix:\n  fruit: [apple, pear]\n  animal: [cat, dog]\n  include:\n    - color: green\n    - color: pink\n      animal: cat\n    - fruit: apple\n      shape: circle\n    - fruit: banana\n    - fruit: banana\n      animal: cat\n",
        );
        let out = expand(Some(&s), Some("docs")).unwrap().unwrap();
        let got: Vec<String> = out.configurations.iter().map(|c| text(&c.matrix)).collect();
        assert_eq!(
            got,
            [
                "fruit=apple animal=cat color=pink shape=circle",
                "fruit=apple animal=dog color=green shape=circle",
                "fruit=pear animal=cat color=pink",
                "fruit=pear animal=dog color=green",
                "fruit=banana",
                "fruit=banana animal=cat",
            ]
        );
        // Ids and names use the vectors' values only, in vector order.
        let c = &out.configurations;
        assert_eq!((c[0].id.as_str(), c[0].name.as_deref()), ("apple_cat", Some("docs (apple, cat)")));
        assert_eq!(c[4].id, "banana");
        assert_eq!(c[5].id, "banana_cat");
        assert_eq!(c[5].strategy.json(), json!({"fail-fast": true, "job-index": 5, "job-total": 6, "max-parallel": 6}));
        assert_eq!(
            c[4].matrix.context(),
            json!({"t": 2, "d": [{"k": "fruit", "v": "banana"}]}),
            "the runner's context-data form"
        );
    }

    #[test]
    fn exclude_is_a_partial_case_insensitive_match_and_ids_stay_unique() {
        let s = strategy(
            "fail-fast: false\nmax-parallel: 2\nmatrix:\n  cfg: [{os: linux, v: 1.5}, {os: Windows, v: 2}]\n  flag: [true, false]\n  exclude:\n    - cfg: {os: windows}\n      flag: false\n",
        );
        let out = expand(Some(&s), Some("t")).unwrap().unwrap();
        assert_eq!(
            ids(&out),
            [
                ("linux_1_5_true", r#"{"cfg":{"os":"linux","v":1.5},"flag":true}"#.to_string()),
                ("linux_1_5_false", r#"{"cfg":{"os":"linux","v":1.5},"flag":false}"#.to_string()),
                ("Windows_2_true", r#"{"cfg":{"os":"Windows","v":2},"flag":true}"#.to_string()),
            ]
        );
        assert_eq!(out.configurations[1].name.as_deref(), Some("t (linux, 1.5, false)"));
        assert_eq!(out.configurations[2].strategy.json()["max-parallel"], json!(2));
        // Two values that differ only in case get distinct ids, as the C# builder's case-insensitive set gives.
        let s = strategy("matrix:\n  v: [A, a, 10]\n");
        let out = expand(Some(&s), None).unwrap().unwrap();
        let got: Vec<&str> = out.configurations.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(got, ["A", "a_2", "_10"]);
    }

    #[test]
    fn no_strategy_is_one_default_job_and_an_expression_waits() {
        let out = expand(None, Some("build")).unwrap().unwrap();
        assert_eq!(out.configurations.len(), 1);
        assert_eq!(out.configurations[0].id, "__default");
        assert_eq!(out.configurations[0].matrix, Data::Null);
        let s = strategy("matrix: ${{ fromJSON(inputs.m) }}\n");
        let s = super::super::template::read(&s, "strategy", &mut super::super::template::Errors::new("f"));
        assert_eq!(expand(Some(&s), None).unwrap(), None);
        let bad = strategy("matrix:\n  os: []\n  exclude:\n    - arch: x\n");
        let e = expand(Some(&bad), None).unwrap_err();
        assert!(e[0].ends_with("Matrix vector 'os' does not contain any values"), "{e:?}");
        assert!(e[1].ends_with("Matrix exclude key 'arch' does not match any key within the matrix"), "{e:?}");
    }

    #[test]
    fn long_names_are_cut() {
        let long = "x".repeat(60);
        let s = strategy(&format!("matrix:\n  a: [{long}]\n  b: [{long}]\n"));
        let out = expand(Some(&s), Some("job")).unwrap().unwrap();
        let n = out.configurations[0].name.clone().unwrap();
        assert_eq!(n.chars().count(), 100);
        assert!(n.ends_with("..."));
        assert_eq!(out.configurations[0].id.chars().count(), 100);
    }
}
