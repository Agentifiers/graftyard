//! Template tokens and the schema-driven reader: a port of `@actions/workflow-parser` 0.3.61's `YamlObjectReader`
//! and `TemplateReader` (MIT, GitHub), with its `workflow-v1.0` schema as data (descriptions removed). The runner's
//! `GitHub.Actions.WorkflowParser` reads the same schema the same way.
//!
//! One deliberate difference: a multi-line string's embedded expressions are read from its value, as the runner's
//! C# `TemplateReader` does, not from its raw YAML source with indentation, as the npm reader does for editor ranges
//! (`tests/m4/wp3/workflows/07-scalars.yml`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use super::expr::{self, Allowed};

/// A template token with its 1-based source position (0 when it has none).
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub t: T,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum T {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Seq(Vec<Node>),
    Map(Vec<(Node, Node)>),
    Expr(String),
    Insert,
}

impl Node {
    pub fn new(t: T) -> Node {
        Node { t, line: 0, col: 0 }
    }
    pub fn str(&self) -> Option<&str> {
        match &self.t {
            T::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn map(&self) -> Option<&[(Node, Node)]> {
        match &self.t {
            T::Map(m) => Some(m),
            _ => None,
        }
    }
    pub fn seq(&self) -> Option<&[Node]> {
        match &self.t {
            T::Seq(s) => Some(s),
            _ => None,
        }
    }
    pub fn is_expr(&self) -> bool {
        matches!(self.t, T::Expr(_) | T::Insert)
    }
    pub fn is_literal(&self) -> bool {
        matches!(self.t, T::Null | T::Bool(_) | T::Num(_) | T::Str(_))
    }
    /// The value of a mapping's key `k`, the first match.
    pub fn get(&self, k: &str) -> Option<&Node> {
        self.map()?.iter().find(|(key, _)| key.str() == Some(k)).map(|(_, v)| v)
    }
    /// Whether any token in the tree is an expression.
    pub fn has_expr(&self) -> bool {
        match &self.t {
            T::Expr(_) | T::Insert => true,
            T::Seq(s) => s.iter().any(Node::has_expr),
            T::Map(m) => m.iter().any(|(k, v)| k.has_expr() || v.has_expr()),
            _ => false,
        }
    }
    /// A literal's string form (`toString` in the oracle).
    pub fn text(&self) -> String {
        match &self.t {
            T::Null => String::new(),
            T::Bool(b) => b.to_string(),
            T::Num(n) => expr::js_num_string(*n),
            T::Str(s) => s.clone(),
            T::Expr(e) => format!("${{{{ {e} }}}}"),
            T::Insert => "${{ insert }}".into(),
            T::Seq(_) => "SequenceToken".into(),
            T::Map(_) => "MappingToken".into(),
        }
    }
    pub fn type_name(&self) -> &'static str {
        match self.t {
            T::Null => "NullToken",
            T::Bool(_) => "BooleanToken",
            T::Num(_) => "NumberToken",
            T::Str(_) => "StringToken",
            T::Seq(_) => "SequenceToken",
            T::Map(_) => "MappingToken",
            T::Expr(_) => "BasicExpressionToken",
            T::Insert => "InsertExpressionToken",
        }
    }
    /// The token's JSON, as both the oracle and the runner's `TemplateTokenJsonConverter` write it.
    pub fn json(&self) -> Value {
        match &self.t {
            T::Null => Value::Null,
            T::Bool(b) => Value::Bool(*b),
            T::Num(n) => expr::num(*n),
            T::Str(s) => Value::String(s.clone()),
            T::Seq(s) => serde_json::json!({"type": 1, "seq": s.iter().map(Node::json).collect::<Vec<_>>()}),
            T::Map(m) => serde_json::json!({
                "type": 2,
                "map": m.iter().map(|(k, v)| serde_json::json!({"Key": k.json(), "Value": v.json()})).collect::<Vec<_>>(),
            }),
            T::Expr(e) => serde_json::json!({"type": 3, "expr": e}),
            T::Insert => serde_json::json!({"type": 4, "expr": "insert"}),
        }
    }
}

/// Errors as the oracle prints them: `file (Line: l, Col: c): message`.
pub struct Errors {
    file: String,
    pub list: Vec<String>,
}

impl Errors {
    pub fn new(file: &str) -> Errors {
        Errors { file: file.into(), list: Vec::new() }
    }
    pub fn at(&mut self, n: &Node, msg: impl std::fmt::Display) {
        self.list.push(if n.line > 0 {
            format!("{} (Line: {}, Col: {}): {msg}", self.file, n.line, n.col)
        } else {
            format!("{}: {msg}", self.file)
        });
    }
}

// ---- YAML

struct Builder {
    stack: Vec<Frame>,
    anchors: HashMap<usize, Node>,
    root: Option<Node>,
}

enum Frame {
    Seq(Node, usize),
    Map(Node, Option<Node>, usize),
}

fn pos(m: Marker) -> (u32, u32) {
    (m.line() as u32, m.col() as u32 + 1)
}

/// The YAML 1.2 core schema's resolution of a plain scalar, as the `yaml` library and the runner's C# reader do.
fn resolve_plain(s: &str) -> T {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return T::Null,
        "true" | "True" | "TRUE" => return T::Bool(true),
        "false" | "False" | "FALSE" => return T::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => return T::Num(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => return T::Num(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" => return T::Num(f64::NAN),
        _ => {}
    }
    let digits = |d: &str, radix: u32| !d.is_empty() && d.chars().all(|c| c.is_digit(radix));
    if let Some(h) = s.strip_prefix("0x")
        && digits(h, 16)
    {
        return T::Num(expr::js_number(s));
    }
    if let Some(o) = s.strip_prefix("0o")
        && digits(o, 8)
    {
        return T::Num(expr::js_number(s));
    }
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits(body, 10) {
        return T::Num(s.parse::<f64>().unwrap_or(f64::NAN));
    }
    // [-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?
    let (mant, exp) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let mant_ok = match mant.split_once('.') {
        Some((i, f)) => (digits(i, 10) && f.chars().all(|c| c.is_ascii_digit())) || (i.is_empty() && digits(f, 10)),
        None => digits(mant, 10),
    };
    let exp_ok = exp.is_none_or(|e| digits(e.strip_prefix(['-', '+']).unwrap_or(e), 10));
    if mant_ok && exp_ok && (exp.is_some() || mant.contains('.')) {
        return T::Num(s.parse::<f64>().unwrap_or(f64::NAN));
    }
    T::Str(s.into())
}

impl Builder {
    fn push(&mut self, n: Node, anchor: usize) {
        if anchor > 0 {
            self.anchors.insert(anchor, n.clone());
        }
        match self.stack.last_mut() {
            None => self.root = Some(n),
            Some(Frame::Seq(s, _)) => {
                if let T::Seq(items) = &mut s.t {
                    items.push(n);
                }
            }
            Some(Frame::Map(m, key, _)) => match key.take() {
                None => {
                    // A block mapping's start mark comes after its first key; the oracle's range starts at the key.
                    if (n.line, n.col) < (m.line, m.col) {
                        (m.line, m.col) = (n.line, n.col);
                    }
                    *key = Some(n);
                }
                Some(k) => {
                    if let T::Map(pairs) = &mut m.t {
                        pairs.push((k, n));
                    }
                }
            },
        }
    }
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, ev: Event, mark: Marker) {
        let (line, col) = pos(mark);
        match ev {
            Event::Scalar(v, style, anchor, tag) => {
                let t = match (&tag, style) {
                    (Some(tag), _) if tag.suffix == "str" => T::Str(v),
                    (_, TScalarStyle::Plain) => resolve_plain(&v),
                    _ => T::Str(v),
                };
                self.push(Node { t, line, col }, anchor);
            }
            Event::SequenceStart(anchor, _) => {
                self.stack.push(Frame::Seq(Node { t: T::Seq(Vec::new()), line, col }, anchor))
            }
            Event::MappingStart(anchor, _) => {
                self.stack.push(Frame::Map(Node { t: T::Map(Vec::new()), line, col }, None, anchor))
            }
            Event::SequenceEnd | Event::MappingEnd => {
                if let Some(Frame::Seq(n, a) | Frame::Map(n, _, a)) = self.stack.pop() {
                    self.push(n, a);
                }
            }
            Event::Alias(id) => {
                let n = self.anchors.get(&id).cloned().unwrap_or(Node { t: T::Null, line, col });
                self.push(n, 0);
            }
            _ => {}
        }
    }
}

/// Reads one YAML document into tokens, or the scanner's error.
pub fn load(src: &str) -> Result<Node, String> {
    let mut b = Builder { stack: Vec::new(), anchors: HashMap::new(), root: None };
    Parser::new_from_str(src)
        .load(&mut b, false)
        .map_err(|e| format!("{} at line {}, column {}", e.info(), e.marker().line(), e.marker().col() + 1))?;
    b.root.ok_or_else(|| "Expected a scalar value, a sequence, or a mapping".into())
}

// ---- Schema

#[derive(Debug)]
enum Def {
    Null,
    Bool,
    Num,
    Str { constant: Option<String>, ignore_case: bool, non_empty: bool },
    Seq(String),
    Map { props: Vec<(String, String, bool)>, loose_key: Option<String>, loose_val: Option<String> },
    OneOf(Vec<String>),
}

#[derive(Debug)]
struct Entry {
    def: Def,
    context: Vec<String>,
}

pub struct Schema {
    defs: HashMap<String, Entry>,
}

/// A JSON object's members in their written order, which the property tables and their required checks keep.
struct Ordered<V>(Vec<(String, V)>);

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Ordered<V> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Vis<V>(std::marker::PhantomData<V>);
        impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Vis<V> {
            type Value = Ordered<V>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Ordered<V>, A::Error> {
                let mut out = Vec::new();
                while let Some(kv) = m.next_entry()? {
                    out.push(kv);
                }
                Ok(Ordered(out))
            }
        }
        d.deserialize_map(Vis(std::marker::PhantomData))
    }
}

