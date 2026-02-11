//! Balanced k-means clustering for SPANN index construction.
//!
//! Implements lambda-regularized k-means with multiple random initializations
//! and convergence checking. Inspired by Chroma's utils.rs (Apache 2.0) and
//! the SPANN paper's hierarchical balanced clustering.

use rand::Rng;

use super::cosine;

const MAX_DISTANCE: f32 = f32::MAX / 10.0;
const NUM_ITERS_CENTER_INIT: usize = 3;
const NUM_ITERS_MAIN_LOOP: usize = 100;
const NUM_ITERS_NO_IMPROVEMENT: usize = 5;

/// Result of k-means clustering.
pub struct ClusteringResult {
    /// Centroid vectors (one per cluster), each normalized.
    pub centroids: Vec<Vec<f32>>,
    /// For each input point (by index), which cluster it was assigned to.
    pub assignments: Vec<usize>,
    /// Number of points in each cluster.
    pub cluster_sizes: Vec<usize>,
    /// Number of non-empty clusters produced.
    pub num_clusters: usize,
}

/// Run balanced k-means clustering on a set of vectors.
///
/// - `vectors`: slice of normalized vectors (each of length `dim`)
/// - `dim`: dimensionality
/// - `k`: target number of clusters
/// - `initial_lambda`: regularization strength for balanced partition sizes
///
/// Returns a `ClusteringResult` with centroids and assignments.
pub fn balanced_kmeans(
    vectors: &[Vec<f32>],
    dim: usize,
    k: usize,
    initial_lambda: f32,
) -> ClusteringResult {
    let n = vectors.len();
    if n == 0 || k == 0 {
        return ClusteringResult {
            centroids: vec![],
            assignments: vec![],
            cluster_sizes: vec![],
            num_clusters: 0,
        };
    }
    let k = k.min(n);

    // Initialize centers using multiple random trials
    let (mut centers, mut counts, lambda) = init_centers(vectors, dim, k, initial_lambda);

    let mut min_dist = MAX_DISTANCE;
    let mut no_improvement = 0;

    // Main k-means loop with lambda regularization
    for _ in 0..NUM_ITERS_MAIN_LOOP {
        let prev_centers = centers.clone();
        let prev_counts = counts.clone();

        let result =
            assign_with_regularization(vectors, &prev_centers, dim, k, lambda, &prev_counts);
        if result.total_distance < min_dist {
            min_dist = result.total_distance;
            no_improvement = 0;
        } else {
            no_improvement += 1;
        }

        // Refine centers by averaging assigned points
        centers = refine_centers(vectors, dim, k, &result, &prev_centers);
        counts = result.cluster_counts;

        // Check convergence
        let diff: f32 = centers
            .iter()
            .zip(prev_centers.iter())
            .map(|(a, b)| cosine_distance(a, b))
            .sum();

        if diff < 1e-3 || no_improvement >= NUM_ITERS_NO_IMPROVEMENT {
            break;
        }
    }

    // Final assignment without regularization, using nearest-point centroids
    let final_centroids = find_nearest_point_centroids(vectors, &centers, dim, k);
    let final_assignment = assign_no_regularization(vectors, &final_centroids, dim, k);

    let mut num_clusters = 0;
    for &c in &final_assignment.cluster_counts {
        if c > 0 {
            num_clusters += 1;
        }
    }

    ClusteringResult {
        centroids: final_centroids,
        assignments: final_assignment.assignments,
        cluster_sizes: final_assignment.cluster_counts,
        num_clusters,
    }
}

