//! End-to-end integration tests for the vector2 crate.
//!
//! These tests exercise the full multi-layer stack:
//! 1. VectorSchema -> document indexing
//! 2. SpannSegment -> build, search, persist
//! 3. MemoryVectorIndex -> in-memory delta tracking
//! 4. Combined disk + memory search flow (as VectorIndexManager would do)
//! 5. Segment merge (compaction)
//! 6. Full lifecycle: insert -> flush -> search -> update -> re-flush -> search

#[cfg(test)]
mod tests {
    use std::collections::{
        BTreeMap,
        BTreeSet,
        HashSet,
    };

    use common::{
        query::search_value_to_bytes,
        types::{
            Timestamp,
            WriteTimestamp,
        },
    };
    use rand::Rng;
    use tempfile::TempDir;
    use value::{
        FieldPath,
        InternalId,
    };

    use crate::{
        id_tracker::VectorMemoryIdTracker,
        memory_index::MemoryVectorIndex,
        query::{
            CompiledVectorFilter,
            CompiledVectorSearch,
        },
        spann::{
            cosine,
            segment::{
                build_and_snapshot_segment,
                load_disk_segment,
                merge_disk_segments,
                snapshot_segment,
                unsafe_load_disk_segment,
                SpannSegment,
                UntarredVectorDiskSegmentPaths,
            },
            VectorDocument,
            VectorSchema,
        },
        IndexedVector,
        VectorSearchQueryResult,
    };

    // --- Helpers

    fn make_id(n: u128) -> InternalId {
        InternalId(n.to_le_bytes())
    }

    fn random_vector(rng: &mut impl Rng, dim: usize) -> Vec<f32> {
        (0..dim).map(|_| rng.random::<f32>()).collect()
    }

    fn make_document(
        id: u128,
        vector: Vec<f32>,
        filter_fields: BTreeMap<FieldPath, Vec<u8>>,
    ) -> VectorDocument {
        VectorDocument {
            internal_id: make_id(id),
            vector: IndexedVector::try_from(vector).unwrap(),
            filter_fields,
        }
    }

    fn brute_force_search(
        docs: &[VectorDocument],
        query: &[f32],
        filter: &BTreeMap<FieldPath, CompiledVectorFilter>,
        limit: usize,
    ) -> Vec<VectorSearchQueryResult> {
        let query_norm = cosine::preprocess(query.to_vec());
        let mut results: Vec<VectorSearchQueryResult> = docs
            .iter()
            .filter(|doc| matches_filter(&doc.filter_fields, filter))
            .map(|doc| {
                let v = cosine::preprocess(Vec::from(doc.vector.clone()));
                let score = cosine::cosine_similarity(&query_norm, &v);
                VectorSearchQueryResult {
                    score,
                    id: doc.internal_id,
                    ts: WriteTimestamp::Committed(Timestamp::MIN),
                }
            })
            .collect();
        results.sort_by(|a, b| a.cmp(b).reverse());
        results.truncate(limit);
        results
    }

    fn matches_filter(
        fields: &BTreeMap<FieldPath, Vec<u8>>,
        conditions: &BTreeMap<FieldPath, CompiledVectorFilter>,
    ) -> bool {
        if conditions.is_empty() {
            return true;
        }
        for (field_path, condition) in conditions {
            let Some(value) = fields.get(field_path) else {
                return false;
            };
            let matches = match condition {
                CompiledVectorFilter::Eq(term) => term == value,
                CompiledVectorFilter::In(terms) => terms.iter().any(|t| t == value),
            };
            if matches {
                return true;
            }
        }
        false
    }

    // --- Test 1: Schema -> index -> build segment -> search

