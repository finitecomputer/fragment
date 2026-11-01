//! The search benchmark (bench/search/README.md): one corpus of markdown and
//! one file of labelled questions, both kept outside the repo, scored on
//! each engine the platform might search with. What it prints and the
//! report it writes hold aggregates only; per-question detail, which names
//! the corpus's pages, stays in the work directory beside the indexes.

mod across;
mod corpus;
mod decoy;
mod embed;
mod lance;
mod metrics;
mod sqlite;

use corpus::{Page, Section, Sectioned};
use embed::{Embedder, Embeddings};
use metrics::Judgments;
use serde::{Deserialize, Serialize};
use sqlite::{Fts, Storage, Vectors, BRAIN_INDEX_DB_MAX_BYTES, BRAIN_WEIGHTS};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Results scored: what an agent reads from one search.
const K: usize = 10;
/// Candidates each leg of a hybrid search brings to the fusion.
const DEPTH: usize = 50;
/// Reciprocal-rank fusion's constant, Finite Brain's and LanceDB's.
const RRF_K: f64 = 60.0;
const QUESTIONS_MAX: usize = 1000;
const RUNS_MAX: usize = 100;
const DECOYS_MAX: usize = 1_000_000;

const USAGE: &str = "usage:
  search-bench embed --corpus <dir> --work <dir> --embeddings-url <url> --embeddings-model <name>
  search-bench run --corpus <dir> --questions <file.jsonl> --work <dir>
      (--embeddings-url <url> --embeddings-model <name> [--query-prefix <text>] | --hashed-embeddings <dims>)
      [--runs <n>] [--decoys <n>] [--report <file.json>]
  search-bench across --corpus <dir> --questions <file.jsonl> --work <dir>
      (--embeddings-url <url> --embeddings-model <name> [--query-prefix <text>] | --hashed-embeddings <dims>)
      [--window <tokens>] [--document-prefix <text>] [--report <file.json>]";

struct Args(HashMap<String, String>);

impl Args {
    fn parse(rest: &[String]) -> Result<Self, String> {
        let mut map = HashMap::new();
        let mut it = rest.iter();
        while let Some(flag) = it.next() {
            let name = flag.strip_prefix("--").ok_or_else(|| format!("unexpected {flag}\n{USAGE}"))?;
            let value = it.next().ok_or_else(|| format!("{flag} wants a value\n{USAGE}"))?;
            if map.insert(name.to_owned(), value.clone()).is_some() {
                return Err(format!("{flag} given twice"));
            }
        }
        Ok(Args(map))
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    fn need(&self, name: &str) -> Result<&str, String> {
        self.get(name).ok_or_else(|| format!("--{name} is required\n{USAGE}"))
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        self.need(name).map(PathBuf::from)
    }

    fn embedder(&self) -> Result<Embedder, String> {
        match (self.get("embeddings-url"), self.get("hashed-embeddings")) {
            (Some(url), None) => Embedder::http(url, self.need("embeddings-model")?, self.get("query-prefix").unwrap_or("")),
            (None, Some(dims)) => Ok(Embedder::Hashed { dims: dims.parse().map_err(|_| format!("--hashed-embeddings {dims}: a count"))? }),
            _ => Err(format!("one of --embeddings-url or --hashed-embeddings\n{USAGE}")),
        }
    }

    fn work(&self) -> Result<PathBuf, String> {
        let work = self.path("work")?;
        std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
        Ok(work)
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let result = match argv.first().map(String::as_str) {
        Some("embed") => Args::parse(&argv[1..]).and_then(|a| embed_corpus(&a)),
        Some("run") => Args::parse(&argv[1..]).and_then(|a| run(&a).map(|report| print!("{}", report.render()))),
        Some("across") => Args::parse(&argv[1..]).and_then(|a| gate(&a).map(|report| print!("{report}"))),
        _ => Err(USAGE.to_owned()),
    };
    if let Err(e) = result {
        eprintln!("search-bench: {e}");
        std::process::exit(1);
    }
}

/// The corpus's pages, each with its sections.
fn load(corpus: &Path) -> Result<(Sectioned, usize), String> {
    let (pages, blobs) = corpus::load(corpus)?;
    Ok((pages.into_iter().map(|p| { let s = corpus::sections(&p).0; (p, s) }).collect(), blobs))
}

fn embed_corpus(args: &Args) -> Result<(), String> {
    let work = args.work()?;
    let (pages, blobs) = load(&args.path("corpus")?)?;
    let inputs: Vec<String> = pages.iter().flat_map(|(_, s)| s.iter().map(Section::embedding_input)).collect();
    eprintln!("{} pages ({blobs} left out as blobs), {} sections", pages.len(), inputs.len());
    let mut embeddings = Embeddings::open(args.embedder()?, &work)?;
    embeddings.documents(&inputs, true)?;
    eprintln!("{} dimensions, {} tokens", embeddings.dims, embeddings.document_tokens);
    Ok(())
}

#[derive(Deserialize, Debug)]
struct Question {
    id: String,
    q: String,
    /// What the question tests (exact, paraphrase, date, …): scores are also
    /// given by kind.
    #[serde(default)]
    kind: String,
    /// The pages that answer it (2) or help (1); none: it has no answer here.
    #[serde(default)]
    relevant: Judgments,
}

fn questions(path: &Path, pages: &[(Page, Vec<Section>)]) -> Result<Vec<Question>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let known: std::collections::HashSet<&str> = pages.iter().map(|(p, _)| p.path.as_str()).collect();
    let mut out: Vec<Question> = Vec::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let q: Question = serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", path.display(), n + 1))?;
        if let Some((page, _)) = q.relevant.iter().find(|(p, _)| !known.contains(p.as_str())) {
            return Err(format!("{}:{}: {page} is not a page of the corpus", path.display(), n + 1));
        }
        if q.relevant.values().any(|&g| g > 2) {
            return Err(format!("{}:{}: a judgment is 0, 1 or 2", path.display(), n + 1));
        }
        if out.iter().any(|o| o.id == q.id) {
            return Err(format!("{}:{}: question {} twice", path.display(), n + 1, q.id));
        }
        out.push(q);
    }
    if out.is_empty() || out.len() > QUESTIONS_MAX {
        return Err(format!("{}: 1 to {QUESTIONS_MAX} questions, not {}", path.display(), out.len()));
    }
    Ok(out)
}

