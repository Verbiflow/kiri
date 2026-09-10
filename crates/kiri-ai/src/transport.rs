use crate::{
    http::{self, RetryPolicy},
    progress::{Observer, silent},
};
use bytes::Bytes;
use futures::StreamExt;
use rig_core::http_client::{
    self as rig_http, HttpClientExt, LazyBody, Request, Response, StreamingResponse,
    multipart::MultipartForm,
};
use std::fmt;

#[derive(Clone)]
pub struct Transport {
    client: Option<reqwest::Client>,
    observer: Observer,
    azure_key: Option<String>,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            client: None,
            observer: silent(),
            azure_key: None,
        }
    }
}
impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KiriTransport(<redacted>)")
    }
}

impl Transport {
    pub fn new(client: reqwest::Client, observer: Observer) -> Self {
        Self {
            client: Some(client),
            observer,
            azure_key: None,
        }
    }
    pub fn azure(mut self, key: String) -> Self {
        self.azure_key = Some(key);
        self
    }

    fn prepare<T: Into<Bytes>>(
        &self,
        request: Request<T>,
    ) -> anyhow::Result<reqwest::RequestBuilder> {
        let (mut parts, body) = request.into_parts();
        let mut url = reqwest::Url::parse(&parts.uri.to_string())?;
        let mut key = None;
        let pairs: Vec<_> = url
            .query_pairs()
            .filter_map(|(name, value)| {
                if name == "key" {
                    key = Some(value.into_owned());
                    None
                } else {
                    Some((name.into_owned(), value.into_owned()))
                }
            })
            .collect();
        if key.is_some() {
            url.set_query(None);
            if !pairs.is_empty() {
                url.query_pairs_mut().extend_pairs(pairs);
            }
        }
        if self.azure_key.is_some() {
            parts.headers.remove(reqwest::header::AUTHORIZATION);
        }
        let client = match &self.client {
            Some(client) => client.clone(),
            None => http::client()?,
        };
        let mut request = client
            .request(parts.method, url)
            .headers(parts.headers)
            .body(body.into());
        if let Some(key) = key {
            request = request.header("x-goog-api-key", key);
        }
        if let Some(key) = &self.azure_key {
            request = request.header("api-key", key);
        }
        Ok(request)
    }
}

fn error(error: anyhow::Error) -> rig_http::Error {
    if error
        .downcast_ref::<kiri_analysis::ContextOverflow>()
        .is_some()
    {
        return rig_http::Error::Instance(Box::new(kiri_analysis::ContextOverflow));
    }
    if let Some(failure) = error.downcast_ref::<http::ProviderFailure>()
        && let Some(status) = failure.status
        && let Ok(status) = reqwest::StatusCode::from_u16(status)
    {
        return rig_http::Error::InvalidStatusCode(status);
    }
    rig_http::Error::Instance(error.into_boxed_dyn_error())
}

impl HttpClientExt for Transport {
    fn send<T, U>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = rig_http::Result<Response<LazyBody<U>>>> + Send + 'static
    where
        T: Into<Bytes> + Send,
        U: From<Bytes> + Send + 'static,
    {
        let request = self.prepare(request);
        let observer = self.observer.clone();
        async move {
            let response =
                http::send_with_retry(request.map_err(error)?, RetryPolicy::default(), &observer)
                    .await
                    .map_err(error)?;
            let mut result = Response::builder().status(response.status());
            if let Some(headers) = result.headers_mut() {
                *headers = response.headers().clone();
            }
            let body: LazyBody<U> = Box::pin(async move {
                Ok(U::from(Bytes::from(
                    http::bytes(response).await.map_err(error)?,
                )))
            });
            result.body(body).map_err(Into::into)
        }
    }

    fn send_multipart<U>(
        &self,
        _request: Request<MultipartForm>,
    ) -> impl Future<Output = rig_http::Result<Response<LazyBody<U>>>> + Send + 'static
    where
        U: From<Bytes> + Send + 'static,
    {
        futures::future::ready(Err(error(anyhow::anyhow!(
            "Commit analysis does not accept multipart requests"
        ))))
    }

    fn send_streaming<T>(
        &self,
        request: Request<T>,
    ) -> impl Future<Output = rig_http::Result<StreamingResponse>> + Send
    where
        T: Into<Bytes> + Send,
    {
        let request = self.prepare(request);
        let observer = self.observer.clone();
        async move {
            let response =
                http::send_with_retry(request.map_err(error)?, RetryPolicy::default(), &observer)
                    .await
                    .map_err(error)?;
            let mut result = Response::builder().status(response.status());
            if let Some(headers) = result.headers_mut() {
                *headers = response.headers().clone();
            }
            let stream = response.bytes_stream().scan(0_usize, |size, item| {
                let result = item
                    .map_err(|_| error(anyhow::anyhow!("Provider stream interrupted")))
                    .and_then(|bytes| {
                        *size += bytes.len();
                        if *size > http::RESPONSE_LIMIT {
                            Err(error(anyhow::anyhow!("Provider stream exceeded 1 MiB")))
                        } else {
                            Ok(bytes)
                        }
                    });
                futures::future::ready(Some(result))
            });
            let stream: rig_http::sse::BoxedStream = Box::pin(stream);
            result.body(stream).map_err(Into::into)
        }
    }
}
