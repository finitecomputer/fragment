//! What serde reads and writes of the wire's types, read from crates/proto's
//! source rather than inferred from their schemas. A schema is schemars'
//! reading of serde's attributes, and it holds only for those schemars
//! reads as serde does; a sample cannot show where it does not. So the wire
//! supports a short list of serde's attributes and refuses the rest, each
//! refusal naming the type, the field and the attribute.
//!
//! Read with `syn` from the files this binary was built from: serde_derive
//! reads these same attributes from the same tokens, so the source is where
//! they are. A registry made at derive time would need a derive of our own
//! on every wire type, and a type without it would go unseen; here a type
//! the guard cannot read fails closed instead: a field's type it does not
//! know, a component of the document no type it read is named for, a macro
//! that could make types or impls out of its sight.
//!
//! Supported, on a type reachable from `routes.rs` (and `ErrorBody`):
//! - serde, on a container: `rename_all`, `rename`, `tag` (internally
//!   tagged), `deny_unknown_fields`; on a variant: `rename`, `other`; on a
//!   field: `rename`, `default`, `skip_serializing_if`;
//! - schemars: `extend("default" = …)` (held to serde: defaults.rs),
//!   `extend("x-fragment-open" = true)` on a type `OPEN` lists, `example`;
//! - an enum of unit variants (a string), or one tagged by `tag`;
//! - fields of the wire's types, and of `FOREIGN`'s (`String`, `bool`, the
//!   sized integers, floats, `Option`, `Vec`, `Box`, `BTreeMap`,
//!   `BTreeSet`, `serde_json::Value`, `RawValue`), as their paths resolve;
//! - derives of serde, schemars and the standard library (`DERIVES`);
//! - the hooks and impls written by hand in `HAND_WRITTEN`, each with the
//!   test that holds it to its schema, and the fields in `FLATTENED`.
//!
//! Each attribute is read through `cfg_attr`s, however nested.
//!
//! Refused: every other serde or schemars attribute (`untagged`, `alias`,
//! `skip*`, `content`, `transparent`, `remote`, `bound`, …); a validator's
//! (`validate`, `garde`), which schemars writes as bounds serde never
//! checks; any other derive; an open marker `OPEN` does not list; a hook,
//! or a `Serialize`/`Deserialize`/`JsonSchema` impl, not in `HAND_WRITTEN`
//! (an impl anywhere, a function's body too), and an entry there the
//! source no longer has or whose test it does not; `flatten` but at a
//! `FLATTENED` field; an enum with data and no `tag` (externally tagged);
//! a generic type; a field's type it cannot resolve; a wire name (a rename
//! or a tag) outside `[A-Za-z0-9_-]`; a component of the document no type
//! it read is named for; a macro defined anywhere in the crate, or invoked
//! where an item stands. A macro invoked inside an expression is not read:
//! the crate has none that makes items.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Fields, GenericArgument, Item, Lit, Meta, PathArguments, Token, Type};

/// The hooks and impls written by hand behind the wire, each with the
/// proto test that holds it to its schema, frame by frame or name by name.
/// A new one fails generation until it is listed, its test named; an entry
/// the source no longer has fails too.
pub const HAND_WRITTEN: [(&str, &str); 10] = [
    ("Subscribe: serde(try_from = \"SubscribeFrame\")", "a_subscribe_names_exactly_one_cursor_as_its_decoder_reads_it"),
    ("Subscribe: serde(into = \"SubscribeFrame\")", "a_subscribe_names_exactly_one_cursor_as_its_decoder_reads_it"),
    ("impl JsonSchema for SubscribeFrame", "a_subscribe_names_exactly_one_cursor_as_its_decoder_reads_it"),
    ("Answer: serde(try_from = \"AnswerFrame\")", "a_result_is_a_result_or_a_refusal_as_its_decoder_reads_it"),
    ("Answer: serde(into = \"AnswerFrame\")", "a_result_is_a_result_or_a_refusal_as_its_decoder_reads_it"),
    ("impl JsonSchema for AnswerFrame", "a_result_is_a_result_or_a_refusal_as_its_decoder_reads_it"),
    ("AnswerFrame.result: serde(deserialize_with = \"present\")", "a_result_is_a_result_or_a_refusal_as_its_decoder_reads_it"),
    ("ErrorBody.reason: serde(deserialize_with = \"reason_or_none\")", "refusal_reasons_serialize_and_are_add_only"),
    ("impl Serialize for Reason", "refusal_reasons_serialize_and_are_add_only"),
    ("impl JsonSchema for Reason", "refusal_reasons_serialize_and_are_add_only"),
];

/// The record frame's field read through a `Value` (a tagged frame's
/// fields are buffered, and raw JSON text cannot pass through that
/// buffer): serde's own reading of the record after it, held by the test
/// that decodes every frame the cell sends.
pub const RECORD_THROUGH_VALUE: (&str, &str) = ("LiveOut::Record.0: serde(deserialize_with = \"record_through_value\")", "the_fragment_sends_these_frames");

