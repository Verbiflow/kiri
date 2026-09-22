use anyhow::{Context, Result, bail};
use std::{path::PathBuf, sync::Arc};

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(run());
    // Tokio's stdin uses a blocking read that cannot be cancelled. Once the
    // protocol has drained admitted mutations, do not let that read keep a
    // failed transport alive until the parent happens to close its input.
    runtime.shutdown_background();
    result
}

async fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref() {
        Some(arg) if arg == "--schema" => {
            if args.next().is_some() {
                bail!("Usage: kiri-engine --schema");
            }
            println!("{}", kiri_service::protocol::schema()?);
            Ok(())
        }
        Some(arg) if arg == "--cache-dir" => {
            let root = PathBuf::from(
                args.next()
                    .context("Specify a private analysis cache directory")?,
            );
            if args.next().is_some() {
                bail!("Usage: kiri-engine --cache-dir <directory>");
            }
            kiri_service::stdio::serve_with_cache(
                tokio::io::stdin(),
                tokio::io::stdout(),
                Arc::new(kiri_analysis::cache::FileCache::new(root)),
            )
            .await
        }
        None => kiri_service::stdio::serve(tokio::io::stdin(), tokio::io::stdout()).await,
        _ => bail!("Usage: kiri-engine [--cache-dir <directory> | --schema]"),
    }
}
