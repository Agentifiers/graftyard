//! The GraphQL executor the resolvers run under: a parsed document, its variables, and a tree of `Obj`s, each a GHES
//! type name and a resolver for one field at a time (`design/SHARED.md`, `graphql`: "Unimplemented fields return
//! `undefinedField` errors").
//!
//! It implements what gh's documents use: named operations, variables with defaults, aliases, fragment spreads,
//! inline fragments with a type condition, `__typename`, and `__type(name:)` from `introspect.txt`. Field selection
//! on an object that does not resolve a name is the GraphQL error `undefinedField`, never a silent null.

use std::collections::BTreeMap;

use graphql_parser::query::{
    Definition, Document, Field, FragmentDefinition, OperationDefinition, Selection, SelectionSet, TypeCondition,
    Value as Gv, parse_query,
};
use serde_json::{Map, Value, json};

/// One field's arguments, with variables substituted.
pub type Args = BTreeMap<String, Value>;

/// What a resolver returns for one field.
pub enum Out<'r> {
    /// A leaf: a JSON scalar, enum value, or null.
    Leaf(Value),
    /// An object of a GHES type.
    Obj(Box<dyn Obj + 'r>),
    /// A list of objects.
    List(Vec<Box<dyn Obj + 'r>>),
    /// `null` for a nullable object field.
    Null,
    /// A field error, such as GitHub's `NOT_FOUND`: the field is `null` and the error is listed beside the data.
    Err(&'static str, String),
}

pub fn leaf(v: impl Into<Value>) -> Out<'static> {
    Out::Leaf(v.into())
}

pub fn obj<'r>(o: impl Obj + 'r) -> Out<'r> {
    Out::Obj(Box::new(o))
}

impl<T: Obj + ?Sized> Obj for &T {
    fn type_name(&self) -> &'static str {
        (**self).type_name()
    }
    fn implements(&self) -> &'static [&'static str] {
        (**self).implements()
    }
    fn field<'s>(&'s self, field: &str, args: &Args) -> Option<Out<'s>> {
        (**self).field(field, args)
    }
}

/// An object of a GHES type: its concrete type name, the abstract types it implements, and its fields.
pub trait Obj {
    fn type_name(&self) -> &'static str;
    /// The interfaces and unions this type belongs to, for inline fragments on them.
    fn implements(&self) -> &'static [&'static str] {
        &[]
    }
    /// `None` when this resolver does not serve `field`, which the executor answers with `undefinedField`.
    fn field<'s>(&'s self, field: &str, args: &Args) -> Option<Out<'s>>;
}

/// A GraphQL error as GitHub's server shapes it.
#[derive(Clone, Debug, PartialEq)]
pub struct GqlError {
    pub kind: &'static str,
    pub message: String,
    pub path: Vec<String>,
}

impl GqlError {
    pub fn json(&self) -> Value {
        let mut m = json!({ "type": self.kind, "message": self.message });
        if !self.path.is_empty() {
            m["path"] = json!(self.path);
        }
        m
    }
}

/// The chosen operation: whether it is a mutation, its selection set, and its variables with defaults.
type Operation<'a, 'q> = (bool, &'a SelectionSet<'q, String>, Map<String, Value>);

/// The parsed request: its operation and fragments.
pub struct Request<'q> {
    doc: Document<'q, String>,
    vars: Map<String, Value>,
    op: Option<String>,
}