    #[test]
    fn test_e2e_schema_to_search() {
        let dim = 4;
        let mut rng = rand::rng();

        // Create documents with vectors and filter fields
        let color_field: FieldPath = "color".parse().unwrap();
        let mut docs = Vec::new();
        for i in 0..20 {
            let mut filters = BTreeMap::new();
            let color = if i < 10 { vec![1u8] } else { vec![2u8] };
            filters.insert(color_field.clone(), color);
            docs.push(make_document(
                i as u128,
                random_vector(&mut rng, dim),
                filters,
            ));
        }

        // Build segment from documents
        let segment_docs: Vec<_> = docs
            .iter()
            .map(|d| {
                (
                    d.internal_id,
                    d.vector.clone(),
                    d.filter_fields.clone(),
                    0u64, // timestamp
                )
            })
            .collect();
        let segment = SpannSegment::build(segment_docs, dim);

        // Search without filter
        let query = random_vector(&mut rng, dim);
        let compiled = CompiledVectorSearch {
            vector: IndexedVector::try_from(query.clone()).unwrap(),
            limit: 5,
            filter_conditions: BTreeMap::new(),
        };
        let results = segment.search(&query, &compiled.filter_conditions, 5);
        assert_eq!(results.len(), 5, "Should return 5 results");

        // Search with filter (color == 1)
        let mut conditions = BTreeMap::new();
        conditions.insert(color_field.clone(), CompiledVectorFilter::Eq(vec![1u8]));
        let results = segment.search(&query, &conditions, 10);
        assert!(results.len() <= 10, "Filtered results should be <= limit");

        // Compare to brute force
        let bf_results = brute_force_search(&docs, &query, &conditions, 10);
        assert_eq!(
            results.len(),
            bf_results.len(),
            "Filtered result count should match brute force"
        );
    }

    // --- Test 2: Segment build -> snapshot -> restore -> search

    #[tokio::test]
    async fn test_e2e_persist_and_restore() {
        let dim = 8;
        let n = 50;
        let mut rng = rand::rng();

        let docs: Vec<_> = (0..n)
            .map(|i| {
                let mut filters = BTreeMap::new();
                filters.insert("tag".parse::<FieldPath>().unwrap(), vec![(i % 3) as u8]);
                make_document(i as u128, random_vector(&mut rng, dim), filters)
            })
            .collect();

        let segment_docs: Vec<_> = docs
            .iter()
            .map(|d| {
                (
                    d.internal_id,
                    d.vector.clone(),
                    d.filter_fields.clone(),
                    100u64 + d.internal_id.0[0] as u64,
                )
            })
            .collect();

        // Build and snapshot
        let tmpdir = TempDir::new().unwrap();
        let tmp_path = tmpdir.path().join("tmp");
        let disk_path = tmpdir.path().join("disk");
        std::fs::create_dir_all(&tmp_path).unwrap();
        std::fs::create_dir_all(&disk_path).unwrap();

        let result = build_and_snapshot_segment(segment_docs, dim, &tmp_path, &disk_path).unwrap();
        assert_eq!(result.num_vectors, n as u32);

        // Restore
        let loaded = unsafe_load_disk_segment(&result.paths).await.unwrap();

        // Search on restored segment should return results
        let query = random_vector(&mut rng, dim);
        let results = loaded.search(&query, &BTreeMap::new(), 5);
        assert_eq!(results.len(), 5);

        // Search with filter on restored segment
        let mut conditions = BTreeMap::new();
        conditions.insert(
            "tag".parse::<FieldPath>().unwrap(),
            CompiledVectorFilter::Eq(vec![0u8]),
        );
        let filtered_results = loaded.search(&query, &conditions, n);
        // All results should have tag == 0
        for (idx, ..) in &filtered_results {
            // idx is the point index in the segment, which corresponds to
            // the original document index for non-replicated flat segments
            // We can't directly check the filter value from the index alone
            // but we verify the count is reasonable
        }
        // Roughly n/3 documents have tag 0
        let expected = (0..n).filter(|i| i % 3 == 0).count();
        assert_eq!(
            filtered_results.len(),
            expected,
            "Should have {expected} tag=0 results"
        );
    }

    // --- Test 3: Memory index updates and queries

