/// Normalize a vector so cosine similarity = dot product.
pub fn normalize(v: &[f32]) -> Vec<f32> {
    let norm = dot_product(v, v).sqrt();
    if norm > 0.0 {
        v.iter().map(|x| x / norm).collect()
    } else {
        v.to_vec()
    }
}

/// Cosine similarity between two pre-normalized vectors (= dot product).
#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(
        a.len(),
        b.len(),
        "Vector length mismatch in cosine_similarity"
    );
    dot_product(a, b)
}

/// Preprocess (normalize) a vector for cosine similarity.
pub fn preprocess(v: Vec<f32>) -> Vec<f32> {
    normalize(&v)
}

/// Dot product of two f32 slices. Uses SIMD when available.
#[inline]
pub fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            // Safety: we checked for AVX2+FMA support
            return unsafe { dot_product_avx2(a, b) };
        }
        if is_x86_feature_detected!("sse") {
            // Safety: we checked for SSE support
            return unsafe { dot_product_sse(a, b) };
        }
    }
    dot_product_scalar(a, b)
}

#[inline]
fn dot_product_scalar(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// AVX2+FMA dot product. Processes 8 floats at a time.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn dot_product_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let n = a.len();
    let chunks = n / 8;
    let remainder = n % 8;

    let mut sum = _mm256_setzero_ps();

    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();

    for i in 0..chunks {
        let offset = i * 8;
        let va = _mm256_loadu_ps(a_ptr.add(offset));
        let vb = _mm256_loadu_ps(b_ptr.add(offset));
        sum = _mm256_fmadd_ps(va, vb, sum);
    }

    // Horizontal sum of the 8 floats in the AVX2 register
    // sum = [s0, s1, s2, s3, s4, s5, s6, s7]
    let hi = _mm256_extractf128_ps(sum, 1); // [s4, s5, s6, s7]
    let lo = _mm256_castps256_ps128(sum); // [s0, s1, s2, s3]
    let sum128 = _mm_add_ps(lo, hi); // [s0+s4, s1+s5, s2+s6, s3+s7]
    let shuf = _mm_movehdup_ps(sum128); // [s1+s5, s1+s5, s3+s7, s3+s7]
    let sums = _mm_add_ps(sum128, shuf); // [s0+s1+s4+s5, -, s2+s3+s6+s7, -]
    let shuf2 = _mm_movehl_ps(sums, sums);
    let result = _mm_add_ss(sums, shuf2);
    let mut total = _mm_cvtss_f32(result);

    let start = chunks * 8;
    for i in 0..remainder {
        total += a[start + i] * b[start + i];
    }

    total
}

/// SSE dot product. Processes 4 floats at a time.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
unsafe fn dot_product_sse(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let n = a.len();
    let chunks = n / 4;
    let remainder = n % 4;

    let mut sum = _mm_setzero_ps();

    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();

    for i in 0..chunks {
        let offset = i * 4;
        let va = _mm_loadu_ps(a_ptr.add(offset));
        let vb = _mm_loadu_ps(b_ptr.add(offset));
        let prod = _mm_mul_ps(va, vb);
        sum = _mm_add_ps(sum, prod);
    }

    // Horizontal sum
    let shuf = _mm_movehdup_ps(sum);
    let sums = _mm_add_ps(sum, shuf);
    let shuf2 = _mm_movehl_ps(sums, sums);
    let result = _mm_add_ss(sums, shuf2);
    let mut total = _mm_cvtss_f32(result);

    let start = chunks * 4;
    for i in 0..remainder {
        total += a[start + i] * b[start + i];
    }

    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_unit_vector() {
        let v = vec![1.0, 0.0, 0.0];
        let n = normalize(&v);
        assert!((n[0] - 1.0).abs() < 1e-6);
        assert!((n[1]).abs() < 1e-6);
        assert!((n[2]).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_non_unit() {
        let v = vec![3.0, 4.0];
        let n = normalize(&v);
        let norm: f32 = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((n[0] - 0.6).abs() < 1e-6);
        assert!((n[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_zero_vector() {
        let v = vec![0.0, 0.0, 0.0];
        let n = normalize(&v);
        assert_eq!(n, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = normalize(&[1.0, 2.0, 3.0]);
        let b = normalize(&[1.0, 2.0, 3.0]);
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = normalize(&[1.0, 0.0, 0.0]);
        let b = normalize(&[0.0, 1.0, 0.0]);
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = normalize(&[1.0, 0.0, 0.0]);
        let b = normalize(&[-1.0, 0.0, 0.0]);
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-5);
    }

    #[test]
    fn test_preprocess_matches_normalize() {
        let v = vec![3.0, 4.0, 5.0];
        let preprocessed = preprocess(v.clone());
        let normalized = normalize(&v);
        for (a, b) in preprocessed.iter().zip(normalized.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_dot_product_scalar_matches_simd() {
        let mut rng = rand::rng();
        use rand::Rng;

        // Test various sizes including those that aren't multiples of 4 or 8
        for size in [1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 31, 32, 33, 100, 128, 1536] {
            let a: Vec<f32> = (0..size).map(|_| rng.random::<f32>() * 2.0 - 1.0).collect();
            let b: Vec<f32> = (0..size).map(|_| rng.random::<f32>() * 2.0 - 1.0).collect();

            let scalar = dot_product_scalar(&a, &b);
            let simd = dot_product(&a, &b);

            assert!(
                (scalar - simd).abs() < 1e-4,
                "Mismatch for size {size}: scalar={scalar}, simd={simd}, diff={}",
                (scalar - simd).abs()
            );
        }
    }

    #[test]
    fn test_simd_cosine_high_dim() {
        // Test with OpenAI embedding dimensions
        let mut rng = rand::rng();
        use rand::Rng;
        let dim = 1536;

        let a: Vec<f32> = (0..dim).map(|_| rng.random::<f32>()).collect();
        let b: Vec<f32> = (0..dim).map(|_| rng.random::<f32>()).collect();

        let a_norm = normalize(&a);
        let b_norm = normalize(&b);

        let sim = cosine_similarity(&a_norm, &b_norm);
        // Cosine similarity should be in [-1, 1]
        assert!(
            sim >= -1.0 - 1e-5 && sim <= 1.0 + 1e-5,
            "Cosine similarity should be in [-1, 1], got {sim}"
        );

        // Self-similarity should be ~1.0
        let self_sim = cosine_similarity(&a_norm, &a_norm);
        assert!(
            (self_sim - 1.0).abs() < 1e-5,
            "Self-similarity should be 1.0, got {self_sim}"
        );
    }
}
