# Decision 0009: Vector Database Options for Idiolect

**Status:** Research  
**Date:** 2026-06-06  
**Author:** Research Agent

## Context

Idiolect is a Rust-first speech-to-text correction and learning system. The project currently uses SQLite for metadata storage (sessions, training candidates, adapters, correction memory, history). As the system grows, there's a need for vector similarity search to:

1. **Store improvements** - Embed corrected transcripts and their corrections for similarity-based retrieval
2. **Cluster common issues** - Group similar transcription errors to identify patterns
3. **Semantic search** - Find related transcripts by meaning, not just exact text match
4. **RAG for post-processing** - Retrieve relevant corrections to improve ASR output

## Requirements

Based on the project architecture (ADR 0001 - Rust-First V1 Architecture):

| Requirement | Priority | Notes |
|-------------|----------|-------|
| **Pure Rust / minimal FFI** | Critical | Core crates never expose backend-specific types |
| **Embedded / in-process** | High | No external service dependencies for v1 |
| **SQLite integration** | High | Already using SQLite; prefer extending it |
| **HNSW indexing** | High | Standard for vector similarity search |
| **Metadata filtering** | High | Filter by user_id, session, date, etc. |
| **Quantization support** | Medium | Reduce memory for large datasets |
| **Async support** | Medium | Tokio-based async runtime |
| **License compatibility** | Critical | AGPL-3.0-only project |

## Candidate Options

### 1. sqlite-vector-rs (sqlite-vector)

**Crate:** `sqlite-vector-rs` v0.2.2  
**License:** MIT OR Apache-2.0  
**Repository:** https://github.com/quinnjr/sqlite-vector-rs

| Aspect | Assessment |
|--------|------------|
| **Architecture fit** | ✅ Excellent - Native SQLite extension, loads into existing SQLite connection |
| **Rust purity** | ✅ Pure Rust (loadable extension) |
| **Embedded** | ✅ Runs in-process with SQLite |
| **HNSW** | ✅ Native HNSW indexing |
| **Metadata filtering** | ✅ SQL WHERE clauses on regular columns + vector search |
| **Quantization** | ❌ Not yet (planned) |
| **Async** | ⚠️ Sync only (rusqlite is sync) |
| **Maturity** | ⚠️ Early (0.2.x) |
| **Dependencies** | Minimal - just rusqlite |

**Pros:**
- Zero external dependencies - extends existing SQLite
- Single database file for all data (metadata + vectors)
- ACID transactions across vector + relational data
- Familiar SQL interface
- Can use existing `rusqlite` connection pool

**Cons:**
- Early stage (0.2.x)
- No quantization yet (memory grows with vectors)
- Sync-only (blocks Tokio runtime)
- Limited ecosystem/tooling

**Integration approach:**
```rust
// In idiolect-adapter-sqlite
// Load extension: connection.load_extension("sqlite_vector")?
// Create vector table: CREATE VIRTUAL TABLE vec_items USING vec0(embedding float[384]);
// Insert: INSERT INTO vec_items(rowid, embedding) VALUES (?, ?);
// Search: SELECT rowid, distance FROM vec_items WHERE embedding MATCH ? AND k = 10;
```

---

### 2. embedvec

**Crate:** `embedvec` v0.8.0  
**License:** MIT  
**Repository:** https://github.com/WeaveITMeta/embedvec

| Aspect | Assessment |
|--------|------------|
| **Architecture fit** | ✅ Good - In-process, trait-based abstraction |
| **Rust purity** | ✅ Pure Rust |
| **Embedded** | ✅ In-process |
| **HNSW** | ✅ Native HNSW with quantization |
| **Metadata filtering** | ✅ Filter by metadata during search |
| **Quantization** | ✅ E8/H4 lattice (up to 24.8x compression) |
| **Async** | ✅ Tokio support (feature `async`) |
| **Maturity** | ✅ 0.8.x, active development |
| **Persistence** | ✅ Multiple backends (fjall, rocksdb, sled, pgvector) |
| **Dependencies** | Moderate (fjall, hnsw, etc.) |

**Pros:**
- Purpose-built for vector search with quantization
- Async support with Tokio
- Multiple persistence backends
- Metadata filtering built-in
- Python bindings available (PyO3)

**Cons:**
- Separate storage from SQLite (dual persistence)
- Additional dependency (fjall/rocksdb/sled)
- New crate to maintain
- Not SQL-native

**Integration approach:**
```rust
// New crate: idiolect-adapter-embedvec
// Implement VectorStorePort trait in idiolect-ports
// Use fjall for persistence (embedded, Rust-native)
// Store vectors alongside metadata IDs for joins
```

