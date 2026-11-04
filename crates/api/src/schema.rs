//! The GHES 3.22 GraphQL schema, generated from the pinned SDL by `examples/gen_schema.rs` into `schema.txt`
//! (`design/SHARED.md`, `graphql`). The executor checks every resolver's answer against it, answers `__type` probes
//! from it, and takes interface and union membership from it, so no resolver declares a type fact by hand.

use std::collections::HashMap;
use std::sync::OnceLock;

const SCHEMA: &str = include_str!("schema.txt");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Scalar,
    Object,
    Interface,
    Union,
    Enum,
    InputObject,
}

impl Kind {
    pub fn introspection(self) -> &'static str {
        match self {
            Kind::Scalar => "SCALAR",
            Kind::Object => "OBJECT",
            Kind::Interface => "INTERFACE",
            Kind::Union => "UNION",
            Kind::Enum => "ENUM",
            Kind::InputObject => "INPUT_OBJECT",
        }
    }
}

/// A field's type as the SDL writes it: `[Label!]!` and so on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeRef<'s>(pub &'s str);

impl<'s> TypeRef<'s> {
    pub fn non_null(self) -> bool {
        self.0.ends_with('!')
    }

    /// The type without its outer `!`.
    pub fn nullable(self) -> TypeRef<'s> {
        TypeRef(self.0.strip_suffix('!').unwrap_or(self.0))
    }

    /// The item type of a list, or `None` for a named type.
    pub fn item(self) -> Option<TypeRef<'s>> {
        let t = self.nullable().0;
        t.strip_prefix('[').and_then(|t| t.strip_suffix(']')).map(TypeRef)
    }

    pub fn named(self) -> &'s str {
        self.0.trim_matches(|c| c == '[' || c == ']' || c == '!')
    }
}

#[derive(Debug)]
pub struct Field<'s> {
    pub name: &'s str,
    pub ty: TypeRef<'s>,
    pub deprecated: bool,
}

#[derive(Debug)]
pub struct TypeDef<'s> {
    pub name: &'s str,
    pub kind: Kind,
    /// An object's or interface's interfaces, or a union's members.
    pub related: Vec<&'s str>,
    pub fields: Vec<Field<'s>>,
    /// An enum's values: (name, deprecated).
    pub values: Vec<(&'s str, bool)>,
}

impl<'s> TypeDef<'s> {
    pub fn field(&self, name: &str) -> Option<&Field<'s>> {
        self.fields.iter().find(|f| f.name == name)
    }
}

pub struct Schema {
    types: HashMap<&'static str, TypeDef<'static>>,
    /// The sha256 of the SDL the file was generated from.
    pub sdl_sha256: &'static str,
}

impl Schema {
    pub fn get() -> &'static Schema {
        static S: OnceLock<Schema> = OnceLock::new();
        S.get_or_init(|| Schema::parse(SCHEMA))
    }

    fn parse(text: &'static str) -> Schema {
        let mut types: HashMap<&str, TypeDef> = HashMap::new();
        let mut sha = "";
        let mut cur: Option<&str> = None;
        for line in text.lines() {
            let mut p = line.split(' ');
            match p.next() {
                Some("#") => sha = line.rsplit(' ').next().unwrap_or(""),
                Some("T") => {
                    let (name, kind, related) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
                    let kind = match kind {
                        "S" => Kind::Scalar,
                        "O" => Kind::Object,
                        "I" => Kind::Interface,
                        "U" => Kind::Union,
                        "E" => Kind::Enum,
                        _ => Kind::InputObject,
                    };
                    let related = if related == "-" { Vec::new() } else { related.split(',').collect() };
                    types.insert(name, TypeDef { name, kind, related, fields: Vec::new(), values: Vec::new() });
                    cur = Some(name);
                }
                Some("F") => {
                    let (name, ty, dep) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
                    let t = types.get_mut(cur.unwrap()).unwrap();
                    t.fields.push(Field { name, ty: TypeRef(ty), deprecated: dep == "1" });
                }
                Some("V") => {
                    let (name, dep) = (p.next().unwrap(), p.next().unwrap());
                    types.get_mut(cur.unwrap()).unwrap().values.push((name, dep == "1"));
                }
                _ => {}
            }
        }
        for b in ["String", "Int", "Float", "Boolean", "ID"] {
            types.entry(b).or_insert(TypeDef {
                name: b,
                kind: Kind::Scalar,
                related: vec![],
                fields: vec![],
                values: vec![],
            });
        }
        Schema { types, sdl_sha256: sha }
    }

    pub fn ty(&self, name: &str) -> Option<&TypeDef<'static>> {
        self.types.get(name)
    }

    /// Whether an object of type `object` matches a fragment's type condition `cond`: the same type, an interface it
    /// implements, or a union it belongs to.
    pub fn applies(&self, object: &str, cond: &str) -> bool {
        if object == cond {
            return true;
        }
        match self.ty(cond).map(|t| (t.kind, &t.related)) {
            Some((Kind::Union, members)) => members.contains(&object),
            Some((Kind::Interface, _)) => self.ty(object).is_some_and(|o| o.related.contains(&cond)),
            _ => false,
        }
    }

    pub fn possible_types(&self, abstract_ty: &str) -> Vec<&'static str> {
        let Some(t) = self.ty(abstract_ty) else { return Vec::new() };
        match t.kind {
            Kind::Union => t.related.clone(),
            Kind::Interface => {
                let mut v: Vec<&str> = self
                    .types
                    .values()
                    .filter(|o| o.kind == Kind::Object && o.related.contains(&abstract_ty))
                    .map(|o| o.name)
                    .collect();
                v.sort();
                v
            }
            _ => Vec::new(),
        }
    }

    pub fn count(&self) -> usize {
        self.types.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_is_the_pinned_sdl() {
        let s = Schema::get();
        assert_eq!(
            s.sdl_sha256, "fe9c21b8fa21a2991cbbb9cfa73a13b8f2642691f64d24184b324347b9fe1391",
            "gh/README.md's pin"
        );
        assert!(s.count() > 1_600);
        let release = s.ty("Release").unwrap();
        assert_eq!(release.kind, Kind::Object);
        assert_eq!(release.field("tagName").unwrap().ty, TypeRef("String!"));
        assert_eq!(release.field("releaseAssets").unwrap().ty.named(), "ReleaseAssetConnection");
        assert!(s.applies("PullRequest", "IssueOrPullRequest"), "union");
        assert!(s.applies("PullRequest", "Node"), "interface");
        assert!(!s.applies("Issue", "PullRequest"));
        assert!(s.possible_types("IssueOrPullRequest").contains(&"Issue"));
        let st = s.ty("PullRequestState").unwrap();
        assert_eq!(st.values.iter().map(|v| v.0).collect::<Vec<_>>(), ["CLOSED", "MERGED", "OPEN"]);
        assert_eq!(TypeRef("[Label!]!").item(), Some(TypeRef("Label!")));
    }
}
