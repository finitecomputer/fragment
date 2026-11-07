//! What serde reads and writes of the wire's types, read from crates/proto's
//! source rather than inferred from their schemas. A schema is schemars'
//! reading of serde's attributes (schemars_derive parses them with
//! `serde_derive_internals`, serde's own attribute model); a sample cannot
//! show where a reader does more than its schema says. So the wire
//! supports a short list of serde's attributes and refuses the rest, each
//! refusal naming the type, the field and the attribute. Each supported
//! entry is represented in the lock, proven by a control, or inert:
//!
//! - serde, on a container: `rename_all` (one of serde's rules; the names
//!   it makes are the lock's), `tag` (its branches), `deny_unknown_fields`
//!   (a closed type), `try_from`/`into` (`HAND_WRITTEN`); on a variant:
//!   `other` (a lock fact, `other <type> <value>`); on a field: `rename`
//!   (its name), `default` (held by defaults.rs), `skip_serializing_if`
//!   (`Option::is_none` beside `None`, or `SKIPS`), `deserialize_with`
//!   (`HAND_WRITTEN`);
//! - schemars: `extend("default" = …)` (held by defaults.rs),
//!   `extend("x-fragment-open" = true)` (`OPEN`), `example` (a sample's);
//! - derives (`DERIVES`): serde's and schemars' (the reader, the writer,
//!   the schema), `Debug` (required: what D2 compares), `Default` (a
//!   default's value, held), and the rest, which no JSON passes through;
//! - shapes: a struct of named fields; an enum of unit variants; one
//!   tagged by `tag`, its variants unit, of fields, or of one struct;
//! - field types (`FOREIGN`): the wire's own, `String`, `bool`, `u16`,
//!   `u32`, `u64`, `i64` (ranges), `Option` (`T|null`), `Vec`, `Box`,
//!   `BTreeMap` of `String` keys, `serde_json::Value` and `RawValue` (any).
//!
//! Read with `syn`, the way rustc reads the crate: from `lib.rs`, through
//! its `mod` declarations, each module's file found by its name (a
//! `#[path]` is refused; a file no `mod` reaches is not read). serde_derive
//! reads these same attributes from the same tokens, so the source is where
//! they are. A registry made at derive time would need a derive of our own
//! on every wire type, and a type without it would go unseen; here what the
//! guard cannot read fails closed instead: a field's type it cannot
//! resolve, a trait it cannot name, a component no type it read is named
//! for, a macro that could make items, a `cfg` it does not evaluate.
//!
//! Conditions: `#[cfg(feature = "schema")]` is true (this binary builds
//! proto with it) and `#[cfg(test)]` marks a module of tests alone (its
//! `#[test]`s are read, nothing else); no other `cfg` is evaluated, and
//! none may stand on a wire type, field or variant, or on a module or impl
//! the guard reads. A `cfg_attr` may say `feature = "schema"` alone, and
//! hold schemars' attributes and its derive alone: serde's attributes are
//! the same in every build (the cell's has no `schema` feature).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Fields, GenericArgument, Item, Lit, Meta, PathArguments, Token, Type};

/// The hooks and impls written by hand behind the wire, each with the
/// proto test that holds it to its schema, frame by frame or name by name.
/// A new one fails generation until it is listed, its test named; an entry
/// the source no longer has fails too.
pub const HAND_WRITTEN: [(&str, &str); 11] = [
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
    // a tagged frame's fields are buffered, and raw JSON text cannot pass
    // through that buffer: read through a `Value`, then by serde's own reader
    ("LiveOut::Record.0: serde(deserialize_with = \"record_through_value\")", "the_fragment_sends_these_frames"),
];

/// The types marked open (`extend("x-fragment-open" = true)`), whose new
/// values the lock lets pass for their readers read past one they do not
/// know, and where each reader does: a claim of the clients, not of serde,
/// so each is listed. A new one fails generation until it is.
pub const OPEN: [(&str, &str); 4] = [
    ("ErrorCode", "read by its HTTP status (the CLI's `Resp::refusal`)"),
    ("FragmentKind", "read as `Other` (`#[serde(other)]`, a lock fact)"),
    ("LiveOut", "skipped (the CLI's `channel --follow`, cell/client.mjs, the iOS app)"),
    ("ListWatchFrame", "ignored (the shell's list, the iOS app)"),
];

/// The `skip_serializing_if` predicates the wire's writers use, beyond
/// `Option::is_none` beside an `Option`'s own default (`None`), each as
/// `<predicate> on <type> = <its default>`: each omits exactly its field's
/// default, so its reader fills in the value written, as the named proto
/// test shows: for each value of a boolean; for an integer, its default
/// and values beside it and at its type's ends (`*n == 0`); for a list or
/// a map, empty and not (std's own `is_empty`).
pub const SKIPS: [(&str, &str); 5] = [
    ("std::ops::Not::not on bool = default", "skip_predicates_omit_their_default_alone"),
    ("is_on_the_dock on bool = default = \"on_the_dock\"", "skip_predicates_omit_their_default_alone"),
    ("is_zero on u32 = default", "skip_predicates_omit_their_default_alone"),
    ("Vec::is_empty on Vec = default", "skip_predicates_omit_their_default_alone"),
    ("std::collections::BTreeMap::is_empty on BTreeMap = default", "skip_predicates_omit_their_default_alone"),
];

