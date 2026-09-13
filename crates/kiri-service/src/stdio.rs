use crate::{RepositoryService, Service, jobs::TaskGroup, protocol::*};
use anyhow::{Context, Result, bail};
use futures::future::BoxFuture;
use kiri_analysis::{
    AnalysisCache, AnalysisRuntime, LanguageModel, PreparedAnalysis, cache::MemoryCache,
    progress::Observer,
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
    task::JoinSet,
};

type ModelWait = oneshot::Sender<Result<String>>;
struct PreparedEntry {
    request: u32,
    repo: Arc<RepositoryService>,
    evidence: Arc<PreparedAnalysis>,
    proposal: Mutex<()>,
    published: Mutex<Option<Arc<kiri_analysis::proposal::ProposalEvidence>>>,
    _lease: OwnedSemaphorePermit,
}
struct Session {
    service: Service,
    cache: Arc<dyn AnalysisCache>,
    repositories: Mutex<HashMap<RepoId, Arc<RepositoryService>>>,
    prepared: Mutex<HashMap<u32, Arc<PreparedEntry>>>,
    leases: Arc<Semaphore>,
    model_slots: Semaphore,
    callbacks: Mutex<HashMap<u32, (u32, ModelWait)>>,
    next: AtomicU32,
    output: mpsc::Sender<Frame>,
}
impl Session {
    fn id(&self) -> Result<u32> {
        self.next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| anyhow::anyhow!("Session ID space exhausted"))
    }
    async fn repo(&self, id: RepoId) -> Result<Arc<RepositoryService>> {
        self.repositories
            .lock()
            .await
            .get(&id)
            .cloned()
            .context("Unknown repository handle")
    }
    fn observer(&self, request: u32) -> Observer {
        let output = self.output.clone();
        Arc::new(move |event| {
            let _ = output.try_send(Frame::Progress { request, event });
        })
    }
    async fn reply(&self, id: u32, result: Result<ResultValue>) {
        let frame = match result {
            Ok(result) => Frame::Reply { id, result },
            Err(error) => Frame::Error {
                id,
                code: if error
                    .downcast_ref::<kiri_core::process::MutationCompletionUnknown>()
                    .is_some()
                {
                    ErrorCode::OutcomeUnknown
                } else {
                    ErrorCode::OperationFailed
                },
                message: error.to_string(),
            },
        };
        let _ = self.output.send(frame).await;
    }
    async fn read(self: Arc<Self>, id: u32, command: Command) -> Result<ResultValue> {
        match command {
            Command::Open { path } => {
                let Some(repo) = self.service.discover(path).await? else {
                    return Ok(ResultValue::NotRepository);
                };
                let mut entries = self.repositories.lock().await;
                if entries.len() >= 64 {
                    bail!("Close a repository handle before opening another");
                }
                let handle = RepoId(self.id()?);
                let root = repo.root().to_string_lossy().into_owned();
                repo.warm();
                entries.insert(handle, repo);
                Ok(ResultValue::Opened { repo: handle, root })
            }
            Command::Close { repo } => {
                let entry = self
                    .repositories
                    .lock()
                    .await
                    .remove(&repo)
                    .context("Unknown repository handle")?;
                if !self
                    .repositories
                    .lock()
                    .await
                    .values()
                    .any(|other| other.root() == entry.root())
                {
                    self.service.close(entry.root()).await;
                }
                Ok(ResultValue::Ok)
            }
            Command::Status { repo, fresh, since } => {
                let status = self.repo(repo).await?.status(fresh).await?;
                if since == Some(status.revision) {
                    return Ok(ResultValue::Unchanged {
                        revision: status.revision,
                    });
                }
                Ok(ResultValue::Status {
                    revision: status.revision,
                    status: (*status.status).clone(),
                })
            }
            Command::Preview {
                repo,
                path,
                side,
                large,
            } => {
                let entry = self.repo(repo).await?;
                let status = entry.status(false).await?;
                let file = status
                    .status
                    .files
                    .iter()
                    .find(|file| file.path == path)
                    .context("File is not in the current change set")?;
                let diff = entry.preview(file, side, large).await?;
                Ok(ResultValue::Preview {
                    patch: diff.raw.clone(),
                    partial: diff.truncated,
                    binary: diff.binary,
                    notice: diff.notice.clone(),
                })
            }
            Command::Compare {
                repo,
                path,
                comparison,
            } => Ok(ResultValue::Compared {
                preview: (*self.repo(repo).await?.compare(&path, &comparison).await?).clone(),
            }),
            Command::Log { repo, limit } => Ok(ResultValue::History {
                entries: self
                    .repo(repo)
                    .await?
                    .repository()
                    .workbench_log(limit)
                    .await?,
            }),
            Command::CommitFiles { repo, oid } => Ok(ResultValue::CommitFiles {
                files: self
                    .repo(repo)
                    .await?
                    .repository()
                    .commit_files(&oid)
                    .await?,
            }),
            Command::Files { repo } => Ok(ResultValue::Files {
                paths: self.repo(repo).await?.repository().list_files().await?,
            }),
            Command::Search {
                repo,
                term,
                case_sensitive,
                whole_word,
                regex,
            } => Ok(ResultValue::Search {
                lines: self
                    .repo(repo)
                    .await?
                    .repository()
                    .search(&term, case_sensitive, whole_word, regex)
                    .await?,
            }),
            Command::Operation { repo } => Ok(ResultValue::Operation {
                operation: self.repo(repo).await?.repository().operation().await?,
            }),
            Command::Propose {
                prepared,
                model,
                cache_identity,
                kind,
                instructions,
            } => {
                if model.is_empty()
                    || model.len() > 512
                    || instructions.len() > 12000
                    || cache_identity
                        .as_ref()
                        .is_some_and(|id| id.is_empty() || id.len() > 512)
                {
                    bail!("Invalid model handle or instructions");
                }
                let prepared = self
                    .prepared
                    .lock()
                    .await
                    .get(&prepared)
                    .cloned()
                    .context("Unknown prepared analysis")?;
                let model: Arc<dyn LanguageModel> = Arc::new(RemoteModel {
                    session: self.clone(),
                    request: id,
                    identity: cache_identity.unwrap_or_else(|| model.clone()),
                    model,
                });
                let worker: Arc<dyn LanguageModel> =
                    if let Some(worker) = &prepared.evidence.options.worker_model {
                        Arc::new(RemoteModel {
                            session: self.clone(),
                            request: id,
                            model: worker.clone(),
                            identity: format!("host-worker:{worker}"),
                        })
                    } else {
                        model.clone()
                    };
                let runtime = Arc::new(AnalysisRuntime {
                    model,
                    worker,
                    cache: self.cache.clone(),
                    observer: self.observer(id),
                });
                let _proposal = prepared
                    .proposal
                    .try_lock()
                    .context("An analysis is already running for this capture")?;
                if prepared.published.lock().await.is_some() {
                    bail!(
                        "This capture already has a proposal; prepare a new capture for another analysis"
                    );
                }
                let (reply, evidence) = match kind {
                    ProposalKind::Draft => {
                        let (draft, evidence) = kiri_analysis::proposal::draft_with_evidence(
                            prepared.repo.repository(),
                            prepared.evidence.clone(),
                            runtime,
                            &instructions,
                        )
                        .await?;
                        (ResultValue::Draft { draft }, evidence)
                    }
                    ProposalKind::Plan => {
                        let (plan, evidence) = kiri_analysis::proposal::plan_with_evidence(
                            prepared.repo.repository(),
                            prepared.evidence.clone(),
                            runtime,
                            &instructions,
                        )
                        .await?;
                        (ResultValue::Plan { plan }, evidence)
                    }
                };
                *prepared.published.lock().await = Some(Arc::new(evidence));
                Ok(reply)
            }
            Command::Inspect { prepared, request } => {
                let prepared = self
                    .prepared
                    .lock()
                    .await
                    .get(&prepared)
                    .cloned()
                    .context("Unknown prepared analysis")?;
                let result = kiri_analysis::AnalysisResult {
                    context: String::new(),
                    report: Default::default(),
                    nodes: Vec::new(),
                    root_ids: Vec::new(),
                };
                let published = prepared.published.lock().await.clone();
                let (capture, result) = match published.as_deref() {
                    Some(evidence) => (&evidence.prepared, &evidence.result),
                    None => (&prepared.evidence, &result),
                };
                Ok(ResultValue::Evidence {
                    page: kiri_analysis::inspect(capture, result, &request).await?,
                })
            }
            Command::Manifest { prepared } => {
                let entry = self
                    .prepared
                    .lock()
                    .await
                    .get(&prepared)
                    .cloned()
                    .context("Unknown evidence capture")?;
                let published = entry.published.lock().await.clone();
                let capture = published
                    .as_ref()
                    .map(|evidence| &evidence.prepared)
                    .unwrap_or(&entry.evidence);
                Ok(ResultValue::Manifest {
                    source: capture.snapshot.source().into(),
                    files: capture.files.clone(),
                    roots: published
                        .as_ref()
                        .map(|evidence| evidence.result.root_ids.clone())
                        .unwrap_or_default(),
                    units: capture
                        .units
                        .iter()
                        .map(|unit| SourceUnit {
                            id: unit.id.clone(),
                            bytes: unit.bytes,
                            sources: unit.sources.clone(),
                        })
                        .collect(),
                })
            }
            Command::Release { prepared } => {
                self.prepared.lock().await.remove(&prepared);
                Ok(ResultValue::Ok)
            }
            _ => bail!("Command requires the session control path"),
        }
    }
}
struct RemoteModel {
    session: Arc<Session>,
    request: u32,
    model: String,
    identity: String,
}
impl LanguageModel for RemoteModel {
    fn identity(&self) -> String {
        self.identity.clone()
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        input: &'a str,
        schema: serde_json::Value,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let _slot = self.session.model_slots.acquire().await?;
            let call = self.session.id()?;
            let (reply, response) = oneshot::channel();
            self.session
                .callbacks
                .lock()
                .await
                .insert(call, (self.request, reply));
            self.session
                .output
                .send(Frame::ModelCall {
                    request: self.request,
                    call,
                    model: self.model.clone(),
                    system: system.into(),
                    input: input.into(),
                    schema,
                })
                .await
                .context("Host disconnected")?;
            let result = tokio::time::timeout(Duration::from_secs(120), response).await;
            self.session.callbacks.lock().await.remove(&call);
            result
                .context("Host model request timed out")?
                .context("Host model request cancelled")?
        })
    }
}