impl<'q> Request<'q> {
    pub fn parse(
        query: &'q str,
        vars: Option<Map<String, Value>>,
        op: Option<String>,
    ) -> Result<Request<'q>, GqlError> {
        let doc = parse_query::<String>(query).map_err(|e| GqlError {
            kind: "parseError",
            message: e.to_string(),
            path: Vec::new(),
        })?;
        Ok(Request { doc, vars: vars.unwrap_or_default(), op })
    }

    fn operation(&self) -> Result<Operation<'_, 'q>, GqlError> {
        let mut found = None;
        for d in &self.doc.definitions {
            let Definition::Operation(o) = d else { continue };
            let (name, mutation, set, defs) = match o {
                OperationDefinition::Query(q) => (q.name.as_deref(), false, &q.selection_set, &q.variable_definitions),
                OperationDefinition::Mutation(m) => {
                    (m.name.as_deref(), true, &m.selection_set, &m.variable_definitions)
                }
                OperationDefinition::SelectionSet(s) => (None, false, s, &Vec::new() as &Vec<_>),
                OperationDefinition::Subscription(_) => {
                    return Err(GqlError {
                        kind: "unsupported",
                        message: "subscriptions are not served".into(),
                        path: Vec::new(),
                    });
                }
            };
            if self.op.as_deref().is_none_or(|w| Some(w) == name) {
                let mut vars = self.vars.clone();
                for v in defs {
                    if !vars.contains_key(&v.name)
                        && let Some(d) = &v.default_value
                    {
                        vars.insert(v.name.clone(), to_json(d, &Map::new()));
                    }
                }
                found = Some((mutation, set, vars));
                break;
            }
        }
        found.ok_or_else(|| GqlError { kind: "unknownOperation", message: "no operation to run".into(), path: vec![] })
    }

    fn fragment<'a>(&'a self, name: &str) -> Option<&'a FragmentDefinition<'q, String>> {
        self.doc.definitions.iter().find_map(|d| match d {
            Definition::Fragment(f) if f.name == name => Some(f),
            _ => None,
        })
    }

    pub fn is_mutation(&self) -> Result<bool, GqlError> {
        Ok(self.operation()?.0)
    }

    /// The operation's top-level fields with their arguments, variables substituted, for routing.
    pub fn roots(&self) -> Result<Vec<(String, Args)>, GqlError> {
        let (_, set, vars) = self.operation()?;
        let mut out = Vec::new();
        for s in &set.items {
            if let Selection::Field(f) = s {
                out.push((f.name.clone(), f.arguments.iter().map(|(k, v)| (k.clone(), to_json(v, &vars))).collect()));
            }
        }
        Ok(out)
    }

    /// Runs the operation on `root` and returns GitHub's `{"data": ..}` or `{"data": null, "errors": [..]}`.
    pub fn run(&self, root: &dyn Obj) -> Value {
        let (_, set, vars) = match self.operation() {
            Ok(o) => o,
            Err(e) => return json!({ "errors": [e.json()] }),
        };
        let mut errors = Vec::new();
        let data = Ex { req: self, vars: &vars, errors: &mut errors }.object(root, &[set], &mut Vec::new());
        let list: Vec<Value> = errors.iter().map(GqlError::json).collect();
        if errors.iter().any(|e| e.kind == "undefinedField") {
            // A query error: GitHub answers it with no data.
            json!({ "errors": list })
        } else if errors.is_empty() {
            json!({ "data": data })
        } else {
            json!({ "data": data, "errors": list })
        }
    }
}

fn to_json(v: &Gv<'_, String>, vars: &Map<String, Value>) -> Value {
    match v {
        Gv::Variable(n) => vars.get(n).cloned().unwrap_or(Value::Null),
        Gv::Int(i) => json!(i.as_i64()),
        Gv::Float(f) => json!(f),
        Gv::String(s) => json!(s),
        Gv::Boolean(b) => json!(b),
        Gv::Null => Value::Null,
        Gv::Enum(e) => json!(e),
        Gv::List(l) => Value::Array(l.iter().map(|x| to_json(x, vars)).collect()),
        Gv::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), to_json(x, vars))).collect()),
    }
}

struct Ex<'a, 'q> {
    req: &'a Request<'q>,
    vars: &'a Map<String, Value>,
    errors: &'a mut Vec<GqlError>,
}

impl<'a, 'q> Ex<'a, 'q> {
    fn applies(o: &dyn Obj, cond: &str) -> bool {
        cond == o.type_name() || o.implements().contains(&cond)
    }

