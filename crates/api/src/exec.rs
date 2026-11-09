//! The GraphQL executor the resolvers run under: a parsed document, its variables, and a tree of `Obj`s, each a GHES
//! type name and a resolver for one field at a time (`design/SHARED.md`, `graphql`: "Unimplemented fields return
//! `undefinedField` errors").
//!
//! It implements what gh's documents use: named operations, variables with defaults, aliases, fragment spreads,
//! inline fragments with a type condition, `__typename`, and `__type(name:)`. The schema generated from the pinned
//! GHES 3.22 SDL (`crate::schema`) decides which fields exist, which type conditions apply, and what each `__type`
//! probe answers, and every value a resolver returns is checked against its field's type. A field the schema lacks,
//! or one no resolver serves, is the GraphQL error `undefinedField`, never a silent null. A value that breaks its
//! field's type is the error `INTERNAL` with the field null, so a resolver bug never reaches a client as a wrong
//! shape.

use std::collections::BTreeMap;

use graphql_parser::query::{
    Definition, Document, Field, FragmentDefinition, OperationDefinition, Selection, SelectionSet, TypeCondition,
    Value as Gv, parse_query,
};
use serde_json::{Map, Value, json};

use crate::schema::{Kind, Schema, TypeRef};

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
    fn field<'s>(&'s self, field: &str, args: &Args) -> Option<Out<'s>> {
        (**self).field(field, args)
    }
}

/// An object of a GHES object type: its type name, which must be an object type of the schema, and its fields.
pub trait Obj {
    fn type_name(&self) -> &'static str;
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
        Schema::get().applies(o.type_name(), cond)
    }

    /// The fields of `set` that apply to `o`, in document order, with fragments flattened.
    ///
    /// `seen` is the spec's `visitedFragments`: a fragment spread already expanded for this object is skipped, so a
    /// cycle ends and a chain of repeated spreads expands each fragment once. A skipped spread could only repeat
    /// fields already collected, which `object` merges by response key.
    fn collect(
        &self,
        o: &dyn Obj,
        set: &'a SelectionSet<'q, String>,
        seen: &mut Vec<&'a str>,
        out: &mut Vec<&'a Field<'q, String>>,
    ) {
        for s in &set.items {
            match s {
                Selection::Field(f) => out.push(f),
                Selection::InlineFragment(i) => {
                    let ok = match &i.type_condition {
                        Some(TypeCondition::On(t)) => Self::applies(o, t),
                        None => true,
                    };
                    if ok {
                        self.collect(o, &i.selection_set, seen, out);
                    }
                }
                Selection::FragmentSpread(fs) => {
                    if seen.contains(&fs.fragment_name.as_str()) {
                        continue;
                    }
                    seen.push(&fs.fragment_name);
                    if let Some(f) = self.req.fragment(&fs.fragment_name) {
                        let TypeCondition::On(t) = &f.type_condition;
                        if Self::applies(o, t) {
                            self.collect(o, &f.selection_set, seen, out);
                        }
                    }
                }
            }
        }
    }

    fn object(&mut self, o: &dyn Obj, sets: &[&'a SelectionSet<'q, String>], path: &mut Vec<String>) -> Value {
        // Fields with one response key merge their selection sets and resolve once, as the spec's CollectFields does.
        let mut fields = Vec::new();
        let mut seen = Vec::new();
        for set in sets {
            self.collect(o, set, &mut seen, &mut fields);
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
        let undefined = |path: &[String]| GqlError {
            kind: "undefinedField",
            message: format!("Field '{}' doesn't exist on type '{}'", f.name, o.type_name()),
            path: path.to_vec(),
        };
        let Some(ty) = Schema::get().ty(o.type_name()).and_then(|t| t.field(&f.name)).map(|d| d.ty) else {
            self.errors.push(undefined(path));
            return Value::Null;
        };
        let wrong = |path: &[String], why: String| GqlError {
            kind: "INTERNAL",
            message: format!("{}.{} is {}: {why}", o.type_name(), f.name, ty.0),
            path: path.to_vec(),
        };
        let out = match o.field(&f.name, &args) {
            None => {
                self.errors.push(undefined(path));
                return Value::Null;
            }
            Some(out) => out,
        };
        let checked = match out {
            Out::Leaf(v) => leaf_fits(ty, &v).map(|()| v),
            Out::Null if ty.non_null() => Err("null".into()),
            Out::Null => Ok(Value::Null),
            Out::Err(kind, message) => {
                self.errors.push(GqlError { kind, message, path: path.clone() });
                return Value::Null;
            }
            Out::Obj(c) => match object_fits(ty.nullable(), c.type_name()) {
                Ok(()) => Ok(self.object(c.as_ref(), &sets, path)),
                Err(e) => Err(e),
            },
            Out::List(items) => match ty.item() {
                None => Err("a list".into()),
                Some(item) => {
                    match items.iter().map(|c| object_fits(item.nullable(), c.type_name())).find(Result::is_err) {
                        Some(e) => e.map(|()| Value::Null),
                        None => Ok(Value::Array(
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
                        )),
                    }
                }
            },
        };
        checked.unwrap_or_else(|why| {
            self.errors.push(wrong(path, why));
            Value::Null
        })
    }
}

