//! What serde reads and writes of the wire's types, read from crates/proto's
//! source rather than inferred from their schemas. A schema is schemars'
//! reading of serde's attributes (schemars_derive parses them with
//! `serde_derive_internals`, serde's own attribute model); a sample cannot
//! show where a reader does more than its schema says.
//!
//! The principle: the guard accepts only forms it resolves without
//! evaluating Rust, and refuses everything else. It does not reimplement
//! rustc's name resolution or `cfg` evaluation; it holds proto's source to
//! forms where both are trivial, and proto uses only those:
//!
//! - modules: from `lib.rs`, plain `mod x;` lines, each file found by its
//!   name; a `mod` carries no attribute but a doc comment, `#[cfg(test)]`
//!   (a tests' module: its `#[test]` names alone are read), or
//!   `#[cfg(feature = "schema")]` on a module `SCHEMA_MODULES` lists; no
//!   `mod` inside a function, block or constant; no inner `#![…]`; a file
//!   no `mod` reaches is not read;
//! - names: no glob import, no `use` with an attribute or in a function's
//!   body, no `extern crate`, no name bound twice in one scope (items and
//!   imports), no renamed or patched serde, serde_json or schemars in the
//!   manifests; so a path resolves through a module's own items and plain
//!   imports alone;
//! - wire types: at the top level of a module `WIRE_MODULES` lists, with
//!   no attribute but serde's, schemars', their derives, a doc comment and
//!   `#[default]`, and no `cfg` but `cfg_attr(feature = "schema", …)`
//!   holding schemars' alone (serde reads the same in every build);
//! - routes: `routes.rs`'s constants are plain literals of paths, structs,
//!   tuples and calls, each type a route reaches `ty::<T>()`/`shape::<T>`
//!   of a plain path; no item in any constant;
//! - macros: the standard library's (by bare name, never imported) and
//!   `json!`/`json_schema!` (imported from their crates), a macro inside
//!   another's tokens held to the same, none defined anywhere;
//! - functions an attribute names: `Option::is_none`, `Vec::is_empty`,
//!   `BTreeMap::is_empty`, and `PINNED`'s, at the crate's root, each its
//!   text exactly, no function of their names elsewhere.
//!
//! Within those forms, each supported entry is represented in the lock,
//! proven by a control, registered with a test, or inert (the PR's table):
//!
//! - serde, on a container: `rename_all` (one of serde's rules; the names
//!   it makes are the lock's), `tag` (its branches), `deny_unknown_fields`
//!   (a closed type), `try_from`/`into` (`HAND_WRITTEN`); on a variant:
//!   `other` (a lock fact, `other <type> <value>`); on a field: `rename`
//!   (its name), `default` (plain, or `default_true`; held by defaults.rs),
//!   `skip_serializing_if` (pinned: what it omits is the default its reader
//!   fills in), `deserialize_with` (`HAND_WRITTEN`);
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
//! Read with `syn`: serde_derive reads these same attributes from the same
//! tokens. A registry made at derive time would need a derive of our own
//! on every wire type, and a type without it would go unseen; here what the
//! guard cannot read fails closed instead: a type or function it cannot
//! resolve, a trait it cannot name, a component no type it read is named
//! for.

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

/// The modules the wire's types live in, at their top level alone, each
/// reached from `lib.rs` by a plain `mod` line.
pub const WIRE_MODULES: [&str; 3] = ["crate", "crate::live", "crate::routes"];

/// The modules declared under `#[cfg(feature = "schema")]` (the one
/// spelling allowed on a `mod`, beside `#[cfg(test)]`): the routes' table
/// and the frames' schemas written by hand, which the cell does not build.
pub const SCHEMA_MODULES: [&str; 2] = ["crate::routes", "crate::live::schema"];

/// The functions a `skip_serializing_if` or `default` may name beyond the
/// standard library's (`Option::is_none`, `Vec::is_empty`,
/// `BTreeMap::is_empty`), at the crate's root, each pinned to its text:
/// what it omits or fills in is what its body says, so an edit fails, as
/// does a function of one of these names anywhere else.
pub const PINNED: [(&str, &str); 3] = [
    ("is_default", "fn is_default<T: Default + PartialEq>(v: &T) -> bool { *v == T::default() }"),
    ("is_true", "fn is_true(b: &bool) -> bool { *b }"),
    ("default_true", "fn default_true() -> bool { true }"),
];

