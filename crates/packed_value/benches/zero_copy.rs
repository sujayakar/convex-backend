//! Zero-copy benchmarks measuring the full data path
//!
//! This benchmark suite measures:
//! 1. Document serialization/deserialization
//! 2. Property access patterns (full vs partial)
//! 3. Simulated sync worker operations
//! 4. Memory allocation overhead

#![feature(try_blocks)]

use std::{
    collections::BTreeMap,
    hint::black_box,
};

use criterion::{
    criterion_group,
    criterion_main,
    BenchmarkId,
    Criterion,
    Throughput,
};
use packed_value::{
    ByteBuffer,
    PackedValue,
};
use serde_json::Value as JsonValue;
use value::{
    assert_obj,
    id_v6::DeveloperDocumentId,
    ConvexObject,
    ConvexValue,
    FieldName,
    InternalId,
    Size,
    TableNumber,
};

/// Create a realistic document with various field types
fn create_document(num_fields: usize) -> ConvexObject {
    let idv6 = DeveloperDocumentId::new(
        TableNumber::try_from(123).unwrap(),
        InternalId([0x16; 16]),
    );

    let mut fields = BTreeMap::new();
    fields.insert("_id".parse().unwrap(), ConvexValue::from(idv6));
    fields.insert(
        "_creationTime".parse().unwrap(),
        ConvexValue::from(1669665839541.7861),
    );

    for i in 0..num_fields {
        let field_name: FieldName = format!("field_{}", i).parse().unwrap();
        match i % 5 {
            0 => {
                fields.insert(field_name, ConvexValue::from(i as i64));
            }
            1 => {
                fields.insert(field_name, ConvexValue::try_from(format!("value_{}", i)).unwrap());
            }
            2 => {
                fields.insert(field_name, ConvexValue::from(i as f64 * 1.5));
            }
            3 => {
                fields.insert(field_name, ConvexValue::from(i % 2 == 0));
            }
            4 => {
                // Nested object
                let nested = assert_obj!(
                    "nested_int" => i as i64,
                    "nested_str" => format!("nested_{}", i)
                );
                fields.insert(field_name, ConvexValue::Object(nested));
            }
            _ => unreachable!(),
        }
    }

    fields.try_into().unwrap()
}

/// Benchmark: Full document serialization round-trip
fn bench_document_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("document_roundtrip");

    for num_fields in [10, 50, 100, 500] {
        let doc = create_document(num_fields);
        let doc_value = ConvexValue::Object(doc.clone());
        let doc_size = doc_value.size();
        group.throughput(Throughput::Bytes(doc_size as u64));

        // Current path: ConvexValue -> JSON string -> ConvexValue
        group.bench_with_input(
            BenchmarkId::new("json_roundtrip", num_fields),
            &doc_value,
            |b, v| {
                b.iter(|| {
                    let json_str = v.json_serialize().unwrap();
                    let json_value: JsonValue = serde_json::from_str(&json_str).unwrap();
                    ConvexValue::try_from(black_box(json_value)).unwrap()
                })
            },
        );

        // New path: ConvexValue -> PackedValue -> ConvexValue
        group.bench_with_input(
            BenchmarkId::new("packed_roundtrip", num_fields),
            &doc_value,
            |b, v| {
                b.iter(|| {
                    let packed = PackedValue::<ByteBuffer>::pack(v);
                    ConvexValue::try_from(black_box(packed)).unwrap()
                })
            },
        );

        // Best case: PackedValue clone (zero-copy reference)
        let packed = PackedValue::<ByteBuffer>::pack(&doc_value);
        group.bench_with_input(
            BenchmarkId::new("packed_clone", num_fields),
            &packed,
            |b, v| {
                b.iter(|| black_box(v.clone()))
            },
        );
    }
    group.finish();
}

