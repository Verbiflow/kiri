use anyhow::{Context, Result, bail};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> Result<()> {
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