/// The macros the crate's code may invoke, anywhere but its tests: the
/// standard library's, each by its bare name and never imported, and
/// serde_json's `json` and schemars' `json_schema`, each imported from its
/// crate. None makes an item; a macro inside another's tokens is held to
/// the same list.
const MACROS: [&str; 27] = [
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "format",
    "format_args",
    "write",
    "writeln",
    "print",
    "println",
    "eprint",
    "eprintln",
    "vec",
    "matches",
    "concat",
    "stringify",
    "include_str",
    "line",
    "file",
    "json",
    "json_schema",
];
const IMPORTED_MACROS: [(&str, &str); 2] = [("json", "serde_json::json"), ("json_schema", "schemars::json_schema")];

/// The attributes a wire type, field or variant may carry: serde's,
/// schemars', its derives, a doc comment, and `#[default]` (a variant
/// `derive(Default)` picks); an attribute macro could rewrite the item.
const ATTRIBUTES: [&str; 7] = ["doc", "serde", "schemars", "derive", "cfg_attr", "cfg", "default"];

/// The field types `is_default` may stand on: the standard library's own
/// `Default` and `PartialEq` decide what it omits.
const DEFAULTED: [&str; 6] = ["bool", "u16", "u32", "u64", "i64", "String"];

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
    attrs: Vec<Attribute>,
    trait_path: syn::Path,
    self_ty: Type,
    /// A hand-written `JsonSchema`'s `schema_name`, if it says one.
    schema_name: Option<String>,
}

/// A module's names: its own items, and those it `use`s.
#[derive(Default)]
struct Module {
    items: BTreeSet<String>,
    /// Its functions (not its tests'), by name.
    fns: BTreeSet<String>,
    /// Its constants and statics, by name.
    consts: BTreeSet<String>,
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
    /// Each module's functions, by path (`crate::is_default`).
    fns: BTreeMap<String, syn::ItemFn>,
    /// What the reading itself refuses: a `#[path]`, a `cfg` it does not
    /// evaluate, a module no file is found for, a macro.
    refusals: Vec<String>,
    /// `routes.rs`'s consts and statics, each by its path: where the routes
    /// name their types.
    route_consts: Vec<(String, Expr)>,
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
    /// The crate whose root is `dir/lib.rs`, through its modules, and that
    /// its manifests name serde, serde_json and schemars as themselves: no
    /// dependency of proto's renamed (`package = …`), and no `[patch]` or
    /// `[replace]` in the workspace's, either of which makes `serde::`
    /// another crate.
    pub fn read_dir(dir: &Path) -> Result<Source> {
        let mut files = vec![];
        collect(dir, dir, &mut files)?;
        let mut s = Source::read(&files)?;
        for (manifest, says) in [("../Cargo.toml", "package"), ("../../../Cargo.toml", "[patch"), ("../../../Cargo.toml", "[replace")] {
            let text = std::fs::read_to_string(dir.join(manifest)).with_context(|| format!("read {manifest}"))?;
            let found = text.lines().map(str::trim_start).any(|l| if says == "package" { l.split('=').next().is_some_and(|k| k.trim() == "package") || l.contains(" package =") || l.contains("{ package") } else { l.starts_with(says) });
            if found {
                s.refusals.push(format!("{manifest}: `{says}`, which could make `serde::` (or serde_json, schemars) another crate than the one the guard reads it as"));
            }
        }
        Ok(s)
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
        if let Some(a) = file.attrs.iter().find(|a| !a.path().is_ident("doc")) {
            self.refusals.push(format!("{path}: an inner `#![{}]`, which could make the file another module than its name says", a.path().get_ident().map(ToString::to_string).unwrap_or_default()));
        }
        self.items(files, &file.items, path, module, child_dir, tests_only)
    }

