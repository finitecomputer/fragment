//! LanceDB OSS, as the search service would hold a fragment's sections: one
//! table, BM25 over its text columns (title, headings, context, body, with
//! the brain's weights as boosts), an indexed vector column by cosine, and
//! its own hybrid query (both legs, fused by reciprocal rank, k = 60).

use crate::corpus::Section;
use futures::TryStreamExt;
use lancedb::arrow::arrow_array::{
    types::Float32Type, Array, FixedSizeListArray, RecordBatch, StringArray,
};
use lancedb::arrow::arrow_schema::{DataType, Field, Schema};
use lancedb::index::scalar::{FtsIndexBuilder, FullTextSearchQuery, MultiMatchQuery};
use lancedb::index::vector::IvfFlatIndexBuilder;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase, QueryExecutionOptions, Select};
use lancedb::table::OptimizeAction;
use lancedb::{DistanceType, Table};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TABLE: &str = "sections";
/// The text columns BM25 reads, and their boosts (the brain's weights:
/// title 4, heading 3, path and ancestry 2, body 1).
const TEXT: [(&str, f32); 4] = [("title", 4.0), ("heading", 3.0), ("context", 2.0), ("body", 1.0)];

pub struct Lance {
    rt: tokio::runtime::Runtime,
    dir: PathBuf,
    table: Table,
    dims: usize,
}

/// How long building took, by part.
pub struct Built {
    pub load: Duration,
    pub fts_index: Duration,
    pub vector_index: Duration,
}

fn err(e: impl std::fmt::Display) -> String {
    format!("lancedb: {e}")
}

fn schema(dims: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("heading", DataType::Utf8, false),
        Field::new("context", DataType::Utf8, false),
        Field::new("body", DataType::Utf8, false),
        Field::new("vector", DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dims as i32), false),
    ]))
}

fn batch(rows: &[(&Section, &[f32])], dims: usize) -> Result<RecordBatch, String> {
    let col = |f: &dyn Fn(&Section) -> String| Arc::new(StringArray::from_iter_values(rows.iter().map(|(s, _)| f(s)))) as Arc<dyn Array>;
    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        rows.iter().map(|(_, v)| Some(v.iter().map(|x| Some(*x)).collect::<Vec<_>>())),
        dims as i32,
    );
    RecordBatch::try_new(
        schema(dims),
        vec![
            col(&|s| s.id.clone()),
            col(&|s| s.path.clone()),
            col(&|s| s.title.clone()),
            col(&|s| s.heading.clone().unwrap_or_default()),
            col(&|s| format!("{} {}", s.path, s.ancestry.join(" "))),
            col(&|s| s.body.clone()),
            Arc::new(vectors),
        ],
    )
    .map_err(err)
}

