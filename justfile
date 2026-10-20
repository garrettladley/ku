# list available recipes
default:
    @just --list

# run the complete local quality gate
[group("ci")]
ci: fmt-check lint test test-release doc deny

# alias for the full quality gate
[group("ci")]
check: ci

# format with the pinned toolchain
[group("dev")]
fmt:
    cargo fmt --all

[group("ci")]
fmt-check:
    cargo fmt --all -- --check

[group("ci")]
lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# run workspace tests, accepts extra args: just test -- --nocapture
[group("ci")]
test *ARGS:
    cargo test --workspace --all-features --locked {{ARGS}}

[group("ci")]
test-release:
    cargo test --workspace --all-features --release --locked

# measure source coverage, accepts report options: just coverage --html
[group("ci")]
coverage *ARGS:
    cargo llvm-cov --workspace --all-features --locked --fail-under-lines 94 {{ARGS}}

[group("ci")]
msrv:
    cargo +1.94.0 check --workspace --all-targets --all-features --locked
    cargo +1.94.0 test --workspace --all-features --locked

[group("ci")]
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked

[group("ci")]
deny:
    cargo deny --locked check

[group("ci")]
audit:
    cargo deny --locked check advisories

[group("dev")]
build *ARGS:
    cargo build --workspace --locked {{ARGS}}

[group("dev")]
run *ARGS:
    cargo run -p ku-cli --locked -- {{ARGS}}

[group("dev")]
bench *ARGS:
    cargo bench -p ku-solvers --bench solve --locked -- {{ARGS}}

# compare serial batches with reused Rayon worker pools
[group("dev")]
bench-batch *ARGS:
    cargo bench -p ku-solvers --bench batch --locked -- {{ARGS}}

# compare complete, sparse, ambiguous, and unsatisfiable boards
[group("dev")]
bench-edges *ARGS:
    cargo bench -p ku-solvers --bench edge_cases --locked -- {{ARGS}}

# per-puzzle throughput, latency, nodes, and measured allocations
[group("dev")]
compare dataset repeats="100":
    cargo run -p ku-solvers --example compare --release --locked -- {{quote(dataset)}} {{quote(repeats)}}

# emit optimized solver assembly in target/release/deps/ku_solvers-*.s
[group("dev")]
asm:
    cargo rustc -p ku-solvers --lib --release --locked -- --emit=asm
