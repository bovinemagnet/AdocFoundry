# Vendored stand-ins

## `evalexpr`

acdc-parser (the `adoc-dom` parser) depends on `evalexpr = "13"`, but `evalexpr` 12 and later are licensed AGPL-3.0-only, which AdocFoundry's permissive-only licence policy does not allow. acdc-parser uses exactly two functions from it, `eval_float` and `eval_int`, to evaluate arithmetic operands of `ifeval::[]` conditions.

`vendor/evalexpr` is AdocFoundry's own Apache-2.0 implementation of those two functions (numbers, `+ - * / %`, unary minus and parentheses), substituted for the crates.io package through `[patch.crates-io]` in the workspace `Cargo.toml`. It contains no code from the original crate. Remove the patch if acdc-parser drops the dependency or a permissively licensed release appears.