/// The types a field may name beyond the wire's own, as their paths
/// resolve (the prelude's by name): serde's and serde_json's readings of
/// them are their own, the same everywhere.
const FOREIGN: [&str; 12] = [
    "String",
    "bool",
    "u16",
    "u32",
    "u64",
    "i64",
    "Option",
    "Vec",
    "Box",
    "std::collections::BTreeMap",
    "serde_json::Value",
    "serde_json::value::RawValue",
];

/// The derives a wire type may have, as their paths resolve: serde's,
/// schemars' and the standard library's. `Debug` must be one of them on a
/// type the default checker formats (D2 compares what it prints).
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

/// The prelude's traits, which an impl may name bare.
const PRELUDE_TRAITS: [&str; 16] =
    ["Clone", "Copy", "PartialEq", "Eq", "PartialOrd", "Ord", "Default", "From", "Into", "TryFrom", "TryInto", "AsRef", "AsMut", "Drop", "Iterator", "IntoIterator"];

/// serde's attributes the wire supports, where they stand: those its types
/// use, each audited (the PR's table).
const CONTAINER: [&str; 5] = ["rename_all", "tag", "deny_unknown_fields", "try_from", "into"];
const VARIANT: [&str; 1] = ["other"];
const FIELD: [&str; 4] = ["rename", "default", "skip_serializing_if", "deserialize_with"];
/// Of these, the hooks: each must be listed in `HAND_WRITTEN`.
const HOOKS: [&str; 3] = ["try_from", "into", "deserialize_with"];
/// The keys a schemars `extend` may add: a default (held to serde) and
/// the open marker.
const EXTEND: [&str; 2] = ["default", "x-fragment-open"];
/// serde's `rename_all` rules.
const RENAME_RULES: [&str; 8] = ["lowercase", "UPPERCASE", "PascalCase", "camelCase", "snake_case", "SCREAMING_SNAKE_CASE", "kebab-case", "SCREAMING-KEBAB-CASE"];

/// The attribute namespaces the guard reads: serde's, schemars', derives,
/// and the validators schemars turns into bounds (`validate`, `garde`).
const NAMESPACES: [&str; 5] = ["serde", "schemars", "derive", "validate", "garde"];

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
    /// A struct's fields are named (not a tuple's, nor none).
    named: bool,
    /// The module it is in (`crate::live`), where its names resolve.
    module: String,
    attrs: Vec<Attribute>,
    generics: bool,
    /// A struct's fields; an enum's are its variants'.
    fields: Vec<Field>,
    variants: Option<Vec<Variant>>,
}

/// A trait's impl for a type, as written.
struct Impl {
    module: String,
    trait_path: syn::Path,
    self_ty: Type,
    /// A hand-written `JsonSchema`'s `schema_name`, if it says one.
    schema_name: Option<String>,
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
    impls: Vec<Impl>,
    tests: BTreeSet<String>,
    /// The paths two items of a module share (one a `cfg`'s, say): which
    /// one rustc compiles the guard does not tell, so one reached is refused.
    ambiguous: BTreeSet<String>,
    /// What the reading itself refuses: a `#[path]`, a `cfg` it does not
    /// evaluate, a module no file is found for, a macro.
    refusals: Vec<String>,
    /// `routes.rs`'s consts and statics: where the routes name their types.
    route_consts: Vec<Expr>,
}

/// What a `cfg` says, as the guard evaluates it.
enum Cfg {
    /// None, or `feature = "schema"`: compiled.
    Yes,
    /// `test`: a module of tests alone.
    Test,
    /// Another: not evaluated.
    Other(String),
}

fn cfg_of(attrs: &[Attribute]) -> Cfg {
    let mut found = Cfg::Yes;
    for a in attrs.iter().filter(|a| a.path().is_ident("cfg")) {
        let said = match a.parse_args::<Meta>() {
            Ok(Meta::Path(p)) if p.is_ident("test") => Cfg::Test,
            Ok(m @ Meta::NameValue(_)) if m.path().is_ident("feature") && str_value(&m).as_deref() == Some("schema") => Cfg::Yes,
            _ => Cfg::Other(a.meta.require_list().map(|l| l.tokens.to_string()).unwrap_or_default()),
        };
        found = match (found, said) {
            (Cfg::Other(o), _) | (_, Cfg::Other(o)) => Cfg::Other(o),
            (Cfg::Test, _) | (_, Cfg::Test) => Cfg::Test,
            _ => Cfg::Yes,
        };
    }
    found
}

