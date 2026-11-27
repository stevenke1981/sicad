# Core fixtures

`core-fixtures.ts` holds REAL `promptcad-core` responses, not hand-written ones. They were
produced by piping requests into the release binary (`promptcad-core rpc`), using the
example documents from `examples/cad2/` with the example-only `expected_resolved` key removed:

```text
cd crates/promptcad-core && cargo build --release --locked --bins
cd ../../web && node scripts/gen-core-fixtures.mjs
```

Each export is declared `satisfies CoreResponse`, so `npm run typecheck` fails if the
generated types and the real output ever disagree. Re-run the script after changing the core.