/// Split a set of vectors into two clusters using 2-means.
///
/// Returns (left_centroid, left_indices, right_centroid, right_indices).
pub fn split_2means(
    vectors: &[Vec<f32>],
    dim: usize,
) -> (Vec<f32>, Vec<usize>, Vec<f32>, Vec<usize>) {
    let n = vectors.len();
    if n < 2 {
        let c = if n == 1 {
            vectors[0].clone()
        } else {
            vec![0.0; dim]
        };
        return (c.clone(), (0..n).collect(), c, vec![]);
    }

    // Try 4 random initializations, keep the best
    let mut rng = rand::rng();
    let mut best_c0 = vectors[0].clone();
    let mut best_c1 = vectors[1].clone();
    let mut best_total_dist = f32::MAX;

    for _ in 0..4 {
        let i0 = rng.random_range(0..n);
        let mut i1 = rng.random_range(0..n);
        while i1 == i0 && n > 1 {
            i1 = rng.random_range(0..n);
        }

        let total_dist: f32 = vectors
            .iter()
            .map(|v| cosine_distance(v, &vectors[i0]).min(cosine_distance(v, &vectors[i1])))
            .sum();

        if total_dist < best_total_dist {
            best_total_dist = total_dist;
            best_c0 = vectors[i0].clone();
            best_c1 = vectors[i1].clone();
        }
    }

    let mut c0 = best_c0;
    let mut c1 = best_c1;
    let mut labels = vec![false; n]; // false = left, true = right
    let mut prev_total_dist = f32::MAX;
    let mut no_improvement = 0;

    // 2-means iteration
    for _ in 0..128 {
        let mut total_dist = 0.0;
        for (i, v) in vectors.iter().enumerate() {
            let d0 = cosine_distance(v, &c0);
            let d1 = cosine_distance(v, &c1);
            labels[i] = d1 < d0;
            total_dist += d0.min(d1);
        }

        // Update centers
        let mut new_c0 = vec![0.0; dim];
        let mut new_c1 = vec![0.0; dim];
        let mut count0 = 0usize;
        let mut count1 = 0usize;

        for (i, v) in vectors.iter().enumerate() {
            if labels[i] {
                for (j, val) in v.iter().enumerate() {
                    new_c1[j] += val;
                }
                count1 += 1;
            } else {
                for (j, val) in v.iter().enumerate() {
                    new_c0[j] += val;
                }
                count0 += 1;
            }
        }

        if count0 > 0 {
            new_c0.iter_mut().for_each(|v| *v /= count0 as f32);
        }
        if count1 > 0 {
            new_c1.iter_mut().for_each(|v| *v /= count1 as f32);
        }

        // Check convergence
        let c_dist = cosine_distance(&c0, &c1);
        let relative_diff = if c_dist > f32::EPSILON {
            (cosine_distance(&c0, &new_c0) + cosine_distance(&c1, &new_c1)) / c_dist
        } else {
            0.0
        };

        c0 = new_c0;
        c1 = new_c1;

        if relative_diff < f32::EPSILON {
            break;
        }

        if total_dist >= prev_total_dist {
            no_improvement += 1;
            if no_improvement >= 4 {
                break;
            }
        } else {
            no_improvement = 0;
        }
        prev_total_dist = total_dist;
    }

    // Build output groups
    let mut left_indices = Vec::new();
    let mut right_indices = Vec::new();
    for (i, &label) in labels.iter().enumerate() {
        if label {
            right_indices.push(i);
        } else {
            left_indices.push(i);
        }
    }

    (c0, left_indices, c1, right_indices)
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    1.0 - cosine::cosine_similarity(a, b)
}

struct AssignmentResult {
    cluster_counts: Vec<usize>,
    assignments: Vec<usize>,
    total_distance: f32,
}

fn assign_with_regularization(
    vectors: &[Vec<f32>],
    centers: &[Vec<f32>],
    _dim: usize,
    k: usize,
    lambda: f32,
    prev_counts: &[usize],
) -> AssignmentResult {
    let mut counts = vec![0usize; k];
    let mut assignments = vec![0usize; vectors.len()];
    let mut total_distance = 0.0;

    for (i, v) in vectors.iter().enumerate() {
        let mut min_dist = MAX_DISTANCE;
        let mut min_center = 0;

        for (c, center) in centers.iter().enumerate() {
            let dist = cosine_distance(v, center) + lambda * prev_counts[c] as f32;
            if dist < min_dist {
                min_dist = dist;
                min_center = c;
            }
        }

        total_distance += min_dist;
        counts[min_center] += 1;
        assignments[i] = min_center;
    }

    AssignmentResult {
        cluster_counts: counts,
        assignments,
        total_distance,
    }
}