/// Every engine, built over the same sections and vectors.
struct Engines {
    brain: Fts,
    fts: Fts,
    f32: Vectors,
    i8: Vectors,
    lance: lance::Lance,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum System {
    Brain,
    FbrainLexical,
    FbrainHybrid,
    DoLexical,
    DoHybrid,
    VectorF32,
    VectorI8,
    LanceFts,
    LanceVector,
    LanceHybrid,
}

impl System {
    const ALL: [System; 10] = [
        System::Brain,
        System::FbrainLexical,
        System::FbrainHybrid,
        System::DoLexical,
        System::DoHybrid,
        System::VectorF32,
        System::VectorI8,
        System::LanceFts,
        System::LanceVector,
        System::LanceHybrid,
    ];

    fn name(self) -> &'static str {
        match self {
            System::Brain => "brain today (FTS5, every word's prefix, 12 MiB cap)",
            System::FbrainLexical => "fbrain lexical (FTS5, any token, global BM25)",
            System::FbrainHybrid => "fbrain hybrid (+ float32 scan, RRF)",
            System::DoLexical => "DO baseline lexical (FTS5, any token)",
            System::DoHybrid => "DO baseline hybrid (+ int8 streamed scan, RRF)",
            System::VectorF32 => "vectors alone, float32 exact",
            System::VectorI8 => "vectors alone, int8 exact",
            System::LanceFts => "LanceDB BM25F",
            System::LanceVector => "LanceDB vectors (IVF_FLAT)",
            System::LanceHybrid => "LanceDB hybrid (BM25F + IVF_FLAT, RRF)",
        }
    }

    fn rank(self, e: &Engines, q: &str, v: &[f32]) -> Result<Vec<String>, String> {
        Ok(match self {
            System::Brain => e.brain.brain(q, K)?,
            System::FbrainLexical => e.fts.fbrain(q, DEPTH)?,
            System::FbrainHybrid => sqlite::rrf(&[&e.fts.fbrain(q, DEPTH)?, &e.f32.nearest(v, DEPTH)?], RRF_K),
            System::DoLexical => e.fts.any(q, &BRAIN_WEIGHTS, DEPTH)?,
            System::DoHybrid => sqlite::rrf(&[&e.fts.any(q, &BRAIN_WEIGHTS, DEPTH)?, &e.i8.nearest(v, DEPTH)?], RRF_K),
            System::VectorF32 => e.f32.nearest(v, DEPTH)?,
            System::VectorI8 => e.i8.nearest(v, DEPTH)?,
            System::LanceFts => e.lance.fts(q, DEPTH)?,
            System::LanceVector => e.lance.nearest(v, DEPTH)?,
            System::LanceHybrid => e.lance.hybrid(q, v, DEPTH)?,
        })
    }
}

#[derive(Serialize, Default)]
struct Scores {
    system: String,
    recall_at_10: f64,
    ndcg_at_10: f64,
    success_at_10: f64,
    p50_ms: f64,
    p95_ms: f64,
    /// Recall@10 by the questions' kinds.
    recall_by_kind: BTreeMap<String, f64>,
}

#[derive(Serialize, Default)]
struct Freshness {
    engine: String,
    /// An edited page's write, until the call returns.
    write_ms: f64,
    /// Found at once by a word only the edit added, and by its new vector.
    lexical_visible: bool,
    semantic_visible: bool,
    delete_ms: f64,
    /// Gone at once from both: no section of the deleted page comes back.
    lexical_gone: bool,
    semantic_gone: bool,
    /// LanceDB: bringing its indexes up to the writes, and the same checks
    /// after it.
    optimize_ms: Option<f64>,
    visible_after_optimize: Option<bool>,
}

#[derive(Serialize, Default)]
struct Report {
    pages: usize,
    sections: usize,
    /// Made-up sections added to measure a larger index (decoy.rs).
    decoy_sections: usize,
    corpus_bytes: usize,
    questions: usize,
    unanswerable: usize,
    by_kind: BTreeMap<String, usize>,
    model: String,
    dimensions: usize,
    document_tokens: u64,
    runs: usize,
    query_embedding_p50_ms: f64,
    query_embedding_p95_ms: f64,
    brain_skipped_pages: usize,
    build_ms: BTreeMap<String, f64>,
    index_bytes: BTreeMap<String, u64>,
    /// Of exact float32's top 50, the share each approximate list keeps.
    overlap_at_50: BTreeMap<String, f64>,
    lance_reopen_first_query_ms: f64,
    scores: Vec<Scores>,
    freshness: Vec<Freshness>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            _ => e.metadata().map_or(0, |m| m.len()),
        })
        .sum()
}

