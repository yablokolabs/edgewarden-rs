use criterion::{black_box, criterion_group, criterion_main, Criterion};
use edge_state::{DesiredState, Policy, ReportedState};

fn bench_diff(c: &mut Criterion) {
    let reported = ReportedState {
        policy_version: 39,
        software_version: "1.0.0".into(),
        applied_seq: 1,
    };
    let desired = DesiredState {
        seq: 2,
        policy: Policy {
            version: 42,
            ..Policy::default()
        },
        software_version: "1.0.0".into(),
    };
    c.bench_function("state_diff", |b| {
        b.iter(|| black_box(&reported).diff(black_box(&desired)))
    });
}

criterion_group!(benches, bench_diff);
criterion_main!(benches);