/// The fields the wire flattens, each a struct whose fields schemars and
/// serde both merge into its container's: none today.
pub const FLATTENED: [(&str, &str); 0] = [];

/// The types marked open (`extend("x-fragment-open" = true)`), whose new
/// values the lock lets pass for their readers read past one they do not
/// know, and where each reader does: a claim of the clients, not of serde,
/// so each is listed. A new one fails generation until it is.
pub const OPEN: [(&str, &str); 4] = [
    ("ErrorCode", "read by its HTTP status (the CLI's `Resp::refusal`)"),
    ("FragmentKind", "read as `Other` (`#[serde(other)]`)"),
    ("LiveOut", "skipped (the CLI's `channel --follow`, cell/client.mjs, the iOS app)"),
    ("ListWatchFrame", "ignored (the shell's list, the iOS app)"),
];

/// The types a field may name beyond the wire's own, as their paths
/// resolve (the prelude's by name): serde's and serde_json's readings of
/// them are their own, the same everywhere.
const FOREIGN: [&str; 21] = [
    "String",
    "str",
    "bool",
    "u8",
    "u16",
    "u32",
    "u64",
    "i8",
    "i16",
    "i32",
    "i64",
    "f32",
    "f64",
    "Option",
    "Vec",
    "Box",
    "std::collections::BTreeMap",
    "std::collections::BTreeSet",
    "serde_json::Value",
    "serde_json::value::Value",
    "serde_json::value::RawValue",
];

/// The derives a wire type may have, as their paths resolve: serde's,
/// schemars' and the standard library's, none of which reads or writes
/// JSON another way.
const DERIVES: [&str; 12] = [
    "serde::Serialize",
    "serde::Deserialize",
    "schemars::JsonSchema",
    "Debug",
    "Clone",
    "Copy",
    "PartialEq",
    "Eq",
    "PartialOrd",
    "Ord",
    "Hash",
    "Default",
];

/// serde's attributes the wire supports, where they stand.
const CONTAINER: [&str; 7] = ["rename_all", "rename", "tag", "deny_unknown_fields", "try_from", "from", "into"];
const VARIANT: [&str; 2] = ["rename", "other"];
const FIELD: [&str; 7] = ["rename", "default", "skip_serializing_if", "flatten", "deserialize_with", "serialize_with", "with"];
/// Of these, the hooks: each must be listed in `HAND_WRITTEN`.
const HOOKS: [&str; 6] = ["try_from", "from", "into", "deserialize_with", "serialize_with", "with"];
/// The keys a schemars `extend` may add: a default (held to serde) and
/// the open marker.
const EXTEND: [&str; 2] = ["default", "x-fragment-open"];

/// Where crates/proto's source is: the files this binary was built from.
pub fn proto_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/proto/src")
}

/// A field, a variant's or a struct's.
struct Field {
    /// Its name, or its place in a tuple.
    name: String,
    attrs: Vec<Attribute>,
    ty: Type,
}

struct Variant {
    name: String,
    attrs: Vec<Attribute>,
    fields: Vec<Field>,
    unit: bool,
}

struct Def {
    name: String,
    /// The module it is in (`crate::live`), where its names resolve.
    module: String,
    attrs: Vec<Attribute>,
    generics: bool,
    /// A struct's fields; an enum's are its variants'.
    fields: Vec<Field>,
    variants: Option<Vec<Variant>>,
}

/// A module's names: its own items, and those it `use`s.
#[derive(Default)]
struct Module {
    items: BTreeSet<String>,
    /// A name `use`d, and the path it names.
    uses: BTreeMap<String, Vec<String>>,
    /// Each `use <path>::*`.
    globs: Vec<Vec<String>>,
}

/// What the guard reads of a crate's source.
#[derive(Default)]
pub struct Source {
    /// Each type by its path (`crate::live::Answer`).
    defs: BTreeMap<String, Def>,
    modules: BTreeMap<String, Module>,
    aliases: BTreeMap<String, (String, Type)>,
    /// `(trait, the type's path)` of each `impl Serialize|Deserialize|JsonSchema for`.
    impls: BTreeSet<(String, String)>,
    /// A hand-written `JsonSchema`'s `schema_name`, if it says one.
    schema_names: BTreeMap<String, String>,
    tests: BTreeSet<String>,
    macros: Vec<String>,
    /// `routes.rs`'s consts and statics: where the routes name their types.
    route_consts: Vec<Expr>,
    /// The impls read before the module that names their type was.
    pending_impls: Vec<(String, String, Type, Option<String>)>,
}