    fn items(&mut self, files: &BTreeMap<&str, &str>, items: &[Item], path: &str, module: &str, child_dir: &str, tests_only: bool) -> Result<()> {
        // each name bound once in this scope, an item's or an import's
        let mut m = Module::default();
        let mut bound: BTreeMap<String, usize> = BTreeMap::new();
        for item in items {
            let name = match item {
                Item::Struct(i) => Some(&i.ident),
                Item::Enum(i) => Some(&i.ident),
                Item::Union(i) => Some(&i.ident),
                Item::Type(i) => Some(&i.ident),
                Item::Mod(i) => Some(&i.ident),
                Item::Trait(i) => Some(&i.ident),
                Item::Fn(f) => {
                    if !f.attrs.iter().any(|a| a.path().is_ident("test")) {
                        m.fns.insert(f.sig.ident.to_string());
                    }
                    None
                }
                Item::Const(c) => {
                    m.consts.insert(c.ident.to_string());
                    None
                }
                Item::Static(c) => {
                    m.consts.insert(c.ident.to_string());
                    None
                }
                Item::Use(u) => {
                    if !tests_only && u.attrs.iter().any(|a| !a.path().is_ident("doc")) {
                        self.refusals.push(format!("{module}: a `use` with an attribute: the guard reads plain imports alone"));
                    }
                    if !tests_only && has_glob(&u.tree) {
                        self.refusals.push(format!("{module}: a glob import, whose names (shadowing the prelude's, say) the guard does not list"));
                    }
                    let mut one = Module::default();
                    uses(&u.tree, vec![], &mut one);
                    for n in one.uses.keys() {
                        *bound.entry(n.clone()).or_default() += 1;
                    }
                    uses(&u.tree, vec![], &mut m);
                    None
                }
                _ => None,
            };
            if let Some(n) = name {
                *bound.entry(n.to_string()).or_default() += 1;
                m.items.insert(n.to_string());
            }
        }
        if !tests_only {
            // a known macro's name, imported, names the one listed
            for (n, used) in m.uses.iter().filter(|(n, _)| MACROS.contains(&n.as_str())) {
                let full = used.join("::");
                if !IMPORTED_MACROS.iter().any(|(name, from)| name == n && *from == full) {
                    self.refusals.push(format!("{module}: `use {full}`, an import of a macro's name the guard knows as another's"));
                }
            }
            for (n, _) in bound.iter().filter(|(_, c)| **c > 1) {
                self.refusals.push(format!("{module}::{n}: two bindings of one name in one scope (items and imports): which one a path names is a `cfg`'s or rustc's choice, not the guard's"));
            }
        }
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
                    // a plain `mod`: no attribute but a doc comment, `#[cfg(test)]`, or
                    // `#[cfg(feature = "schema")]` on a module SCHEMA_MODULES lists
                    let mut tests = false;
                    let mut plain = true;
                    for a in i.attrs.iter().filter(|a| !a.path().is_ident("doc")) {
                        let said = a.meta.require_list().ok().filter(|_| a.path().is_ident("cfg")).map(|l| l.tokens.to_string());
                        match said.as_deref() {
                            Some("test") => tests = true,
                            Some("feature = \"schema\"") if SCHEMA_MODULES.contains(&at.as_str()) => {}
                            _ => {
                                plain = false;
                                let text = a.meta.require_list().map(|l| format!("{}({})", a.path().get_ident().map(ToString::to_string).unwrap_or_default(), l.tokens)).unwrap_or_else(|_| a.path().get_ident().map(ToString::to_string).unwrap_or_default());
                                self.refusals.push(format!("{at}: a `mod` with `#[{text}]`: modules are plain `mod` lines (a `#[cfg(test)]`, or `#[cfg(feature = \"schema\")]` on SCHEMA_MODULES', alone)"));
                            }
                        }
                    }
                    if !plain {
                        continue;
                    }
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
                        let name = f.sig.ident.to_string();
                        if module != "crate" && PINNED.iter().any(|(n, _)| *n == name) {
                            self.refusals.push(format!("{module}::{name}: a function of a pinned name outside the crate's root, where a field's attribute might name it in its stead"));
                        }
                        self.fns.insert(format!("{module}::{name}"), f.clone());
                        self.body(&f.block, module, path);
                    }
                }
                Item::Macro(m) => self.refusals.push(format!("{path}: {}!: a macro at item level, which could make a wire type or impl the source guard cannot read", mac_name(&m.mac))),
                Item::ExternCrate(e) => self.refusals.push(format!("{module}: `extern crate {}`, which could name another crate as serde's", e.ident)),
                Item::Const(c) => self.constant(&c.expr, &format!("{module}::{}", c.ident), routes),
                Item::Static(c) => self.constant(&c.expr, &format!("{module}::{}", c.ident), routes),
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
        self.impls.push(Impl { module: module.to_string(), attrs: i.attrs.clone(), trait_path: trait_path.clone(), self_ty: (*i.self_ty).clone(), schema_name });
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
            fn visit_item_mod(&mut self, m: &'a syn::ItemMod) {
                self.macros.push(format!("{}: `mod {}` inside a function's body", self.path, m.ident));
            }
            fn visit_item_use(&mut self, _: &'a syn::ItemUse) {
                self.macros.push(format!("{}: a `use` inside a function's body, whose names the guard does not scope", self.path));
            }
            fn visit_macro(&mut self, m: &'a syn::Macro) {
                self.macros.extend(unknown_macros(m).into_iter().map(|n| format!("{}: {n}!, a macro the guard does not know (MACROS), which could make an item", self.path)));
            }
        }
        let mut v = Inner { found: vec![], macros: vec![], path };
        syn::visit::Visit::visit_block(&mut v, block);
        self.refusals.extend(v.macros);
        for i in v.found {
            self.impl_item(&i, module, path);
        }
    }

    /// A constant's initializer: no item inside it (a `mod`, a `use`, a
    /// type, a macro); a route constant's is a plain literal besides.
    fn constant(&mut self, e: &Expr, at: &str, route: bool) {
        struct Items(Vec<String>);
        impl<'a> syn::visit::Visit<'a> for Items {
            fn visit_item(&mut self, i: &'a Item) {
                self.0.push(match i {
                    Item::Mod(m) => format!("`mod {}`", m.ident),
                    Item::Use(_) => "a `use`".into(),
                    Item::Macro(_) => "a macro".into(),
                    _ => "an item".into(),
                });
            }
        }
        impl Items {
            fn note(&mut self, m: &syn::Macro) {
                self.0.extend(unknown_macros(m).into_iter().map(|n| format!("{n}!, a macro the guard does not know (MACROS)")));
            }
        }
        struct Macros<'b>(&'b mut Items);
        impl<'a> syn::visit::Visit<'a> for Macros<'_> {
            fn visit_macro(&mut self, m: &'a syn::Macro) {
                self.0.note(m);
            }
        }
        let mut found = Items(vec![]);
        syn::visit::Visit::visit_expr(&mut found, e);
        syn::visit::Visit::visit_expr(&mut Macros(&mut found), e);
        for what in found.0 {
            self.refusals.push(format!("{at}: {what} inside a constant's initializer, whose names the guard does not scope"));
        }
        if route {
            self.route_consts.push((at.to_string(), e.clone()));
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

    /// A function an attribute names (`"is_default"`, `"Option::is_none"`),
    /// by its full path as it resolves from `module`: one of the crate's
    /// modules' (`crate::is_default`), or a method of another crate's type
    /// (`Option::is_none`, `std::collections::BTreeMap::is_empty`).
    fn named_fn(&self, text: &str, module: &str, depth: usize) -> Option<String> {
        if depth > 16 {
            return None;
        }
        let p: syn::Path = syn::parse_str(text).ok()?;
        if p.segments.iter().any(|s| !s.arguments.is_none()) {
            return None;
        }
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        let (last, prefix) = segs.split_last()?;
        if prefix.is_empty() {
            let here = self.modules.get(module)?;
            if here.fns.contains(last) {
                return Some(format!("{module}::{last}"));
            }
            return here.uses.get(last).and_then(|used| self.named_fn(&used.join("::"), module, depth + 1));
        }
        if let Some(m) = self.local_module(prefix, module) {
            return self.modules.get(&m)?.fns.contains(last).then(|| format!("{m}::{last}"));
        }
        match self.resolve_path(prefix, module, 0)? {
            Resolved::Foreign(t) => Some(format!("{t}::{last}")),
            _ => None,
        }
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

    /// Every module read, by path (the tests allow wire types in each).
    #[cfg(test)]
    pub fn modules(&self) -> Vec<&str> {
        self.modules.keys().map(String::as_str).collect()
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
        for (_, e) in &self.route_consts {
            syn::visit::Visit::visit_expr(&mut found, e);
        }
        found.0.into_iter().map(|t| (t, "crate::routes".to_string())).collect()
    }
}

impl Source {
    /// That a route constant's initializer is a plain literal: arrays,
    /// references, tuples, literals, and structs; `ty::<T>()` or
    /// `shape::<T>` of a plain path `T`; and values each one of these: a
    /// constant of the routes' own module, `None`, an enum's variant
    /// (`Type::Variant`, or one imported by name), and a call of `Some` or
    /// of a variant. Nothing else (a block, a `let`, a macro, a closure, a
    /// value or a function of another module) is read.
    fn plain(&self, e: &Expr, at: &str, module: &str, refusals: &mut Vec<String>) {
        let inner: Vec<&Expr> = match e {
            Expr::Array(a) => a.elems.iter().collect(),
            Expr::Tuple(t) => t.elems.iter().collect(),
            Expr::Reference(r) => vec![&r.expr],
            Expr::Paren(p) => vec![&p.expr],
            Expr::Group(g) => vec![&g.expr],
            Expr::Struct(s) if s.rest.is_none() && s.qself.is_none() => s.fields.iter().map(|f| &f.expr).collect(),
            Expr::Call(c) => c.args.iter().collect(),
            _ => vec![],
        };
        for i in inner {
            self.plain(i, at, module, refusals);
        }
        match e {
            Expr::Array(_) | Expr::Tuple(_) | Expr::Reference(_) | Expr::Paren(_) | Expr::Group(_) | Expr::Lit(_) => {}
            Expr::Struct(s) if s.rest.is_none() && s.qself.is_none() => {}
            Expr::Call(c) => {
                let callee = match &*c.func {
                    Expr::Path(p) if p.qself.is_none() => self.value(&p.path, module, true),
                    _ => Err("a call of something other than a path".into()),
                };
                if let Err(why) = callee {
                    refusals.push(format!("{at}: {why}"));
                }
            }
            Expr::Path(p) if p.qself.is_none() => {
                if let Err(why) = self.value(&p.path, module, false) {
                    refusals.push(format!("{at}: {why}"));
                }
            }
            _ => refusals.push(format!("{at}: a route constant of another form than plain literals of paths, structs and `ty::<T>()` (a block, a `let`, a macro, a closure, …)")),
        }
    }

    /// A value a route constant names: `ty::<T>`/`shape::<T>` of a plain
    /// path, a constant of `module`, `None`, `Some` (called), or a variant.
    fn value(&self, p: &syn::Path, module: &str, called: bool) -> std::result::Result<(), String> {
        let segs: Vec<&syn::PathSegment> = p.segments.iter().collect();
        let names: Vec<String> = segs.iter().map(|s| s.ident.to_string()).collect();
        if let [s] = segs.as_slice() {
            if s.ident == "ty" || s.ident == "shape" {
                let plain_type = match &s.arguments {
                    PathArguments::AngleBracketed(a) => match a.args.iter().collect::<Vec<_>>().as_slice() {
                        [GenericArgument::Type(Type::Path(t))] => t.qself.is_none() && t.path.segments.iter().all(|s| s.arguments.is_none()),
                        _ => false,
                    },
                    _ => false,
                };
                return if plain_type { Ok(()) } else { Err(format!("`{}::<…>` of another type than a plain path", s.ident)) };
            }
        }
        if segs.iter().any(|s| !s.arguments.is_none()) {
            return Err(format!("`{}`, a path with type arguments other than `ty::<T>`/`shape::<T>`", names.join("::")));
        }
        let here = self.modules.get(module).ok_or("no such module")?;
        let is_enum = |prefix: &[String]| match self.resolve_path(prefix, module, 0) {
            Some(Resolved::Def(d)) => self.defs.get(&d).is_some_and(|d| d.variants.is_some()),
            _ => false,
        };
        let ok = match names.as_slice() {
            [n] if called => n == "Some",
            [n] if n == "None" || here.consts.contains(n) => true,
            // a variant imported by name (`use ErrorCode::{NotFound}`)
            [n] => here.uses.get(n).is_some_and(|used| used.len() >= 2 && is_enum(&used[..used.len() - 1])),
            [ty, _] => is_enum(std::slice::from_ref(ty)),
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(format!("`{}`, a value of another form than its module's constants, `None`, `Some`, an enum's variants and `ty::<T>()`: a route names no value or function of another module", names.join("::")))
        }
    }
}

/// Whether a `use` tree holds a glob.
fn has_glob(tree: &syn::UseTree) -> bool {
    match tree {
        syn::UseTree::Path(p) => has_glob(&p.tree),
        syn::UseTree::Glob(_) => true,
        syn::UseTree::Group(g) => g.items.iter().any(has_glob),
        _ => false,
    }
}

/// The macros a macro invokes the guard does not know: itself, unless
/// `MACROS` lists it, and each `name!(…)` inside its tokens.
fn unknown_macros(m: &syn::Macro) -> Vec<String> {
    let mut out = vec![];
    let segs: Vec<String> = m.path.segments.iter().map(|s| s.ident.to_string()).collect();
    let known = match segs.as_slice() {
        [name] => MACROS.contains(&name.as_str()),
        path => IMPORTED_MACROS.iter().any(|(_, from)| *from == path.join("::")),
    };
    if !known {
        out.push(segs.join("::"));
    }
    nested_macros(m.tokens.clone(), &mut out);
    out
}

/// Each `name!(…)` in a token stream, a keyword's `!` aside (`if !(…)`).
fn nested_macros(tokens: proc_macro2::TokenStream, out: &mut Vec<String>) {
    use proc_macro2::TokenTree;
    const KEYWORDS: [&str; 12] = ["if", "while", "match", "return", "in", "else", "let", "as", "break", "yield", "move", "await"];
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    for (i, t) in trees.iter().enumerate() {
        if let TokenTree::Group(g) = t {
            nested_macros(g.stream(), out);
        }
        if let (TokenTree::Ident(name), Some(TokenTree::Punct(bang)), Some(TokenTree::Group(_))) = (t, trees.get(i + 1), trees.get(i + 2)) {
            let name = name.to_string();
            if bang.as_char() == '!' && !KEYWORDS.contains(&name.as_str()) && !MACROS.contains(&name.as_str()) {
                out.push(name);
            }
        }
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

/// What may stand beside the routes' types by hand: `HAND_WRITTEN` and
/// `OPEN`, and where they live (`WIRE_MODULES`), or a test's own.
pub struct Allowed<'a> {
    pub hand_written: &'a [(&'a str, &'a str)],
    pub open: &'a [(&'a str, &'a str)],
    /// The modules wire types may live in (`WIRE_MODULES`, or a test's).
    pub wire_modules: &'a [&'a str],
}

impl Allowed<'static> {
    pub fn wire() -> Allowed<'static> {
        Allowed { hand_written: &HAND_WRITTEN, open: &OPEN, wire_modules: &WIRE_MODULES }
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
        facts: vec![],
        refusals: source.refusals.clone(),
    };
    for (r, module) in roots {
        g.ty(r, "a route", module, true);
    }
    while let Some(path) = g.queue.pop() {
        g.def(&source.defs[&path], &path);
    }
    // the routes' constants: plain literals
    for (at, e) in &source.route_consts {
        let module = at.rsplit_once("::").map_or("crate", |(m, _)| m);
        source.plain(e, at, module, &mut g.refusals);
    }
    // where each lives: the top level of a module WIRE_MODULES lists
    for path in &g.reached {
        let d = &source.defs[path];
        if !allowed.wire_modules.contains(&d.module.as_str()) {
            g.refusals.push(format!("{}: a wire type in `{}`: wire types live at the top level of {:?} alone", d.name, d.module, allowed.wire_modules));
        }
    }
    // each pinned function is its text, wherever it is defined
    for (name, text) in PINNED {
        if let Some(f) = source.fns.get(&format!("crate::{name}")) {
            if let Some(a) = f.attrs.iter().find(|a| !a.path().is_ident("doc")) {
                g.refusals.push(format!("crate::{name}: under `#[{}]`, an attribute that could rewrite its pinned text", a.path().get_ident().map(ToString::to_string).unwrap_or_default()));
            }
            let mut f = f.clone();
            f.attrs.clear();
            if syn::parse_str::<syn::ItemFn>(text).ok().as_ref() != Some(&f) {
                g.refusals.push(format!("crate::{name}: not its pinned text (`{text}`): what it omits or fills in is what that body says, so an edit fails"));
            }
        }
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
        if let Some(a) = i.attrs.iter().find(|a| !a.path().is_ident("doc") && !a.path().is_ident("cfg")) {
            g.refusals.push(format!("an impl for {name} under `#[{}]`, an attribute that could rewrite it", a.path().get_ident().map(ToString::to_string).unwrap_or_default()));
        }
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
    let mut stale = vec![];
    listed(&g.hand, allowed.hand_written, "HAND_WRITTEN", &mut stale);
    listed(&g.open, allowed.open, "OPEN", &mut stale);
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
    facts: Vec<String>,
    refusals: Vec<String>,
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
        for a in attrs.iter().filter(|a| !a.path().get_ident().is_some_and(|i| ATTRIBUTES.contains(&i.to_string().as_str()))) {
            let name = a.path().segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::");
            self.refusals.push(format!("{at}: `#[{name}]`, an attribute the guard does not read, which could rewrite the item (an attribute macro)"));
        }
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
                        // a default function by its full path
                        "default" => {
                            default = Some(match str_value(m) {
                                None => "default".to_string(),
                                Some(f) => format!("default = {}", self.source.named_fn(&f, module, 0).unwrap_or_else(|| format!("{f} (unresolved)"))),
                            })
                        }
                        "skip_serializing_if" => skip = str_value(m),
                        _ => {}
                    }
                }
                "schemars" => self.schemars(at, m),
                "validate" | "garde" => self.validator(at, ns),
                _ => {}
            }
        }
        if let Some(d) = default.as_deref().and_then(|d| d.strip_prefix("default = ")).filter(|d| *d != "crate::default_true") {
            self.refusals.push(format!("{at}: `#[serde(default = …)]` naming `{d}`: a default function is a pinned one (`crate::default_true`) alone, or a plain `#[serde(default)]`"));
        }
        if let Some(pred) = skip {
            self.skip(at, &pred, &f.ty, default.as_deref(), module);
        }
        self.ty(&f.ty, at, module, true);
    }

    /// A `skip_serializing_if` omits exactly its field's default, so what
    /// its reader fills in is what was written: each predicate by its full
    /// path, beside the default whose value it omits, and nothing else.
    fn skip(&mut self, at: &str, pred: &str, ty: &Type, default: Option<&str>, module: &str) {
        let head = match self.source.resolve(ty, module) {
            Some(Resolved::Foreign(f)) => f.rsplit("::").next().unwrap_or(&f).to_string(),
            _ => "?".to_string(),
        };
        let named = self.source.named_fn(pred, module, 0);
        let ok = match named.as_deref() {
            Some("Option::is_none") => head == "Option" && matches!(default, None | Some("default")),
            Some("Vec::is_empty") => head == "Vec" && default == Some("default"),
            Some("std::collections::BTreeMap::is_empty") => head == "BTreeMap" && default == Some("default"),
            Some("crate::is_default") => DEFAULTED.contains(&head.as_str()) && default == Some("default"),
            Some("crate::is_true") => head == "bool" && default == Some("default = crate::default_true"),
            _ => false,
        };
        if !ok {
            let named = named.unwrap_or_else(|| "a function the guard cannot resolve".into());
            self.refusals.push(format!(
                "{at}: `skip_serializing_if = \"{pred}\"` ({named}, on `{head}`, beside {}): a writer omits a value by `Option::is_none` beside `None`, `Vec::is_empty` or `BTreeMap::is_empty` beside the empty default, `crate::is_default` beside `#[serde(default)]` on {DEFAULTED:?}, or `crate::is_true` beside `default = \"default_true\"`, and so omits exactly the value its reader fills in",
                default.unwrap_or("no default")
            ));
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