pub async fn serve<R, W>(input: R, output: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    serve_with_cache(input, output, Arc::new(MemoryCache::default())).await
}

pub async fn serve_with_cache<R, W>(
    mut input: R,
    mut output: W,
    cache: Arc<dyn AnalysisCache>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel(64);
    let session = Arc::new(Session {
        service: Service::default(),
        cache,
        repositories: Mutex::new(HashMap::new()),
        prepared: Mutex::new(HashMap::new()),
        leases: Arc::new(Semaphore::new(16)),
        model_slots: Semaphore::new(32),
        callbacks: Mutex::new(HashMap::new()),
        next: AtomicU32::new(1),
        output: tx,
    });
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let bytes = serde_json::to_vec(&frame)?;
            if bytes.len() > MAX_FRAME_BYTES {
                bail!("Response frame exceeds the transport budget");
            }
            output.write_u32(bytes.len() as u32).await?;
            output.write_all(&bytes).await?;
            output.flush().await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let mut reads = TaskGroup::default();
    let mut writes = JoinSet::new();
    let mut ready = false;
    let mut last_request = 0;
    let outcome = async {
        loop {
            let length = match input.read_u32().await {
                Ok(length) => length as usize,
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(error.into()),
            };
            if length == 0 || length > MAX_FRAME_BYTES {
                bail!("Invalid request frame length");
            }
            let mut bytes = vec![0; length];
            input.read_exact(&mut bytes).await?;
            let value: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => {
                    session
                        .output
                        .send(Frame::Error {
                            id: 0,
                            code: ErrorCode::InvalidRequest,
                            message: "Invalid JSON request".into(),
                        })
                        .await?;
                    continue;
                }
            };
            let id = value
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .and_then(|id| u32::try_from(id).ok())
                .unwrap_or(0);
            if id <= last_request {
                session
                    .output
                    .send(Frame::Error {
                        id,
                        code: ErrorCode::InvalidRequest,
                        message: "Request IDs must increase; requests are never replayed".into(),
                    })
                    .await?;
                continue;
            }
            last_request = id;
            let request: Request = match serde_json::from_value(value) {
                Ok(request) => request,
                Err(_) => {
                    session
                        .output
                        .send(Frame::Error {
                            id,
                            code: ErrorCode::InvalidRequest,
                            message: "Invalid request arguments".into(),
                        })
                        .await?;
                    continue;
                }
            };
            if let Command::Hello { version, schema_hash } = request.command {
                if version != VERSION || schema_hash.as_deref() != schema()?["schema_hash"].as_str() {
                    session
                        .output
                        .send(Frame::Error {
                            id,
                            code: ErrorCode::ProtocolMismatch,
                            message: format!("Expected protocol {VERSION} with the matching schema. Update the engine and client together."),
                        })
                        .await?;
                    continue;
                }
                ready = true;
                session
                    .reply(
                        id,
                        Ok(ResultValue::Hello {
                            version: VERSION,
                            capabilities: vec![
                                "git".into(),
                                "scoped-commits".into(),
                                "hybrid-analysis".into(),
                                "host-models".into(),
                            ],
                        }),
                    )
                    .await;
                continue;
            }
            if !ready {
                bail!("Protocol handshake required");
            }
            match request.command {
                Command::ModelResult { call, result } => {
                    if let Some((_, reply)) = session.callbacks.lock().await.remove(&call) {
                        let result = match result {
                            ModelResult::Text { text } if text.len() <= 1024 * 1024 => Ok(text),
                            ModelResult::Error { code: Some(code), .. } if code == "context" => Err(kiri_analysis::ContextOverflow.into()),
                            ModelResult::Error { message, .. } => Err(anyhow::anyhow!(
                                "{}",
                                message.chars().take(2000).collect::<String>()
                            )),
                            _ => Err(anyhow::anyhow!("Model response exceeded its budget")),
                        };
                        let _ = reply.send(result);
                    }
                    session.reply(id, Ok(ResultValue::Ok)).await;
                }
                Command::Cancel { request } => {
                    let stopped = reads.stop(&request).await;
                    let mut entries = session.prepared.lock().await;
                    let previous = entries.len();
                    entries.retain(|_, entry| entry.request != request);
                    let released = entries.len() != previous;
                    drop(entries);
                    if !stopped && !released {
                        session
                            .reply(
                                id,
                                Err(anyhow::anyhow!(
                                    "Request is finished or is a non-cancellable mutation"
                                )),
                            )
                            .await;
                        continue;
                    }
                    session
                        .callbacks
                        .lock()
                        .await
                        .retain(|_, (owner, _)| *owner != request);
                    session
                        .output
                        .send(Frame::Error {
                            id: request,
                            code: ErrorCode::Cancelled,
                            message:
                                "Read or analysis cancelled; mutations are not cancelled or retried"
                                    .into(),
                        })
                        .await?;
                    session.reply(id, Ok(ResultValue::Ok)).await;
                }
                Command::Fence { repo } => {
                    let entry = match session.repo(repo).await {
                        Ok(entry) => entry,
                        Err(error) => {
                            session.reply(id, Err(error)).await;
                            continue;
                        }
                    };
                    let operation = entry.fence();
                    let session = session.clone();
                    reads.replace(id, async move {
                        session
                            .reply(id, operation.await.map(|_| ResultValue::Ok))
                            .await;
                    });
                }
                Command::StageAll { repo, side } => {
                    let entry = match session.repo(repo).await {
                        Ok(entry) => entry,
                        Err(error) => {
                            session.reply(id, Err(error)).await;
                            continue;
                        }
                    };
                    let operation = entry.stage_all(side);
                    let session = session.clone();
                    writes.spawn(async move {
                        session
                            .reply(id, operation.await.map(|_| ResultValue::Ok))
                            .await;
                    });
                }
                Command::CommitMessage {
                    repo,
                    message,
                    amend,
                } => {
                    let entry = match session.repo(repo).await {
                        Ok(entry) => entry,
                        Err(error) => {
                            session.reply(id, Err(error)).await;
                            continue;
                        }
                    };
                    let operation = entry.commit_message(message, amend);
                    let session = session.clone();
                    writes.spawn(async move {
                        session
                            .reply(
                                id,
                                operation.await.map(|oid| ResultValue::Committed { oid }),
                            )
                            .await;
                    });
                }
                Command::Push { repo, branch } => {
                    let entry = match session.repo(repo).await {
                        Ok(entry) => entry,
                        Err(error) => {
                            session.reply(id, Err(error)).await;
                            continue;
                        }
                    };
                    let operation = entry.push(branch);
                    let session = session.clone();
                    writes.spawn(async move {
                        session
                            .reply(id, operation.await.map(|_| ResultValue::Ok))
                            .await;
                    });
                }
                Command::Stage { repo, paths, side } => {
                    let entry = match session.repo(repo).await {
                        Ok(entry) => entry,
                        Err(error) => {
                            session.reply(id, Err(error)).await;
                            continue;
                        }
                    };
                    let operation = entry.stage(paths, side);
                    let session = session.clone();
                    writes.spawn(async move {
                        session
                            .reply(id, operation.await.map(|_| ResultValue::Ok))
                            .await;
                    });
                }
                Command::Commit { repo, draft } => {
                    let entry = match session.repo(repo).await {
                        Ok(entry) => entry,
                        Err(error) => {
                            session.reply(id, Err(error)).await;
                            continue;
                        }
                    };
                    let operation = entry.commit(draft);
                    let session = session.clone();
                    writes.spawn(async move {
                        session
                            .reply(
                                id,
                                operation.await.map(|oid| ResultValue::Committed { oid }),
                            )
                            .await;
                    });
                }
                Command::ApplyPlan { repo, plan } => {
                    let entry = match session.repo(repo).await {
                        Ok(entry) => entry,
                        Err(error) => {
                            session.reply(id, Err(error)).await;
                            continue;
                        }
                    };
                    let operation = entry.apply_plan(plan, |_, _| {});
                    let session = session.clone();
                    writes.spawn(async move {
                        session
                            .reply(
                                id,
                                operation
                                    .await
                                    .map(|commits| ResultValue::Applied { commits }),
                            )
                            .await;
                    });
                }
                Command::Prepare {
                    repo,
                    scope,
                    paths,
                    options,
                } => {
                    let entry = match session.repo(repo).await {
                        Ok(entry) => entry,
                        Err(error) => {
                            session.reply(id, Err(error)).await;
                            continue;
                        }
                    };
                    let lease = match session.leases.clone().try_acquire_owned() {
                        Ok(lease) => lease,
                        Err(_) => {
                            session
                                .reply(
                                    id,
                                    Err(anyhow::anyhow!(
                                        "Release an earlier analysis before preparing another"
                                    )),
                                )
                                .await;
                            continue;
                        }
                    };
                    let capture = entry.capture_changes(scope);
                    let session = session.clone();
                    reads.replace(id, async move {
                        let result = async {
                            let snapshot = capture.await?;
                            let prepared = Arc::new(
                                kiri_analysis::prepare_captured(
                                    entry.repository(),
                                    snapshot,
                                    paths.as_deref(),
                                    options,
                                    session.observer(id),
                                )
                                .await?,
                            );
                            let key = session.id()?;
                            let info = ResultValue::Prepared {
                                prepared: key,
                                files: prepared.files.len(),
                                bytes: prepared.input_bytes,
                                chunks: prepared.units.len(),
                                estimated_calls: prepared.estimated_calls,
                            };
                            session.prepared.lock().await.insert(
                                key,
                                Arc::new(PreparedEntry {
                                    request: id,
                                    repo: entry,
                                    evidence: prepared,
                                    proposal: Mutex::new(()),
                                    published: Mutex::new(None),
                                    _lease: lease,
                                }),
                            );
                            Ok(info)
                        }
                        .await;
                        session.reply(id, result).await;
                    });
                }
                command => {
                    if reads.len() >= 32 {
                        session
                            .reply(id, Err(anyhow::anyhow!("Too many concurrent requests")))
                            .await;
                        continue;
                    }
                    let session = session.clone();
                    reads.replace(id, async move {
                        let result = session.clone().read(id, command).await;
                        session.reply(id, result).await;
                    });
                }
            }
            while writes.try_join_next().is_some() {}
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    drop(reads);
    session.callbacks.lock().await.clear();
    while writes.join_next().await.is_some() {}
    drop(session);
    writer.await??;
    outcome
}
