# verus-lint

Lint and analysis tool for Verus codebases (work in progress, private).

It reads Verus's VIR log (`--log vir`), loads functions and their uses into an
embedded DuckDB database, and runs SQL rules against it. See `DESIGN.md`.

```sh
verus-lint run --workspace . --toolchain "./coral/verify"   # extract, then run the rules
verus-lint extract --crate my-crate                          # facts only
verus-lint check --param min_fns=10                          # rules only
verus-lint query "SELECT count(*) FROM functions"
```

Only the Verus releases in `src/version.rs` are accepted (exit status 3
otherwise). Licensed under MIT.