/// Benchmark: Partial property access (simulates lazy hydration)
fn bench_partial_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("partial_access");

    for num_fields in [50, 100, 500] {
        let doc = create_document(num_fields);
        let doc_value = ConvexValue::Object(doc.clone());

        // Current: Full deserialization to access one field
        let json_str = doc_value.json_serialize().unwrap();
        group.bench_with_input(
            BenchmarkId::new("json_one_field", num_fields),
            &json_str,
            |b, s| {
                b.iter(|| {
                    let json_value: JsonValue = serde_json::from_str(s).unwrap();
                    let obj = json_value.as_object().unwrap();
                    black_box(obj.get("field_0").unwrap().clone())
                })
            },
        );

        // New: Lazy access to one field via PackedValue
        let packed = PackedValue::<ByteBuffer>::pack(&doc_value);
        group.bench_with_input(
            BenchmarkId::new("packed_one_field", num_fields),
            &packed,
            |b, p| {
                b.iter(|| {
                    let opened = p.clone().open().unwrap();
                    match opened {
                        packed_value::OpenedValue::Object(obj) => {
                            black_box(obj.get("field_0").unwrap())
                        }
                        _ => None,
                    }
                })
            },
        );

        // Access 10% of fields
        let fields_to_access: Vec<String> =
            (0..num_fields).step_by(10).map(|i| format!("field_{}", i)).collect();

        group.bench_with_input(
            BenchmarkId::new("json_10pct_fields", num_fields),
            &(&json_str, &fields_to_access),
            |b, (s, fields)| {
                b.iter(|| {
                    let json_value: JsonValue = serde_json::from_str(s).unwrap();
                    let obj = json_value.as_object().unwrap();
                    for field in fields.iter() {
                        black_box(obj.get(field));
                    }
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("packed_10pct_fields", num_fields),
            &(&packed, &fields_to_access),
            |b, (p, fields)| {
                b.iter(|| {
                    let opened = (*p).clone().open().unwrap();
                    match opened {
                        packed_value::OpenedValue::Object(obj) => {
                            for field in fields.iter() {
                                black_box(obj.get(field).unwrap());
                            }
                        }
                        _ => {}
                    }
                })
            },
        );
    }
    group.finish();
}

/// Benchmark: Simulated sync worker message serialization
fn bench_sync_message(c: &mut Criterion) {
    let mut group = c.benchmark_group("sync_message");

    // Simulate a transition with multiple document updates
    for num_docs in [1, 10, 50] {
        let docs: Vec<ConvexValue> = (0..num_docs)
            .map(|_| ConvexValue::Object(create_document(20)))
            .collect();

        let total_size: usize = docs.iter().map(|d| d.size()).sum();
        group.throughput(Throughput::Bytes(total_size as u64));

        // Current: Serialize each doc to JSON, combine into message
        group.bench_with_input(
            BenchmarkId::new("json_serialize", num_docs),
            &docs,
            |b, docs| {
                b.iter(|| {
                    let json_docs: Vec<String> = docs
                        .iter()
                        .map(|d| d.json_serialize().unwrap())
                        .collect();
                    // Simulate combining into a message
                    let message = serde_json::json!({
                        "type": "Transition",
                        "modifications": json_docs.iter()
                            .map(|s| serde_json::from_str::<JsonValue>(s).unwrap())
                            .collect::<Vec<_>>()
                    });
                    black_box(serde_json::to_string(&message).unwrap())
                })
            },
        );

        // New: Pack each doc, combine raw bytes
        group.bench_with_input(
            BenchmarkId::new("packed_serialize", num_docs),
            &docs,
            |b, docs| {
                b.iter(|| {
                    let packed_docs: Vec<PackedValue<ByteBuffer>> = docs
                        .iter()
                        .map(|d| PackedValue::pack(d))
                        .collect();
                    // Simulate combining - just measure the packing overhead
                    let total_bytes: usize = packed_docs.iter().map(|p| p.size()).sum();
                    black_box(total_bytes)
                })
            },
        );

        // Pre-packed passthrough (zero-copy ideal case)
        let pre_packed: Vec<PackedValue<ByteBuffer>> =
            docs.iter().map(|d| PackedValue::pack(d)).collect();
        group.bench_with_input(
            BenchmarkId::new("packed_passthrough", num_docs),
            &pre_packed,
            |b, packed_docs| {
                b.iter(|| {
                    // Ideal case: just clone references
                    let cloned: Vec<_> = packed_docs.iter().map(|p| p.clone()).collect();
                    let total_bytes: usize = cloned.iter().map(|p| p.size()).sum();
                    black_box(total_bytes)
                })
            },
        );
    }
    group.finish();
}

/// Benchmark: Memory allocation patterns
fn bench_memory_allocations(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory");

    let doc = create_document(100);
    let doc_value = ConvexValue::Object(doc);

    // Measure allocation count for JSON path
    group.bench_function("json_alloc_pattern", |b| {
        b.iter(|| {
            let json_str = doc_value.json_serialize().unwrap();
            let json_value: JsonValue = serde_json::from_str(&json_str).unwrap();
            black_box(ConvexValue::try_from(json_value).unwrap())
        })
    });

    // Measure allocation count for packed path
    group.bench_function("packed_alloc_pattern", |b| {
        b.iter(|| {
            let packed = PackedValue::<ByteBuffer>::pack(&doc_value);
            black_box(ConvexValue::try_from(packed).unwrap())
        })
    });

    // Zero-copy clone pattern
    let packed = PackedValue::<ByteBuffer>::pack(&doc_value);
    group.bench_function("packed_zero_copy_clone", |b| {
        b.iter(|| {
            // This should be nearly free - just reference counting
            black_box(packed.clone())
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_document_roundtrip,
    bench_partial_access,
    bench_sync_message,
    bench_memory_allocations,
);
criterion_main!(benches);