fn assign_no_regularization(
    vectors: &[Vec<f32>],
    centers: &[Vec<f32>],
    _dim: usize,
    k: usize,
) -> AssignmentResult {
    let mut counts = vec![0usize; k];
    let mut assignments = vec![0usize; vectors.len()];
    let mut total_distance = 0.0;

    for (i, v) in vectors.iter().enumerate() {
        let mut min_dist = MAX_DISTANCE;
        let mut min_center = 0;

        for (c, center) in centers.iter().enumerate() {
            let dist = cosine_distance(v, center);
            if dist < min_dist {
                min_dist = dist;
                min_center = c;
            }
        }

        total_distance += min_dist;
        counts[min_center] += 1;
        assignments[i] = min_center;
    }

    AssignmentResult {
        cluster_counts: counts,
        assignments,
        total_distance,
    }
}

fn init_centers(
    vectors: &[Vec<f32>],
    dim: usize,
    k: usize,
    initial_lambda: f32,
) -> (Vec<Vec<f32>>, Vec<usize>, f32) {
    let n = vectors.len();
    let mut rng = rand::rng();
    let mut best_centers = vec![vec![0.0; dim]; k];
    let mut best_counts = vec![0usize; k];
    let mut best_lambda = 0.0;
    let mut min_dist = MAX_DISTANCE;

    for _ in 0..NUM_ITERS_CENTER_INIT {
        let mut centers = Vec::with_capacity(k);
        for _ in 0..k {
            let idx = rng.random_range(0..n);
            centers.push(vectors[idx].clone());
        }

        // Assign without regularization to measure quality
        let result = assign_no_regularization(vectors, &centers, dim, k);

        if result.total_distance < min_dist {
            min_dist = result.total_distance;
            best_centers = centers;
            best_lambda = refine_lambda(&result.cluster_counts, min_dist, n, initial_lambda);
            best_counts = result.cluster_counts;
        }
    }

    (best_centers, best_counts, best_lambda)
}

fn refine_lambda(counts: &[usize], total_distance: f32, n: usize, initial_lambda: f32) -> f32 {
    let max_count = counts.iter().copied().max().unwrap_or(0);
    if max_count == 0 || n == 0 {
        return 0.0;
    }
    let avg_distance = total_distance / n as f32;
    let lambda = avg_distance / (n as f32 * initial_lambda);
    f32::max(0.0, lambda)
}

fn refine_centers(
    vectors: &[Vec<f32>],
    dim: usize,
    k: usize,
    result: &AssignmentResult,
    prev_centers: &[Vec<f32>],
) -> Vec<Vec<f32>> {
    let mut new_centers = vec![vec![0.0; dim]; k];

    // Sum up vectors per cluster
    for (i, v) in vectors.iter().enumerate() {
        let c = result.assignments[i];
        for (j, val) in v.iter().enumerate() {
            new_centers[c][j] += val;
        }
    }

    // Average or fall back to previous center
    for c in 0..k {
        let count = result.cluster_counts[c];
        if count > 0 {
            new_centers[c].iter_mut().for_each(|v| *v /= count as f32);
        } else {
            new_centers[c] = prev_centers[c].clone();
        }
    }

    new_centers
}

