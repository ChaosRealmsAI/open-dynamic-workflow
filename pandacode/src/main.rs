mod agent;
mod cli;
mod client;
mod config;
mod fx;
mod io;
mod models;
mod output;
mod prompt;
mod providers;
mod runtimes;
mod session;

use anyhow::Result;
use clap::Parser;
use cli::{BambooRuntimeCommand, Cli, Commands, RuntimeCommand};
use serde_json::json;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let wants_json = command_wants_json(&cli.command);
    if let Err(error) = run(cli).await {
        if error.downcast_ref::<io::JsonAlreadyEmitted>().is_some() {
            std::process::exit(1);
        }
        if wants_json {
            print_json_error(&error);
        } else {
            eprintln!("Error: {error:#}");
        }
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::ClaudeHook(args) => runtimes::claude::record_hook(args),
        Commands::Run(args) => runtimes::run_agent_task(args, "exec").await,
        Commands::Resume(args) => runtimes::run_agent_task(args, "resume").await,
        Commands::Answer(args) => runtimes::answer_agent(args).await,
        Commands::Status(args) => runtimes::session_agent(args, "status").await,
        Commands::Logs(args) => runtimes::logs_agent(args).await,
        Commands::Artifacts(args) => runtimes::session_agent(args, "artifacts").await,
        Commands::Interrupt(args) => runtimes::session_agent(args, "interrupt").await,
        Commands::Stop(args) => runtimes::session_agent(args, "stop").await,
        Commands::Doctor(args) => runtimes::doctor(args).await,
        Commands::List(args) => runtimes::list_all(args),
        Commands::Models(args) => runtimes::models_all(args).await,
        Commands::Codex(command) => runtimes::codex::run(command),
        Commands::Claude(command) => runtimes::claude::run(command),
        Commands::Bamboo(command) => runtimes::bamboo::run(*command).await,
    }
}

fn print_json_error(error: &anyhow::Error) {
    let chain = error
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>();
    let payload = json!({
        "ok": false,
        "state": "failed",
        "error": {
            "message": error.to_string(),
            "chain": chain
        }
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(text) => println!("{text}"),
        Err(_) => println!(
            "{{\"ok\":false,\"state\":\"failed\",\"error\":{{\"message\":\"{}\"}}}}",
            error
        ),
    }
}

fn command_wants_json(command: &Commands) -> bool {
    match command {
        Commands::ClaudeHook(_) => false,
        Commands::Run(args) | Commands::Resume(args) => args.common.json,
        Commands::Answer(args) => args.common.json,
        Commands::Status(args)
        | Commands::Artifacts(args)
        | Commands::Interrupt(args)
        | Commands::Stop(args) => args.common.json,
        Commands::Logs(args) => args.common.json,
        Commands::Doctor(args) | Commands::List(args) | Commands::Models(args) => args.json,
        Commands::Codex(command) | Commands::Claude(command) => runtime_command_wants_json(command),
        Commands::Bamboo(command) => bamboo_command_wants_json(command),
    }
}

fn runtime_command_wants_json(command: &RuntimeCommand) -> bool {
    match command {
        RuntimeCommand::Exec(args) | RuntimeCommand::Resume(args) => args.json,
        RuntimeCommand::Answer(args) => args.json,
        RuntimeCommand::Status(args)
        | RuntimeCommand::Artifacts(args)
        | RuntimeCommand::Interrupt(args)
        | RuntimeCommand::Stop(args) => args.json,
        RuntimeCommand::Logs(args) => args.json,
        RuntimeCommand::Model(args) => args.json,
        RuntimeCommand::Models(args)
        | RuntimeCommand::List(args)
        | RuntimeCommand::Doctor(args) => args.json,
    }
}

fn bamboo_command_wants_json(command: &BambooRuntimeCommand) -> bool {
    match command {
        BambooRuntimeCommand::Exec(args) | BambooRuntimeCommand::Resume(args) => args.common.json,
        BambooRuntimeCommand::Answer(args) => args.json,
        BambooRuntimeCommand::Status(args)
        | BambooRuntimeCommand::Artifacts(args)
        | BambooRuntimeCommand::Interrupt(args)
        | BambooRuntimeCommand::Stop(args) => args.json,
        BambooRuntimeCommand::Logs(args) => args.json,
        BambooRuntimeCommand::Model(args) => args.common.json,
        BambooRuntimeCommand::Models(args) | BambooRuntimeCommand::List(args) => args.common.json,
        BambooRuntimeCommand::Doctor(args) => args.common.json,
    }
}
