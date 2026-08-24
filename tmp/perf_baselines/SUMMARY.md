# Lumia perf baseline — 2026-08-20

## Environment

| Field | Value |
|-------|--------|
| date | 2026-08-20T13:39:19+08:00 |
| commit (baseline) | `3de7c9c` |
| host | nixos / Intel Xeon E5-2698B v3 @ 2.00GHz / 32 CPUs |
| gate | `RUNS=5 BENCH_SHIELD=0` `scripts/bench_all.sh` |
| raw log | `baseline_20260820_133919.txt` |

## Baseline aggregate (median)

| Suite | time med (s) | RSS med |
|-------|-------------:|---------|
| bench_cpu | 0.1374 | 14.2 MiB |
| bench_app | **1.0534** | **149.2 MiB** |
| bench_str | 0.0362 | 9.1 MiB |
| gc_churn | 0.1768 | 35.7 MiB |

### `bench_app` scenario split (baseline)

| Scenario | wall | RSS |
|----------|-----:|----:|
| word_freq | ~0.013 s | ~4 MiB |
| pipe_hof | ~0.011 s | ~3 MiB |
| map_bulk | ~0.192 s | ~42 MiB |
| set_churn | **~0.83 s** | **~109 MiB** |

## Optimizations landed

### 1. Unique HashOrdered grow (`5cb9f20`)

Unique-but-full Map/Set grow+rehash instead of Overlay; batch materialize; elems flatten-once.

| Metric | Baseline | After #1 |
|--------|-------:|------:|
| bench_app | 1.053 s / 149 MiB | 0.828 s / 29 MiB |

### 2. Native Set union/intersect/diff (this commit)

HIR no longer desugars algebra to `elems`+insert loops; RT builds one sized table.

| Metric | After #1 | After #2 | vs baseline |
|--------|-------:|------:|------------:|
| set_churn | ~0.68 s | **~0.074 s** | **~11×** |
| bench_app | 0.828 s / 29 MiB | **~0.24 s / ~33 MiB** | **~4.4× time** |

### 3. HashOrdered `order_start` prefix remove (`d66505a`)

| Metric | After #2 | After #3 | vs baseline |
|--------|-------:|------:|------------:|
| map_bulk | ~0.14 s | **~0.023 s** | **~8×** |
| bench_app | ~0.24 s | **~0.11 s** | **~9.5×** |

Checksums unchanged.

### 4. Set unique builder: find-or-claim + pow2 probe

Fused `open_hash_find_or_claim` (one probe vs contains+claim); power-of-two mask; known-absent grow. Same fuse on unique Map upsert.

| Metric | After #3 | After #4 |
|--------|-------:|------:|
| set_churn | ~0.074 s | **~0.063 s** |
| bench_app | ~0.11 s | **~0.099 s** |

Also: NSW nonneg propagates through NSW Add/Mul (more `nuw`); `lumia build` skips `cargo -p lumia_rt` when the archive is fresher than RT/ABI sources (`LUMIA_FORCE_RT_BUILD` to override). Release `bench_cpu` remains Domain-SR dominated — NSW does not move that suite.

### 5. Compile wall: link / `LIBRARY_PATH`

`build_app` was ~13s because a bloated `LIBRARY_PATH` (re-`source scripts/env.sh` stacking nix store globs) made the linker crawl ~2k dirs. Frontend/`--llvm-opt` were negligible (`check` ~0.02s; O3 vs none within noise).

| Fix | Effect |
|-----|--------|
| `env.sh` idempotent + ≤1 dir/family | re-source stays ~5 lib dirs |
| link clears `LIBRARY_PATH` | bloated parent env no longer slows `lumia build` |
| default `clang -fuse-ld=lld` | slightly faster than GNU ld |

| Metric | Before | After |
|--------|-------:|------:|
| `build --release bench_app` | ~13 s | **~0.26 s** |

### 6. Set grow / algebra / barrier peeps

Cell-walk grow (no `set_elem_at` redecode); `write_bytes` hash init; algebra `set_elem_at_unguarded` + known-new put; non-float `key_eq` identity; Int skips write barrier.

| Metric | After #4 | After #6 |
|--------|-------:|------:|
| set_churn | ~0.062 s | **~0.052 s** |
| bench_app | ~0.099 s | **~0.086 s** |