impl Source {
    /// The crate whose root is `dir/lib.rs`, through its modules.
    pub fn read_dir(dir: &Path) -> Result<Source> {
        let mut files = vec![];
        collect(dir, dir, &mut files)?;
        Source::read(&files)
    }

    /// The crate of these files, each `(path, text)`, from `lib.rs`
    /// through its `mod` declarations.
    pub fn read(files: &[(String, String)]) -> Result<Source> {
        let files: BTreeMap<&str, &str> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        let mut s = Source::default();
        s.file(&files, "lib.rs", "crate", "", false)?;
        Ok(s)
    }

    fn file(&mut self, files: &BTreeMap<&str, &str>, path: &str, module: &str, child_dir: &str, tests_only: bool) -> Result<()> {
        let text = files.get(path).with_context(|| format!("crates/proto/src/{path}: no such file"))?;
        let file = syn::parse_file(text).with_context(|| format!("crates/proto/src/{path} parses"))?;
        self.items(files, &file.items, path, module, child_dir, tests_only)
    }

    fn items(&mut self, files: &BTreeMap<&str, &str>, items: &[Item], path: &str, module: &str, child_dir: &str, tests_only: bool) -> Result<()> {
        let mut names = BTreeSet::new();
        let mut m = Module::default();
        for item in items {
            let name = match item {
                Item::Struct(i) => Some(&i.ident),
                Item::Enum(i) => Some(&i.ident),
                Item::Union(i) => Some(&i.ident),
                Item::Type(i) => Some(&i.ident),
                Item::Mod(i) => Some(&i.ident),
                Item::Trait(i) => Some(&i.ident),
                Item::Use(u) => {
                    uses(&u.tree, vec![], &mut m);
                    None
                }
                _ => None,
            };
            if let Some(n) = name {
                if !names.insert(n.to_string()) && !tests_only {
                    self.ambiguous.insert(format!("{module}::{n}"));
                }
            }
        }
        m.items = names;
        self.modules.insert(module.to_string(), m);
        let routes = module == "crate::routes";
        for item in items {
            if tests_only {
                match item {
                    Item::Fn(f) if f.attrs.iter().any(|a| a.path().is_ident("test")) => {
                        self.tests.insert(f.sig.ident.to_string());
                    }
                    Item::Mod(i) => {
                        if let Some((_, inner)) = &i.content {
                            self.items(files, inner, path, &format!("{module}::{}", i.ident), &format!("{child_dir}{}/", i.ident), true)?;
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match item {
                Item::Struct(i) => {
                    let def = Def {
                        name: i.ident.to_string(),
                        named: matches!(i.fields, Fields::Named(_)),
                        module: module.to_string(),
                        attrs: i.attrs.clone(),
                        generics: !i.generics.params.is_empty(),
                        fields: fields_of(&i.fields),
                        variants: None,
                    };
                    self.defs.insert(format!("{module}::{}", def.name), def);
                }
                Item::Enum(i) => {
                    let variants = i
                        .variants
                        .iter()
                        .map(|v| Variant { name: v.ident.to_string(), attrs: v.attrs.clone(), fields: fields_of(&v.fields), unit: matches!(v.fields, Fields::Unit) })
                        .collect();
                    let def = Def { name: i.ident.to_string(), named: true, module: module.to_string(), attrs: i.attrs.clone(), generics: !i.generics.params.is_empty(), fields: vec![], variants: Some(variants) };
                    self.defs.insert(format!("{module}::{}", def.name), def);
                }
                Item::Type(i) => {
                    if !matches!(cfg_of(&i.attrs), Cfg::Yes) {
                        self.refusals.push(format!("{module}::{}: a `cfg` on a type alias", i.ident));
                    }
                    self.aliases.insert(format!("{module}::{}", i.ident), (module.to_string(), (*i.ty).clone()));
                }
                Item::Impl(i) => match cfg_of(&i.attrs) {
                    Cfg::Test => {}
                    Cfg::Other(c) => self.refusals.push(format!("{path}: an impl under `#[cfg({c})]`, which the guard does not evaluate")),
                    Cfg::Yes => self.impl_item(i, module, path),
                },
                Item::Mod(i) => {
                    let at = format!("{module}::{}", i.ident);
                    if i.attrs.iter().any(|a| a.path().is_ident("path")) {
                        self.refusals.push(format!("{at}: a `#[path]` module: the guard finds a module's file by its name"));
                        continue;
                    }
                    let tests = match cfg_of(&i.attrs) {
                        Cfg::Yes => false,
                        Cfg::Test => true,
                        Cfg::Other(c) => {
                            self.refusals.push(format!("{at}: a module under `#[cfg({c})]`, which the guard does not evaluate"));
                            continue;
                        }
                    };
                    let dir = format!("{child_dir}{}/", i.ident);
                    match &i.content {
                        Some((_, inner)) => self.items(files, inner, path, &at, &dir, tests)?,
                        None => {
                            let (flat, nested) = (format!("{child_dir}{}.rs", i.ident), format!("{child_dir}{}/mod.rs", i.ident));
                            let found = [flat, nested].into_iter().find(|f| files.contains_key(f.as_str()));
                            match found {
                                Some(f) => self.file(files, &f, &at, &dir, tests)?,
                                None => self.refusals.push(format!("{at}: no file for the module (`{child_dir}{}.rs` or its `mod.rs`)", i.ident)),
                            }
                        }
                    }
                }
                Item::Fn(f) => {
                    if f.attrs.iter().any(|a| a.path().is_ident("test")) {
                        self.tests.insert(f.sig.ident.to_string());
                    } else {
                        self.body(&f.block, module, path);
                    }
                }
                Item::Macro(m) => self.refusals.push(format!("{path}: {}!: a macro at item level, which could make a wire type or impl the source guard cannot read", mac_name(&m.mac))),
                Item::Const(c) if routes => self.route_consts.push((*c.expr).clone()),
                Item::Static(c) if routes => self.route_consts.push((*c.expr).clone()),
                _ => {}
            }
        }
        Ok(())
    }

    fn impl_item(&mut self, i: &syn::ItemImpl, module: &str, path: &str) {
        for it in &i.items {
            match it {
                syn::ImplItem::Macro(m) => self.refusals.push(format!("{path}: {}! in an impl: a macro the source guard cannot read through", mac_name(&m.mac))),
                syn::ImplItem::Fn(f) => self.body(&f.block, module, path),
                _ => {}
            }
        }
        let Some((_, trait_path, _)) = &i.trait_ else { return };
        let schema_name = i.items.iter().find_map(|it| match it {
            syn::ImplItem::Fn(f) if f.sig.ident == "schema_name" => first_str(&f.block),
            _ => None,
        });
        self.impls.push(Impl { module: module.to_string(), trait_path: trait_path.clone(), self_ty: (*i.self_ty).clone(), schema_name });
    }

    /// The impls and macro definitions in a function's body.
    fn body(&mut self, block: &syn::Block, module: &str, path: &str) {
        struct Inner<'a> {
            found: Vec<syn::ItemImpl>,
            macros: Vec<String>,
            path: &'a str,
        }
        impl<'a> syn::visit::Visit<'a> for Inner<'_> {
            fn visit_item_impl(&mut self, i: &'a syn::ItemImpl) {
                self.found.push(i.clone());
            }
            fn visit_item_macro(&mut self, m: &'a syn::ItemMacro) {
                self.macros.push(format!("{}: {}!: a macro at item level, in a function's body", self.path, mac_name(&m.mac)));
            }
        }
        let mut v = Inner { found: vec![], macros: vec![], path };
        syn::visit::Visit::visit_block(&mut v, block);
        self.refusals.extend(v.macros);
        for i in v.found {
            self.impl_item(&i, module, path);
        }
    }

    /// What a type's path names, seen from `module`: one of the crate's
    /// types (its path), or one of another crate's (its path, or the
    /// prelude's name).
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
            return Some(Resolved::Local(path));
        }
        if let Some(used) = here.uses.get(name) {
            return self.resolve_path(used, module, depth + 1);
        }
        // a glob of one of the crate's modules that holds or uses the name
        for g in &here.globs {
            if let Some(m) = self.local_module(g, module) {
                if self.modules.get(&m).is_some_and(|t| t.items.contains(name) || t.uses.contains_key(name)) {
                    return self.resolve_name(name, &m, depth + 1);
                }
            }
        }
        Some(Resolved::Foreign(name.to_string()))
    }