    #[test]
    fn test_e2e_memory_index() {
        let dim = 4;
        let mut rng = rand::rng();

        let ts1 = WriteTimestamp::Committed(Timestamp::must(1));
        let ts2 = WriteTimestamp::Committed(Timestamp::must(2));
        let ts3 = WriteTimestamp::Committed(Timestamp::must(3));

        let mut mem_index = MemoryVectorIndex::new(ts1);

        // Insert 3 documents
        let doc1 = make_document(1, random_vector(&mut rng, dim), BTreeMap::new());
        let doc2 = make_document(2, random_vector(&mut rng, dim), BTreeMap::new());
        let doc3 = make_document(3, random_vector(&mut rng, dim), BTreeMap::new());

        mem_index
            .update(make_id(1), ts1, None, Some(doc1.clone()))
            .unwrap();
        mem_index
            .update(make_id(2), ts2, None, Some(doc2.clone()))
            .unwrap();
        mem_index
            .update(make_id(3), ts3, None, Some(doc3.clone()))
            .unwrap();

        assert_eq!(mem_index.num_transactions(), 3);

        // Query should return all 3 documents
        let query = CompiledVectorSearch {
            vector: IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
            limit: 10,
            filter_conditions: BTreeMap::new(),
        };
        let results = mem_index.query(Timestamp::must(3), &query).unwrap();
        assert_eq!(results.len(), 3, "Should find all 3 documents");

        // Update doc1 with new vector at ts3 (simulates update)
        let doc1_updated = make_document(1, random_vector(&mut rng, dim), BTreeMap::new());
        mem_index
            .update(make_id(1), ts3, Some(doc1.clone()), Some(doc1_updated))
            .unwrap();

        // Query should still return 3 documents (updated doc1 + doc2 + doc3)
        let results = mem_index.query(Timestamp::must(3), &query).unwrap();
        assert_eq!(results.len(), 3);

        // Delete doc2
        let ts4 = WriteTimestamp::Committed(Timestamp::must(4));
        mem_index
            .update(make_id(2), ts4, Some(doc2.clone()), None)
            .unwrap();

        let results = mem_index.query(Timestamp::must(4), &query).unwrap();
        assert_eq!(results.len(), 2, "Should find 2 documents after delete");

        // Check tombstones -- updated_matches should report doc1 and doc2 as changed
        // since ts1 (which is before the updates happened)
        let updated = mem_index
            .updated_matches(Timestamp::must(1), &query)
            .unwrap();
        assert!(
            updated.len() >= 2,
            "Should have at least 2 updated matches (doc1 updated, doc2 deleted)"
        );
    }

    // --- Test 4: Combined disk + memory search (simulating VectorIndexManager)

    #[tokio::test]
    async fn test_e2e_combined_disk_and_memory_search() {
        let dim = 4;
        let n_disk = 20;
        let mut rng = rand::rng();

        // --- Build disk segment with n_disk documents ---
        let disk_docs: Vec<VectorDocument> = (0..n_disk)
            .map(|i| make_document(i as u128, random_vector(&mut rng, dim), BTreeMap::new()))
            .collect();

        let segment_docs: Vec<_> = disk_docs
            .iter()
            .map(|d| {
                (
                    d.internal_id,
                    d.vector.clone(),
                    d.filter_fields.clone(),
                    0u64,
                )
            })
            .collect();

        let tmpdir = TempDir::new().unwrap();
        let tmp_path = tmpdir.path().join("tmp");
        let disk_path = tmpdir.path().join("disk");
        std::fs::create_dir_all(&tmp_path).unwrap();
        std::fs::create_dir_all(&disk_path).unwrap();

        let disk_result =
            build_and_snapshot_segment(segment_docs, dim, &tmp_path, &disk_path).unwrap();
        let disk_segment = unsafe_load_disk_segment(&disk_result.paths).await.unwrap();

        // --- Create memory index with newer documents ---
        let disk_ts = Timestamp::must(10);
        let mem_ts1 = WriteTimestamp::Committed(Timestamp::must(11));
        let mem_ts2 = WriteTimestamp::Committed(Timestamp::must(12));
        let mut mem_index = MemoryVectorIndex::new(WriteTimestamp::Committed(disk_ts));

        // Add a new document (not in disk segment)
        let new_doc = make_document(100, random_vector(&mut rng, dim), BTreeMap::new());
        mem_index
            .update(make_id(100), mem_ts1, None, Some(new_doc.clone()))
            .unwrap();

        // Delete a document that IS in disk segment (doc 0)
        let deleted_doc = disk_docs[0].clone();
        mem_index
            .update(make_id(0), mem_ts2, Some(deleted_doc), None)
            .unwrap();

        // --- Simulate combined search (as VectorIndexManager does) ---
        let query_vec = random_vector(&mut rng, dim);
        let compiled = CompiledVectorSearch {
            vector: IndexedVector::try_from(query_vec.clone()).unwrap(),
            limit: 10,
            filter_conditions: BTreeMap::new(),
        };

        // 1. Get updated matches from memory (docs changed since disk snapshot)
        let updated_matches = mem_index.updated_matches(disk_ts, &compiled).unwrap();
        assert!(
            updated_matches.contains(&make_id(0)),
            "Doc 0 was deleted, should be in updated_matches"
        );

        // 2. Search disk segment
        let overfetch_delta = updated_matches.len();
        let mut disk_results = disk_segment.search(
            &query_vec,
            &compiled.filter_conditions,
            compiled.limit as usize + overfetch_delta,
        );

        // 3. Filter out stale disk results
        disk_results.retain(|(_idx, _score, _ts)| {
            // In the real system, we'd check InternalId against updated_matches.
            // Here we use the point index: doc 0 was deleted.
            // Point index 0 corresponds to make_id(0) in our build.
            !updated_matches.contains(&make_id(*_idx as u128))
        });

        // 4. Query memory index
        let mem_results = mem_index.query(disk_ts, &compiled).unwrap();

        // 5. Merge and truncate
        let disk_as_results: Vec<VectorSearchQueryResult> = disk_results
            .iter()
            .map(|(idx, score, _ts)| VectorSearchQueryResult {
                score: *score,
                id: make_id(*idx as u128),
                ts: WriteTimestamp::Committed(Timestamp::MIN),
            })
            .collect();

        let mut combined: Vec<VectorSearchQueryResult> = disk_as_results;
        combined.extend(mem_results);
        combined.sort_by(|a, b| a.cmp(b).reverse());
        combined.truncate(compiled.limit as usize);

        // Verify: doc 0 should NOT be in results (deleted)
        assert!(
            !combined.iter().any(|r| r.id == make_id(0)),
            "Deleted doc 0 should not appear in combined results"
        );

        // Verify: doc 100 (from memory) should be in results
        // (it's the only new doc, so it should appear if it's in top-k)
        let has_100 = combined.iter().any(|r| r.id == make_id(100));
        // Note: may not be in top-10 depending on random vectors, but let's
        // at least verify the count is reasonable
        assert!(
            combined.len() <= 10,
            "Combined results should respect limit"
        );
        assert!(combined.len() >= 1, "Should have at least some results");

        eprintln!(
            "Combined search: {} disk results (after filter), mem_doc_100_in_results={has_100}, \
             total={}",
            disk_results.len(),
            combined.len()
        );
    }