#[derive(Deserialize)]
struct RawSchema {
    definitions: Ordered<Ordered<Value>>,
}

const SCHEMA_JSON: &str = include_str!("workflow-v1.0.json");

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
}

impl Schema {
    pub fn get() -> &'static Schema {
        static S: OnceLock<Schema> = OnceLock::new();
        S.get_or_init(|| Schema::load(SCHEMA_JSON))
    }

    fn load(json: &str) -> Schema {
        let raw: RawSchema = serde_json::from_str(json).expect("the bundled workflow schema parses");
        let mut defs = HashMap::new();
        let scalar = |def| Entry { def, context: Vec::new() };
        defs.insert("null".into(), scalar(Def::Null));
        defs.insert("boolean".into(), scalar(Def::Bool));
        defs.insert("number".into(), scalar(Def::Num));
        defs.insert("string".into(), scalar(Def::Str { constant: None, ignore_case: false, non_empty: false }));
        defs.insert("sequence".into(), scalar(Def::Seq("any".into())));
        defs.insert(
            "mapping".into(),
            scalar(Def::Map { props: Vec::new(), loose_key: Some("string".into()), loose_val: Some("any".into()) }),
        );
        let any = ["null", "boolean", "number", "string", "sequence", "mapping"];
        defs.insert("any".into(), scalar(Def::OneOf(any.iter().map(|s| s.to_string()).collect())));
        for (name, body) in raw.definitions.0 {
            let mut context = Vec::new();
            let mut def = None;
            for (k, v) in &body.0 {
                match k.as_str() {
                    "context" => context = strings(v),
                    "description" => {}
                    _ if def.is_some() => {}
                    "null" => def = Some(Def::Null),
                    "boolean" => def = Some(Def::Bool),
                    "number" => def = Some(Def::Num),
                    "string" => {
                        def = Some(Def::Str {
                            constant: v.get("constant").and_then(Value::as_str).map(String::from),
                            ignore_case: v.get("ignore-case").and_then(Value::as_bool).unwrap_or(false),
                            non_empty: v.get("require-non-empty").and_then(Value::as_bool).unwrap_or(false),
                        })
                    }
                    "sequence" => {
                        def = Some(Def::Seq(v.get("item-type").and_then(Value::as_str).unwrap_or("any").into()))
                    }
                    "mapping" => {
                        let props = v
                            .get("properties")
                            .map(|p| {
                                let p: Ordered<Value> =
                                    serde_json::from_value(p.clone()).unwrap_or(Ordered(Vec::new()));
                                p.0.into_iter()
                                    .map(|(pn, pv)| match pv {
                                        Value::String(t) => (pn, t, false),
                                        o => {
                                            let t = o.get("type").and_then(Value::as_str).unwrap_or("any").into();
                                            (pn, t, o.get("required").and_then(Value::as_bool).unwrap_or(false))
                                        }
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let s = |k| v.get(k).and_then(Value::as_str).map(String::from);
                        def =
                            Some(Def::Map { props, loose_key: s("loose-key-type"), loose_val: s("loose-value-type") });
                    }
                    "one-of" => def = Some(Def::OneOf(strings(v))),
                    "allowed-values" => {
                        let mut names = Vec::new();
                        for a in strings(v) {
                            let n = format!("{name}-{a}");
                            let c = Def::Str { constant: Some(a), ignore_case: false, non_empty: false };
                            defs.insert(n.clone(), Entry { def: c, context: Vec::new() });
                            names.push(n);
                        }
                        def = Some(Def::OneOf(names));
                    }
                    _ => {}
                }
            }
            if let Some(def) = def {
                defs.insert(name, Entry { def, context });
            }
        }
        Schema { defs }
    }

    fn entry(&self, name: &str) -> &Entry {
        self.defs.get(name).unwrap_or_else(|| &self.defs["any"])
    }

    /// A definition's own allowed context (for `job-if` and `step-if`, which no parent adds to).
    pub fn context(&self, name: &str) -> &[String] {
        &self.entry(name).context
    }

    fn scalars<'a>(&'a self, name: &str, out: &mut Vec<&'a Def>) {
        match &self.entry(name).def {
            Def::OneOf(names) => {
                for n in names {
                    self.scalars(n, out);
                }
            }
            d @ (Def::Null | Def::Bool | Def::Num | Def::Str { .. }) => out.push(d),
            _ => {}
        }
    }

    fn of_type(&self, name: &str, want: fn(&Def) -> bool) -> Vec<&Def> {
        let d = &self.entry(name).def;
        if want(d) {
            return vec![d];
        }
        match d {
            Def::OneOf(names) => names.iter().map(|n| &self.entry(n).def).filter(|d| want(d)).collect(),
            _ => Vec::new(),
        }
    }
}

fn matches(d: &Def, n: &Node) -> bool {
    match (d, &n.t) {
        (Def::Null, T::Null) | (Def::Bool, T::Bool(_)) | (Def::Num, T::Num(_)) => true,
        (Def::Str { constant: Some(c), ignore_case, .. }, T::Str(s)) => {
            if *ignore_case {
                c.to_uppercase() == s.to_uppercase()
            } else {
                c == s
            }
        }
        (Def::Str { non_empty, .. }, T::Str(s)) => !non_empty || !s.is_empty(),
        _ => false,
    }
}

// ---- The reader

/// A definition with the allowed context its ancestors and itself give (`DefinitionInfo`).
#[derive(Clone)]
struct Info<'s> {
    name: &'s str,
    allowed: Vec<String>,
}

const BACKGROUND_PROPS: [&str; 4] = ["background", "wait", "wait-all", "cancel"];
const BACKGROUND_ONLY: [&str; 3] = ["wait", "wait-all", "cancel"];

struct Reader<'a> {
    schema: &'static Schema,
    errs: &'a mut Errors,
}