impl Source {
    /// The source of every `.rs` file under `dir`, a module of the crate
    /// each (`lib.rs` its root).
    pub fn read_dir(dir: &Path) -> Result<Source> {
        let mut files = vec![];
        collect(dir, &mut files)?;
        let mut texts = vec![];
        for f in files {
            let text = std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?;
            texts.push((f.strip_prefix(dir).unwrap_or(&f).display().to_string(), text));
        }
        Source::read(&texts)
    }

    /// The source of these files, each `(path, text)`: `lib.rs` is the
    /// crate's root, `<name>.rs` its module `name`.
    pub fn read(files: &[(String, String)]) -> Result<Source> {
        let mut s = Source::default();
        for (path, text) in files {
            let file = syn::parse_file(text).with_context(|| format!("crates/proto/src/{path} parses"))?;
            let stem = path.trim_end_matches(".rs");
            let module = if stem == "lib" { "crate".to_string() } else { format!("crate::{}", stem.replace('/', "::")) };
            s.items(&file.items, &module, stem == "routes")?;
            // the impls and macros anywhere in it, a function's body too
            let mut v = Anywhere { module: vec![module.clone()], path, source: &mut s };
            syn::visit::Visit::visit_file(&mut v, &file);
        }
        for (t, module, ty, name) in std::mem::take(&mut s.pending_impls) {
            let at = format!("impl {t} in {module}");
            let Some(Resolved::Def(path)) = s.resolve(&ty, &module) else { bail!("{at}: for a type the guard cannot resolve") };
            if let Some(name) = name {
                s.schema_names.insert(path.clone(), name);
            }
            s.impls.insert((t, path));
        }
        Ok(s)
    }

