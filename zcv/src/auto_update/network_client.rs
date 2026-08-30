use std::any::type_name;
use std::mem;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Result;
use futures::{AsyncReadExt as _, FutureExt as _, TryStreamExt as _};
use gpui::http_client::{
    AsyncBody, HttpClient, Inner, RedirectPolicy, Request, Response, Url, http,
};
use reqwest::header::{HeaderMap, HeaderValue};

pub(super) fn new() -> Result<Arc<dyn HttpClient>> {
    let user_agent = HeaderValue::from_str(&format!(
        "Zcv/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ))?;
    let mut headers = HeaderMap::new();
    headers.insert(http::header::USER_AGENT, user_agent.clone());
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .connect_timeout(Duration::from_secs(10))
        .default_headers(headers)
        .build()?;

    Ok(Arc::new(UpdateHttpClient {
        client,
        runtime: runtime().handle().clone(),
        user_agent,
    }))
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("无法初始化自动更新 HTTP 运行时")
    })
}

struct UpdateHttpClient {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
    user_agent: HeaderValue,
}

impl HttpClient for UpdateHttpClient {
    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let runtime = self.runtime.clone();
        async move {
            let (parts, body) = request.into_parts();
            let body = match body.0 {
                Inner::Empty => reqwest::Body::default(),
                Inner::Bytes(bytes) => bytes.into_inner().into(),
                Inner::AsyncReader(mut reader) => {
                    let mut bytes = Vec::new();
                    reader.read_to_end(&mut bytes).await?;
                    bytes.into()
                }
            };
            let mut request = client
                .request(parts.method, parts.uri.to_string())
                .headers(parts.headers)
                .body(body);
            if let Some(policy) = parts.extensions.get::<RedirectPolicy>() {
                request = request.redirect_policy(match policy {
                    RedirectPolicy::NoFollow => reqwest::redirect::Policy::none(),
                    RedirectPolicy::FollowLimit(limit) => {
                        reqwest::redirect::Policy::limited(*limit as usize)
                    }
                    RedirectPolicy::FollowAll => reqwest::redirect::Policy::limited(100),
                });
            }

            let mut response = runtime.spawn(async move { request.send().await }).await??;
            let headers = mem::take(response.headers_mut());
            let mut builder = http::Response::builder()
                .status(response.status().as_u16())
                .version(response.version());
            *builder.headers_mut().expect("响应头 builder 应可用") = headers;
            let body = response
                .bytes_stream()
                .map_err(std::io::Error::other)
                .into_async_read();
            Ok(builder.body(AsyncBody::from_reader(body))?)
        }
        .boxed()
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }
}
