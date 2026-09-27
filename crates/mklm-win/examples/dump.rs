//! Prints the read-only system snapshot. Pass `--all` to include non-present keyboards.
//!
//! `cargo run -p mklm-win --example dump [-- --all]`

fn main() -> Result<(), mklm_win::Error> {
    let include_non_present = std::env::args().skip(1).any(|arg| arg == "--all");
    let report = mklm_win::snapshot_report(mklm_win::SnapshotOptions {
        include_non_present,
    })?;
    println!("{:#?}", report.snapshot);
    for issue in &report.issues {
        eprintln!("issue: {issue}");
    }
    Ok(())
}