    /// The crate's module a `use` path names from `module`, if it is one.
    fn local_module(&self, p: &[String], module: &str) -> Option<String> {
        let (first, rest) = p.split_first()?;
        let mut at = match first.as_str() {
            "crate" => "crate".to_string(),
            "self" => module.to_string(),
            "super" => module.rsplit_once("::")?.0.to_string(),
            f if self.modules.get(module).is_some_and(|m| m.items.contains(f)) => format!("{module}::{f}"),
            _ => return None,
        };
        for s in rest {
            at = if s == "super" { at.rsplit_once("::")?.0.to_string() } else { format!("{at}::{s}") };
        }
        self.modules.contains_key(&at).then_some(at)
    }

    /// A derive's or a trait's path as it resolves from `module`: another
    /// crate's (`serde::Serialize`), the crate's own (`crate::…`), or its
    /// bare name where nothing names it otherwise (the prelude's).
    fn path_of(&self, p: &syn::Path, module: &str) -> String {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        match self.resolve_path(&segs, module, 0) {
            Some(Resolved::Foreign(f)) => f,
            Some(Resolved::Def(d) | Resolved::Alias(d) | Resolved::Local(d)) => d,
            None => segs.join("::"),
        }
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
    /// Another of the crate's items (a module, a trait), by its path.
    Local(String),
    /// Another crate's by its path, or the prelude's by its name.
    Foreign(String),
}

fn mac_name(m: &syn::Macro) -> String {
    m.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
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

/// Each `.rs` file under `dir`, by its path from `root`.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let p = entry?.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if p.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
            out.push((p.strip_prefix(root).unwrap_or(&p).display().to_string(), text));
        }
    }
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

