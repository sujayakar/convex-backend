//! f32 -> f16 scalar quantization for vector storage.

use half::f16;

/// Quantize f32 vectors to f16.
pub fn quantize_f32_to_f16(vectors: &[f32]) -> Vec<u8> {
    let f16_values: Vec<f16> = vectors.iter().map(|&v| f16::from_f32(v)).collect();
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            f16_values.as_ptr() as *const u8,
            f16_values.len() * std::mem::size_of::<f16>(),
        )
    };
    bytes.to_vec()
}

pub fn dequantize_f16_to_f32(data: &[u8]) -> Vec<f32> {
    let num_elements = data.len() / std::mem::size_of::<f16>();
    let f16_values: &[f16] =
        unsafe { std::slice::from_raw_parts(data.as_ptr() as *const f16, num_elements) };
    f16_values.iter().map(|v| v.to_f32()).collect()
}

/// Compute dot product directly on f16 data (promoting to f32 for the math).
/// This avoids allocating a full f32 vector for each distance computation.
#[inline]
pub fn dot_product_f16(a_f16_bytes: &[u8], b_f32: &[f32], dimension: usize) -> f32 {
    debug_assert_eq!(a_f16_bytes.len(), dimension * 2);
    debug_assert_eq!(b_f32.len(), dimension);

    let a_f16: &[f16] =
        unsafe { std::slice::from_raw_parts(a_f16_bytes.as_ptr() as *const f16, dimension) };

    let mut sum = 0.0f32;
    for i in 0..dimension {
        sum += a_f16[i].to_f32() * b_f32[i];
    }
    sum
}

#[derive(Debug, Clone)]
pub struct QuantizationStats {
    pub max_abs_error: f32,
    pub mean_abs_error: f32,
    pub num_elements: usize,
}

pub fn quantization_error(original: &[f32]) -> QuantizationStats {
    let mut max_err = 0.0f32;
    let mut sum_err = 0.0f32;
    let n = original.len();

    for &v in original {
        let quantized = f16::from_f32(v).to_f32();
        let err = (v - quantized).abs();
        max_err = max_err.max(err);
        sum_err += err;
    }

    QuantizationStats {
        max_abs_error: max_err,
        mean_abs_error: if n > 0 { sum_err / n as f32 } else { 0.0 },
        num_elements: n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_f16() {
        let original = vec![1.0f32, -0.5, 0.0, 3.14159, -100.0, 0.001];
        let quantized = quantize_f32_to_f16(&original);
        let recovered = dequantize_f16_to_f32(&quantized);

        assert_eq!(recovered.len(), original.len());
        for (o, r) in original.iter().zip(recovered.iter()) {
            // f16 has ~3 decimal digits of precision
            assert!(
                (o - r).abs() < 0.01 * o.abs().max(1.0),
                "Mismatch: original={o}, recovered={r}"
            );
        }
    }

    #[test]
    fn test_dot_product_f16_accuracy() {
        use rand::Rng;
        let mut rng = rand::rng();
        let dim = 128;

        let a: Vec<f32> = (0..dim).map(|_| rng.random::<f32>() * 2.0 - 1.0).collect();
        let b: Vec<f32> = (0..dim).map(|_| rng.random::<f32>() * 2.0 - 1.0).collect();

        let a_f16_bytes = quantize_f32_to_f16(&a);
        let f16_dot = dot_product_f16(&a_f16_bytes, &b, dim);

        // Compare to f32 dot product with the dequantized values
        let a_dequant = dequantize_f16_to_f32(&a_f16_bytes);
        let f32_dot: f32 = a_dequant.iter().zip(b.iter()).map(|(x, y)| x * y).sum();

        assert!(
            (f16_dot - f32_dot).abs() < 1e-3,
            "f16 dot product mismatch: f16={f16_dot}, f32={f32_dot}"
        );
    }

    #[test]
    fn test_quantization_error_stats() {
        let values = vec![1.0f32, 2.0, 3.0, 0.5, -1.5];
        let stats = quantization_error(&values);
        assert!(stats.max_abs_error < 0.01);
        assert!(stats.mean_abs_error < 0.01);
        assert_eq!(stats.num_elements, 5);
    }

    #[test]
    fn test_quantization_preserves_cosine_similarity() {
        use rand::Rng;

        use crate::spann::cosine;

        let mut rng = rand::rng();
        let dim = 1536; // OpenAI embedding size

        let a: Vec<f32> = (0..dim).map(|_| rng.random::<f32>()).collect();
        let b: Vec<f32> = (0..dim).map(|_| rng.random::<f32>()).collect();

        let a_norm = cosine::preprocess(a.clone());
        let b_norm = cosine::preprocess(b.clone());
        let f32_sim = cosine::cosine_similarity(&a_norm, &b_norm);

        // Quantize and dequantize, then compute similarity
        let a_q = dequantize_f16_to_f32(&quantize_f32_to_f16(&a_norm));
        let b_q = dequantize_f16_to_f32(&quantize_f32_to_f16(&b_norm));
        let f16_sim = cosine::cosine_similarity(&a_q, &b_q);

        // f16 quantization should preserve cosine similarity to within ~0.01
        assert!(
            (f32_sim - f16_sim).abs() < 0.02,
            "Cosine similarity mismatch: f32={f32_sim}, f16={f16_sim}, diff={}",
            (f32_sim - f16_sim).abs()
        );
    }
}