    /// The fields of `set` that apply to `o`, in document order, with fragments flattened.
    fn collect(&self, o: &dyn Obj, set: &'a SelectionSet<'q, String>, out: &mut Vec<&'a Field<'q, String>>) {
        for s in &set.items {
            match s {
                Selection::Field(f) => out.push(f),
                Selection::InlineFragment(i) => {
                    let ok = match &i.type_condition {
                        Some(TypeCondition::On(t)) => Self::applies(o, t),
                        None => true,
                    };
                    if ok {
                        self.collect(o, &i.selection_set, out);
                    }
                }
                Selection::FragmentSpread(fs) => {
                    if let Some(f) = self.req.fragment(&fs.fragment_name) {
                        let TypeCondition::On(t) = &f.type_condition;
                        if Self::applies(o, t) {
                            self.collect(o, &f.selection_set, out);
                        }
                    }
                }
            }
        }
    }

    fn object(&mut self, o: &dyn Obj, sets: &[&'a SelectionSet<'q, String>], path: &mut Vec<String>) -> Value {
        // Fields with one response key merge their selection sets and resolve once, as the spec's CollectFields does.
        let mut fields = Vec::new();
        for set in sets {
            self.collect(o, set, &mut fields);
        }
        let mut grouped: Vec<(String, Vec<&'a Field<'q, String>>)> = Vec::new();
        for f in fields {
            let key = f.alias.clone().unwrap_or_else(|| f.name.clone());
            match grouped.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(f),
                None => grouped.push((key, vec![f])),
            }
        }
        let mut m = Map::new();
        for (key, fs) in grouped {
            path.push(key.clone());
            let v = self.field(o, &fs, path);
            path.pop();
            m.insert(key, v);
        }
        Value::Object(m)
    }

    fn field(&mut self, o: &dyn Obj, fs: &[&'a Field<'q, String>], path: &mut Vec<String>) -> Value {
        let f = fs[0];
        let sets: Vec<&'a SelectionSet<'q, String>> = fs.iter().map(|f| &f.selection_set).collect();
        if f.name == "__typename" {
            return json!(o.type_name());
        }
        let args: Args = f.arguments.iter().map(|(k, v)| (k.clone(), to_json(v, self.vars))).collect();
        if f.name == "__type" {
            return introspect(args.get("name").and_then(Value::as_str).unwrap_or(""), &f.selection_set);
        }
        match o.field(&f.name, &args) {
            Some(Out::Leaf(v)) => v,
            Some(Out::Null) => Value::Null,
            Some(Out::Err(kind, message)) => {
                self.errors.push(GqlError { kind, message, path: path.clone() });
                Value::Null
            }
            Some(Out::Obj(c)) => self.object(c.as_ref(), &sets, path),
            Some(Out::List(items)) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        path.push(i.to_string());
                        let v = self.object(c.as_ref(), &sets, path);
                        path.pop();
                        v
                    })
                    .collect(),
            ),
            None => {
                self.errors.push(GqlError {
                    kind: "undefinedField",
                    message: format!("Field '{}' doesn't exist on type '{}'", f.name, o.type_name()),
                    path: path.clone(),
                });
                Value::Null
            }
        }
    }
}

const INTROSPECT: &str = include_str!("introspect.txt");

/// `__type(name:)` for the types gh probes: `fields(includeDeprecated: true){name}` or
/// `enumValues(includeDeprecated: true){name}`, from the pinned GHES 3.22 SDL (`gh/introspect.py`).
fn introspect(name: &str, set: &SelectionSet<'_, String>) -> Value {
    let Some(line) = INTROSPECT.lines().find(|l| l.split(' ').next() == Some(name)) else { return Value::Null };
    let mut parts = line.split(' ').skip(1);
    let kind = parts.next();
    let names: Vec<Value> = parts.map(|p| json!({ "name": p.trim_end_matches('!') })).collect();
    let mut m = Map::new();
    for s in &set.items {
        let Selection::Field(f) = s else { continue };
        let key = f.alias.clone().unwrap_or_else(|| f.name.clone());
        let v = match (f.name.as_str(), kind) {
            ("fields", Some("F")) | ("enumValues", Some("E")) => Value::Array(names.clone()),
            ("name", _) => json!(name),
            _ => Value::Null,
        };
        m.insert(key, v);
    }
    Value::Object(m)
}