/// An attribute's items of `NAMESPACES`, each with whether it stood in a
/// `cfg_attr(feature = "schema", …)`; any other `cfg_attr`, or one nested
/// in another, is refused.
fn metas(attrs: &[Attribute]) -> Result<Vec<(String, Meta, bool)>> {
    let mut out = vec![];
    for a in attrs {
        let ns = a.path().get_ident().map(ToString::to_string).unwrap_or_default();
        if NAMESPACES.contains(&ns.as_str()) {
            for m in a.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
                out.push((ns.clone(), m, false));
            }
        } else if ns == "cfg_attr" {
            let inner = a.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            let schema = inner.first().is_some_and(|p| p.path().is_ident("feature") && str_value(p).as_deref() == Some("schema"));
            if !schema {
                bail!("`#[cfg_attr({}, …)]`: the guard evaluates `feature = \"schema\"` alone", inner.first().map(|p| p.path().get_ident().map(ToString::to_string).unwrap_or_default()).unwrap_or_default());
            }
            for m in inner.iter().skip(1) {
                let Meta::List(l) = m else { continue };
                let ns = l.path.get_ident().map(ToString::to_string).unwrap_or_default();
                if ns == "cfg_attr" {
                    bail!("a `cfg_attr` nested in another, which the guard does not read");
                }
                if NAMESPACES.contains(&ns.as_str()) {
                    for m in l.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)? {
                        out.push((ns.clone(), m, true));
                    }
                }
            }
        }
    }
    Ok(out)
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

/// A variant's wire name under its enum's `rename_all`, as serde's
/// `RenameRule::apply_to_variant` makes it.
fn renamed(name: &str, rule: Option<&str>) -> Option<String> {
    let snake = |upper: bool, sep: char| {
        let mut out = String::new();
        for (i, c) in name.chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                out.push(sep);
            }
            out.extend(if upper { c.to_uppercase().collect::<Vec<_>>() } else { c.to_lowercase().collect() });
        }
        out
    };
    Some(match rule {
        None | Some("PascalCase") => name.to_string(),
        Some("lowercase") => name.to_ascii_lowercase(),
        Some("UPPERCASE") => name.to_ascii_uppercase(),
        Some("camelCase") => {
            let mut c = name.chars();
            c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
        Some("snake_case") => snake(false, '_'),
        Some("SCREAMING_SNAKE_CASE") => snake(true, '_'),
        Some("kebab-case") => snake(false, '-'),
        Some("SCREAMING-KEBAB-CASE") => snake(true, '-'),
        _ => return None,
    })
}

/// What may stand beside the routes' types by hand: `HAND_WRITTEN`, `OPEN`
/// and `SKIPS`, or a test's own.
pub struct Allowed<'a> {
    pub hand_written: &'a [(&'a str, &'a str)],
    pub open: &'a [(&'a str, &'a str)],
    pub skips: &'a [(&'a str, &'a str)],
}

impl Allowed<'static> {
    pub fn wire() -> Allowed<'static> {
        Allowed { hand_written: &HAND_WRITTEN, open: &OPEN, skips: &SKIPS }
    }
}

#[derive(Debug)]
/// What the guard found: the types reached, and the facts serde's source
/// says that their schemas do not, for the lock (`other <type> <value>`: a
/// reader reads a value it does not know as this one).
pub struct Guarded {
    /// The types reached, by path (the tests read which).
    #[cfg_attr(not(test), allow(dead_code))]
    pub reached: BTreeSet<String>,
    pub facts: Vec<String>,
}

