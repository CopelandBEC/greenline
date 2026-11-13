//! Command line for agents and scripts: read a drawing set, propose markups
//! for review in the app, render, measure and export. `greenline-cli guide` first.

fn main() {
    std::process::exit(greenline::cli::main(std::env::args().skip(1).collect()));
}