fn rows<'a>(sections: &'a [Section], vector_of: &HashMap<&str, &'a [f32]>) -> Vec<(&'a Section, &'a [f32])> {
    sections.iter().map(|s| (s, vector_of[s.id.as_str()])).collect()
}

fn timed<T>(f: impl FnOnce() -> Result<T, String>) -> Result<(T, Duration), String> {
    let started = Instant::now();
    let out = f()?;
    Ok((out, started.elapsed()))
}

fn run(args: &Args) -> Result<Report, String> {
    let work = args.work()?;
    let runs: usize = args.get("runs").unwrap_or("5").parse().map_err(|_| "--runs: a count")?;
    if runs == 0 || runs > RUNS_MAX {
        return Err(format!("--runs: 1 to {RUNS_MAX}"));
    }
    let (pages, _) = load(&args.path("corpus")?)?;
    let questions = questions(&args.path("questions")?, &pages)?;
    let mut embeddings = Embeddings::open(args.embedder()?, &work)?;
    let sections: Vec<&Section> = pages.iter().flat_map(|(_, s)| s.iter()).collect();
    eprintln!("{} pages, {} sections, {} questions", pages.len(), sections.len(), questions.len());
    let inputs: Vec<String> = sections.iter().map(|s| s.embedding_input()).collect();
    let vectors = embeddings.documents(&inputs, true)?;
    let decoys: usize = args.get("decoys").unwrap_or("0").parse().map_err(|_| "--decoys: a count")?;
    if decoys > DECOYS_MAX {
        return Err(format!("--decoys: at most {DECOYS_MAX}"));
    }
    let (decoy_pages, decoy_vectors) = decoy::make(decoys, embeddings.dims);
    let mut vector_of: HashMap<&str, &[f32]> = sections.iter().zip(&vectors).map(|(s, v)| (s.id.as_str(), v.as_slice())).collect();
    for ((_, s), v) in decoy_pages.iter().zip(&decoy_vectors) {
        vector_of.insert(s[0].id.as_str(), v.as_slice());
    }
    // the corpus's pages first, as the brain would have indexed them before any decoy
    let indexed: Vec<&(Page, Vec<Section>)> = pages.iter().chain(&decoy_pages).collect();

    let mut report = Report {
        pages: pages.len(),
        sections: sections.len(),
        corpus_bytes: pages.iter().map(|(p, _)| p.text.len()).sum(),
        questions: questions.len(),
        unanswerable: questions.iter().filter(|q| q.relevant.values().all(|&g| g == 0)).count(),
        model: embeddings.embedder.id(),
        dimensions: embeddings.dims,
        document_tokens: embeddings.document_tokens,
        runs,
        decoy_sections: decoys,
        ..Report::default()
    };
    for q in &questions {
        *report.by_kind.entry(q.kind.clone()).or_default() += 1;
    }

    eprintln!("building the indexes");
    let index = work.join("index");
    std::fs::create_dir_all(&index).map_err(|e| e.to_string())?;
    let (brain, took) = timed(|| {
        let mut fts = Fts::create(&index.join("brain.sqlite3"), Some(BRAIN_INDEX_DB_MAX_BYTES))?;
        for (p, s) in &indexed {
            fts.put_page(&p.path, &s.iter().collect::<Vec<_>>())?;
        }
        Ok(fts)
    })?;
    report.build_ms.insert("brain FTS5".into(), ms(took));
    report.brain_skipped_pages = brain.skipped_pages;
    let (fts, took) = timed(|| {
        let mut fts = Fts::create(&index.join("fts.sqlite3"), None)?;
        for (p, s) in &indexed {
            fts.put_page(&p.path, &s.iter().collect::<Vec<_>>())?;
        }
        Ok(fts)
    })?;
    report.build_ms.insert("FTS5".into(), ms(took));
    let mut vector_store = |name: &str, storage: Storage| -> Result<Vectors, String> {
        let (v, took) = timed(|| {
            let mut v = Vectors::create(&index.join(format!("vec-{name}.sqlite3")), storage)?;
            for (p, s) in &indexed {
                v.put_page(&p.path, &rows(s, &vector_of))?;
            }
            Ok(v)
        })?;
        report.build_ms.insert(format!("{name} vectors"), ms(took));
        Ok(v)
    };
    let f32 = vector_store("f32", Storage::F32)?;
    let i8 = vector_store("i8", Storage::I8)?;
    let all_rows: Vec<(&Section, &[f32])> = indexed.iter().flat_map(|(_, s)| rows(s, &vector_of)).collect();
    let (lance, built) = lance::Lance::create(&index.join("lance"), &all_rows)?;
    report.build_ms.insert("LanceDB load".into(), ms(built.load));
    report.build_ms.insert("LanceDB BM25 indexes".into(), ms(built.fts_index));
    report.build_ms.insert("LanceDB IVF_FLAT index".into(), ms(built.vector_index));
    report.index_bytes.insert("brain FTS5".into(), sqlite::file_bytes(&brain.conn));
    report.index_bytes.insert("FTS5".into(), sqlite::file_bytes(&fts.conn));
    report.index_bytes.insert("f32 vectors".into(), sqlite::file_bytes(&f32.conn));
    report.index_bytes.insert("i8 vectors".into(), sqlite::file_bytes(&i8.conn));
    report.index_bytes.insert("LanceDB".into(), dir_bytes(&index.join("lance")));
    let mut e = Engines { brain, fts, f32, i8, lance };

    eprintln!("embedding the questions");
    let mut qvecs = Vec::new();
    let mut qtimes = Vec::new();
    for q in &questions {
        let (v, took) = embeddings.query(&q.q)?;
        qvecs.push(v);
        qtimes.push(ms(took));
    }
    report.query_embedding_p50_ms = metrics::percentile(&qtimes, 50.0);
    report.query_embedding_p95_ms = metrics::percentile(&qtimes, 95.0);

    eprintln!("searching: {} questions × {} systems × {runs} runs", questions.len(), System::ALL.len());
    let mut rankings: BTreeMap<(System, usize), Vec<String>> = BTreeMap::new();
    let mut latency: BTreeMap<System, Vec<f64>> = BTreeMap::new();
    for (i, (q, v)) in questions.iter().zip(&qvecs).enumerate() {
        for system in System::ALL {
            let ranked = system.rank(&e, &q.q, v)?;
            let mut times: Vec<f64> = (0..runs).map(|_| timed(|| system.rank(&e, &q.q, v)).map(|(_, t)| ms(t))).collect::<Result<_, _>>()?;
            times.sort_by(f64::total_cmp);
            latency.entry(system).or_default().push(times[times.len() / 2]);
            rankings.insert((system, i), ranked);
        }
    }
    let answerable: Vec<usize> = (0..questions.len()).filter(|&i| questions[i].relevant.values().any(|&g| g > 0)).collect();
    for system in System::ALL {
        let per = |f: fn(&[String], &Judgments, usize) -> f64, which: &dyn Fn(usize) -> bool| {
            let xs: Vec<f64> = answerable.iter().filter(|&&i| which(i)).map(|&i| f(&rankings[&(system, i)], &questions[i].relevant, K)).collect();
            metrics::mean(&xs)
        };
        let mut by_kind = BTreeMap::new();
        for kind in report.by_kind.keys() {
            if answerable.iter().any(|&i| &questions[i].kind == kind) {
                by_kind.insert(kind.clone(), per(metrics::recall, &|i| &questions[i].kind == kind));
            }
        }
        let lat = &latency[&system];
        report.scores.push(Scores {
            system: system.name().into(),
            recall_at_10: per(metrics::recall, &|_| true),
            ndcg_at_10: per(metrics::ndcg, &|_| true),
            success_at_10: per(metrics::success, &|_| true),
            p50_ms: metrics::percentile(lat, 50.0),
            p95_ms: metrics::percentile(lat, 95.0),
            recall_by_kind: by_kind,
        });
    }
    for (name, system) in [("int8 scan", System::VectorI8), ("LanceDB IVF_FLAT", System::LanceVector)] {
        let shares: Vec<f64> = (0..questions.len())
            .map(|i| {
                let exact = &rankings[&(System::VectorF32, i)];
                let kept = rankings[&(system, i)].iter().filter(|id| exact.contains(id)).count();
                kept as f64 / exact.len().max(1) as f64
            })
            .collect();
        report.overlap_at_50.insert(name.into(), metrics::mean(&shares));
    }
    write_details(&work, &questions, &rankings)?;

    eprintln!("freshness");
    report.freshness = freshness(&mut e, &mut embeddings, &pages, &questions)?;
    let (_, took) = timed(|| {
        e.lance.reopen()?;
        e.lance.hybrid(&questions[0].q, &qvecs[0], DEPTH)
    })?;
    report.lance_reopen_first_query_ms = ms(took);

    if let Some(path) = args.get("report") {
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap()).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(report)
}

