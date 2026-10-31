//! Embeddings: from an OpenAI-compatible `/v1/embeddings` on this machine
//! (the shape of the model route's planned embedding slot), or hashed words
//! for the synthetic fixtures' tests. Vectors are cached by model and input,
//! so every engine gets the same ones and a second run embeds nothing.

use crate::corpus::{sha256_hex, words};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use std::path::Path;
use std::time::{Duration, Instant};

/// Inputs one request carries.
const BATCH: usize = 32;
/// A request's wait, at most: a CPU-bound model takes a while for a batch.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

pub enum Embedder {
    /// An OpenAI-compatible endpoint, which must be this machine's own.
    Http { url: url::Url, model: String, query_prefix: String },
    /// Words hashed into `dims` signed buckets: no model, for tests only.
    Hashed { dims: usize },
}

impl Embedder {
    /// An endpoint on this machine, or an error: a private corpus never
    /// leaves it to be embedded. The URL is parsed once, by the parser the
    /// HTTP client uses, and that parse is both checked and called, so the
    /// host checked is the host sent to.
    pub fn http(url: &str, model: &str, query_prefix: &str) -> Result<Self, String> {
        let parsed = url::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
        if parsed.scheme() != "http" {
            return Err(format!("{url}: an embeddings URL is plain http to this machine"));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(format!("{url}: an embeddings URL carries no user-info"));
        }
        let local = match parsed.host() {
            Some(url::Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
            Some(url::Host::Ipv6(ip)) => ip == std::net::Ipv6Addr::LOCALHOST,
            Some(url::Host::Domain(name)) => name == "localhost",
            None => false,
        };
        if !local {
            return Err(format!("{url}: embeddings come from a model on this machine (127.0.0.1, localhost or [::1]) only"));
        }
        Ok(Embedder::Http { url: parsed, model: model.to_owned(), query_prefix: query_prefix.to_owned() })
    }

    pub fn id(&self) -> String {
        match self {
            Embedder::Http { model, .. } => model.clone(),
            Embedder::Hashed { dims } => format!("hashed-{dims}"),
        }
    }

    /// `inputs`' vectors, unit length, and the tokens the model counted.
    fn embed(&self, inputs: &[String]) -> Result<(Vec<Vec<f32>>, u64), String> {
        match self {
            Embedder::Hashed { dims } => Ok((inputs.iter().map(|t| hashed(t, *dims)).collect(), 0)),
            Embedder::Http { url, model, .. } => {
                #[derive(Deserialize)]
                struct Item {
                    index: usize,
                    embedding: Vec<f32>,
                }
                #[derive(Deserialize)]
                struct Usage {
                    prompt_tokens: u64,
                }
                #[derive(Deserialize)]
                struct Response {
                    data: Vec<Item>,
                    usage: Option<Usage>,
                }
                let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
                let response: Response = agent
                    .request_url("POST", url)
                    .send_json(serde_json::json!({ "model": model, "input": inputs }))
                    .map_err(|e| format!("{url}: {e}"))?
                    .into_json()
                    .map_err(|e| format!("{url}: {e}"))?;
                if response.data.len() != inputs.len() {
                    return Err(format!("{url}: {} vectors for {} inputs", response.data.len(), inputs.len()));
                }
                let mut out = vec![Vec::new(); inputs.len()];
                for item in response.data {
                    let slot = out.get_mut(item.index).ok_or_else(|| format!("{url}: vector index {} out of range", item.index))?;
                    *slot = unit(item.embedding);
                }
                Ok((out, response.usage.map_or(0, |u| u.prompt_tokens)))
            }
        }
    }
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

fn hashed(text: &str, dims: usize) -> Vec<f32> {
    let mut v = vec![0f32; dims];
    for w in words(text) {
        let h = sha256_hex(w.as_bytes());
        let n = u64::from_str_radix(&h[..16], 16).unwrap();
        v[(n % dims as u64) as usize] += if n >> 63 == 0 { 1.0 } else { -1.0 };
    }
    unit(v)
}

/// An embedder with its cache, and what it has counted.
pub struct Embeddings {
    pub embedder: Embedder,
    cache: Connection,
    pub dims: usize,
    /// Tokens of every document input the corpus holds (cached ones too).
    pub document_tokens: u64,
}

impl Embeddings {
    pub fn open(embedder: Embedder, work: &Path) -> Result<Self, String> {
        let cache = Connection::open(work.join("embeddings.sqlite3")).map_err(|e| e.to_string())?;
        cache
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS vectors (model TEXT NOT NULL, input TEXT NOT NULL, tokens INTEGER NOT NULL,
                 vector BLOB NOT NULL, PRIMARY KEY (model, input))",
            )
            .map_err(|e| e.to_string())?;
        Ok(Embeddings { embedder, cache, dims: 0, document_tokens: 0 })
    }

    /// `inputs`' vectors, from the cache or the model.
    pub fn documents(&mut self, inputs: &[String], progress: bool) -> Result<Vec<Vec<f32>>, String> {
        let model = self.embedder.id();
        let keys: Vec<String> = inputs.iter().map(|t| sha256_hex(t.as_bytes())).collect();
        let mut out: Vec<Option<Vec<f32>>> = Vec::with_capacity(inputs.len());
        {
            let mut get = self.cache.prepare("SELECT tokens, vector FROM vectors WHERE model = ? AND input = ?").map_err(|e| e.to_string())?;
            for key in &keys {
                let hit = get
                    .query_row(params![model, key], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
                    .optional()
                    .map_err(|e| e.to_string())?;
                out.push(hit.map(|(tokens, blob)| {
                    self.document_tokens += tokens as u64;
                    from_blob(&blob)
                }));
            }
        }
        let missing: Vec<usize> = (0..inputs.len()).filter(|&i| out[i].is_none()).collect();
        let started = Instant::now();
        for (n, batch) in missing.chunks(BATCH).enumerate() {
            let texts: Vec<String> = batch.iter().map(|&i| inputs[i].clone()).collect();
            let (vectors, tokens) = self.embedder.embed(&texts)?;
            // a batch's tokens, shared out by length
            let chars: usize = texts.iter().map(|t| t.chars().count().max(1)).sum();
            let tx = self.cache.transaction().map_err(|e| e.to_string())?;
            for ((&i, text), v) in batch.iter().zip(&texts).zip(vectors) {
                let share = (tokens as f64 * text.chars().count().max(1) as f64 / chars as f64).round() as i64;
                self.document_tokens += share as u64;
                tx.execute("INSERT OR REPLACE INTO vectors VALUES (?, ?, ?, ?)", params![model, keys[i], share, to_blob(&v)])
                    .map_err(|e| e.to_string())?;
                out[i] = Some(v);
            }
            tx.commit().map_err(|e| e.to_string())?;
            if progress && (n % 10 == 9 || (n + 1) * BATCH >= missing.len()) {
                let done = ((n + 1) * BATCH).min(missing.len());
                eprintln!("  embedded {done}/{} new sections ({:.0}s)", missing.len(), started.elapsed().as_secs_f64());
            }
        }
        let out: Vec<Vec<f32>> = out.into_iter().map(|v| v.expect("every input embedded")).collect();
        if let Some(first) = out.first() {
            self.dims = first.len();
            assert!(out.iter().all(|v| v.len() == self.dims), "one model, one dimension");
        }
        Ok(out)
    }

    /// A question's vector, never cached, and how long the model took.
    pub fn query(&self, q: &str) -> Result<(Vec<f32>, Duration), String> {
        let text = match &self.embedder {
            Embedder::Http { query_prefix, .. } => format!("{query_prefix}{q}"),
            Embedder::Hashed { .. } => q.to_owned(),
        };
        let started = Instant::now();
        let (mut v, _) = self.embedder.embed(&[text])?;
        Ok((v.pop().expect("one vector"), started.elapsed()))
    }
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_machine_embeds() {
        for local in [
            "http://127.0.0.1:18091/v1/embeddings",
            "http://localhost:8080/v1/embeddings",
            "http://[::1]:8080/v1/embeddings",
            "HTTP://LOCALHOST:8080/v1/embeddings",
            "http://127.1:8080/v1/embeddings",
        ] {
            assert!(Embedder::http(local, "m", "").is_ok(), "{local}");
        }
        for remote in [
            "https://api.example.com/v1/embeddings",
            "http://10.0.0.5:8080/v1/embeddings",
            "http://127.0.0.1.evil.example/v1",
            "http://localhost.evil.example/v1",
            // User-info: the authority's host is the part after '@'.
            "http://127.0.0.1:80@remote.example/v1/embeddings",
            "http://127.0.0.1@remote.example/v1/embeddings",
            "http://[::1]:80@remote.example/v1/embeddings",
            "http://localhost:x@remote.example/v1/embeddings",
            // User-info on a local host is refused too: nothing to authenticate.
            "http://user:pass@127.0.0.1:8080/v1/embeddings",
            "http://remote.example#@127.0.0.1/v1/embeddings",
            "http://remote.example?@127.0.0.1/v1/embeddings",
            "file:///v1/embeddings",
            "127.0.0.1:8080/v1/embeddings",
        ] {
            assert!(Embedder::http(remote, "m", "").is_err(), "{remote}");
        }
    }

    /// The request goes to the address the check passed, not to a second
    /// reading of the same string.
    #[test]
    fn the_checked_address_is_the_one_called() {
        let Ok(Embedder::Http { url, .. }) = Embedder::http("http://127.1:18091/v1/embeddings", "m", "") else { panic!() };
        assert_eq!(url.as_str(), "http://127.0.0.1:18091/v1/embeddings");
    }

    #[test]
    fn hashed_vectors_are_unit_and_stable() {
        let a = hashed("release rollback plan", 64);
        assert_eq!(a, hashed("release rollback plan", 64));
        assert!((a.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-5);
    }
}
