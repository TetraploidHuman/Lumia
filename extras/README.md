# extras/ — optional domain modules (not language std)

Thin foreign wrappers for CogniNucleus / EFE benchmarks. These are **not** part of
the Lumia language standard library (`std.io` / `std.option` / …).

```lumia
import extras.cn.{nucleusStep, hebbian}
import extras.efe.{actionScores}
```

Resolved from the compiler workspace `extras/` tree (same layout idea as `std/`,
but intentionally outside the language core). Used by `examples/bench_cn_*.lm`
and `scripts/bench_cn_*.sh`.