fn find_nearest_point_centroids(
    vectors: &[Vec<f32>],
    centers: &[Vec<f32>],
    _dim: usize,
    k: usize,
) -> Vec<Vec<f32>> {
    let mut nearest = vec![(f32::MAX, 0usize); k];

    for (i, v) in vectors.iter().enumerate() {
        for (c, center) in centers.iter().enumerate() {
            let dist = cosine_distance(v, center);
            if dist < nearest[c].0 {
                nearest[c] = (dist, i);
            }
        }
    }

    nearest
        .iter()
        .enumerate()
        .map(|(c, (_, idx))| {
            if *idx < vectors.len() {
                vectors[*idx].clone()
            } else {
                centers[c].clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize_vec(v: &[f32]) -> Vec<f32> {
        cosine::normalize(v)
    }

    #[test]
    fn test_balanced_kmeans_well_separated() {
        // Two well-separated clusters in 3D
        let dim = 3;
        let mut vectors = Vec::new();

        // Cluster A: near [1, 0, 0]
        for i in 0..50 {
            let v = normalize_vec(&[100.0 + i as f32 * 0.01, 0.1, 0.1]);
            vectors.push(v);
        }
        // Cluster B: near [0, 1, 0]
        for i in 0..50 {
            let v = normalize_vec(&[0.1, 100.0 + i as f32 * 0.01, 0.1]);
            vectors.push(v);
        }

        let result = balanced_kmeans(&vectors, dim, 2, 100.0);
        assert_eq!(result.num_clusters, 2);

        // Check that clusters are reasonably balanced
        for &size in &result.cluster_sizes {
            if size > 0 {
                // Each cluster should have roughly 50 points
                assert!(
                    size >= 20 && size <= 80,
                    "Cluster size {} is too unbalanced",
                    size
                );
            }
        }

        // Check that same-cluster points got the same assignment
        let cluster_a = result.assignments[0];
        for i in 1..50 {
            assert_eq!(
                result.assignments[i], cluster_a,
                "Point {i} in cluster A should have same assignment"
            );
        }
        let cluster_b = result.assignments[50];
        for i in 51..100 {
            assert_eq!(
                result.assignments[i], cluster_b,
                "Point {i} in cluster B should have same assignment"
            );
        }
        assert_ne!(cluster_a, cluster_b, "Clusters should be different");
    }

    #[test]
    fn test_balanced_kmeans_single_cluster() {
        let dim = 3;
        let vectors: Vec<Vec<f32>> = (0..20)
            .map(|i| normalize_vec(&[1.0 + i as f32 * 0.001, 0.5, 0.3]))
            .collect();

        let result = balanced_kmeans(&vectors, dim, 1, 100.0);
        assert_eq!(result.num_clusters, 1);
        assert_eq!(result.cluster_sizes[0], 20);
        for &a in &result.assignments {
            assert_eq!(a, 0);
        }
    }

    #[test]
    fn test_balanced_kmeans_empty() {
        let result = balanced_kmeans(&[], 3, 5, 100.0);
        assert_eq!(result.num_clusters, 0);
        assert!(result.assignments.is_empty());
    }

    #[test]
    fn test_split_2means_basic() {
        let dim = 2;
        let mut vectors = Vec::new();

        // Left cluster: near [1, 0]
        for i in 0..30 {
            vectors.push(normalize_vec(&[10.0 + i as f32 * 0.01, 0.1]));
        }
        // Right cluster: near [0, 1]
        for i in 0..30 {
            vectors.push(normalize_vec(&[0.1, 10.0 + i as f32 * 0.01]));
        }

        let (c0, left, c1, right) = split_2means(&vectors, dim);

        // Both clusters should be non-empty
        assert!(!left.is_empty(), "Left cluster should not be empty");
        assert!(!right.is_empty(), "Right cluster should not be empty");

        // Total should be 60
        assert_eq!(left.len() + right.len(), 60);

        // Centroids should be different
        let dist = cosine_distance(&c0, &c1);
        assert!(
            dist > 0.1,
            "Centroids should be well-separated, got distance {dist}"
        );
    }

    #[test]
    fn test_split_2means_single_point() {
        let dim = 3;
        let vectors = vec![normalize_vec(&[1.0, 2.0, 3.0])];
        let (_, left, _, right) = split_2means(&vectors, dim);
        assert_eq!(left.len() + right.len(), 1);
    }

    #[test]
    fn test_split_2means_empty() {
        let dim = 3;
        let (_, left, _, right) = split_2means(&[], dim);
        assert!(left.is_empty());
        assert!(right.is_empty());
    }

    #[test]
    fn test_balanced_kmeans_partition_sizes() {
        // Test that with lambda regularization, clusters are reasonably balanced
        let dim = 4;
        let mut rng = rand::rng();
        let vectors: Vec<Vec<f32>> = (0..200)
            .map(|_| {
                let v: Vec<f32> = (0..dim).map(|_| rng.random::<f32>()).collect();
                normalize_vec(&v)
            })
            .collect();

        let result = balanced_kmeans(&vectors, dim, 4, 100.0);

        // All points should be assigned
        assert_eq!(result.assignments.len(), 200);
        let total: usize = result.cluster_sizes.iter().sum();
        assert_eq!(total, 200);

        // No cluster should have more than 3x the average
        let avg = 200 / 4;
        for &size in &result.cluster_sizes {
            if size > 0 {
                assert!(
                    size <= avg * 3,
                    "Cluster size {size} exceeds 3x average {avg}"
                );
            }
        }
    }
}
