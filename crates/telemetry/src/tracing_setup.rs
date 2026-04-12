//! Tracing and logging initialization.
//!
//! Sets up structured logging with tracing-subscriber.
//! JSON output in production, pretty human-readable in development.

use tracing_subscriber::{fmt, EnvFilter, prelude::*};

pub fn init_tracing(json_output: bool) {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    if json_output {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().pretty())
            .init();
    }
}