    fn items(&mut self, items: &[Item], module: &str, routes: bool) -> Result<()> {
        let m = self.modules.entry(module.to_string()).or_default();
        for item in items {
            match item {
                Item::Struct(i) => {
                    m.items.insert(i.ident.to_string());
                }
                Item::Enum(i) => {
                    m.items.insert(i.ident.to_string());
                }
                Item::Type(i) => {
                    m.items.insert(i.ident.to_string());
                }
                Item::Mod(i) => {
                    m.items.insert(i.ident.to_string());
                }
                Item::Use(u) => uses(&u.tree, vec![], m),
                _ => {}
            }
        }
        for item in items {
            match item {
                Item::Struct(i) => {
                    let def = Def { name: i.ident.to_string(), module: module.to_string(), attrs: i.attrs.clone(), generics: !i.generics.params.is_empty(), fields: fields_of(&i.fields), variants: None };
                    self.defs.insert(format!("{module}::{}", def.name), def);
                }
                Item::Enum(i) => {
                    let variants = i
                        .variants
                        .iter()
                        .map(|v| Variant { name: v.ident.to_string(), attrs: v.attrs.clone(), fields: fields_of(&v.fields), unit: matches!(v.fields, Fields::Unit) })
                        .collect();
                    let def = Def { name: i.ident.to_string(), module: module.to_string(), attrs: i.attrs.clone(), generics: !i.generics.params.is_empty(), fields: vec![], variants: Some(variants) };
                    self.defs.insert(format!("{module}::{}", def.name), def);
                }
                Item::Type(i) => {
                    self.aliases.insert(format!("{module}::{}", i.ident), (module.to_string(), (*i.ty).clone()));
                }
                Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        self.items(inner, &format!("{module}::{}", m.ident), routes)?;
                    }
                }
                Item::Fn(f) => {
                    if f.attrs.iter().any(|a| a.path().is_ident("test")) {
                        self.tests.insert(f.sig.ident.to_string());
                    }
                }
                Item::Const(c) if routes => self.route_consts.push((*c.expr).clone()),
                Item::Static(c) if routes => self.route_consts.push((*c.expr).clone()),
                _ => {}
            }
        }
        Ok(())
    }

    /// What a type's path names, seen from `module`: one of the crate's
    /// types (its path), or one of another crate's (its name).
    fn resolve(&self, t: &Type, module: &str) -> Option<Resolved> {
        let Type::Path(p) = t else { return None };
        if p.qself.is_some() {
            return None;
        }
        let segs: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.resolve_path(&segs, module, 0)
    }

    fn resolve_path(&self, segs: &[String], module: &str, depth: usize) -> Option<Resolved> {
        if depth > 16 {
            return None;
        }
        let (first, rest) = segs.split_first()?;
        // where the path starts
        let start = match first.as_str() {
            "crate" => "crate".to_string(),
            "self" => module.to_string(),
            "super" => {
                let mut at = module.rsplit_once("::").map(|(p, _)| p.to_string())?;
                let mut rest = rest;
                while rest.first().is_some_and(|s| s == "super") {
                    at = at.rsplit_once("::").map(|(p, _)| p.to_string())?;
                    rest = &rest[1..];
                }
                let mut full = vec!["crate".to_string()];
                full.extend(at.split("::").skip(1).map(str::to_string));
                full.extend(rest.iter().cloned());
                return if rest.is_empty() { None } else { self.resolve_path(&full, module, depth + 1) };
            }
            _ => {
                let here = self.modules.get(module)?;
                if rest.is_empty() {
                    return self.resolve_name(first, module, depth);
                }
                if here.items.contains(first) {
                    format!("{module}::{first}")
                } else if let Some(used) = here.uses.get(first) {
                    let mut full = used.clone();
                    full.extend(rest.iter().cloned());
                    return self.resolve_path(&full, module, depth + 1);
                } else {
                    // another crate's: its path
                    return Some(Resolved::Foreign(segs.join("::")));
                }
            }
        };
        if rest.is_empty() {
            return None;
        }
        let mut at = start;
        for (i, s) in rest.iter().enumerate() {
            if i == rest.len() - 1 {
                return self.resolve_name(s, &at, depth);
            }
            at = format!("{at}::{s}");
        }
        None
    }

    /// A name as `module` sees it: its own item, one it `use`s, one of a
    /// glob's, else the prelude's or another crate's.
    fn resolve_name(&self, name: &str, module: &str, depth: usize) -> Option<Resolved> {
        let here = self.modules.get(module)?;
        let path = format!("{module}::{name}");
        if self.defs.contains_key(&path) {
            return Some(Resolved::Def(path));
        }
        if self.aliases.contains_key(&path) {
            return Some(Resolved::Alias(path));
        }
        if here.items.contains(name) {
            return None;
        }
        if let Some(used) = here.uses.get(name) {
            return self.resolve_path(used, module, depth + 1);
        }
        for g in &here.globs {
            let mut full = g.clone();
            full.push(name.to_string());
            if let Some(r @ (Resolved::Def(_) | Resolved::Alias(_))) = self.resolve_path(&full, module, depth + 1) {
                return Some(r);
            }
        }
        Some(Resolved::Foreign(name.to_string()))
    }

    /// A derive's path as it resolves from `module`: through its `use`s and
    /// globs, to the crate it is of (`serde::Serialize`), or its own name.
    fn derive_path(&self, p: &syn::Path, module: &str) -> String {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        match segs.as_slice() {
            [name] => self.used(name, module, 0).unwrap_or_else(|| name.clone()),
            _ => segs.join("::"),
        }
    }

    /// The path a name `use`d in `module` stands for, through `super`s and
    /// globs, if it is another crate's.
    fn used(&self, name: &str, module: &str, depth: usize) -> Option<String> {
        if depth > 16 {
            return None;
        }
        let here = self.modules.get(module)?;
        let local = |p: &[String]| -> Option<String> {
            match p.first().map(String::as_str) {
                Some("crate") => Some(p.join("::")),
                Some("self") => Some(std::iter::once(module.to_string()).chain(p[1..].iter().cloned()).collect::<Vec<_>>().join("::")),
                Some("super") => module.rsplit_once("::").map(|(up, _)| std::iter::once(up.to_string()).chain(p[1..].iter().cloned()).collect::<Vec<_>>().join("::")),
                Some(first) if here.items.contains(first) => Some(format!("{module}::{}", p.join("::"))),
                _ => None,
            }
        };
        if let Some(p) = here.uses.get(name) {
            return match local(p) {
                Some(full) => {
                    let (m, n) = full.rsplit_once("::")?;
                    self.used(n, m, depth + 1)
                }
                None => Some(p.join("::")),
            };
        }
        for g in &here.globs {
            if let Some(m) = local(g) {
                if let Some(found) = self.used(name, &m, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// The types `routes.rs`'s consts name through `ty::<T>()` and
    /// `shape::<T>`, the routes' bodies, answers, frames and parameters.
    pub fn route_types(&self) -> Vec<(Type, String)> {
        struct Turbofish(Vec<Type>);
        impl<'a> syn::visit::Visit<'a> for Turbofish {
            fn visit_path(&mut self, p: &'a syn::Path) {
                if let Some(last) = p.segments.last() {
                    if last.ident == "ty" || last.ident == "shape" {
                        if let PathArguments::AngleBracketed(args) = &last.arguments {
                            self.0.extend(args.args.iter().filter_map(|a| match a {
                                GenericArgument::Type(t) => Some(t.clone()),
                                _ => None,
                            }));
                        }
                    }
                }
                syn::visit::visit_path(self, p);
            }
        }
        let mut found = Turbofish(vec![]);
        for e in &self.route_consts {
            syn::visit::Visit::visit_expr(&mut found, e);
        }
        found.0.into_iter().map(|t| (t, "crate::routes".to_string())).collect()
    }
}

/// What a path names.
enum Resolved {
    /// One of the crate's types, by its path.
    Def(String),
    /// One of the crate's `type` aliases, by its path.
    Alias(String),
    /// Another crate's by its path, or the prelude's by its name.
    Foreign(String),
}

/// Each `impl Serialize|Deserialize|JsonSchema for` and each macro
/// definition or item-level invocation in a file, wherever it stands.
struct Anywhere<'a> {
    module: Vec<String>,
    path: &'a str,
    source: &'a mut Source,
}

impl<'a> syn::visit::Visit<'a> for Anywhere<'_> {
    fn visit_item_mod(&mut self, m: &'a syn::ItemMod) {
        let at = format!("{}::{}", self.module.last().cloned().unwrap_or_default(), m.ident);
        self.module.push(at);
        syn::visit::visit_item_mod(self, m);
        self.module.pop();
    }

    fn visit_item_impl(&mut self, i: &'a syn::ItemImpl) {
        if let Some(trait_name) = i.trait_.as_ref().and_then(|(_, t, _)| t.segments.last()).map(|s| s.ident.to_string()) {
            if matches!(trait_name.as_str(), "Serialize" | "Deserialize" | "JsonSchema") {
                let name = (trait_name == "JsonSchema")
                    .then(|| {
                        i.items.iter().find_map(|it| match it {
                            syn::ImplItem::Fn(f) if f.sig.ident == "schema_name" => first_str(&f.block),
                            _ => None,
                        })
                    })
                    .flatten();
                let module = self.module.last().cloned().unwrap_or_default();
                self.source.pending_impls.push((trait_name, module, (*i.self_ty).clone(), name));
            }
        }
        syn::visit::visit_item_impl(self, i);
    }

    fn visit_item_macro(&mut self, m: &'a syn::ItemMacro) {
        self.source.macros.push(format!("{}: {}!", self.path, m.mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()));
        syn::visit::visit_item_macro(self, m);
    }

    fn visit_impl_item_macro(&mut self, m: &'a syn::ImplItemMacro) {
        self.source.macros.push(format!("{}: {}! in an impl", self.path, m.mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()));
        syn::visit::visit_impl_item_macro(self, m);
    }
}

/// A `use` tree's names, each with the path it names.
fn uses(tree: &syn::UseTree, mut prefix: Vec<String>, m: &mut Module) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            uses(&p.tree, prefix, m);
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last().cloned() {
                    m.uses.insert(last, prefix);
                }
            } else {
                prefix.push(name.clone());
                m.uses.insert(name, prefix);
            }
        }
        syn::UseTree::Rename(r) => {
            prefix.push(r.ident.to_string());
            m.uses.insert(r.rename.to_string(), prefix);
        }
        syn::UseTree::Glob(_) => m.globs.push(prefix),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                uses(t, prefix.clone(), m);
            }
        }
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let p = entry?.path();
        if p.is_dir() {
            collect(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
    out.sort();
    Ok(())
}

fn fields_of(fields: &Fields) -> Vec<Field> {
    fields
        .iter()
        .enumerate()
        .map(|(i, f)| Field { name: f.ident.as_ref().map_or_else(|| i.to_string(), ToString::to_string), attrs: f.attrs.clone(), ty: f.ty.clone() })
        .collect()
}

/// The first string literal in a block: a `schema_name`'s.
fn first_str(block: &syn::Block) -> Option<String> {
    struct First(Option<String>);
    impl<'a> syn::visit::Visit<'a> for First {
        fn visit_lit_str(&mut self, s: &'a syn::LitStr) {
            self.0.get_or_insert_with(|| s.value());
        }
    }
    let mut f = First(None);
    syn::visit::Visit::visit_block(&mut f, block);
    f.0
}

/// The attribute namespaces the guard reads: serde's, schemars', derives,
/// and the validators schemars turns into bounds (`validate`, `garde`).
const NAMESPACES: [&str; 5] = ["serde", "schemars", "derive", "validate", "garde"];

/// An attribute's items of `NAMESPACES`, through `cfg_attr`s, however
/// nested.
fn metas(attrs: &[Attribute]) -> Result<Vec<(String, Meta)>> {
    let mut out = vec![];
    for a in attrs {
        let ns = a.path().get_ident().map(ToString::to_string).unwrap_or_default();
        if NAMESPACES.contains(&ns.as_str()) {
            for m in a.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
                out.push((ns.clone(), m));
            }
        } else if ns == "cfg_attr" {
            cfg_attr(&a.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?, &mut out)?;
        }
    }
    Ok(out)
}

