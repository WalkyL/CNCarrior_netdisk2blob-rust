// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use admin_tui::{parse_args, run};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let output = match parse_args(&args) {
        Ok(config) => run(config).await,
        Err(error) => Err(error),
    };
    match output {
        Ok(output) => {
            println!("{output}");
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}
