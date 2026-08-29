use std::{path::Path, sync::mpsc, time::Duration};

use anyhow::Result;
use notify::{EventKind, RecursiveMode, Watcher};

use crate::{config::Config, generator};

pub fn run(config: &Config, base: &Path) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })?;
    watcher.watch(&base.join(&config.migrations_dir), RecursiveMode::Recursive)?;
    watcher.watch(&base.join(&config.queries_dir), RecursiveMode::Recursive)?;

    run_generation(config, base);
    println!("watching migrations and queries");
    loop {
        let event = receiver.recv()??;
        if matches!(event.kind, EventKind::Access(_))
            || event
                .paths
                .iter()
                .any(|path| path.file_name().is_some_and(|name| name == "schema.sql"))
        {
            continue;
        }
        std::thread::sleep(Duration::from_millis(100));
        while receiver.try_recv().is_ok() {}
        run_generation(config, base);
    }
}

fn run_generation(config: &Config, base: &Path) {
    match generator::generate(config, base) {
        Ok(report) => println!("generated {} changed file(s)", report.written.len()),
        Err(error) => eprintln!("generation failed: {error:#}"),
    }
}