/// A `cfg_attr`'s attributes (all but its predicate), each read as if it
/// stood alone.
fn cfg_attr(inner: &Punctuated<Meta, Token![,]>, out: &mut Vec<(String, Meta)>) -> Result<()> {
    for m in inner.iter().skip(1) {
        let Meta::List(l) = m else { continue };
        let ns = l.path.get_ident().map(ToString::to_string).unwrap_or_default();
        if NAMESPACES.contains(&ns.as_str()) {
            for m in l.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
                out.push((ns.clone(), m));
            }
        } else if ns == "cfg_attr" {
            cfg_attr(&l.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?, out)?;
        }
    }
    Ok(())
}

fn key(m: &Meta) -> String {
    m.path().get_ident().map(ToString::to_string).unwrap_or_else(|| "?".into())
}

/// A `key = "value"`'s value.
fn str_value(m: &Meta) -> Option<String> {
    match m {
        Meta::NameValue(nv) => match &nv.value {
            Expr::Lit(l) => match &l.lit {
                Lit::Str(s) => Some(s.value()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// A name the wire writes, as the lock reads it: letters, digits, `_` and
/// `-` alone (lock.rs's paths are dotted, its lines spaced).
pub fn wire_name_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// What may stand beside the routes' types by hand: `HAND_WRITTEN`,
/// `FLATTENED` and the record's reader, or a test's own.
pub struct Allowed<'a> {
    pub hand_written: &'a [(&'a str, &'a str)],
    pub flattened: &'a [(&'a str, &'a str)],
    pub open: &'a [(&'a str, &'a str)],
}

impl Allowed<'static> {
    pub fn wire() -> Allowed<'static> {
        static HAND: std::sync::OnceLock<Vec<(&'static str, &'static str)>> = std::sync::OnceLock::new();
        let hand = HAND.get_or_init(|| HAND_WRITTEN.iter().copied().chain([RECORD_THROUGH_VALUE]).collect());
        Allowed { hand_written: hand, flattened: &FLATTENED, open: &OPEN }
    }
}

/// Every type `roots` reach in `source` (each root with the module it is
/// named in) holds to the supported list, and every component of the
/// document is one of them: the paths of the types reached.
pub fn guard(source: &Source, roots: &[(Type, String)], components: &[String], allowed: &Allowed) -> Result<BTreeSet<String>> {
    if let Some(m) = source.macros.first() {
        bail!("{m}: a macro at item level, which could make a wire type or impl the source guard cannot read (xtask/src/wire/source.rs)");
    }
    let mut g = Guard { source, reached: BTreeSet::new(), queue: vec![], hand: BTreeSet::new(), flat: BTreeSet::new(), open: BTreeSet::new(), refusals: vec![] };
    for (r, module) in roots {
        g.ty(r, "a route", module);
    }
    while let Some(path) = g.queue.pop() {
        g.def(&source.defs[&path]);
    }
    // a hand-written impl of a type reached
    for (t, path) in &source.impls {
        if g.reached.contains(path) {
            g.hand.insert(format!("impl {t} for {}", source.defs[path].name));
        }
    }
    for found in &g.hand {
        match allowed.hand_written.iter().find(|(k, _)| k == found) {
            None => g.refusals.push(format!("{found}: written by hand, so its schema is no evidence of what serde reads; list it in HAND_WRITTEN with the test that holds it to its schema, or derive it")),
            Some((_, test)) if !source.tests.contains(*test) => g.refusals.push(format!("{found}: HAND_WRITTEN names its test `{test}`, which crates/proto does not have")),
            Some(_) => {}
        }
    }
    for (k, _) in allowed.hand_written.iter().filter(|(k, _)| !g.hand.contains(*k)) {
        g.refusals.push(format!("HAND_WRITTEN lists `{k}`, which no wire type has now: take it out"));
    }
    for (k, _) in allowed.flattened.iter().filter(|(k, _)| !g.flat.contains(*k)) {
        g.refusals.push(format!("FLATTENED lists `{k}`, which no wire type has now: take it out"));
    }
    for found in g.open.iter().filter(|o| !allowed.open.iter().any(|(k, _)| k == *o)) {
        g.refusals.push(format!("{found}: marked open, so the lock lets a new value of it pass: list it in OPEN, saying where its readers read past a value they do not know"));
    }
    for (k, _) in allowed.open.iter().filter(|(k, _)| !g.open.contains(*k)) {
        g.refusals.push(format!("OPEN lists `{k}`, which no wire type marks open now: take it out"));
    }
    // each component is a type the guard read
    let names: BTreeSet<String> = g.reached.iter().map(|p| schema_name(source, p)).collect();
    for c in components.iter().filter(|c| !names.contains(*c)) {
        g.refusals.push(format!("component {c}: no type the source guard read from crates/proto is named so, so what serde reads of it is unseen"));
    }
    if !g.refusals.is_empty() {
        g.refusals.sort();
        g.refusals.dedup();
        bail!("the wire's types use what the wire does not support (xtask/src/wire/source.rs):\n  {}", g.refusals.join("\n  "));
    }
    Ok(g.reached)
}

/// The name schemars gives a type's schema: a hand-written one's, its
/// serde `rename`, else its own.
fn schema_name(source: &Source, path: &str) -> String {
    if let Some(n) = source.schema_names.get(path) {
        return n.clone();
    }
    let d = &source.defs[path];
    metas(&d.attrs).ok().and_then(|ms| ms.iter().find(|(ns, m)| ns == "serde" && key(m) == "rename").and_then(|(_, m)| str_value(m))).unwrap_or_else(|| d.name.clone())
}

struct Guard<'s> {
    source: &'s Source,
    reached: BTreeSet<String>,
    queue: Vec<String>,
    /// The hooks and impls written by hand that the types reached have.
    hand: BTreeSet<String>,
    flat: BTreeSet<String>,
    /// The types reached that are marked open.
    open: BTreeSet<String>,
    refusals: Vec<String>,
}

impl Guard<'_> {
    /// Each type a field's type names, seen from `module`.
    fn ty(&mut self, t: &Type, at: &str, module: &str) {
        match t {
            Type::Path(p) if p.qself.is_none() => {
                let Some(last) = p.path.segments.last() else { return };
                if let PathArguments::AngleBracketed(args) = &last.arguments {
                    for a in &args.args {
                        match a {
                            GenericArgument::Type(inner) => self.ty(inner, at, module),
                            GenericArgument::Lifetime(_) => {}
                            _ => self.refusals.push(format!("{at}: a type argument the source guard cannot read")),
                        }
                    }
                }
                match self.source.resolve(t, module) {
                    Some(Resolved::Def(path)) => {
                        if self.reached.insert(path.clone()) {
                            self.queue.push(path);
                        }
                    }
                    Some(Resolved::Alias(path)) => {
                        let (m, target) = self.source.aliases[&path].clone();
                        self.ty(&target, at, &m);
                    }
                    Some(Resolved::Foreign(name)) if FOREIGN.contains(&name.as_str()) => {}
                    Some(Resolved::Foreign(name)) => {
                        self.refusals.push(format!("{at}: a type `{name}` the source guard cannot read: a wire type is one of crates/proto's, or one of {FOREIGN:?}"));
                    }
                    None => self.refusals.push(format!("{at}: a type the source guard cannot resolve from {module}")),
                }
            }
            Type::Reference(r) => self.ty(&r.elem, at, module),
            Type::Paren(p) => self.ty(&p.elem, at, module),
            Type::Group(g) => self.ty(&g.elem, at, module),
            _ => self.refusals.push(format!("{at}: a type the source guard cannot read")),
        }
    }

    fn def(&mut self, d: &Def) {
        let at = d.name.clone();
        if d.generics {
            self.refusals.push(format!("{at}: a generic wire type: the guard reads no type arguments"));
        }
        let ms = match metas(&d.attrs) {
            Ok(ms) => ms,
            Err(e) => return self.refusals.push(format!("{at}: an attribute the source guard cannot parse: {e}")),
        };
        let mut tagged = false;
        let (mut reads_through, mut writes_through) = (false, false);
        for (ns, m) in &ms {
            match ns.as_str() {
                "serde" => {
                    let k = key(m);
                    if !CONTAINER.contains(&k.as_str()) {
                        self.refuse_serde(&at, &k);
                        continue;
                    }
                    self.name_attr(&at, &k, m);
                    tagged |= k == "tag";
                    reads_through |= k == "from" || k == "try_from";
                    writes_through |= k == "into";
                    if HOOKS.contains(&k.as_str()) {
                        let v = str_value(m).unwrap_or_default();
                        self.hand.insert(format!("{at}: serde({k} = \"{v}\")"));
                        // the type it reads or writes through is the wire's too
                        match syn::parse_str::<Type>(&v) {
                            Ok(t) => self.ty(&t, &format!("{at}'s {k}"), &d.module),
                            Err(_) => self.refusals.push(format!("{at}: serde({k} = {v:?}) names no type")),
                        }
                    }
                }
                "schemars" => self.schemars(&at, m),
                "validate" | "garde" => self.validator(&at, ns),
                "derive" => {
                    let full = self.source.derive_path(m.path(), &d.module);
                    if !DERIVES.contains(&full.as_str()) {
                        self.refusals.push(format!("{at}: `#[derive({full})]`, a derive the source guard does not know, which could read or write JSON its own way: the wire's types derive serde's, schemars' and the standard library's alone"));
                    }
                }
                _ => {}
            }
        }
        // read and written through another type alone: its own fields are
        // no one's on the wire (that type's are)
        if reads_through && writes_through {
            return;
        }
        for f in &d.fields {
            self.field(&format!("{at}.{}", f.name), f, &d.module);
        }
        if let Some(variants) = &d.variants {
            if !tagged && variants.iter().any(|v| !v.unit) {
                self.refusals.push(format!("{at}: an enum with data and no `#[serde(tag = …)]` (externally tagged, or `untagged`): the wire supports unit enums and internally tagged ones"));
            }
            for v in variants {
                let vat = format!("{at}::{}", v.name);
                match metas(&v.attrs) {
                    Ok(ms) => {
                        for (ns, m) in &ms {
                            match ns.as_str() {
                                "serde" => {
                                    let k = key(m);
                                    if !VARIANT.contains(&k.as_str()) {
                                        self.refuse_serde(&vat, &k);
                                    } else {
                                        self.name_attr(&vat, &k, m);
                                        if k == "other" && !v.unit {
                                            self.refusals.push(format!("{vat}: `#[serde(other)]` on a variant with data"));
                                        }
                                    }
                                }
                                "schemars" => self.schemars(&vat, m),
                                "validate" | "garde" => self.validator(&vat, ns),
                                _ => {}
                            }
                        }
                    }
                    Err(e) => self.refusals.push(format!("{vat}: an attribute the source guard cannot parse: {e}")),
                }
                for f in &v.fields {
                    self.field(&format!("{vat}.{}", f.name), f, &d.module);
                }
            }
        }
    }

    fn field(&mut self, at: &str, f: &Field, module: &str) {
        match metas(&f.attrs) {
            Ok(ms) => {
                for (ns, m) in &ms {
                    match ns.as_str() {
                        "serde" => {
                            let k = key(m);
                            if !FIELD.contains(&k.as_str()) {
                                self.refuse_serde(at, &k);
                                continue;
                            }
                            self.name_attr(at, &k, m);
                            if HOOKS.contains(&k.as_str()) {
                                self.hand.insert(format!("{at}: serde({k} = \"{}\")", str_value(m).unwrap_or_default()));
                            }
                            if k == "flatten" {
                                let key = format!("{at}: serde(flatten)");
                                self.flat.insert(key.clone());
                                if !FLATTENED.iter().any(|(k, _)| *k == key) {
                                    self.refusals.push(format!("{key}: a flattened field, which serde reads by buffering and schemars writes beside its container's: the wire supports only those FLATTENED lists"));
                                }
                            }
                        }
                        "schemars" => self.schemars(at, m),
                        "validate" | "garde" => self.validator(at, ns),
                        _ => {}
                    }
                }
            }
            Err(e) => self.refusals.push(format!("{at}: an attribute the source guard cannot parse: {e}")),
        }
        self.ty(&f.ty, at, module);
    }

    /// A rename's or a tag's wire name: one the lock can write.
    fn name_attr(&mut self, at: &str, k: &str, m: &Meta) {
        if matches!(k, "rename" | "tag") {
            match str_value(m) {
                Some(v) if wire_name_ok(&v) => {}
                Some(v) => self.refusals.push(format!("{at}: serde({k} = {v:?}): a wire name of letters, digits, `_` and `-` alone, as the lock's paths are dotted (a `.` would name another shape's field)")),
                None => self.refusals.push(format!("{at}: serde({k}) of another form than `{k} = \"…\"`")),
            }
        }
    }

    fn refuse_serde(&mut self, at: &str, k: &str) {
        let why = match k {
            "untagged" => "serde reads the first branch that reads, so no sample of a branch shows its own branch's reading",
            "alias" => "the schema does not hold an alias, so dropping one narrows what is read with the lock unchanged",
            "skip" | "skip_serializing" | "skip_deserializing" => "a field one side never touches, which the schema says both do",
            _ => "an attribute the wire does not support, so its schema is no evidence of what serde does",
        };
        self.refusals.push(format!("{at}: `#[serde({k})]`: {why}"));
    }

    fn validator(&mut self, at: &str, ns: &str) {
        self.refusals.push(format!("{at}: `#[{ns}(…)]`, which schemars writes as bounds serde never checks, so the lock would hold a bound no reader keeps"));
    }

    fn schemars(&mut self, at: &str, m: &Meta) {
        match (key(m).as_str(), m) {
            ("example", _) => {}
            ("extend", Meta::List(l)) => match l.parse_args_with(Punctuated::<syn::ExprAssign, Token![,]>::parse_terminated) {
                Ok(pairs) => {
                    for p in pairs {
                        let k = match &*p.left {
                            Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. }) => s.value(),
                            _ => String::new(),
                        };
                        if k == "x-fragment-open" {
                            self.open.insert(at.to_string());
                        }
                        if !EXTEND.contains(&k.as_str()) {
                            self.refusals.push(format!("{at}: `#[schemars(extend({k:?} = …))]`: a schema keyword written by hand, which is no evidence of what serde reads: the wire's `extend` says a default or the open marker alone"));
                        }
                    }
                }
                Err(e) => self.refusals.push(format!("{at}: a schemars extend the source guard cannot parse: {e}")),
            },
            (k, _) => self.refusals.push(format!("{at}: `#[schemars({k})]`: a schema written another way than by its serde attributes")),
        }
    }
}
