# verus-lint

Lint and analysis tool for Verus codebases (work in progress, private).

It reads Verus's VIR log (`--log vir`), loads functions and their uses into an
embedded DuckDB database, and runs SQL rules against it. See `DESIGN.md`.

```sh
verus-lint run --workspace . --toolchain "./coral/verify"   # extract, then run the rules
verus-lint extract --crate my-crate --exclude sea-lion-cuda-sys   # facts only; also reads verus-lint.toml
verus-lint check --rules lints/sql --param min_fns=10        # your rules only (none are built in; see examples/)
verus-lint query "SELECT count(*) FROM functions"
```

Only the Verus releases in `src/version.rs` are accepted (exit status 3
otherwise). Licensed under MIT.
