use std::{path::Path, sync::mpsc, time::Duration};

use anyhow::Result;
use console::style;
use notify::{EventKind, RecursiveMode, Watcher};
use rusqlite::Connection;

use crate::{commands::generate, D1CConfig};

pub fn run(conn: &Connection, config: &D1CConfig) -> Result<()> {
    println!("{} Watching for changes...", style("👀").cyan());
    println!("   - Queries: {}", config.queries_dir);

    let (tx, rx) = mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res| match res {
        Ok(event) => {
            let _ = tx.send(event);
        }
        Err(e) => println!("watch error: {:?}", e),
    })?;

    watcher.watch(Path::new(&config.queries_dir), RecursiveMode::Recursive)?;

    // Initial generation
    run_generate(conn, config);

    loop {
        match rx.recv() {
            Ok(event) => {
                // Filter out Access events (reads) which can cause loops if tools scan the dir
                if let EventKind::Access(_) = event.kind {
                    continue;
                }

                // Ignore schema.sql changes to avoid infinite loops since we generate it
                if event
                    .paths
                    .iter()
                    .any(|p| p.file_name().map(|n| n == "schema.sql").unwrap_or(false))
                {
                    continue;
                }

                // Filter out noise events (unknown/other) if they are causing issues,
                // but usually Modify/Create/Remove are what we want.

                // Debounce
                std::thread::sleep(Duration::from_millis(100));
                while let Ok(_) = rx.try_recv() {}

                // Log path for debugging
                let path_name = event
                    .paths
                    .first()
                    .map(|p| {
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    })
                    .unwrap_or_else(|| "unknown".to_string());

                println!("{} Change detected in {}", style("🔄").green(), path_name);
                run_generate(conn, config);
            }
            Err(e) => println!("watch error: {:?}", e),
        }
    }
}

fn run_generate(conn: &Connection, config: &D1CConfig) {
    match generate::run(conn, config) {
        Ok(_) => {
            println!("{} Generated {}", style("✅").green(), config.out_dir);
        }
        Err(e) => {
            // Use eprintln for errors to be standard compliant
            eprintln!("{} Error: {}", style("❌").red(), e);
        }
    }
}
