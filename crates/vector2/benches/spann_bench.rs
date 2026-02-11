//! Benchmarks for the SPANN vector index.
//!
//! Measures:
//! - Segment build time at various scales
//! - Search latency (flat vs clustered)
//! - Cosine similarity (SIMD vs scalar baseline)
//! - Recall at various nprobe values

use std::collections::{
    BTreeMap,
    HashSet,
};

use criterion::{
    criterion_group,
    criterion_main,
    measurement::WallTime,
    BenchmarkGroup,
    BenchmarkId,
    Criterion,
};
use rand::Rng;
use value::{
    FieldPath,
    InternalId,
};
use vector::{
    spann::{
        cosine,
        segment::{
            SpannSegment,
            DEFAULT_NPROBE,
        },
    },
    IndexedVector,
};

fn make_id(n: u128) -> InternalId {
    InternalId(n.to_le_bytes())
}

fn random_vector(rng: &mut impl Rng, dim: usize) -> Vec<f32> {
    (0..dim).map(|_| rng.random::<f32>()).collect()
}

fn random_docs(
    rng: &mut impl Rng,
    n: usize,
    dim: usize,
) -> Vec<(InternalId, IndexedVector, BTreeMap<FieldPath, Vec<u8>>, u64)> {
    (0..n)
        .map(|i| {
            (
                make_id(i as u128),
                IndexedVector::try_from(random_vector(rng, dim)).unwrap(),
                BTreeMap::new(),
                i as u64,
            )
        })
        .collect()
}

fn brute_force_topk(raw_vectors: &[Vec<f32>], query: &[f32], k: usize) -> Vec<(usize, f32)> {
    let q = cosine::preprocess(query.to_vec());
    let mut results: Vec<(usize, f32)> = raw_vectors
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let v_norm = cosine::preprocess(v.clone());
            (i, cosine::cosine_similarity(&q, &v_norm))
        })
        .collect();
    results.sort_by(|a, b| b.1.total_cmp(&a.1));
    results.truncate(k);
    results
}

// ============================================================================
// Cosine similarity benchmarks
// ============================================================================

fn bench_cosine(c: &mut Criterion) {
    let mut group = c.benchmark_group("cosine_similarity");
    let mut rng = rand::rng();

    for dim in [128, 256, 768, 1536] {
        let a = cosine::preprocess(random_vector(&mut rng, dim));
        let b = cosine::preprocess(random_vector(&mut rng, dim));

        group.bench_with_input(BenchmarkId::new("dot_product", dim), &dim, |bench, _| {
            bench.iter(|| cosine::dot_product(&a, &b));
        });
    }
    group.finish();
}

// ============================================================================
// Segment build benchmarks
// ============================================================================

fn bench_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("segment_build");
    group.sample_size(10);
    let mut rng = rand::rng();

    for (n, dim) in [(100, 128), (500, 128), (1000, 128), (500, 1536)] {
        let docs = random_docs(&mut rng, n, dim);

        group.bench_with_input(
            BenchmarkId::new(format!("n{n}_d{dim}"), n),
            &n,
            |bench, _| {
                bench.iter(|| SpannSegment::build(docs.clone(), dim));
            },
        );
    }
    group.finish();
}

// ============================================================================
// Search benchmarks
// ============================================================================

fn bench_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("segment_search");
    let mut rng = rand::rng();

    for (n, dim) in [(100, 128), (500, 128), (1000, 128), (500, 1536)] {
        let docs = random_docs(&mut rng, n, dim);
        let segment = SpannSegment::build(docs, dim);
        let query = random_vector(&mut rng, dim);

        group.bench_with_input(
            BenchmarkId::new(format!("n{n}_d{dim}_k10"), n),
            &n,
            |bench, _| {
                bench.iter(|| segment.search(&query, &BTreeMap::new(), 10));
            },
        );
    }
    group.finish();
}

// ============================================================================
// Recall measurement (not a benchmark, but prints recall table)
// ============================================================================

fn bench_recall(c: &mut Criterion) {
    let mut group = c.benchmark_group("recall_measurement");
    group.sample_size(10);
    let mut rng = rand::rng();

    let dim = 128;
    let n = 2000;
    let k = 10;
    let num_queries = 50;

    let raw_vectors: Vec<Vec<f32>> = (0..n).map(|_| random_vector(&mut rng, dim)).collect();
    let docs: Vec<_> = raw_vectors
        .iter()
        .enumerate()
        .map(|(i, v)| {
            (
                make_id(i as u128),
                IndexedVector::try_from(v.clone()).unwrap(),
                BTreeMap::new(),
                i as u64,
            )
        })
        .collect();

    let segment = SpannSegment::build(docs, dim);
    let queries: Vec<Vec<f32>> = (0..num_queries)
        .map(|_| random_vector(&mut rng, dim))
        .collect();

    // Measure recall at various nprobe values
    for nprobe in [1, 2, 4, 8, 16, 32, 64] {
        let nprobe_actual = nprobe.min(segment.num_centroids);
        let mut total_recall = 0.0f32;

        for query in &queries {
            let ground_truth = brute_force_topk(&raw_vectors, query, k);
            let approx = segment.search_with_nprobe(query, &BTreeMap::new(), k, nprobe_actual);

            let true_set: HashSet<usize> = ground_truth.iter().map(|(i, _)| *i).collect();
            let approx_set: HashSet<usize> = approx.iter().map(|(i, ..)| *i as usize).collect();
            let recall = true_set.intersection(&approx_set).count() as f32 / k as f32;
            total_recall += recall;
        }

        let avg_recall = total_recall / num_queries as f32;
        eprintln!(
            "n={n}, dim={dim}, k={k}, nprobe={nprobe_actual}, centroids={}, \
             recall@{k}={avg_recall:.3}",
            segment.num_centroids
        );

        // Benchmark the search at this nprobe
        group.bench_with_input(
            BenchmarkId::new(format!("nprobe{nprobe}"), nprobe),
            &nprobe,
            |bench, _| {
                let query = &queries[0];
                bench
                    .iter(|| segment.search_with_nprobe(query, &BTreeMap::new(), k, nprobe_actual));
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_cosine,
    bench_build,
    bench_search,
    bench_recall
);
criterion_main!(benches);