---

### 3. qdrant-edge

**Crate:** `qdrant-edge` v0.7.2  
**License:** Apache-2.0  
**Repository:** https://github.com/qdrant/qdrant

| Aspect | Assessment |
|--------|------------|
| **Architecture fit** | ✅ Good - In-process embedded engine |
| **Rust purity** | ✅ Pure Rust |
| **Embedded** | ✅ Designed for embedded/mobile |
| **HNSW** | ✅ Full HNSW implementation |
| **Metadata filtering** | ✅ Payload filtering |
| **Quantization** | ✅ Scalar, product quantization |
| **Async** | ✅ Async-first |
| **Maturity** | ⚠️ 0.7.x, early embedded variant |
| **Dependencies** | Moderate |

**Pros:**
- Same API as Qdrant server (easy migration to server later)
- Rich filtering and payload support
- Quantization built-in
- Actively maintained by Qdrant team

**Cons:**
- Separate storage engine (not SQLite)
- Early stage for embedded variant
- Larger binary size
- Different data model (collections vs tables)

---

### 4. qdrant-client (with external Qdrant server)

**Crate:** `qdrant-client` v1.18.0  
**License:** Apache-2.0

| Aspect | Assessment |
|--------|------------|
| **Architecture fit** | ❌ Poor - Requires external service |
| **Embedded** | ❌ No - client/server |
| **Maturity** | ✅ 1.x, production-ready |
| **Features** | ✅ Full Qdrant feature set |

**Verdict:** Not suitable for v1 (violates embedded/offline-first principle). Could be v2+ option for multi-user/server deployment.

---

### 5. velesdb-core

**Crate:** `velesdb-core` v1.16.0  
**License:** Unknown (check)  
**Repository:** https://github.com/cyberlife-coder/velesdb

| Aspect | Assessment |
|--------|------------|
| **Architecture fit** | ⚠️ Separate engine |
| **Rust purity** | ✅ Pure Rust |
| **Embedded** | ✅ In-process |
| **HNSW** | ✅ |
| **Quantization** | ⚠️ GPU features suggest advanced |
| **Maturity** | ⚠️ 1.x but less known |
| **License** | ❓ Unknown - need to verify |

**Verdict:** Need license verification. Less ecosystem adoption.

---

### 6. seekstorm

**Crate:** `seekstorm` v3.2.1  
**License:** Apache-2.0  
**Repository:** https://github.com/SeekStorm/SeekStorm

| Aspect | Assessment |
|--------|------------|
| **Architecture fit** | ⚠️ Full search engine, not just vectors |
| **Embedded** | ✅ Library + server |
| **Maturity** | ✅ 3.x |
| **Scope** | Too broad (lexical + vector) |

**Verdict:** Overkill for vector-only needs.

---

## Comparison Matrix

| Criteria | sqlite-vector-rs | embedvec | qdrant-edge | qdrant-client (server) |
|----------|------------------|----------|-------------|------------------------|
| **SQLite integration** | ⭐⭐⭐ Native | ⭐ Separate | ⭐ Separate | ⭐ Separate |
| **Zero external deps** | ⭐⭐⭐ Yes | ⭐⭐ fjall/rocksdb | ⭐⭐ Yes | ❌ Server |
| **Async/tokio** | ❌ Sync only | ⭐⭐⭐ Yes | ⭐⭐⭐ Yes | ⭐⭐⭐ Yes |
| **Quantization** | ❌ Planned | ⭐⭐⭐ E8/H4 | ⭐⭐⭐ Yes | ⭐⭐⭐ Yes |
| **Metadata filtering** | ⭐⭐ SQL WHERE | ⭐⭐⭐ Built-in | ⭐⭐⭐ Payload | ⭐⭐⭐ Payload |
| **Maturity** | ⭐ 0.2.x | ⭐⭐ 0.8.x | ⭐ 0.7.x | ⭐⭐⭐ 1.18.x |
| **License** | ✅ MIT/Apache | ✅ MIT | ✅ Apache-2.0 | ✅ Apache-2.0 |
| **Binary size** | ⭐⭐⭐ Minimal | ⭐⭐ Moderate | ⭐⭐ Moderate | ⭐ Client only |
| **Migration path** | ⭐⭐⭐ Same DB | ⭐⭐ New crate | ⭐⭐ New crate | ⭐⭐⭐ Server later |

---

## Recommendation

### Primary: **sqlite-vector-rs** (for v1)