/// Whether an object of `object` may stand where the schema says `ty`.
fn object_fits(ty: TypeRef<'_>, object: &str) -> Result<(), String> {
    let s = Schema::get();
    let named = ty.named();
    if ty.item().is_some() {
        return Err(format!("an object of {object} where a list was expected"));
    }
    match s.ty(object).map(|t| t.kind) {
        Some(Kind::Object) if s.applies(object, named) => Ok(()),
        _ => Err(format!("an object of {object}")),
    }
}

/// Whether a leaf value fits a scalar or enum type: GraphQL's result coercion, checked.
fn leaf_fits(ty: TypeRef<'_>, v: &Value) -> Result<(), String> {
    if v.is_null() {
        return if ty.non_null() { Err("null".into()) } else { Ok(()) };
    }
    if let Some(item) = ty.item() {
        let Value::Array(a) = v else { return Err(format!("{v}, not a list")) };
        return a.iter().try_for_each(|x| leaf_fits(item, x));
    }
    let name = ty.named();
    let Some(t) = Schema::get().ty(name) else { return Err(format!("unknown type {name}")) };
    let ok = match (t.kind, name) {
        (Kind::Enum, _) => v.as_str().is_some_and(|s| t.values.iter().any(|(n, _)| *n == s)),
        (Kind::Scalar, "Int") => v.as_i64().is_some_and(|i| i32::try_from(i).is_ok()),
        (Kind::Scalar, "Float") => v.is_number(),
        (Kind::Scalar, "Boolean") => v.is_boolean(),
        (Kind::Scalar, "ID") => v.is_string() || v.is_i64(),
        // Every other scalar in the GHES schema (String, DateTime, URI, HTML, GitObjectID, BigInt, ...) is a string.
        (Kind::Scalar, _) => v.is_string(),
        _ => false,
    };
    if ok { Ok(()) } else { Err(format!("{v}")) }
}