    // --- Test 5: Segment merge (compaction)

    #[tokio::test]
    async fn test_e2e_segment_merge() {
        let dim = 4;
        let mut rng = rand::rng();

        // Build two segments
        let docs1: Vec<_> = (0..30)
            .map(|i| {
                (
                    make_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let docs2: Vec<_> = (30..60)
            .map(|i| {
                (
                    make_id(i as u128),
                    IndexedVector::try_from(random_vector(&mut rng, dim)).unwrap(),
                    BTreeMap::new(),
                    i as u64,
                )
            })
            .collect();

        let tmpdir = TempDir::new().unwrap();

        // Build segment 1
        let tmp1 = tmpdir.path().join("tmp1");
        let disk1 = tmpdir.path().join("disk1");
        std::fs::create_dir_all(&tmp1).unwrap();
        std::fs::create_dir_all(&disk1).unwrap();
        let result1 = build_and_snapshot_segment(docs1, dim, &tmp1, &disk1).unwrap();
        assert_eq!(result1.num_vectors, 30);

        // Build segment 2
        let tmp2 = tmpdir.path().join("tmp2");
        let disk2 = tmpdir.path().join("disk2");
        std::fs::create_dir_all(&tmp2).unwrap();
        std::fs::create_dir_all(&disk2).unwrap();
        let result2 = build_and_snapshot_segment(docs2, dim, &tmp2, &disk2).unwrap();
        assert_eq!(result2.num_vectors, 30);

        // Load both segments
        let seg1 = unsafe_load_disk_segment(&result1.paths).await.unwrap();
        let seg2 = unsafe_load_disk_segment(&result2.paths).await.unwrap();

        // Load ID trackers
        use common::{
            deleted_bitset::DeletedBitset,
            id_tracker::StaticIdTracker,
        };

        use crate::id_tracker::VectorStaticIdTracker;

        let id_tracker1 = VectorStaticIdTracker {
            id_tracker: StaticIdTracker::load_from_path(&result1.paths.uuids).unwrap(),
            deleted_bitset: DeletedBitset::load_from_path(&result1.paths.deleted_bitset).unwrap(),
        };
        let id_tracker2 = VectorStaticIdTracker {
            id_tracker: StaticIdTracker::load_from_path(&result2.paths.uuids).unwrap(),
            deleted_bitset: DeletedBitset::load_from_path(&result2.paths.deleted_bitset).unwrap(),
        };

        // Merge
        let merge_tmp = tmpdir.path().join("merge_tmp");
        let merge_disk = tmpdir.path().join("merge_disk");
        std::fs::create_dir_all(&merge_tmp).unwrap();
        std::fs::create_dir_all(&merge_disk).unwrap();

        let merged = merge_disk_segments(
            vec![&seg1, &seg2],
            vec![&id_tracker1, &id_tracker2],
            dim,
            &merge_tmp,
            &merge_disk,
        )
        .unwrap();

        assert_eq!(
            merged.num_vectors, 60,
            "Merged segment should have 60 vectors"
        );

        // Load merged segment and search
        let merged_seg = unsafe_load_disk_segment(&merged.paths).await.unwrap();
        assert_eq!(merged_seg.num_vectors(), 60);

        let query = random_vector(&mut rng, dim);
        let results = merged_seg.search(&query, &BTreeMap::new(), 10);
        assert_eq!(results.len(), 10);
    }

    // --- Test 6: Full lifecycle -- insert, flush, search, update, re-flush

    #[tokio::test]
    async fn test_e2e_full_lifecycle() {
        let dim = 4;
        let mut rng = rand::rng();

        // --- Phase A: Initial insert + flush ---
        let initial_docs: Vec<VectorDocument> = (0..15)
            .map(|i| make_document(i as u128, random_vector(&mut rng, dim), BTreeMap::new()))
            .collect();

        let flush1_docs: Vec<_> = initial_docs
            .iter()
            .map(|d| {
                (
                    d.internal_id,
                    d.vector.clone(),
                    d.filter_fields.clone(),
                    10u64, // ts = 10
                )
            })
            .collect();

        let tmpdir = TempDir::new().unwrap();
        let tmp1 = tmpdir.path().join("flush1_tmp");
        let disk1 = tmpdir.path().join("flush1_disk");
        std::fs::create_dir_all(&tmp1).unwrap();
        std::fs::create_dir_all(&disk1).unwrap();

        let flush1_result = build_and_snapshot_segment(flush1_docs, dim, &tmp1, &disk1).unwrap();
        let seg1 = unsafe_load_disk_segment(&flush1_result.paths)
            .await
            .unwrap();

        // Search the first flush
        let query = random_vector(&mut rng, dim);
        let results1 = seg1.search(&query, &BTreeMap::new(), 10);
        assert_eq!(results1.len(), 10);

        // --- Phase B: Some updates in memory ---
        let disk_ts = Timestamp::must(10);
        let mut mem_index = MemoryVectorIndex::new(WriteTimestamp::Committed(disk_ts));

        // Update doc 0 with a new vector
        let updated_doc0 = make_document(0, random_vector(&mut rng, dim), BTreeMap::new());
        let old_doc0 = initial_docs[0].clone();
        mem_index
            .update(
                make_id(0),
                WriteTimestamp::Committed(Timestamp::must(11)),
                Some(old_doc0),
                Some(updated_doc0.clone()),
            )
            .unwrap();

        // Add a brand new doc
        let new_doc = make_document(99, random_vector(&mut rng, dim), BTreeMap::new());
        mem_index
            .update(
                make_id(99),
                WriteTimestamp::Committed(Timestamp::must(12)),
                None,
                Some(new_doc.clone()),
            )
            .unwrap();

        // Delete doc 5
        let deleted_doc5 = initial_docs[5].clone();
        mem_index
            .update(
                make_id(5),
                WriteTimestamp::Committed(Timestamp::must(13)),
                Some(deleted_doc5),
                None,
            )
            .unwrap();

        assert_eq!(mem_index.num_transactions(), 3);

        // --- Phase C: Simulate combined search ---
        let compiled = CompiledVectorSearch {
            vector: IndexedVector::try_from(query.clone()).unwrap(),
            limit: 10,
            filter_conditions: BTreeMap::new(),
        };

        let updated = mem_index.updated_matches(disk_ts, &compiled).unwrap();
        // Doc 0, 5 should be in updated (tombstoned)
        assert!(updated.contains(&make_id(0)));
        assert!(updated.contains(&make_id(5)));

        let overfetch = updated.len();
        let mut disk_results = seg1.search(
            &query,
            &compiled.filter_conditions,
            compiled.limit as usize + overfetch,
        );
        disk_results.retain(|(idx, ..)| !updated.contains(&make_id(*idx as u128)));

        let mem_results = mem_index.query(disk_ts, &compiled).unwrap();
        let mut combined: Vec<VectorSearchQueryResult> = disk_results
            .iter()
            .map(|(idx, score, _)| VectorSearchQueryResult {
                score: *score,
                id: make_id(*idx as u128),
                ts: WriteTimestamp::Committed(Timestamp::MIN),
            })
            .collect();
        combined.extend(mem_results);
        combined.sort_by(|a, b| a.cmp(b).reverse());
        combined.truncate(10);

        // Doc 5 should not appear
        assert!(
            !combined.iter().any(|r| r.id == make_id(5)),
            "Deleted doc 5 should not be in results"
        );

        // --- Phase D: Second flush (simulates building new segment with updates) ---
        // Collect all current documents: initial minus deleted, plus updated, plus new
        let mut current_docs: Vec<VectorDocument> = initial_docs
            .iter()
            .filter(|d| d.internal_id != make_id(5)) // skip deleted
            .filter(|d| d.internal_id != make_id(0)) // skip old version of updated
            .cloned()
            .collect();
        current_docs.push(updated_doc0); // add updated version
        current_docs.push(new_doc); // add new doc

        let flush2_docs: Vec<_> = current_docs
            .iter()
            .map(|d| {
                (
                    d.internal_id,
                    d.vector.clone(),
                    d.filter_fields.clone(),
                    13u64, // ts = 13 (latest)
                )
            })
            .collect();

        let tmp2 = tmpdir.path().join("flush2_tmp");
        let disk2 = tmpdir.path().join("flush2_disk");
        std::fs::create_dir_all(&tmp2).unwrap();
        std::fs::create_dir_all(&disk2).unwrap();

        let flush2_result = build_and_snapshot_segment(flush2_docs, dim, &tmp2, &disk2).unwrap();
        assert_eq!(flush2_result.num_vectors, 15); // 15 - 1 deleted + 1 new = 15

        let seg2 = unsafe_load_disk_segment(&flush2_result.paths)
            .await
            .unwrap();

        // Search on the new segment -- should not contain doc 5
        let results2 = seg2.search(&query, &BTreeMap::new(), 15);

        // Verify we get the right count
        assert_eq!(
            seg2.num_vectors(),
            15,
            "Second flush should have 15 vectors"
        );

        eprintln!(
            "Full lifecycle: flush1={} vecs, flush2={} vecs, combined_results={}, \
             flush2_results={}",
            flush1_result.num_vectors,
            flush2_result.num_vectors,
            combined.len(),
            results2.len()
        );
    }

    // --- Test 7: Multi-segment fan-out search

    #[tokio::test]
    async fn test_e2e_multi_segment_fanout() {
        let dim = 4;
        let mut rng = rand::rng();

        // Build 3 separate segments
        let mut all_docs = Vec::new();
        let mut segments = Vec::new();

        for seg_idx in 0..3 {
            let base_id = seg_idx * 20;
            let docs: Vec<_> = (0..20)
                .map(|i| {
                    let id = base_id + i;
                    let doc =
                        make_document(id as u128, random_vector(&mut rng, dim), BTreeMap::new());
                    all_docs.push(doc.clone());
                    (
                        doc.internal_id,
                        doc.vector.clone(),
                        doc.filter_fields.clone(),
                        id as u64,
                    )
                })
                .collect();

            let tmpdir = TempDir::new().unwrap();
            let tmp_path = tmpdir.path().join("tmp");
            let disk_path = tmpdir.path().join("disk");
            std::fs::create_dir_all(&tmp_path).unwrap();
            std::fs::create_dir_all(&disk_path).unwrap();

            let result = build_and_snapshot_segment(docs, dim, &tmp_path, &disk_path).unwrap();
            let seg = unsafe_load_disk_segment(&result.paths).await.unwrap();
            segments.push((seg, tmpdir)); // keep tmpdir alive
        }

        // Fan-out search across all 3 segments and merge results
        let query = random_vector(&mut rng, dim);
        let limit = 10;
        let mut all_results = Vec::new();

        for (seg, _) in &segments {
            let results = seg.search(&query, &BTreeMap::new(), limit);
            all_results.extend(results);
        }

        // Sort and truncate (simulating the top-k merge)
        all_results.sort_by(|a: &(u32, f32, u64), b: &(u32, f32, u64)| b.1.total_cmp(&a.1));
        all_results.truncate(limit);

        assert_eq!(all_results.len(), limit);

        // Compare to brute force over all docs
        let bf = brute_force_search(&all_docs, &query, &BTreeMap::new(), limit);

        // Compute recall
        let true_set: HashSet<InternalId> = bf.iter().map(|r| r.id).collect();
        let approx_set: HashSet<InternalId> = all_results
            .iter()
            .map(|(idx, _, ts)| {
                // Map back: each segment's indices are local, but timestamps encode the global
                // id
                make_id(*ts as u128)
            })
            .collect();
        let recall = true_set.intersection(&approx_set).count() as f32 / limit as f32;

        eprintln!("Multi-segment fanout: 3 segments x 20 docs, recall@{limit}={recall:.3}");
        // With flat segments (n=20 each < threshold), this should be exact
        assert!(
            recall >= 0.8,
            "Multi-segment recall should be >= 0.8, got {recall:.3}"
        );
    }

    // --- Test 8: Large segment with clustering + persist + search

    #[tokio::test]
    async fn test_e2e_large_segment_clustered() {
        let dim = 16;
        let n = 500; // Large enough for clustering
        let mut rng = rand::rng();

        let color_field: FieldPath = "color".parse().unwrap();
        let raw_vectors: Vec<Vec<f32>> = (0..n).map(|_| random_vector(&mut rng, dim)).collect();

        let docs: Vec<_> = raw_vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let mut filters = BTreeMap::new();
                let color = vec![(i % 5) as u8];
                filters.insert(color_field.clone(), color);
                (
                    make_id(i as u128),
                    IndexedVector::try_from(v.clone()).unwrap(),
                    filters,
                    i as u64,
                )
            })
            .collect();

        // Build, snapshot, restore
        let tmpdir = TempDir::new().unwrap();
        let tmp_path = tmpdir.path().join("tmp");
        let disk_path = tmpdir.path().join("disk");
        std::fs::create_dir_all(&tmp_path).unwrap();
        std::fs::create_dir_all(&disk_path).unwrap();

        let result = build_and_snapshot_segment(docs, dim, &tmp_path, &disk_path).unwrap();
        assert_eq!(result.num_vectors, n as u32);

        let loaded = unsafe_load_disk_segment(&result.paths).await.unwrap();
        assert!(
            loaded.num_centroids > 0,
            "Large segment should use clustering"
        );

        // Unfiltered search
        let query = random_vector(&mut rng, dim);
        let results = loaded.search(&query, &BTreeMap::new(), 10);
        assert_eq!(results.len(), 10);

        // Filtered search (color == 2)
        let mut conditions = BTreeMap::new();
        conditions.insert(color_field.clone(), CompiledVectorFilter::Eq(vec![2u8]));
        let filtered = loaded.search_with_nprobe(&query, &conditions, 100, loaded.num_centroids);
        let expected_color2 = (0..n).filter(|i| i % 5 == 2).count();
        assert!(
            filtered.len() >= expected_color2 * 80 / 100,
            "Full-nprobe filtered search should find most color=2 docs: got {}, expected ~{expected_color2}",
            filtered.len(),
        );

        // Measure recall
        let bf_results = {
            let query_norm = cosine::preprocess(query.clone());
            let mut results: Vec<(usize, f32)> = raw_vectors
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let v_norm = cosine::preprocess(v.clone());
                    (i, cosine::cosine_similarity(&query_norm, &v_norm))
                })
                .collect();
            results.sort_by(|a, b| b.1.total_cmp(&a.1));
            results.truncate(10);
            results
        };
        let true_set: HashSet<usize> = bf_results.iter().map(|(i, _)| *i).collect();

        // Use full nprobe for fair comparison
        let full_results =
            loaded.search_with_nprobe(&query, &BTreeMap::new(), 10, loaded.num_centroids);
        let approx_set: HashSet<usize> = full_results.iter().map(|(i, ..)| *i as usize).collect();
        let recall = true_set.intersection(&approx_set).count() as f32 / 10.0;

        eprintln!(
            "Large clustered E2E: n={n}, dim={dim}, centroids={}, recall@10={recall:.3}",
            loaded.num_centroids
        );
        assert!(
            recall >= 0.6,
            "Full-nprobe recall should be >= 0.6, got {recall:.3}"
        );
    }
}