/// Each question's top results by system, and the pages in them no
/// judgment covers: the pool to judge next. It names the corpus's pages,
/// so it stays in the work directory.
fn write_details(work: &Path, questions: &[Question], rankings: &BTreeMap<(System, usize), Vec<String>>) -> Result<(), String> {
    let mut out = String::new();
    for (i, q) in questions.iter().enumerate() {
        let mut systems = BTreeMap::new();
        let mut unjudged: Vec<&str> = Vec::new();
        for system in System::ALL {
            let top: Vec<&String> = rankings[&(system, i)].iter().take(K).collect();
            for id in &top {
                let page = id.rsplit_once('#').map_or(id.as_str(), |(p, _)| p);
                if !q.relevant.contains_key(page) && !unjudged.contains(&page) {
                    unjudged.push(page);
                }
            }
            systems.insert(format!("{system:?}"), top);
        }
        out += &serde_json::json!({ "id": q.id, "q": q.q, "unjudged": unjudged, "top": systems }).to_string();
        out.push('\n');
    }
    let path = work.join("details.jsonl");
    std::fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))
}

/// An edit and a delete on one page, seen by each engine: is the change
/// visible to the very next search?
fn freshness(e: &mut Engines, embeddings: &mut Embeddings, pages: &[(Page, Vec<Section>)], questions: &[Question]) -> Result<Vec<Freshness>, String> {
    // the page the first answerable question is best answered by
    let target = questions
        .iter()
        .find_map(|q| q.relevant.iter().filter(|(_, &g)| g > 0).max_by_key(|(_, &g)| g).map(|(p, _)| p.clone()))
        .ok_or("no question has an answer to edit")?;
    let (page, _) = pages.iter().find(|(p, _)| p.path == target).expect("judged pages are the corpus's");
    let nonce = format!("zq{:x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() % (1 << 40));
    let edited = Page { path: page.path.clone(), text: format!("{}\n\nFreshness marker {nonce}: the harbour lantern audit moved to the north annex.\n", page.text) };
    let (sections, _) = corpus::sections(&edited);
    let inputs: Vec<String> = sections.iter().map(Section::embedding_input).collect();
    let vectors = embeddings.documents(&inputs, false)?;
    let rows: Vec<(&Section, &[f32])> = sections.iter().zip(&vectors).map(|(s, v)| (s, v.as_slice())).collect();
    let marked = sections.iter().position(|s| s.body.contains(&nonce)).expect("the marker is in a section");
    let (marked_id, marked_vec) = (sections[marked].id.clone(), vectors[marked].clone());
    let path = page.path.as_str();
    let of_page = |ids: &[String]| ids.iter().any(|id| id.rsplit_once('#').map_or(id.as_str(), |(p, _)| p) == path);
    let refs: Vec<&Section> = sections.iter().collect();

    let mut out = Vec::new();
    // SQLite: the FTS5 table and the int8 vectors, as one Durable Object would write them
    let (_, write) = timed(|| {
        e.fts.put_page(path, &refs)?;
        e.i8.put_page(path, &rows)
    })?;
    let lexical_visible = of_page(&e.fts.any(&nonce, &BRAIN_WEIGHTS, K)?);
    let semantic_visible = e.i8.nearest(&marked_vec, 1)? == [marked_id.clone()];
    let (_, delete) = timed(|| {
        e.fts.delete_page(path)?;
        e.i8.delete_page(path)
    })?;
    out.push(Freshness {
        engine: "SQLite FTS5 + int8 vectors".into(),
        write_ms: ms(write),
        lexical_visible,
        semantic_visible,
        delete_ms: ms(delete),
        lexical_gone: !of_page(&e.fts.any(&nonce, &BRAIN_WEIGHTS, K)?),
        semantic_gone: !of_page(&e.i8.nearest(&marked_vec, K)?),
        ..Freshness::default()
    });

    let (_, write) = timed(|| e.lance.put_page(path, &rows))?;
    let lexical_visible = of_page(&e.lance.fts(&nonce, K)?);
    let semantic_visible = e.lance.nearest(&marked_vec, 1)? == [marked_id.clone()];
    let (_, optimize) = timed(|| e.lance.optimize())?;
    let after = of_page(&e.lance.fts(&nonce, K)?) && e.lance.nearest(&marked_vec, 1)? == [marked_id.clone()];
    let (_, delete) = timed(|| e.lance.delete_page(path))?;
    out.push(Freshness {
        engine: "LanceDB".into(),
        write_ms: ms(write),
        lexical_visible,
        semantic_visible,
        delete_ms: ms(delete),
        lexical_gone: !of_page(&e.lance.fts(&nonce, K)?),
        semantic_gone: !of_page(&e.lance.nearest(&marked_vec, K)?),
        optimize_ms: Some(ms(optimize)),
        visible_after_optimize: Some(after),
    });
    Ok(out)
}

impl Report {
    fn render(&self) -> String {
        let mut s = String::new();
        s += &format!(
            "{} pages, {} sections (+{} decoys), {:.1} MB of markdown; {} questions ({} without an answer here)\n",
            self.pages,
            self.sections,
            self.decoy_sections,
            self.corpus_bytes as f64 / 1e6,
            self.questions,
            self.unanswerable
        );
        s += &format!("by kind: {}\n", self.by_kind.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", "));
        s += &format!(
            "embeddings: {} ({} dimensions), {} corpus tokens; a question's embedding p50 {:.0} ms, p95 {:.0} ms (not in the latencies below)\n",
            self.model, self.dimensions, self.document_tokens, self.query_embedding_p50_ms, self.query_embedding_p95_ms
        );
        s += &format!("brain today: {} pages left out by its 12 MiB index cap\n\n", self.brain_skipped_pages);
        s += "| system | Recall@10 | nDCG@10 | Success@10 | p50 ms | p95 ms |\n|---|---:|---:|---:|---:|---:|\n";
        for x in &self.scores {
            s += &format!("| {} | {:.3} | {:.3} | {:.3} | {:.2} | {:.2} |\n", x.system, x.recall_at_10, x.ndcg_at_10, x.success_at_10, x.p50_ms, x.p95_ms);
        }
        let kinds: Vec<&String> = self.scores.first().map(|x| x.recall_by_kind.keys().collect()).unwrap_or_default();
        if !kinds.is_empty() {
            s += &format!("\nRecall@10 by kind\n\n| system | {} |\n|---|{}\n", kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(" | "), "---:|".repeat(kinds.len()));
            for x in &self.scores {
                s += &format!("| {} | {} |\n", x.system, kinds.iter().map(|k| format!("{:.3}", x.recall_by_kind[*k])).collect::<Vec<_>>().join(" | "));
            }
        }
        s += "\n| index | build ms |\n|---|---:|\n";
        for (k, v) in &self.build_ms {
            s += &format!("| {k} | {v:.0} |\n");
        }
        s += "\n| index | bytes |\n|---|---:|\n";
        for (k, v) in &self.index_bytes {
            s += &format!("| {k} | {v} |\n");
        }
        s += &format!(
            "\nof exact float32's top 50: {}\nLanceDB reopened, its first hybrid search: {:.1} ms\n",
            self.overlap_at_50.iter().map(|(k, v)| format!("{k} keeps {v:.3}")).collect::<Vec<_>>().join(", "),
            self.lance_reopen_first_query_ms
        );
        s += "\n| engine | write ms | lexical visible | semantic visible | delete ms | lexical gone | semantic gone | optimize ms | after optimize |\n|---|---:|---|---|---:|---|---|---:|---|\n";
        for f in &self.freshness {
            s += &format!(
                "| {} | {:.1} | {} | {} | {:.1} | {} | {} | {} | {} |\n",
                f.engine,
                f.write_ms,
                f.lexical_visible,
                f.semantic_visible,
                f.delete_ms,
                f.lexical_gone,
                f.semantic_gone,
                f.optimize_ms.map_or("".into(), |x| format!("{x:.0}")),
                f.visible_after_optimize.map_or("".into(), |x| x.to_string())
            );
        }
        s
    }
}

