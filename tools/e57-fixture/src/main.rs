/*
 * Small fixture generation and verification tool for E57 files.
 *
 * It generates tiny E57 fixtures fully in memory from a fixed seed
 * (no external point cloud data needed) and verifies the round-trip
 * of XML section, binary sections, compressed vector data and CRC
 * checksums through the public API of the e57 library.
 * It also checks that locally corrupted files are rejected.
 *
 * Usage:
 *   e57-fixture gate [--seed N]
 *   e57-fixture generate <out.e57> [--case NAME] [--seed N]
 *   e57-fixture verify <file.e57> [--case NAME] [--seed N]
 */

use anyhow::{bail, Context, Result};
use e57_fixture::{generate, run_gate, verify_bytes, Budgets, FixtureCase, DEFAULT_SEED};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage:");
        eprintln!("  e57-fixture gate [--seed N]");
        eprintln!("  e57-fixture generate <out.e57> [--case NAME] [--seed N]");
        eprintln!("  e57-fixture verify <file.e57> [--case NAME] [--seed N]");
        bail!("Missing command");
    }

    let command = args[1].as_str();
    let positional: Vec<&String> = args
        .iter()
        .skip(2)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let seed = option_value(&args, "--seed")
        .map(|s| s.parse::<u64>().context("Invalid seed value"))
        .transpose()?
        .unwrap_or(DEFAULT_SEED);
    let case = option_value(&args, "--case")
        .map(|name| {
            FixtureCase::from_name(name).with_context(|| format!("Unknown fixture case '{name}'"))
        })
        .transpose()?
        .unwrap_or(FixtureCase::MultiPrototype);

    match command {
        "gate" => {
            let report = run_gate(seed, &FixtureCase::ALL, &Budgets::default())
                .map_err(|failure| anyhow::anyhow!("{failure}"))?;
            print!("{report}");
            println!("Fixture gate passed.");
        }
        "generate" => {
            let out = positional
                .first()
                .context("Usage: e57-fixture generate <out.e57> [--case NAME] [--seed N]")?;
            let fixture = generate(case, seed).map_err(|failure| anyhow::anyhow!("{failure}"))?;
            std::fs::write(out, &fixture.bytes)
                .with_context(|| format!("Failed to write fixture file '{out}'"))?;
            println!(
                "Generated fixture '{}' (seed {seed:#018x}, {} bytes) to '{out}'",
                case.name(),
                fixture.bytes.len()
            );
        }
        "verify" => {
            let file = positional
                .first()
                .context("Usage: e57-fixture verify <file.e57> [--case NAME] [--seed N]")?;
            let bytes = std::fs::read(file)
                .with_context(|| format!("Failed to read fixture file '{file}'"))?;
            let fixture = generate(case, seed).map_err(|failure| anyhow::anyhow!("{failure}"))?;
            let report = verify_bytes(case, seed, &bytes, &fixture.expected, &Budgets::default())
                .map_err(|failure| anyhow::anyhow!("{failure}"))?;
            println!(
                "Verified '{}' ({} bytes, {} records, xml-depth {}): round-trip ok",
                case.name(),
                report.file_bytes,
                report.records_checked,
                report.xml_depth
            );
        }
        other => bail!("Unknown command '{other}'"),
    }
    Ok(())
}

fn option_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}
