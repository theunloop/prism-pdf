# Release-candidate performance baseline

This is the first committed Criterion baseline for the §7 hot paths: parse, serialize, merge, and
declarative composition. It was recorded on 2026-08-24 in the aarch64 Linux devcontainer with the
release profile. Hardware-dependent absolute times are orientation, not universal pass/fail limits;
Criterion's saved statistical baseline is the comparison mechanism on the same runner.

Commands:

```bash
cargo bench -p prismpdf-document --bench document_operations -- \
  --noplot --sample-size 20 --warm-up-time 1 --measurement-time 1
cargo bench -p prismpdf-layout --bench composition -- \
  --noplot --sample-size 20 --warm-up-time 1 --measurement-time 1
```

| Operation | Fixture | Estimate interval |
|---|---:|---:|
| parse | 1 page | 1.108–1.122 µs |
| parse | 10 pages | 2.674–2.698 µs |
| parse | 100 pages | 17.34–17.65 µs |
| serialize | 1 page | 4.725–4.823 µs |
| serialize | 10 pages | 38.90–39.70 µs |
| serialize | 100 pages | 342.3–344.9 µs |
| merge | four 1-page documents | 24.90–30.77 µs |
| merge | four 10-page documents | 208.1–209.2 µs |
| merge | four 100-page documents | 1.969–2.015 ms |
| compose | 10 line items | 153.4–154.3 µs |
| compose | 100 line items | 1.542–1.557 ms |
| compose | 1,000 line items | 15.49–15.59 ms |

There is no earlier committed performance baseline, so this release candidate cannot make a
historical regression claim. It establishes the comparison point required for subsequent release
candidates. Throughput is close to linear across fixture sizes; no scaling anomaly was observed.

The release-candidate validation rerun on 2026-08-24 reproduced the document-operation intervals.
Composition showed runner variance across consecutive samples: the first run measured
145.9–147.7 µs, 1.541–1.555 ms, and 15.30–15.41 ms, while a later run measured
159.4–181.0 µs, 1.585–1.645 ms, and 16.06–16.20 ms. The largest stable-fixture difference from
the baseline was about 0.61 ms for a 1,000-item document. That does not materially affect the
composition journey and does not change the linear scaling conclusion; future release candidates
should compare on a quiet dedicated runner before attributing a change to code.

For a same-runner comparison, preserve Criterion's `target/criterion` data and use its baseline
flags. A material regression is one whose confidence interval no longer overlaps the baseline and
whose magnitude affects a documented journey; explain or fix it before release.