impl<'a> Reader<'a> {
    fn info<'s>(&self, parent: &Info<'_>, name: &'s str) -> Info<'s> {
        let own = &self.schema.entry(name).context;
        let mut allowed = parent.allowed.clone();
        for c in own {
            if !allowed.iter().any(|a| a.eq_ignore_ascii_case(c)) {
                allowed.push(c.clone());
            }
        }
        Info { name, allowed }
    }

    fn read(&mut self, n: &Node, info: &Info<'_>) -> Node {
        match &n.t {
            T::Seq(items) => {
                let seqs = self.schema.of_type(info.name, |d| matches!(d, Def::Seq(_)));
                let mut out = Node { t: T::Seq(Vec::new()), ..*n };
                if let Some(Def::Seq(item)) = seqs.first() {
                    let ii = self.info(info, item);
                    out.t = T::Seq(items.iter().map(|i| self.read(i, &ii)).collect());
                } else {
                    self.errs.at(n, "A sequence was not expected");
                }
                out
            }
            T::Map(pairs) => {
                let maps = self.schema.of_type(info.name, |d| matches!(d, Def::Map { .. }));
                match maps.first() {
                    None => {
                        self.errs.at(n, "A mapping was not expected");
                        Node { t: T::Map(Vec::new()), ..*n }
                    }
                    Some(Def::Map { props, loose_key: Some(lk), loose_val }) if maps.len() == 1 && props.is_empty() => {
                        let ki = self.info(info, lk);
                        let vi = self.info(info, loose_val.as_deref().unwrap_or("any"));
                        self.all_loose(n, pairs, info, &ki, &vi)
                    }
                    _ => self.well_known(n, pairs, info, maps),
                }
            }
            _ => {
                let s = self.parse_scalar(n, info);
                self.validate(s, info)
            }
        }
    }

