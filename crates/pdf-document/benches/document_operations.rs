#![allow(clippy::expect_used)] // Generated fixtures: failures invalidate the benchmark setup.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use pdf_document::{Builder, Document, PageSpec, StdFont, merge};
use std::hint::black_box;

const PAGE_COUNTS: [usize; 3] = [1, 10, 100];

fn fixture(page_count: usize) -> Vec<u8> {
    let mut builder = Builder::new();
    for page in 0..page_count {
        let lines = (0..40)
            .map(|line| {
                format!(
                    "BT /F1 10 Tf 72 {} Td (Page {page}, line {line}) Tj ET\n",
                    740 - line * 16
                )
            })
            .collect::<String>();
        builder.add_page(PageSpec::new(lines.into_bytes()).standard_font("F1", StdFont::Helvetica));
    }
    builder.build()
}

fn parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for page_count in PAGE_COUNTS {
        let bytes = fixture(page_count);
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(page_count),
            &bytes,
            |b, bytes| {
                // Cloning the owned input is setup, not parser work.
                b.iter_batched(
                    || bytes.clone(),
                    |input| black_box(Document::open(input).expect("benchmark fixture must parse")),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize");
    for page_count in PAGE_COUNTS {
        let document = Document::open(fixture(page_count)).expect("benchmark fixture must parse");
        group.throughput(Throughput::Elements(page_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(page_count),
            &document,
            |b, document| {
                b.iter(|| black_box(document.save().expect("benchmark fixture must serialize")));
            },
        );
    }
    group.finish();
}

fn merge_documents(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge");
    for page_count in PAGE_COUNTS {
        let inputs = (0..4)
            .map(|_| Document::open(fixture(page_count)).expect("benchmark fixture must parse"))
            .collect::<Vec<_>>();
        let documents = inputs.iter().collect::<Vec<_>>();
        group.throughput(Throughput::Elements((page_count * documents.len()) as u64));
        group.bench_with_input(
            BenchmarkId::new("four_documents", page_count),
            &documents,
            |b, documents| {
                b.iter(|| black_box(merge(black_box(documents)).expect("fixtures must merge")));
            },
        );
    }
    group.finish();
}

criterion_group!(benches, parse, serialize, merge_documents);
criterion_main!(benches);
