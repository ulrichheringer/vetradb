//! Bounded deterministic persistence-model mutation campaign, not coverage-guided codec fuzzing.
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: fuzz-foundation <seed:u64> <cases:0..100000>".into());
    }
    let seed = args[0].parse::<u64>().map_err(|e| e.to_string())?;
    let cases = args[1].parse::<u32>().map_err(|e| e.to_string())?;
    vetra_test_support::campaign(seed, cases)?;
    println!("persistence model: seed={seed} cases={cases} passed");
    Ok(())
}