/// Every type `roots` reach in `source` (each root with the module it is
/// named in) holds to the supported list, and every component of the
/// document is one of them.
pub fn guard(source: &Source, roots: &[(Type, String)], components: &[String], allowed: &Allowed) -> Result<Guarded> {
    let mut g = Guard {
        source,
        reached: BTreeSet::new(),
        formatted: BTreeSet::new(),
        queue: vec![],
        hand: BTreeSet::new(),
        open: BTreeSet::new(),
        skips: BTreeSet::new(),
        facts: vec![],
        refusals: source.refusals.clone(),
        allowed,
    };
    for (r, module) in roots {
        g.ty(r, "a route", module, true);
    }
    while let Some(path) = g.queue.pop() {
        g.def(&source.defs[&path], &path);
    }
    for path in g.reached.intersection(&source.ambiguous) {
        g.refusals.push(format!("{}: two items of one name in its module, so which one rustc compiles (a `cfg`'s choice) the guard cannot tell", source.defs[path].name));
    }
    // D2 reads what `Debug` prints: derived, it prints every field
    for path in &g.formatted {
        let d = &source.defs[path];
        let debug = metas(&d.attrs).unwrap_or_default().iter().any(|(ns, m, _)| ns == "derive" && source.path_of(m.path(), &d.module) == "Debug");
        if !debug {
            g.refusals.push(format!("{}: no `#[derive(Debug)]`: the default checker compares what a value's `Debug` prints, which only the derived one prints whole", d.name));
        }
    }
    // each impl of a type reached: serde's, schemars' and `Debug` by hand
    // are listed; a trait the guard cannot name is refused
    for i in &source.impls {
        let Some(Resolved::Def(path)) = source.resolve(&i.self_ty, &i.module) else { continue };
        if !g.reached.contains(&path) {
            continue;
        }
        let name = &source.defs[&path].name;
        let t = source.path_of(&i.trait_path, &i.module);
        let kind = match t.as_str() {
            "serde::Serialize" | "serde::ser::Serialize" => Some("Serialize"),
            "serde::Deserialize" | "serde::de::Deserialize" => Some("Deserialize"),
            "schemars::JsonSchema" => Some("JsonSchema"),
            "std::fmt::Debug" | "core::fmt::Debug" => Some("Debug"),
            other if other.starts_with("serde::") || other.starts_with("schemars::") => Some("serde's"),
            other if !other.contains("::") && !PRELUDE_TRAITS.contains(&other) => {
                g.refusals.push(format!("impl {other} for {name}: a trait the source guard cannot name, so it cannot tell whether it reads or writes JSON"));
                None
            }
            _ => None,
        };
        if let Some(kind) = kind {
            g.hand.insert(format!("impl {kind} for {name}"));
        }
    }
    let listed = |found: &BTreeSet<String>, list: &[(&str, &str)], what: &str, g: &mut Vec<String>| {
        for (k, _) in list.iter().filter(|(k, _)| !found.contains(*k)) {
            g.push(format!("{what} lists `{k}`, which no wire type has now: take it out"));
        }
    };
    for found in &g.hand {
        match allowed.hand_written.iter().find(|(k, _)| k == found) {
            None => g.refusals.push(format!("{found}: written by hand, so its schema is no evidence of what serde reads; list it in HAND_WRITTEN with the test that holds it to its schema, or derive it")),
            Some((_, test)) if !source.tests.contains(*test) => g.refusals.push(format!("{found}: HAND_WRITTEN names its test `{test}`, which crates/proto does not have")),
            Some(_) => {}
        }
    }
    for (k, test) in allowed.skips.iter().filter(|(k, _)| g.skips.contains(*k)) {
        if !source.tests.contains(*test) {
            g.refusals.push(format!("{k}: SKIPS names its test `{test}`, which crates/proto does not have"));
        }
    }
    let mut stale = vec![];
    listed(&g.hand, allowed.hand_written, "HAND_WRITTEN", &mut stale);
    listed(&g.open, allowed.open, "OPEN", &mut stale);
    listed(&g.skips, allowed.skips, "SKIPS", &mut stale);
    g.refusals.extend(stale);
    for found in g.open.iter().filter(|o| !allowed.open.iter().any(|(k, _)| k == *o)) {
        g.refusals.push(format!("{found}: marked open, so the lock lets a new value of it pass: list it in OPEN, saying where its readers read past a value they do not know"));
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
    g.facts.sort();
    Ok(Guarded { reached: g.reached, facts: g.facts })
}

/// The name schemars gives a type's schema: a hand-written one's, else its
/// own (a container's `rename` is refused).
fn schema_name(source: &Source, path: &str) -> String {
    let hand = source.impls.iter().find_map(|i| match (source.resolve(&i.self_ty, &i.module), &i.schema_name) {
        (Some(Resolved::Def(p)), Some(n)) if p == path => Some(n.clone()),
        _ => None,
    });
    if let Some(n) = hand {
        return n;
    }
    source.defs[path].name.clone()
}

struct Guard<'s> {
    source: &'s Source,
    reached: BTreeSet<String>,
    /// The types reached whose values the default checker formats (all but
    /// those reached only as another's `try_from`/`from`/`into`).
    formatted: BTreeSet<String>,
    queue: Vec<String>,
    /// The hooks and impls written by hand that the types reached have.
    hand: BTreeSet<String>,
    /// The types reached that are marked open.
    open: BTreeSet<String>,
    /// The `SKIPS` entries the fields reached use.
    skips: BTreeSet<String>,
    facts: Vec<String>,
    refusals: Vec<String>,
    allowed: &'s Allowed<'s>,
}