/// `__type(name:)`: `name`, `kind`, and `fields` or `enumValues` with `includeDeprecated`, each item's `name`,
/// from the generated schema, so gh's field probes see exactly GHES 3.22's lists.
fn introspect(name: &str, set: &SelectionSet<'_, String>) -> Value {
    let Some(t) = Schema::get().ty(name) else { return Value::Null };
    let mut m = Map::new();
    for s in &set.items {
        let Selection::Field(f) = s else { continue };
        let key = f.alias.clone().unwrap_or_else(|| f.name.clone());
        let all = f.arguments.iter().any(|(k, v)| k == "includeDeprecated" && matches!(v, Gv::Boolean(true)));
        let names = |items: Vec<(&str, bool)>| {
            let pick = |n: &str| {
                let mut o = Map::new();
                for x in &f.selection_set.items {
                    if let Selection::Field(x) = x {
                        let v = if x.name == "name" { json!(n) } else { Value::Null };
                        o.insert(x.alias.clone().unwrap_or_else(|| x.name.clone()), v);
                    }
                }
                Value::Object(o)
            };
            Value::Array(items.into_iter().filter(|(_, dep)| all || !dep).map(|(n, _)| pick(n)).collect())
        };
        let v = match f.name.as_str() {
            "name" => json!(name),
            "kind" => json!(t.kind.introspection()),
            "fields" if matches!(t.kind, Kind::Object | Kind::Interface) => {
                names(t.fields.iter().map(|x| (x.name, x.deprecated)).collect())
            }
            "enumValues" if t.kind == Kind::Enum => names(t.values.clone()),
            _ => Value::Null,
        };
        m.insert(key, v);
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        ty: &'static str,
        number: Value,
    }

    impl Obj for Fake {
        fn type_name(&self) -> &'static str {
            self.ty
        }
        fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
            Some(match f {
                "number" => Out::Leaf(self.number.clone()),
                "state" => leaf("OPEN"),
                "title" => Out::Null,
                "repository" => obj(Fake { ty: "Issue", number: json!(1) }),
                _ => return None,
            })
        }
    }

    struct Root(Fake);

    impl Obj for Root {
        fn type_name(&self) -> &'static str {
            "Query"
        }
        fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
            (f == "node").then(|| obj(&self.0))
        }
    }

    fn run(q: &str, number: Value) -> Value {
        Request::parse(q, None, None).unwrap().run(&Root(Fake { ty: "PullRequest", number }))
    }

    #[test]
    fn every_value_is_checked_against_the_generated_schema() {
        let ok = run(r#"{ node(id: "x") { ... on PullRequest { number state } } }"#, json!(2));
        assert_eq!(ok, json!({ "data": { "node": { "number": 2, "state": "OPEN" } } }));
        // A string where `Int!` stands, a null where `String!` stands, and an `Issue` where `Repository!` stands.
        for (q, n) in [("number", json!("2")), ("title", json!(2)), ("repository { __typename }", json!(2))] {
            let r = run(&format!(r#"{{ node(id: "x") {{ ... on PullRequest {{ {q} }} }} }}"#), n);
            assert_eq!(r["errors"][0]["type"], "INTERNAL", "{q}: {r}");
        }
        // A field GHES does not have is `undefinedField` before any resolver runs.
        let r = run(r#"{ node(id: "x") { ... on PullRequest { nope } } }"#, json!(2));
        assert_eq!(r["errors"][0]["type"], "undefinedField");
        // A fragment on an interface the object implements applies; one on an unrelated type does not.
        let r = run(r#"{ node(id: "x") { ... on Closable { __typename } ... on Issue { number } } }"#, json!(2));
        assert_eq!(r, json!({ "data": { "node": { "__typename": "PullRequest" } } }));
    }

    #[test]
    fn type_probes_list_exactly_ghes_fields() {
        let q = r#"{ Release: __type(name: "Release") { fields(includeDeprecated: true) { name } } }"#;
        let r = Request::parse(q, None, None).unwrap().run(&Root(Fake { ty: "Issue", number: json!(1) }));
        let names: Vec<&str> =
            r["data"]["Release"]["fields"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
        let want: Vec<&str> = Schema::get().ty("Release").unwrap().fields.iter().map(|f| f.name).collect();
        assert_eq!(names, want);
        assert!(names.contains(&"isLatest") && names.contains(&"immutable"));
        // gh's own probe asks with `includeDeprecated: true`; `Issue.timeline` is deprecated in GHES 3.22.
        let probe = |all: bool| {
            let q = format!(r#"{{ t: __type(name: "Issue") {{ fields(includeDeprecated: {all}) {{ name }} }} }}"#);
            let r = Request::parse(&q, None, None).unwrap().run(&Root(Fake { ty: "Issue", number: json!(1) }));
            r["data"]["t"]["fields"].as_array().unwrap().iter().any(|f| f["name"] == "timeline")
        };
        assert!(probe(true), "a deprecated field is listed when asked for");
        assert!(!probe(false), "and only then");
    }
}
