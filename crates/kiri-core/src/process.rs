use anyhow::{Context, Result, bail};
use std::{
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

pub struct MutationOutput {
    pub status: ExitStatus,
    pub stdout_excerpt: Vec<u8>,
    pub stderr_excerpt: Vec<u8>,
    pub logs_abbreviated: bool,
}

#[derive(Debug)]
pub struct MutationCompletionUnknown;
impl std::fmt::Display for MutationCompletionUnknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Git completion could not be observed. Check repository history and state before retrying.")
    }
}
impl std::error::Error for MutationCompletionUnknown {}

async fn drain_log(
    mut reader: impl tokio::io::AsyncRead + Unpin,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::new();
    let mut abbreviated = false;
    let mut buffer = [0; 8192];
    loop {
        let size = reader.read(&mut buffer).await?;
        if size == 0 {
            break;
        }
        let retain = size.min(65536_usize.saturating_sub(retained.len()));
        retained.extend_from_slice(&buffer[..retain]);
        abbreviated |= retain < size;
    }
    Ok((retained, abbreviated))
}

pub async fn run_mutation(
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<MutationOutput> {
    let mut child = command
        .kill_on_drop(true)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start Git mutation")?;
    let stdout = child.stdout.take().context("Missing mutation stdout")?;
    let stderr = child.stderr.take().context("Missing mutation stderr")?;
    let stdin = child.stdin.take();
    let execution = async {
        let write = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                if let Err(error) = stdin.write_all(input).await
                    && error.kind() != std::io::ErrorKind::BrokenPipe
                {
                    return Err(error);
                }
                if let Err(error) = stdin.shutdown().await
                    && error.kind() != std::io::ErrorKind::BrokenPipe
                {
                    return Err(error);
                }
            }
            Ok::<_, std::io::Error>(())
        };
        let ((stdout_excerpt, out_cut), (stderr_excerpt, err_cut), (), status) =
            tokio::try_join!(drain_log(stdout), drain_log(stderr), write, child.wait())?;
        Ok::<_, std::io::Error>(MutationOutput {
            status,
            stdout_excerpt,
            stderr_excerpt,
            logs_abbreviated: out_cut || err_cut,
        })
    };
    match tokio::time::timeout(timeout, execution).await {
        Ok(Ok(output)) => Ok(output),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(MutationCompletionUnknown.into())
        }
    }
}

pub async fn run(
    mut command: Command,
    input: Option<&[u8]>,
    limit: usize,
    timeout: Duration,
) -> Result<Output> {
    let mut child = command
        .kill_on_drop(true)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start process")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Missing process stdout")?
        .take(limit as u64 + 1);
    let mut stderr = child.stderr.take().context("Missing process stderr")?;
    let stdin = child.stdin.take();
    let execution = async {
        let read_output = async {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await?;
            let truncated = bytes.len() > limit;
            if truncated {
                bytes.truncate(limit);
                child.kill().await?;
            }
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((bytes, truncated, status))
        };
        let read_error = async {
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = stderr.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                let remaining = 65536_usize.saturating_sub(bytes.len());
                bytes.extend_from_slice(&buffer[..n.min(remaining)]);
            }
            Ok::<_, std::io::Error>(bytes)
        };
        let write_input = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                stdin.write_all(input).await?;
                stdin.shutdown().await?;
            }
            Ok::<_, std::io::Error>(())
        };
        let ((stdout, truncated, status), stderr, ()) =
            tokio::try_join!(read_output, read_error, write_input)?;
        Ok::<_, anyhow::Error>(Output {
            status,
            stdout,
            stderr,
            truncated,
        })
    };
    match tokio::time::timeout(timeout, execution).await {
        Ok(result) => result,
        Err(_) => bail!(
            "Operation exceeded {} seconds; it was cancelled",
            timeout.as_secs_f32()
        ),
    }
}