    fn key(&mut self, k: &Node, info: &Info<'_>) -> Node {
        let k = self.parse_scalar(k, info);
        if k.is_expr() || matches!(k.t, T::Str(_)) {
            return k;
        }
        Node { t: T::Str(k.text()), ..k }
    }

    fn well_known(&mut self, n: &Node, pairs: &[(Node, Node)], info: &Info<'_>, mut defs: Vec<&'static Def>) -> Node {
        let (loose_key, loose_val) = match defs[0] {
            Def::Map { loose_key, loose_val, .. } => (loose_key.clone(), loose_val.clone()),
            _ => (None, None),
        };
        let mut seen: Vec<String> = Vec::new();
        let mut has_expr_key = false;
        let mut out = Vec::new();
        for (k, v) in pairs {
            let key = self.key(k, info);
            if key.is_expr() {
                has_expr_key = true;
                if info.allowed.is_empty() {
                    self.errs.at(&key, "A template expression is not allowed in this context");
                } else {
                    let ai = self.info(info, "any");
                    out.push((key, self.read(v, &ai)));
                }
                continue;
            }
            let name = key.text();
            if !name.is_empty() {
                let up = name.to_uppercase();
                if seen.contains(&up) {
                    self.errs.at(&key, format!("'{name}' is already defined"));
                    continue;
                }
                seen.push(up);
            }
            let prop = defs.iter().find_map(|d| prop_type(d, &name)).map(String::from);
            if let Some(ty) = prop {
                defs.retain(|d| prop_type(d, &name).is_some());
                if BACKGROUND_PROPS.contains(&name.as_str()) {
                    self.errs.at(&key, format!("Unexpected value '{name}'"));
                    continue;
                }
                let vi = self.info(info, self.schema_name(&ty));
                out.push((key, self.read(v, &vi)));
                continue;
            }
            if let Some(lk) = &loose_key {
                let ki = self.info(info, self.schema_name(lk));
                let key = self.validate(key, &ki);
                let vi = self.info(info, self.schema_name(loose_val.as_deref().unwrap_or("any")));
                out.push((key, self.read(v, &vi)));
                continue;
            }
            self.errs.at(&key, format!("Unexpected value '{name}'"));
        }
        if defs.len() > 1 {
            let mut hits: Vec<(&str, usize)> = Vec::new();
            for d in &defs {
                if let Def::Map { props, .. } = d {
                    for (p, _, _) in props {
                        match hits.iter_mut().find(|h| h.0 == p) {
                            Some(h) => h.1 += 1,
                            None => hits.push((p, 1)),
                        }
                    }
                }
            }
            let mut once: Vec<&str> =
                hits.into_iter().filter(|h| h.1 == 1 && !BACKGROUND_ONLY.contains(&h.0)).map(|h| h.0).collect();
            once.sort_unstable();
            self.errs.at(
                n,
                format!(
                    "There's not enough info to determine what you meant. Add one of these properties: {}",
                    once.join(", ")
                ),
            );
        } else if defs.len() == 1
            && !has_expr_key
            && let Def::Map { props, .. } = defs[0]
        {
            for (p, _, required) in props {
                if *required && !seen.contains(&p.to_uppercase()) {
                    self.errs.at(n, format!("Required property is missing: {p}"));
                }
            }
        }
        Node { t: T::Map(out), ..*n }
    }

