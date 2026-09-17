use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_backoff(c: &mut Criterion) {
    use std::time::Duration;
    c.bench_function("backoff_delay", |b| {
        b.iter(|| {
            edge_protocol::Backoff::delay_for_attempt(
                black_box(Duration::from_millis(100)),
                black_box(Duration::from_secs(30)),
                black_box(3),
            )
        })
    });
}

criterion_group!(benches, bench_backoff);
criterion_main!(benches);