impl Guard<'_> {
    /// Each type a field's type names, seen from `module`; `formatted`
    /// unless named as another's proxy.
    fn ty(&mut self, t: &Type, at: &str, module: &str, formatted: bool) {
        match t {
            Type::Path(p) if p.qself.is_none() => {
                let Some(last) = p.path.segments.last() else { return };
                // `Some(None)` is written as `null` and read back as `None`
                if last.ident == "Option" {
                    if let PathArguments::AngleBracketed(args) = &last.arguments {
                        if let Some(GenericArgument::Type(Type::Path(inner))) = args.args.first() {
                            if inner.path.segments.last().is_some_and(|s| s.ident == "Option") {
                                self.refusals.push(format!("{at}: an `Option` of an `Option`, whose `Some(None)` is written as `null` and read back as `None`"));
                            }
                        }
                    }
                }
                if let PathArguments::AngleBracketed(args) = &last.arguments {
                    for a in &args.args {
                        match a {
                            GenericArgument::Type(inner) => self.ty(inner, at, module, formatted),
                            _ => self.refusals.push(format!("{at}: a type argument the source guard cannot read")),
                        }
                    }
                }
                match self.source.resolve(t, module) {
                    Some(Resolved::Def(path)) => {
                        if formatted {
                            self.formatted.insert(path.clone());
                        }
                        if self.reached.insert(path.clone()) {
                            self.queue.push(path);
                        }
                    }
                    Some(Resolved::Alias(path)) => {
                        let (m, target) = self.source.aliases[&path].clone();
                        self.ty(&target, at, &m, formatted);
                    }
                    Some(Resolved::Foreign(name)) if FOREIGN.contains(&name.as_str()) => {
                        // a map's keys are strings, as its schema says
                        if name.ends_with("BTreeMap") {
                            let key = match &last.arguments {
                                PathArguments::AngleBracketed(args) => args.args.first().and_then(|a| match a {
                                    GenericArgument::Type(k) => self.source.resolve(k, module),
                                    _ => None,
                                }),
                                _ => None,
                            };
                            if !matches!(key, Some(Resolved::Foreign(k)) if k == "String") {
                                self.refusals.push(format!("{at}: a map whose keys are not `String`, which its schema does not say"));
                            }
                        }
                    }
                    Some(Resolved::Foreign(name)) => {
                        self.refusals.push(format!("{at}: a type `{name}` the source guard cannot read: a wire type is one of crates/proto's, or one of {FOREIGN:?}"));
                    }
                    Some(Resolved::Local(path)) => self.refusals.push(format!("{at}: `{path}`, no type the source guard reads")),
                    None => self.refusals.push(format!("{at}: a type the source guard cannot resolve from {module}")),
                }
            }
            Type::Paren(p) => self.ty(&p.elem, at, module, formatted),
            Type::Group(g) => self.ty(&g.elem, at, module, formatted),
            _ => self.refusals.push(format!("{at}: a type the source guard cannot read (a reference, a tuple, an array, …)")),
        }
    }

    fn metas_of(&mut self, attrs: &[Attribute], at: &str) -> Vec<(String, Meta, bool)> {
        match metas(attrs) {
            Ok(ms) => {
                for (ns, m, conditional) in &ms {
                    let schema_only = ns == "schemars" || (ns == "derive" && m.path().segments.last().is_some_and(|s| s.ident == "JsonSchema"));
                    if *conditional && !schema_only {
                        self.refusals.push(format!("{at}: `{ns}` under `cfg_attr(feature = \"schema\", …)`: serde's reading is the same in every build, the cell's (no `schema`) too"));
                    }
                }
                ms
            }
            Err(e) => {
                self.refusals.push(format!("{at}: {e}"));
                vec![]
            }
        }
    }

    fn def(&mut self, d: &Def, path: &str) {
        let at = d.name.clone();
        if d.generics {
            self.refusals.push(format!("{at}: a generic wire type: the guard reads no type arguments"));
        }
        if !matches!(cfg_of(&d.attrs), Cfg::Yes) {
            self.refusals.push(format!("{at}: a wire type under a `cfg`: which definition rustc compiles is the build's"));
        }
        let ms = self.metas_of(&d.attrs, &at);
        let mut tagged = false;
        let mut rename_all = None;
        let (mut reads_through, mut writes_through) = (false, false);
        for (ns, m, _) in &ms {
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
                    if k == "rename_all" {
                        rename_all = str_value(m);
                        if !rename_all.as_deref().is_some_and(|r| RENAME_RULES.contains(&r)) {
                            self.refusals.push(format!("{at}: a `rename_all` of another form than `rename_all = \"<one of serde's rules>\"` (a split one writes one name and reads another, where the lock holds one)"));
                        }
                    }
                    if HOOKS.contains(&k.as_str()) {
                        let v = str_value(m).unwrap_or_default();
                        self.hand.insert(format!("{at}: serde({k} = \"{v}\")"));
                        // the type it reads or writes through is the wire's too
                        match syn::parse_str::<Type>(&v) {
                            Ok(t) => self.ty(&t, &format!("{at}'s {k}"), &d.module, false),
                            Err(_) => self.refusals.push(format!("{at}: serde({k} = {v:?}) names no type")),
                        }
                    }
                }
                "schemars" => self.schemars(&at, m),
                "validate" | "garde" => self.validator(&at, ns),
                "derive" => {
                    let full = self.source.path_of(m.path(), &d.module);
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
        if d.variants.is_none() && !d.named {
            self.refusals.push(format!("{at}: a tuple or unit struct, which the wire's shapes do not hold: the wire's structs name their fields"));
        }
        for f in &d.fields {
            self.field(&format!("{at}.{}", f.name), f, &d.module);
        }
        let Some(variants) = &d.variants else { return };
        if !tagged && variants.iter().any(|v| !v.unit) {
            self.refusals.push(format!("{at}: an enum with data and no `#[serde(tag = …)]` (externally tagged, or `untagged`): the wire supports unit enums and internally tagged ones"));
        }
        for v in variants {
            let vat = format!("{at}::{}", v.name);
            // a tagged newtype's content is written as fields beside the tag
            if tagged && !v.unit {
                if let [only] = v.fields.as_slice() {
                    if only.name == "0" {
                        let content = match self.source.resolve(&only.ty, &d.module) {
                            Some(Resolved::Def(p)) => self.source.defs.get(&p).map(|c| c.variants.is_none() && c.named),
                            _ => None,
                        };
                        if content != Some(true) {
                            self.refusals.push(format!("{vat}: a tagged variant holding no struct: serde writes a tagged newtype's content as fields beside its tag, so it holds one"));
                        }
                    }
                }
                if v.fields.len() > 1 && v.fields.iter().all(|f| f.name.parse::<usize>().is_ok()) {
                    self.refusals.push(format!("{vat}: a tagged tuple variant"));
                }
            }
            if !matches!(cfg_of(&v.attrs), Cfg::Yes) {
                self.refusals.push(format!("{vat}: a variant under a `cfg`"));
            }
            let wire = renamed(&v.name, rename_all.as_deref());
            let mut other = false;
            for (ns, m, _) in &self.metas_of(&v.attrs, &vat) {
                match ns.as_str() {
                    "serde" => {
                        let k = key(m);
                        if !VARIANT.contains(&k.as_str()) {
                            self.refuse_serde(&vat, &k);
                            continue;
                        }
                        other |= k == "other";
                    }
                    "schemars" => self.schemars(&vat, m),
                    "validate" | "garde" => self.validator(&vat, ns),
                    _ => {}
                }
            }
            if other {
                // a reader reads a value it does not know as this one: a fact
                // the schema does not hold, so the lock does
                match (v.unit, wire) {
                    (true, Some(w)) => self.facts.push(format!("other {} {w}", schema_name(self.source, path))),
                    (false, _) => self.refusals.push(format!("{vat}: `#[serde(other)]` on a variant with data")),
                    (true, None) => self.refusals.push(format!("{vat}: `#[serde(other)]` under a `rename_all` the guard does not know")),
                }
            }
            for f in &v.fields {
                self.field(&format!("{vat}.{}", f.name), f, &d.module);
            }
        }
    }

    fn field(&mut self, at: &str, f: &Field, module: &str) {
        if !matches!(cfg_of(&f.attrs), Cfg::Yes) {
            self.refusals.push(format!("{at}: a field under a `cfg`"));
        }
        let mut default: Option<String> = None;
        let mut skip: Option<String> = None;
        for (ns, m, _) in &self.metas_of(&f.attrs, at) {
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
                    match k.as_str() {
                        "default" => default = Some(str_value(m).map_or_else(|| "default".to_string(), |f| format!("default = \"{f}\""))),
                        "skip_serializing_if" => skip = str_value(m),
                        _ => {}
                    }
                }
                "schemars" => self.schemars(at, m),
                "validate" | "garde" => self.validator(at, ns),
                _ => {}
            }
        }
        if let Some(pred) = skip {
            self.skip(at, &pred, &f.ty, default.as_deref(), module);
        }
        self.ty(&f.ty, at, module, true);
    }

    /// A `skip_serializing_if` omits exactly its field's default, so what
    /// its reader fills in is what was written: `Option::is_none` beside an
    /// `Option`'s own default, or a `SKIPS` entry, held by its test.
    fn skip(&mut self, at: &str, pred: &str, ty: &Type, default: Option<&str>, module: &str) {
        let head = match self.source.resolve(ty, module) {
            Some(Resolved::Foreign(f)) => f.rsplit("::").next().unwrap_or(&f).to_string(),
            _ => "?".to_string(),
        };
        if pred == "Option::is_none" && head == "Option" && matches!(default, None | Some("default")) {
            return;
        }
        let key = format!("{pred} on {head} = {}", default.unwrap_or("none"));
        if self.allowed.skips.iter().any(|(k, _)| *k == key) {
            self.skips.insert(key);
        } else {
            self.refusals.push(format!("{at}: `skip_serializing_if = \"{pred}\"` ({key}): a writer's predicate not known to omit exactly the field's default, so a value it omits may be read back as another; list it in SKIPS with a test that it holds for the default alone"));
        }
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
            "flatten" => "serde reads a flattened field by buffering its container, and schemars writes it beside the container's own: the wire supports none",
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
