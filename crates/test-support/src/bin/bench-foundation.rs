//! Test-harness throughput only; does not measure database performance.
fn main() -> Result<(), String> {
    let started = std::time::Instant::now();
    vetra_test_support::campaign(42, 10_000)?;
    println!(
        "model-only: 10000 cases in {:?}; not a database benchmark",
        started.elapsed()
    );
    Ok(())
}