    /// Interns a definition name for the life of the schema.
    fn schema_name(&self, name: &str) -> &'static str {
        self.schema.defs.get_key_value(name).map_or("any", |(k, _)| k.as_str())
    }

    fn all_loose(&mut self, n: &Node, pairs: &[(Node, Node)], info: &Info<'_>, ki: &Info<'_>, vi: &Info<'_>) -> Node {
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for (k, v) in pairs {
            let key = self.key(k, info);
            if key.is_expr() {
                if info.allowed.is_empty() {
                    self.errs.at(&key, "A template expression is not allowed in this context");
                } else {
                    out.push((key, self.read(v, vi)));
                }
                continue;
            }
            let name = key.text();
            if !name.is_empty() {
                let up = name.to_uppercase();
                if seen.contains(&up) {
                    self.errs.at(&key, format!("'{name}' is already defined"));
                    continue;
                }
                seen.push(up);
            }
            let key = self.validate(key, ki);
            out.push((key, self.read(v, vi)));
        }
        Node { t: T::Map(out), ..*n }
    }

    fn validate(&mut self, n: Node, info: &Info<'_>) -> Node {
        match &n.t {
            T::Expr(_) => {
                if info.allowed.is_empty() {
                    self.errs.at(&n, "A template expression is not allowed in this context");
                }
                n
            }
            T::Insert => {
                self.errs.at(&n, format!("Unexpected value '{}'", n.text()));
                n
            }
            _ => {
                let mut defs = Vec::new();
                self.schema.scalars(info.name, &mut defs);
                if defs.iter().any(|d| matches(d, &n)) {
                    return n;
                }
                if !matches!(n.t, T::Str(_)) {
                    let s = Node { t: T::Str(n.text()), ..n };
                    if defs.iter().any(|d| matches(d, &s)) {
                        return s;
                    }
                }
                self.errs.at(&n, format!("Unexpected value '{}'", n.text()));
                n
            }
        }
    }

    /// Splits a string at `${{ }}` into literal and expression segments (`parseScalar`).
    fn parse_scalar(&mut self, n: &Node, info: &Info<'_>) -> Node {
        let T::Str(raw) = &n.t else { return n.clone() };
        if !raw.contains("${{") {
            return n.clone();
        }
        let cs: Vec<char> = raw.chars().collect();
        let find = |from: usize| {
            (from..cs.len().saturating_sub(2)).find(|&i| cs[i] == '$' && cs[i + 1] == '{' && cs[i + 2] == '{')
        };
        enum Seg {
            Lit(String),
            Expr(String),
            Insert,
        }
        let mut segs: Vec<Seg> = Vec::new();
        let mut errored = false;
        let allowed = Allowed::new(&info.allowed);
        let mut start = find(0);
        let mut i = 0;
        while i < cs.len() {
            if Some(i) == start {
                let s = i;
                let mut end = None;
                let mut in_str = false;
                i += 3;
                while i < cs.len() {
                    if cs[i] == '\'' {
                        in_str = !in_str;
                    } else if !in_str && cs[i] == '}' && cs[i - 1] == '}' {
                        end = Some(i);
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                let Some(e) = end else {
                    self.errs.at(
                        n,
                        "The expression is not closed. An unescaped ${{ sequence was found, but the closing }} sequence was not found.",
                    );
                    return n.clone();
                };
                let inner: String = cs[s + 3..e - 1].iter().collect();
                let trimmed = inner.trim();
                if trimmed.is_empty() {
                    self.errs.at(n, "An expression was expected");
                    errored = true;
                } else if trimmed == "insert" {
                    if s != 0 || i < cs.len() {
                        self.errs.at(
                            n,
                            "The directive 'insert' is not allowed in this context. Directives are not supported for expressions that are embedded within a string. Directives are only supported when the entire value is an expression.",
                        );
                        return n.clone();
                    }
                    segs.push(Seg::Insert);
                } else if let Err(e) = expr::parse(trimmed, &allowed) {
                    self.errs.at(n, e);
                    errored = true;
                } else {
                    segs.push(Seg::Expr(trimmed.into()));
                }
                start = find(i);
            } else {
                let to = start.unwrap_or(cs.len());
                let text: String = cs[i..to].iter().collect();
                match segs.last_mut() {
                    Some(Seg::Lit(l)) => l.push_str(&text),
                    _ => segs.push(Seg::Lit(text)),
                }
                i = to;
            }
        }
        if errored {
            return n.clone();
        }
        let t = match segs.as_slice() {
            [Seg::Expr(e)] => match literal_string(e) {
                Some(s) => T::Str(s),
                None => T::Expr(e.clone()),
            },
            [Seg::Insert] => T::Insert,
            _ => {
                let mut fmt = String::new();
                let mut args = String::new();
                let mut k = 0;
                for s in &segs {
                    match s {
                        Seg::Lit(l) => fmt.push_str(&l.replace('\'', "''").replace('{', "{{").replace('}', "}}")),
                        Seg::Expr(e) => {
                            fmt.push_str(&format!("{{{k}}}"));
                            k += 1;
                            args.push_str(", ");
                            args.push_str(e);
                        }
                        Seg::Insert => {}
                    }
                }
                T::Expr(format!("format('{fmt}'{args})"))
            }
        };
        Node { t, ..*n }
    }
}

fn prop_type<'d>(d: &'d Def, name: &str) -> Option<&'d str> {
    match d {
        Def::Map { props, .. } => props.iter().find(|p| p.0 == name).map(|p| p.1.as_str()),
        _ => None,
    }
}

/// An expression that is only string literals, such as `'{{ x }}'`, is that string (`getExpressionString`).
fn literal_string(e: &str) -> Option<String> {
    let mut out = String::new();
    let mut in_str = false;
    for (i, c) in e.chars().enumerate() {
        if c == '\'' {
            in_str = !in_str;
            if in_str && i != 0 {
                out.push(c);
            }
        } else if !in_str {
            return None;
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// Reads `root` against the schema definition `name`, adding the oracle's errors to `errs`.
pub fn read(root: &Node, name: &str, errs: &mut Errors) -> Node {
    let schema = Schema::get();
    let mut r = Reader { schema, errs };
    let info = r.info(&Info { name: "any", allowed: Vec::new() }, r.schema_name(name));
    r.read(root, &info)
}