/// The quality gate's bar (FIN-238, decision 5): the federated answer's
/// nDCG@10 within this share of the union index's.
const GATE_SHARE: f64 = 0.95;

/// FIN-238's quality gate (src/across.rs): the corpus cut by wiki, each
/// searched as a fragment's index, merged as the route merges, against one
/// index over the whole corpus and Finite Brain's hybrid. Prints and
/// writes aggregates; each question's top pages go to the work directory
/// (`across-details.jsonl`), the pool to judge.
fn gate(args: &Args) -> Result<String, String> {
    let work = args.work()?;
    let window: u32 = args.get("window").unwrap_or("8192").parse().map_err(|_| "--window: a token count")?;
    let prefix = args.get("document-prefix");
    let (pages, _) = load(&args.path("corpus")?)?;
    let questions = questions(&args.path("questions")?, &pages)?;
    let mut embeddings = Embeddings::open(args.embedder()?, &work)?;
    let cuts = across::cut(&pages, window, prefix);
    let chunks: Vec<&fragment_core::searchable::Chunk> = cuts.iter().flat_map(|c| c.chunks.iter().map(|(_, ch, _, _)| ch)).collect();
    eprintln!("{} pages, {} chunks, {} questions", pages.len(), chunks.len(), questions.len());
    let inputs: Vec<String> = chunks.iter().map(|c| c.input.clone()).collect();
    let vectors = embeddings.documents(&inputs, true)?;
    let vector_of: HashMap<String, Vec<f32>> = chunks.iter().map(|c| c.hash.clone()).zip(vectors).collect();
    let dims = embeddings.dims;
    let dir = work.join("across");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    eprintln!("building the indexes");
    let union = across::Index::build(&dir.join("union.sqlite3"), &cuts.iter().collect::<Vec<_>>(), &vector_of, dims)?;
    let mut by_wiki: BTreeMap<&str, Vec<&across::Cut>> = BTreeMap::new();
    for ((page, _), cut) in pages.iter().zip(&cuts) {
        by_wiki.entry(across::wiki_of(&page.path)).or_default().push(cut);
    }
    let fragments: Vec<across::Index> = by_wiki
        .values()
        .enumerate()
        .map(|(i, cuts)| across::Index::build(&dir.join(format!("wiki-{i}.sqlite3")), cuts, &vector_of, dims))
        .collect::<Result<_, _>>()?;
    // Finite Brain's hybrid: its lexical search over sections, fused with an
    // exact float32 scan (a section by its best chunk)
    let _ = std::fs::remove_file(dir.join("fbrain.sqlite3"));
    let mut fts = Fts::create(&dir.join("fbrain.sqlite3"), None)?;
    for (p, s) in &pages {
        fts.put_page(&p.path, &s.iter().collect::<Vec<_>>())?;
    }
    let units: Vec<(&str, Vec<f32>)> = cuts
        .iter()
        .flat_map(|c| c.chunks.iter().map(|(section, ch, _, _)| (section.as_str(), fragment_core::search_index::unit(&vector_of[&ch.hash]).unwrap_or_default())))
        .collect();
    let nearest = |v: &[f32]| -> Vec<String> {
        let u = fragment_core::search_index::unit(v).unwrap_or_default();
        let mut best: HashMap<&str, f32> = HashMap::new();
        for (section, c) in &units {
            let d: f32 = c.iter().zip(&u).map(|(a, b)| a * b).sum();
            let e = best.entry(section).or_insert(f32::NEG_INFINITY);
            *e = e.max(d);
        }
        let mut all: Vec<(&str, f32)> = best.into_iter().collect();
        all.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        all.into_iter().take(DEPTH).map(|(s, _)| s.to_string()).collect()
    };
    const SYSTEMS: [&str; 7] = [
        "federated (by wiki, the route's merge)",
        "union (one index, the shipped engine)",
        "fbrain hybrid",
        "federated + pooled word statistics (bench prototype, B)",
        "union, exact float32 meaning leg (diagnosis C1)",
        "union, fbrain's words leg (diagnosis C2)",
        "union, legs 50 deep (diagnosis C3)",
    ];
    eprintln!("searching: {} questions over {} wikis", questions.len(), fragments.len());
    let mut rankings: Vec<[Vec<String>; 7]> = Vec::new();
    for q in &questions {
        let (v, _) = embeddings.query(&q.q)?;
        let fed = across::federated(&fragments, &q.q, &v, K)?;
        let uni: Vec<String> = union.search(&q.q, &v, K)?.into_iter().map(|h| h.section).collect();
        let fb: Vec<String> = sqlite::rrf(&[&fts.fbrain(&q.q, DEPTH)?, &nearest(&v)], RRF_K).into_iter().take(K).collect();
        let pooled = across::federated_pooled(&fragments, &q.q, &v, K)?;
        let sections = |hits: Vec<across::Hit>| -> Vec<String> { hits.into_iter().map(|h| h.section).collect() };
        let exact = sections(union.search_variant(&q.q, &v, K, None, across::Variant { exact: true, ..Default::default() }, None)?);
        let fb_words = fts.fbrain(&q.q, 100)?;
        let words = sections(union.search_variant(&q.q, &v, K, None, across::Variant::default(), Some(&fb_words))?);
        let shallow = sections(union.search_variant(&q.q, &v, K, None, across::Variant { depth: 50, ..Default::default() }, None)?);
        rankings.push([fed, uni, fb, pooled, exact, words, shallow]);
    }
    let answerable: Vec<usize> = (0..questions.len()).filter(|&i| questions[i].relevant.values().any(|&g| g > 0)).collect();
    let score = |s: usize, f: fn(&[String], &Judgments, usize) -> f64, which: &dyn Fn(usize) -> bool| {
        metrics::mean(&answerable.iter().filter(|&&i| which(i)).map(|&i| f(&rankings[i][s], &questions[i].relevant, K)).collect::<Vec<_>>())
    };
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for &i in &answerable {
        *kinds.entry(questions[i].kind.clone()).or_default() += 1;
    }
    let systems: Vec<serde_json::Value> = (0..SYSTEMS.len())
        .map(|s| {
            let by_kind: BTreeMap<&String, f64> = kinds.keys().map(|k| (k, score(s, metrics::ndcg, &|i| &questions[i].kind == k))).collect();
            serde_json::json!({
                "system": SYSTEMS[s],
                "ndcg_at_10": score(s, metrics::ndcg, &|_| true),
                "recall_at_10": score(s, metrics::recall, &|_| true),
                "success_at_10": score(s, metrics::success, &|_| true),
                "ndcg_by_kind": by_kind,
            })
        })
        .collect();
    let ndcg = |s: usize| systems[s]["ndcg_at_10"].as_f64().unwrap_or(0.0);
    let passed = ndcg(0) >= GATE_SHARE * ndcg(1) && ndcg(0) >= ndcg(2);
    let mut sizes: Vec<usize> = fragments.iter().map(across::Index::chunks).collect();
    sizes.sort_unstable();
    let report = serde_json::json!({
        "model": embeddings.embedder.id(), "dimensions": dims, "window": window, "document_prefix": prefix,
        "pages": pages.len(), "chunks": union.chunks(), "wikis": fragments.len(), "wiki_chunks_sorted": sizes,
        "questions": questions.len(), "answerable": answerable.len(), "answerable_by_kind": kinds,
        "systems": systems,
        "gate": { "federated_over_union": ndcg(0) / ndcg(1).max(f64::MIN_POSITIVE), "bar": GATE_SHARE, "federated_minus_fbrain": ndcg(0) - ndcg(2), "passed": passed },
        "pooled_prototype": { "over_union": ndcg(3) / ndcg(1).max(f64::MIN_POSITIVE), "minus_fbrain": ndcg(3) - ndcg(2) },
    });
    // each question's top pages by system: names the corpus's pages, so it stays in the work directory
    let mut details = String::new();
    for (q, r) in questions.iter().zip(&rankings) {
        let pages = |ids: &[String]| ids.iter().map(|id| id.rsplit_once('#').map_or(id.as_str(), |(p, _)| p).to_string()).collect::<Vec<_>>();
        details.push_str(&serde_json::json!({ "id": q.id, "federated": pages(&r[0]), "union": pages(&r[1]), "fbrain": pages(&r[2]), "pooled": pages(&r[3]), "exact": pages(&r[4]), "fbrain_words": pages(&r[5]), "depth50": pages(&r[6]) }).to_string());
        details.push('\n');
    }
    std::fs::write(work.join("across-details.jsonl"), details).map_err(|e| e.to_string())?;
    if let Some(path) = args.get("report") {
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap()).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&report).unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The synthetic fixtures, end to end on hashed vectors: every system
    /// answers, every engine sees an edit and a delete at once, and the
    /// brain's every-word rule misses what any-word rules find.
    #[test]
    fn synthetic_fixtures_end_to_end() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/synthetic");
        let work = std::env::temp_dir().join(format!("search-bench-e2e-{}", std::process::id()));
        let args = Args(
            [
                ("corpus", root.join("corpus")),
                ("questions", root.join("questions.jsonl")),
                ("work", work.clone()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_string_lossy().into_owned()))
            .chain([("hashed-embeddings".to_owned(), "64".to_owned()), ("runs".to_owned(), "1".to_owned())])
            .collect(),
        );
        let report = run(&args).unwrap();
        assert_eq!((report.pages, report.questions, report.unanswerable), (18, 12, 1));
        assert_eq!(report.scores.len(), System::ALL.len());
        let recall = |name: &str| report.scores.iter().find(|s| s.system.starts_with(name)).unwrap().recall_at_10;
        assert!(recall("brain today") < recall("fbrain lexical"));
        assert!(recall("DO baseline hybrid") > 0.5 && recall("LanceDB hybrid") > 0.5);
        assert_eq!(report.overlap_at_50["int8 scan"], 1.0);
        for f in &report.freshness {
            assert!(f.lexical_visible && f.semantic_visible && f.lexical_gone && f.semantic_gone, "{}", f.engine);
        }
        assert!(report.render().contains("| LanceDB hybrid"));
        std::fs::remove_dir_all(&work).unwrap();
    }

    /// The quality gate on the synthetic fixtures, on hashed vectors: the
    /// corpus cut by its folders, every system scored, the verdict written;
    /// and one fragment holding the whole corpus answers, merged, nearly as
    /// the union index does: the same first hit, and eight of its ten. (The
    /// merge places a hit among the asked hits, `asked_limit`, not among
    /// every hit its legs found, so the order below the first may move.)
    #[test]
    fn across_gate_end_to_end() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/synthetic");
        let work = std::env::temp_dir().join(format!("search-bench-across-{}", std::process::id()));
        let report_path = work.join("report.json");
        std::fs::create_dir_all(&work).unwrap();
        let args = Args(
            [("corpus", root.join("corpus")), ("questions", root.join("questions.jsonl")), ("work", work.clone()), ("report", report_path.clone())]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_string_lossy().into_owned()))
                .chain([("hashed-embeddings".to_owned(), "64".to_owned()), ("document-prefix".to_owned(), "search_document: ".to_owned())])
                .collect(),
        );
        gate(&args).unwrap();
        let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!((report["pages"].as_u64(), report["wikis"].as_u64(), report["answerable"].as_u64()), (Some(18), Some(8), Some(11)));
        assert_eq!(report["systems"].as_array().map(Vec::len), Some(7));
        assert!(report["gate"]["passed"].is_boolean());

        let (pages, _) = load(&root.join("corpus")).unwrap();
        let mut embeddings = Embeddings::open(Embedder::Hashed { dims: 64 }, &work).unwrap();
        let cuts = across::cut(&pages, 8192, None);
        let chunks: Vec<&fragment_core::searchable::Chunk> = cuts.iter().flat_map(|c| c.chunks.iter().map(|(_, ch, _, _)| ch)).collect();
        let vectors = embeddings.documents(&chunks.iter().map(|c| c.input.clone()).collect::<Vec<_>>(), false).unwrap();
        let vector_of: HashMap<String, Vec<f32>> = chunks.iter().map(|c| c.hash.clone()).zip(vectors).collect();
        let all: Vec<&across::Cut> = cuts.iter().collect();
        let union = across::Index::build(&work.join("one-union.sqlite3"), &all, &vector_of, 64).unwrap();
        let one = [across::Index::build(&work.join("one-fragment.sqlite3"), &all, &vector_of, 64).unwrap()];
        for q in questions(&root.join("questions.jsonl"), &pages).unwrap() {
            let (v, _) = embeddings.query(&q.q).unwrap();
            let alone: Vec<String> = union.search(&q.q, &v, K).unwrap().into_iter().map(|h| h.section).collect();
            let merged = across::federated(&one, &q.q, &v, K).unwrap();
            assert_eq!(merged.first(), alone.first(), "{}", q.id);
            assert!(merged.iter().filter(|s| alone.contains(s)).count() >= 8, "{}: {merged:?} vs {alone:?}", q.id);
            // a fragment's words leg scored with its own statistics as pooled ones is its own: the OR query's BM25
            let own = across::Pooled::of(&one, &q.q).unwrap();
            let plain: Vec<String> = one[0].search(&q.q, &v, K).unwrap().into_iter().map(|h| h.section).collect();
            let pooled: Vec<String> = one[0].search_with(&q.q, &v, K, Some(&own)).unwrap().into_iter().map(|h| h.section).collect();
            assert_eq!(pooled, plain, "{}: pooled statistics of one fragment are its own", q.id);
        }
        std::fs::remove_dir_all(&work).unwrap();
    }

    #[test]
    fn questions_must_name_the_corpus_pages() {
        let dir = std::env::temp_dir().join(format!("search-bench-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pages = vec![(Page { path: "a.md".into(), text: String::new() }, Vec::new())];
        let check = |line: &str| {
            std::fs::write(dir.join("q.jsonl"), line).unwrap();
            questions(&dir.join("q.jsonl"), &pages)
        };
        assert!(check(r#"{"id":"1","q":"x","relevant":{"a.md":2}}"#).is_ok());
        assert!(check(r#"{"id":"1","q":"x","relevant":{"b.md":2}}"#).unwrap_err().contains("not a page"));
        assert!(check(r#"{"id":"1","q":"x","relevant":{"a.md":3}}"#).unwrap_err().contains("0, 1 or 2"));
        assert!(check("{\"id\":\"1\",\"q\":\"x\"}\n{\"id\":\"1\",\"q\":\"y\"}").unwrap_err().contains("twice"));
        assert!(check("").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
