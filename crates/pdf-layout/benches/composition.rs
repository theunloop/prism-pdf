#![allow(clippy::expect_used)] // A benchmark fixture failure invalidates the measurement setup.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use pdf_layout::{Composition, PageStyle, TextStyle};
use std::hint::black_box;

const ITEM_COUNTS: [usize; 3] = [10, 100, 1_000];

fn fixture(item_count: usize) -> Composition {
    Composition::new().page(PageStyle::a4(48.0), |page| {
        page.content().column(|column| {
            column.spacing(4.0);
            for item in 0..item_count {
                let line =
                    format!("Line item {item}: a wrapping description for composition throughput");
                column.item().text(&line, TextStyle::new().size(9.0));
            }
        });
    })
}

fn compose(c: &mut Criterion) {
    let mut group = c.benchmark_group("compose");
    for item_count in ITEM_COUNTS {
        group.throughput(Throughput::Elements(item_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(item_count),
            &item_count,
            |b, &count| {
                b.iter_batched(
                    || fixture(count),
                    |composition| {
                        black_box(composition.build().expect("fixture must compose"));
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(benches, compose);
criterion_main!(benches);