fn ids(batches: Vec<RecordBatch>) -> Vec<String> {
    let mut out = Vec::new();
    for b in batches {
        let col = b.column_by_name("id").expect("id selected");
        let col = col.as_any().downcast_ref::<StringArray>().expect("id is text");
        out.extend(col.iter().map(|s| s.unwrap_or_default().to_owned()));
    }
    out
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

impl Lance {
    /// A table of `rows` in `dir`, indexed.
    pub fn create(dir: &Path, rows: &[(&Section, &[f32])]) -> Result<(Self, Built), String> {
        let _ = std::fs::remove_dir_all(dir);
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(err)?;
        let dims = rows.first().ok_or("no rows to index")?.1.len();
        let started = Instant::now();
        let table = rt.block_on(async {
            let db = lancedb::connect(dir.to_str().ok_or("a UTF-8 path")?).execute().await.map_err(err)?;
            let batches: Vec<RecordBatch> = rows.chunks(4096).map(|c| batch(c, dims)).collect::<Result<_, _>>()?;
            db.create_table(TABLE, batches).execute().await.map_err(err)
        })?;
        let load = started.elapsed();
        let started = Instant::now();
        rt.block_on(async {
            for (column, _) in TEXT {
                table.create_index(&[column], Index::FTS(FtsIndexBuilder::default())).execute().await.map_err(err)?;
            }
            Ok::<_, String>(())
        })?;
        let fts_index = started.elapsed();
        let started = Instant::now();
        rt.block_on(async {
            table
                .create_index(&["vector"], Index::IvfFlat(IvfFlatIndexBuilder::default().distance_type(DistanceType::Cosine)))
                .execute()
                .await
                .map_err(err)
        })?;
        let vector_index = started.elapsed();
        Ok((Lance { rt, dir: dir.to_owned(), table, dims }, Built { load, fts_index, vector_index }))
    }

    /// The table opened again from its files, as a service starting cold.
    pub fn reopen(&mut self) -> Result<(), String> {
        self.table = self.rt.block_on(async {
            let db = lancedb::connect(self.dir.to_str().unwrap()).execute().await.map_err(err)?;
            db.open_table(TABLE).execute().await.map_err(err)
        })?;
        Ok(())
    }

    fn fts_query(q: &str) -> Result<FullTextSearchQuery, String> {
        let columns = TEXT.iter().map(|(c, _)| c.to_string()).collect();
        let boosts = TEXT.iter().map(|(_, b)| *b).collect();
        let multi = MultiMatchQuery::try_new(q.to_owned(), columns).and_then(|m| m.try_with_boosts(boosts)).map_err(err)?;
        Ok(FullTextSearchQuery::new_query(multi.into()))
    }

    pub fn fts(&self, q: &str, limit: usize) -> Result<Vec<String>, String> {
        let query = Self::fts_query(q)?;
        self.rt.block_on(async {
            let s = self.table.query().full_text_search(query).select(Select::columns(&["id"])).limit(limit).execute().await.map_err(err)?;
            Ok(ids(s.try_collect().await.map_err(err)?))
        })
    }

    pub fn nearest(&self, v: &[f32], limit: usize) -> Result<Vec<String>, String> {
        self.rt.block_on(async {
            let s = self
                .table
                .query()
                .nearest_to(v)
                .map_err(err)?
                .distance_type(DistanceType::Cosine)
                .select(Select::columns(&["id"]))
                .limit(limit)
                .execute()
                .await
                .map_err(err)?;
            Ok(ids(s.try_collect().await.map_err(err)?))
        })
    }

    /// LanceDB's own hybrid query: both legs `limit` deep, fused by RRF.
    pub fn hybrid(&self, q: &str, v: &[f32], limit: usize) -> Result<Vec<String>, String> {
        let query = Self::fts_query(q)?;
        self.rt.block_on(async {
            let s = self
                .table
                .query()
                .full_text_search(query)
                .nearest_to(v)
                .map_err(err)?
                .distance_type(DistanceType::Cosine)
                .select(Select::columns(&["id"]))
                .limit(limit)
                .execute_hybrid(QueryExecutionOptions::default())
                .await
                .map_err(err)?;
            Ok(ids(s.try_collect().await.map_err(err)?))
        })
    }

    /// A page's sections replaced: its rows deleted, its new ones added.
    pub fn put_page(&self, path: &str, rows: &[(&Section, &[f32])]) -> Result<(), String> {
        let b = batch(rows, self.dims)?;
        self.rt.block_on(async {
            self.table.delete(format!("path = {}", quote(path)).as_str()).await.map_err(err)?;
            self.table.add(b).execute().await.map_err(err)?;
            Ok(())
        })
    }

    pub fn delete_page(&self, path: &str) -> Result<(), String> {
        self.rt.block_on(async { self.table.delete(format!("path = {}", quote(path)).as_str()).await.map(|_| ()).map_err(err) })
    }

    /// Indexes brought up to the table (new rows into them, deleted out).
    pub fn optimize(&self) -> Result<(), String> {
        self.rt.block_on(async { self.table.optimize(OptimizeAction::All).await.map(|_| ()).map_err(err) })
    }
}
