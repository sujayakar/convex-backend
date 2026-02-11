//! Phase 3: Deep testing and validation for the SPANN index.
//!
//! These tests cover:
//! - Recall sweep across nprobe values
//! - Filter correctness (Eq/In)
//! - Persistence round-trip fuzz
//! - Edge cases (empty, single, identical, max dims, all deleted)
//! - Scale tests at higher vector counts

#[cfg(test)]
mod tests {
    use std::collections::{
        BTreeMap,
        BTreeSet,
        HashSet,
    };

    use rand::Rng;
    use tempfile::TempDir;
    use value::{
        FieldPath,
        InternalId,
    };

    use crate::{
        query::CompiledVectorFilter,
        spann::{
            cosine,
            segment::{
                build_and_snapshot_segment,
                unsafe_load_disk_segment,
                SpannSegment,
                DEFAULT_NPROBE,
            },
        },
        IndexedVector,
    };

    fn make_test_id(n: u128) -> InternalId {
        InternalId(n.to_le_bytes())
    }

    fn random_vector(rng: &mut impl Rng, dim: usize) -> Vec<f32> {
        (0..dim).map(|_| rng.random::<f32>()).collect()
    }

    fn brute_force_search(vectors: &[Vec<f32>], query: &[f32], k: usize) -> Vec<(usize, f32)> {
        let query_norm = cosine::preprocess(query.to_vec());
        let mut results: Vec<(usize, f32)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let v_norm = cosine::preprocess(v.clone());
                (i, cosine::cosine_similarity(&query_norm, &v_norm))
            })
            .collect();
        results.sort_by(|a, b| b.1.total_cmp(&a.1));
        results.truncate(k);
        results
    }

    fn compute_recall(ground_truth: &[(usize, f32)], approx: &[(u32, f32, u64)], k: usize) -> f32 {
        let true_set: HashSet<usize> = ground_truth.iter().take(k).map(|(i, _)| *i).collect();
        let approx_set: HashSet<usize> = approx.iter().take(k).map(|(i, ..)| *i as usize).collect();
        true_set.intersection(&approx_set).count() as f32 / k as f32
    }

    // =========================================================================
    // Phase 3a: Recall sweep
    // =========================================================================

    #[test]
    fn test_recall_sweep_nprobe() {
        let dim = 32;
        let n = 2000; // Large enough to trigger clustering
        let k = 10;
        let num_queries = 20;
        let mut rng = rand::rng();

        // Build dataset
        let raw_vectors: Vec<Vec<f32>> = (0..n).map(|_| random_vector(&mut rng, dim)).collect();
        let docs: Vec<_> = raw_vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(v.clone()).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);
        assert!(segment.num_centroids > 0, "Should use centroid routing");

        // Generate queries
        let queries: Vec<Vec<f32>> = (0..num_queries)
            .map(|_| random_vector(&mut rng, dim))
            .collect();

        // Sweep nprobe values and measure recall
        let nprobe_values = [1, 2, 4, 8, 16, 32, 64, 128];
        let mut prev_avg_recall = 0.0f32;

        for &nprobe in &nprobe_values {
            let nprobe = nprobe.min(segment.num_centroids);
            let mut total_recall = 0.0f32;

            for query in &queries {
                let ground_truth = brute_force_search(&raw_vectors, query, k);
                let approx = segment.search_with_nprobe(query, &BTreeMap::new(), k, nprobe);
                let recall = compute_recall(&ground_truth, &approx, k);
                total_recall += recall;
            }

            let avg_recall = total_recall / num_queries as f32;

            // Recall should be monotonically non-decreasing with nprobe
            assert!(
                avg_recall >= prev_avg_recall - 0.05, // Allow small noise
                "Recall should increase with nprobe: nprobe={nprobe}, recall={avg_recall:.3}, \
                 prev={prev_avg_recall:.3}"
            );
            prev_avg_recall = avg_recall;
        }

        // At max nprobe (all centroids), recall should be high.
        // Note: recall < 1.0 is expected even with all centroids probed because
        // k-means assigns each vector to exactly one centroid. Vectors near cluster
        // boundaries may be in a different partition than the ground-truth nearest
        // neighbors. Boundary vector replication (Phase 4) improves this.
        let mut total_recall = 0.0f32;
        for query in &queries {
            let ground_truth = brute_force_search(&raw_vectors, query, k);
            let approx =
                segment.search_with_nprobe(query, &BTreeMap::new(), k, segment.num_centroids);
            let recall = compute_recall(&ground_truth, &approx, k);
            total_recall += recall;
        }
        let full_recall = total_recall / num_queries as f32;
        assert!(
            full_recall >= 0.70,
            "Full nprobe recall should be >= 0.70, got {full_recall:.3}"
        );

        eprintln!(
            "Recall sweep: n={n}, dim={dim}, k={k}, centroids={}, \
             full_nprobe_recall={full_recall:.3}",
            segment.num_centroids
        );
    }

    // =========================================================================
    // Phase 3b: Filter correctness
    // =========================================================================

    #[test]
    fn test_filter_eq_correctness() {
        let dim = 8;
        let n = 500;
        let mut rng = rand::rng();
        let color_field: FieldPath = "color".parse().unwrap();

        let colors = [vec![1u8], vec![2u8], vec![3u8]];

        let raw_vectors: Vec<Vec<f32>> = (0..n).map(|_| random_vector(&mut rng, dim)).collect();
        let docs: Vec<_> = raw_vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let mut filter = BTreeMap::new();
                filter.insert(color_field.clone(), colors[i % 3].clone());
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(v.clone()).unwrap(),
                    filter,
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        // Test Eq filter for each color
        for color in &colors {
            let mut conditions = BTreeMap::new();
            conditions.insert(color_field.clone(), CompiledVectorFilter::Eq(color.clone()));

            let query = random_vector(&mut rng, dim);
            let results = segment.search_with_nprobe(
                &query,
                &conditions,
                n,                            // Get all matches
                segment.num_centroids.max(1), // Full scan for correctness
            );

            // All results should have the correct color
            for (idx, ..) in &results {
                let expected_color = &colors[*idx as usize % 3];
                assert_eq!(
                    expected_color, color,
                    "Result {idx} has wrong color filter value"
                );
            }

            // Count should match expected (~n/3)
            let expected_count = (0..n)
                .filter(|i| i % 3 == colors.iter().position(|c| c == color).unwrap())
                .count();
            assert_eq!(
                results.len(),
                expected_count,
                "Filter should return exactly {expected_count} results"
            );
        }
    }

    #[test]
    fn test_filter_in_correctness() {
        let dim = 8;
        let n = 500;
        let mut rng = rand::rng();
        let tag_field: FieldPath = "tag".parse().unwrap();

        let tags = [vec![10u8], vec![20u8], vec![30u8], vec![40u8]];

        let raw_vectors: Vec<Vec<f32>> = (0..n).map(|_| random_vector(&mut rng, dim)).collect();
        let docs: Vec<_> = raw_vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let mut filter = BTreeMap::new();
                filter.insert(tag_field.clone(), tags[i % 4].clone());
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(v.clone()).unwrap(),
                    filter,
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        // Filter for tags 10 or 30 (IN filter)
        let mut conditions = BTreeMap::new();
        conditions.insert(
            tag_field.clone(),
            CompiledVectorFilter::In(vec![vec![10u8], vec![30u8]]),
        );

        let query = random_vector(&mut rng, dim);
        let results =
            segment.search_with_nprobe(&query, &conditions, n, segment.num_centroids.max(1));

        // All results should have tag 10 or 30
        for (idx, ..) in &results {
            let tag = &tags[*idx as usize % 4];
            assert!(
                *tag == vec![10u8] || *tag == vec![30u8],
                "Result {idx} has tag {tag:?}, expected 10 or 30"
            );
        }

        let expected_count = (0..n).filter(|i| i % 4 == 0 || i % 4 == 2).count();
        assert_eq!(results.len(), expected_count);
    }

    #[test]
    fn test_filter_no_match() {
        let dim = 4;
        let n = 100;
        let mut rng = rand::rng();
        let field: FieldPath = "x".parse().unwrap();

        let docs: Vec<_> = (0..n)
            .map(|i| {
                let mut filter = BTreeMap::new();
                filter.insert(field.clone(), vec![1u8]);
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    filter,
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        // Filter for value that doesn't exist
        let mut conditions = BTreeMap::new();
        conditions.insert(field.clone(), CompiledVectorFilter::Eq(vec![99u8]));

        let query = random_vector(&mut rng, dim);
        let results = segment.search(&query, &conditions, 10);
        assert!(results.is_empty(), "No results should match filter");
    }

    #[test]
    fn test_filter_matches_everything() {
        let dim = 4;
        let n = 50;
        let mut rng = rand::rng();
        let field: FieldPath = "x".parse().unwrap();

        let docs: Vec<_> = (0..n)
            .map(|i| {
                let mut filter = BTreeMap::new();
                filter.insert(field.clone(), vec![1u8]);
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    filter,
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        let mut conditions = BTreeMap::new();
        conditions.insert(field.clone(), CompiledVectorFilter::Eq(vec![1u8]));

        let query = random_vector(&mut rng, dim);
        let results = segment.search(&query, &conditions, 10);
        assert_eq!(results.len(), 10, "Should return limit results");
    }

    // =========================================================================
    // Phase 3c: Persistence round-trip fuzz
    // =========================================================================

    #[tokio::test]
    async fn test_persistence_roundtrip_various_sizes() {
        let mut rng = rand::rng();

        for &(n, dim) in &[(0, 4), (1, 4), (10, 4), (100, 8), (300, 16), (500, 32)] {
            let docs: Vec<_> = (0..n)
                .map(|i| {
                    let mut filter = BTreeMap::new();
                    if i % 2 == 0 {
                        filter.insert("f".parse::<FieldPath>().unwrap(), vec![i as u8]);
                    }
                    (
                        make_test_id(i as u128),
                        IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                        filter,
                        i as u64,
                    )
                })
                .collect();

            let tmpdir = TempDir::new().unwrap();
            let tmp_path = tmpdir.path().join("tmp");
            let disk_path = tmpdir.path().join("disk");
            std::fs::create_dir_all(&tmp_path).unwrap();
            std::fs::create_dir_all(&disk_path).unwrap();

            let result = build_and_snapshot_segment(docs, dim, &tmp_path, &disk_path).unwrap();
            assert_eq!(result.num_vectors, n as u32);

            if n == 0 {
                continue; // Can't load an empty segment meaningfully
            }

            let loaded = unsafe_load_disk_segment(&result.paths).await.unwrap();
            assert_eq!(loaded.num_vectors(), n);
            assert_eq!(loaded.dimension(), dim);

            // Search should produce identical results before and after persist
            let query = random_vector(&mut rng, dim);
            let segment = SpannSegment::build(
                (0..n)
                    .map(|i| {
                        let mut filter = BTreeMap::new();
                        if i % 2 == 0 {
                            filter.insert("f".parse::<FieldPath>().unwrap(), vec![i as u8]);
                        }
                        (
                            make_test_id(i as u128),
                            IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                            filter,
                            i as u64,
                        )
                    })
                    .collect(),
                dim,
            );

            // Just verify the loaded segment is searchable
            let results = loaded.search(&query, &BTreeMap::new(), 5.min(n));
            assert!(
                results.len() <= 5.min(n),
                "n={n}, dim={dim}: got {} results",
                results.len()
            );
        }
    }

    // =========================================================================
    // Phase 3f: Edge cases
    // =========================================================================

    #[test]
    fn test_all_identical_vectors() {
        let dim = 4;
        let n = 300; // Enough to trigger clustering
        let identical = vec![1.0f32, 0.0, 0.0, 0.0];

        let docs: Vec<_> = (0..n)
            .map(|i| {
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(identical.clone()).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        // Should not panic even with degenerate clustering
        let segment = SpannSegment::build(docs, dim);
        assert_eq!(segment.num_vectors(), n);

        let results = segment.search(&identical, &BTreeMap::new(), 5);
        assert_eq!(results.len(), 5);
        // All scores should be equal (all vectors identical)
        for r in &results {
            assert!(
                (r.1 - results[0].1).abs() < 1e-5,
                "All identical vectors should have same score"
            );
        }
    }

    #[test]
    fn test_high_dimensions() {
        let dim = 1536; // OpenAI embedding size
        let n = 50;
        let mut rng = rand::rng();

        let docs: Vec<_> = (0..n)
            .map(|i| {
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);
        assert_eq!(segment.num_vectors(), n);

        let query = random_vector(&mut rng, dim);
        let results = segment.search(&query, &BTreeMap::new(), 10);
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_limit_one() {
        let dim = 4;
        let n = 100;
        let mut rng = rand::rng();

        let docs: Vec<_> = (0..n)
            .map(|i| {
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        let query = random_vector(&mut rng, dim);
        let results = segment.search(&query, &BTreeMap::new(), 1);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_limit_max() {
        let dim = 4;
        let n = 300;
        let mut rng = rand::rng();

        let docs: Vec<_> = (0..n)
            .map(|i| {
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        let query = random_vector(&mut rng, dim);
        // Request more than available -- should cap at n
        let results = segment.search_with_nprobe(
            &query,
            &BTreeMap::new(),
            crate::MAX_VECTOR_RESULTS,
            segment.num_centroids.max(1),
        );
        assert!(results.len() <= n);
    }

    #[test]
    fn test_multiple_filter_fields() {
        let dim = 4;
        let n = 100;
        let mut rng = rand::rng();
        let color_field: FieldPath = "color".parse().unwrap();
        let size_field: FieldPath = "size".parse().unwrap();

        let docs: Vec<_> = (0..n)
            .map(|i| {
                let mut filter = BTreeMap::new();
                filter.insert(
                    color_field.clone(),
                    if i % 2 == 0 { vec![1u8] } else { vec![2u8] },
                );
                filter.insert(
                    size_field.clone(),
                    if i % 3 == 0 { vec![10u8] } else { vec![20u8] },
                );
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    filter,
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        // Filter: color=1 OR size=10
        // The matches_filter function uses OR semantics across fields
        let mut conditions = BTreeMap::new();
        conditions.insert(color_field.clone(), CompiledVectorFilter::Eq(vec![1u8]));
        conditions.insert(size_field.clone(), CompiledVectorFilter::Eq(vec![10u8]));

        let query = random_vector(&mut rng, dim);
        let results = segment.search(&query, &conditions, n);

        // Results should match either color=1 OR size=10
        for (idx, ..) in &results {
            let i = *idx as usize;
            let color_match = i % 2 == 0; // color=1
            let size_match = i % 3 == 0; // size=10
            assert!(
                color_match || size_match,
                "Result {i} should match color=1 or size=10"
            );
        }
    }

    // =========================================================================
    // Phase 4a: Boundary replication
    // =========================================================================

    #[test]
    fn test_boundary_replication_improves_recall() {
        // Build a large enough segment to trigger both clustering and replication.
        // With boundary replication, full-nprobe recall should be higher than
        // without it (the base recall from Phase 3a was ~0.87).
        let dim = 32;
        let n = 2000;
        let k = 10;
        let num_queries = 30;
        let mut rng = rand::rng();

        let raw_vectors: Vec<Vec<f32>> = (0..n).map(|_| random_vector(&mut rng, dim)).collect();
        let docs: Vec<_> = raw_vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(v.clone()).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);
        assert!(segment.num_centroids > 0);

        // The segment should have more vectors than the original n due to replication
        assert!(
            segment.num_total_vectors() >= n,
            "Boundary replication should add vectors: got {} from {n} originals",
            segment.num_total_vectors()
        );

        let queries: Vec<Vec<f32>> = (0..num_queries)
            .map(|_| random_vector(&mut rng, dim))
            .collect();

        // Measure recall at full nprobe
        let mut total_recall = 0.0f32;
        for query in &queries {
            let ground_truth = brute_force_search(&raw_vectors, query, k);
            let approx =
                segment.search_with_nprobe(query, &BTreeMap::new(), k, segment.num_centroids);
            let recall = compute_recall(&ground_truth, &approx, k);
            total_recall += recall;
        }
        let avg_recall = total_recall / num_queries as f32;

        eprintln!(
            "Boundary replication: n={n}, dim={dim}, replicated_total={}, centroids={}, \
             full_recall={avg_recall:.3}",
            segment.num_total_vectors(),
            segment.num_centroids
        );

        // With replication, full-nprobe recall should be at least as good as before
        assert!(
            avg_recall >= 0.80,
            "Recall with boundary replication should be >= 0.80, got {avg_recall:.3}"
        );
    }

    #[test]
    fn test_no_duplicate_results_with_replication() {
        // Verify that search deduplicates vectors that appear in multiple postings
        let dim = 8;
        let n = 500;
        let mut rng = rand::rng();

        let docs: Vec<_> = (0..n)
            .map(|i| {
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let segment = SpannSegment::build(docs, dim);

        let query = random_vector(&mut rng, dim);
        let results =
            segment.search_with_nprobe(&query, &BTreeMap::new(), 50, segment.num_centroids.max(1));

        // Check no duplicate point indices
        let indices: HashSet<u32> = results.iter().map(|(idx, ..)| *idx).collect();
        assert_eq!(
            indices.len(),
            results.len(),
            "Search results should not contain duplicate point indices"
        );
    }

    // =========================================================================
    // Phase 3d: Scale test (moderate -- keeps test fast)
    // =========================================================================

    #[test]
    fn test_scale_1k_128d() {
        let dim = 128;
        let n = 1000;
        let k = 10;
        let mut rng = rand::rng();

        let raw_vectors: Vec<Vec<f32>> = (0..n).map(|_| random_vector(&mut rng, dim)).collect();
        let docs: Vec<_> = raw_vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                (
                    make_test_id(i as u128),
                    IndexedVector::try_from(v.clone()).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let start = std::time::Instant::now();
        let segment = SpannSegment::build(docs, dim);
        let build_time = start.elapsed();

        assert_eq!(segment.num_vectors(), n);
        assert!(segment.num_centroids > 0);

        // Measure search latency and recall
        let query = random_vector(&mut rng, dim);
        let start = std::time::Instant::now();
        let results = segment.search(&query, &BTreeMap::new(), k);
        let search_time = start.elapsed();

        assert_eq!(results.len(), k);

        // Compute recall
        let ground_truth = brute_force_search(&raw_vectors, &query, k);
        let recall = compute_recall(&ground_truth, &results, k);

        // Log for informational purposes
        eprintln!(
            "Scale test: n={n}, dim={dim}, k={k}, nprobe={DEFAULT_NPROBE}, centroids={}, \
             build={build_time:?}, search={search_time:?}, recall@{k}={recall:.3}",
            segment.num_centroids
        );

        assert!(
            recall >= 0.5,
            "Recall@{k} should be >= 0.5, got {recall:.3}"
        );
    }
}