**Rationale:**
1. **Architectural alignment** - Extends existing SQLite, no new storage engine
2. **Simplicity** - Single database file, ACID across vectors + metadata
3. **Rust-first** - Pure Rust loadable extension
4. **Zero infrastructure** - No additional processes or files
5. **Familiar interface** - SQL queries with vector extensions

**Implementation Plan:**
1. Add `sqlite-vector-rs` to `idiolect-adapter-sqlite` dependencies
2. Create migration `0005_vector_embeddings.sql` with virtual table
3. Add `VectorStorePort` trait to `idiolect-ports/src/storage.rs`
4. Implement in `idiolect-adapter-sqlite`
5. Use 384-dim embeddings (compatible with `all-MiniLM-L6-v2` or similar)

**Migration sketch:**
```sql
-- 0005_vector_embeddings.sql
SELECT vec_load_extension();  -- Load the extension

CREATE VIRTUAL TABLE training_candidate_embeddings USING vec0(
    embedding float[384],
    +training_candidate_id INTEGER,  -- metadata column for filtering
    +user_id TEXT,
    +created_at TEXT
);

-- Trigger to auto-populate from training_candidates
CREATE TRIGGER training_candidate_embedding_insert
AFTER INSERT ON training_candidates
BEGIN
    -- Embedding generated externally, inserted via application
END;
```

### Secondary: **embedvec** (if sqlite-vector-rs proves insufficient)

**When to consider:**
- Quantization becomes critical (large embedding volumes)
- Async vector operations needed without blocking Tokio
- More advanced filtering/ranking required

**Migration path:** Add `idiolect-adapter-embedvec` crate implementing same `VectorStorePort` trait.

---

## Vector Store Port Design

Add to `idiolect-ports/src/storage.rs`:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VectorSearchResult {
    pub id: i64,           -- training_candidate_id or custom
    pub score: f32,        -- cosine similarity (0.0-1.0) or distance
    pub metadata: serde_json::Value,  -- flexible metadata
}

#[derive(Clone, Debug)]
pub struct VectorInsert {
    pub id: i64,
    pub embedding: Vec<f32>,  -- 384 dimensions
    pub metadata: serde_json::Value,
}

pub trait VectorStorePort {
    type Error;

    fn insert(&mut self, vectors: Vec<VectorInsert>) -> Result<(), Self::Error>;
    fn search(
        &self,
        query_embedding: &[f32],
        limit: u32,
        filter: Option<serde_json::Value>,  // metadata filter
    ) -> Result<Vec<VectorSearchResult>, Self::Error>;
    fn delete(&mut self, ids: &[i64]) -> Result<(), Self::Error>;
    fn count(&self) -> Result<u64, Self::Error>;
}
```

---

## Embedding Model Selection

For local, privacy-preserving embeddings:

| Model | Dimensions | Size | Quality | License |
|-------|------------|------|---------|---------|
| `all-MiniLM-L6-v2` | 384 | 90MB | Good | Apache-2.0 |
| `all-MiniLM-L12-v2` | 384 | 120MB | Better | Apache-2.0 |
| `bge-small-en-v1.5` | 384 | 130MB | Excellent | MIT |
| `e5-small-v2` | 384 | 130MB | Excellent | MIT |

**Recommendation:** `all-MiniLM-L6-v2` (384-dim, fast, good quality, small)

**Integration:** Use `candle` or `ort` (ONNX Runtime) for inference in a new `idiolect-adapter-embedding` crate.

---

## Next Steps

1. **Prototype** - Add sqlite-vector-rs to a test branch, verify compilation and basic operations
2. **Benchmark** - Measure insert/search latency with 10k/100k/1M vectors
3. **Memory test** - Verify memory usage with and without quantization
4. **Integration test** - Full round-trip: correction → embedding → search → retrieval
5. **Decision** - Accept or pivot to embedvec based on results

---

## References

- [sqlite-vector-rs docs](https://docs.rs/sqlite-vector-rs)
- [embedvec docs](https://docs.rs/embedvec)
- [qdrant-edge docs](https://docs.rs/qdrant-edge)
- [Sentence Transformers models](https://huggingface.co/sentence-transformers)
- [ADR 0001: Rust-First V1 Architecture](./0001-rust-first-v1-architecture.md)
- [ADR 0002: SQLite Storage Adapter](./0002-sqlite-storage-adapter.md)
- [Future: Transcript Search](./../future/004-transcript-search.md)
- [Future: AI Post-Processing](./../future/008-ai-post-processing.md)