#![feature(try_blocks)]
#![feature(panic_update_hook)]

//! Nitpick CLI: deterministic simulation testing for the Convex storage engine.
//!
//! Usage:
//!   nitpick replay <SCENARIO> <SEED>     Run a specific scenario with a given seed
//!   nitpick batch <SCENARIO>             Run many simulations with random seeds

use clap::{
    Parser,
    Subcommand,
};
use cmd_util::env::config_test;
use isolate::configure_v8_for_determinism;
use nitpick::{
    framework::{
        batch::run_batch,
        runner::{
            run_scenario,
            run_scenario_deterministic,
            Config,
        },
    },
    scenarios::{
        counter::CounterScenario,
        counter_js::CounterJsScenario,
        elle::ElleScenario,
        elle_js::ElleJsScenario,
        index_query_js::IndexQueryJsScenario,
        link_ring::LinkRingScenario,
        link_ring_js::LinkRingJsScenario,
        scheduled_js::ScheduledJsScenario,
    },
};

#[derive(Parser)]
#[command(name = "nitpick", about = "Deterministic simulation testing for Convex")]
struct Cli {
    /// Concurrent transactions per simulation
    #[arg(short, long, default_value_t = 5)]
    concurrency: usize,

    /// Number of transactions per simulation
    #[arg(short, long, default_value_t = 100)]
    transactions: usize,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Replay a specific simulation with a given seed
    Replay {
        /// Scenario name
        scenario: String,
        /// Pseudorandom seed
        seed: u64,
    },
    /// Run a batch of simulations with random seeds
    Batch {
        /// Scenario name
        scenario: String,
        /// Number of simulations
        #[arg(short, long, default_value_t = 1000)]
        simulations: usize,
        /// Number of worker threads
        #[arg(long, default_value_t = 4)]
        threads: usize,
    },
    /// List available scenarios
    List,
    /// Internal: run a determinism check in a fresh subprocess.
    /// Both run1 and run2 execute here with clean V8 state.
    #[command(hide = true)]
    DeterminismCheck {
        /// Scenario name
        scenario: String,
        /// Pseudorandom seed
        seed: u64,
    },
}

const SCENARIOS: &[&str] = &[
    "counter",
    "counter_js",
    "elle",
    "elle_js",
    "index_query_js",
    "link_ring",
    "link_ring_js",
    "scheduled_js",
];

fn main() -> anyhow::Result<()> {
    config_test();
    // Enable deterministic V8 execution before any V8 initialization occurs.
    // This disables background JIT compilation and concurrent GC sweeping,
    // eliminating non-determinism from V8's shared platform thread pool when
    // multiple simulations run in parallel.
    configure_v8_for_determinism();
    let cli = Cli::parse();

    match cli.command {
        Commands::List => {
            println!("Available scenarios:");
            for name in SCENARIOS {
                println!("  {name}");
            }
        },
        Commands::Replay { scenario, seed } => {
            let config = Config {
                transactions: cli.transactions,
                concurrency: cli.concurrency,
                seed,
            };
            run_scenario_by_name(&scenario, config)?;
        },
        Commands::Batch {
            scenario,
            simulations,
            threads,
        } => {
            let result = run_batch_by_name(
                &scenario,
                cli.concurrency,
                cli.transactions,
                simulations,
                threads,
            )?;
            if let Some((seed, err)) = result.failed {
                eprintln!("\nFailed on seed {seed}:");
                eprintln!("{err:?}");
                eprintln!(
                    "\nReplay: cargo run -p nitpick -- replay {scenario} {seed} -c {} -t {}",
                    cli.concurrency, cli.transactions
                );
                std::process::exit(1);
            }
            println!(
                "Batch complete: {}/{} passed",
                result.passed, result.total
            );
        },
        Commands::DeterminismCheck { scenario, seed } => {
            let config = Config {
                transactions: cli.transactions,
                concurrency: cli.concurrency,
                seed,
            };
            run_determinism_check_by_name(&scenario, config)?;
        },
    }
    Ok(())
}

fn run_scenario_by_name(name: &str, config: Config) -> anyhow::Result<()> {
    match name {
        "counter" => run_scenario(CounterScenario, config),
        "counter_js" => run_scenario(CounterJsScenario, config),
        "elle" => run_scenario(ElleScenario::default(), config),
        "elle_js" => run_scenario(ElleJsScenario, config),
        "index_query_js" => run_scenario(IndexQueryJsScenario, config),
        "link_ring" => run_scenario(LinkRingScenario::default(), config),
        "link_ring_js" => run_scenario(LinkRingJsScenario::default(), config),
        "scheduled_js" => run_scenario(ScheduledJsScenario, config),
        _ => {
            anyhow::bail!(
                "Unknown scenario: {name}. Available: {}",
                SCENARIOS.join(", ")
            )
        },
    }
}

/// Run a determinism check for a given scenario. Called from the subprocess.
fn run_determinism_check_by_name(name: &str, config: Config) -> anyhow::Result<()> {
    match name {
        "counter" => run_scenario_deterministic(CounterScenario, config),
        "counter_js" => run_scenario_deterministic(CounterJsScenario, config),
        "elle" => run_scenario_deterministic(ElleScenario::default(), config),
        "elle_js" => run_scenario_deterministic(ElleJsScenario, config),
        "index_query_js" => run_scenario_deterministic(IndexQueryJsScenario, config),
        "link_ring" => run_scenario_deterministic(LinkRingScenario::default(), config),
        "link_ring_js" => run_scenario_deterministic(LinkRingJsScenario::default(), config),
        "scheduled_js" => run_scenario_deterministic(ScheduledJsScenario, config),
        _ => {
            anyhow::bail!(
                "Unknown scenario: {name}. Available: {}",
                SCENARIOS.join(", ")
            )
        },
    }
}

fn run_batch_by_name(
    name: &str,
    concurrency: usize,
    transactions: usize,
    simulations: usize,
    threads: usize,
) -> anyhow::Result<nitpick::framework::batch::BatchResult> {
    let result = match name {
        "counter" => run_batch(CounterScenario, concurrency, transactions, simulations, threads),
        "counter_js" => {
            run_batch(CounterJsScenario, concurrency, transactions, simulations, threads)
        },
        "elle" => run_batch(
            ElleScenario::default(),
            concurrency,
            transactions,
            simulations,
            threads,
        ),
        "elle_js" => run_batch(ElleJsScenario, concurrency, transactions, simulations, threads),
        "index_query_js" => {
            run_batch(IndexQueryJsScenario, concurrency, transactions, simulations, threads)
        },
        "link_ring" => run_batch(
            LinkRingScenario::default(),
            concurrency,
            transactions,
            simulations,
            threads,
        ),
        "link_ring_js" => run_batch(
            LinkRingJsScenario::default(),
            concurrency,
            transactions,
            simulations,
            threads,
        ),
        "scheduled_js" => {
            run_batch(ScheduledJsScenario, concurrency, transactions, simulations, threads)
        },
        _ => {
            anyhow::bail!(
                "Unknown scenario: {name}. Available: {}",
                SCENARIOS.join(", ")
            )
        },
    };
    Ok(result)
}
