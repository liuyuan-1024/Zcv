use std::{
    io::{self, BufRead as _, BufReader, Read as _, Write as _},
    process::{Child, ChildStderr, ChildStdout, Stdio},
    sync::Arc,
};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::{AsyncReadExt as _, FutureExt as _, TryStreamExt as _};
use gpui::http_client::{
    AsyncBody, HttpClient, Inner, RedirectPolicy, Request, Response, Url,
    http::{
        self, HeaderName, HeaderValue, StatusCode, Version,
        header::{LOCATION, USER_AGENT},
    },
};
use gpui_util::new_std_command;

pub(super) fn new() -> Result<Arc<dyn HttpClient>> {
    Ok(Arc::new(build_client(false)?))
}

fn build_client(no_proxy: bool) -> Result<UpdateHttpClient> {
    let user_agent_text = format!(
        "Zcv/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let user_agent = HeaderValue::from_str(&user_agent_text)?;
    Ok(UpdateHttpClient {
        user_agent,
        user_agent_text,
        no_proxy,
    })
}

struct UpdateHttpClient {
    user_agent: HeaderValue,
    user_agent_text: String,
    no_proxy: bool,
}

impl HttpClient for UpdateHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, Result<Response<AsyncBody>>> {
        let user_agent_text = self.user_agent_text.clone();
        let no_proxy = self.no_proxy;
        async move {
            let (parts, body) = request.into_parts();
            let body_bytes = match body.0 {
                Inner::Empty => None,
                Inner::Bytes(bytes) => {
                    let bytes = bytes.into_inner().to_vec();
                    (!bytes.is_empty()).then_some(bytes)
                }
                Inner::AsyncReader(mut reader) => {
                    let mut bytes = Vec::new();
                    reader.read_to_end(&mut bytes).await?;
                    (!bytes.is_empty()).then_some(bytes)
                }
            };
            let redirect_policy = parts.extensions.get::<RedirectPolicy>().cloned();
            let method = parts.method.as_str().to_owned();
            let uri = parts.uri.to_string();
            let headers = parts
                .headers
                .iter()
                .filter(|(name, _)| *name != USER_AGENT)
                .map(|(name, value)| {
                    let value = value
                        .to_str()
                        .with_context(|| format!("请求头 {name} 不是合法 ASCII"))?;
                    Ok(format!("{name}: {value}"))
                })
                .collect::<Result<Vec<_>>>()?;

            smol::unblock(move || {
                execute_curl(
                    &method,
                    &uri,
                    &user_agent_text,
                    &headers,
                    redirect_policy,
                    no_proxy,
                    body_bytes,
                )
            })
            .await
        }
        .boxed()
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }
}

fn execute_curl(
    method: &str,
    uri: &str,
    user_agent: &str,
    headers: &[String],
    redirect_policy: Option<RedirectPolicy>,
    no_proxy: bool,
    body_bytes: Option<Vec<u8>>,
) -> Result<Response<AsyncBody>> {
    let mut command = new_std_command("curl");
    command
        .args([
            "--silent",
            "--show-error",
            "--include",
            "--connect-timeout",
            "10",
            "--user-agent",
            user_agent,
            "-X",
            method,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if no_proxy {
        command.args(["--noproxy", "*"]);
    }

    let follows_redirects = match redirect_policy {
        Some(RedirectPolicy::FollowLimit(limit)) => {
            command.args(["--location", "--max-redirs", &limit.to_string()]);
            true
        }
        Some(RedirectPolicy::FollowAll) => {
            command.args(["--location", "--max-redirs", "100"]);
            true
        }
        Some(RedirectPolicy::NoFollow) | None => false,
    };

    for header in headers {
        command.args(["-H", header]);
    }

    if body_bytes.is_some() {
        command.args(["--data-binary", "@-"]).stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    command.args(["--", uri]);

    let mut child = command.spawn().context("无法启动系统 curl 客户端")?;
    if let Some(bytes) = body_bytes {
        let mut stdin = child.stdin.take().expect("已配置 stdin 管道");
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
    }

    let stdout = child.stdout.take().expect("已配置 stdout 管道");
    let mut stderr = child.stderr.take().expect("已配置 stderr 管道");
    let mut reader = BufReader::new(stdout);

    let parsed = match read_final_response_head(&mut reader, follows_redirects) {
        Ok(parsed) => parsed,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let mut stderr_text = String::new();
            let _ = stderr.read_to_string(&mut stderr_text);
            let detail = stderr_text.trim();
            if detail.is_empty() {
                return Err(error);
            }
            bail!("HTTP 请求失败：{detail}");
        }
    };

    let mut builder = http::Response::builder()
        .status(parsed.status)
        .version(parsed.version);
    for (name, value) in parsed.headers {
        builder = builder.header(name, value);
    }

    let (tx, rx) = smol::channel::bounded::<io::Result<Vec<u8>>>(4);
    std::thread::spawn(move || stream_curl_body(reader, stderr, child, tx));

    Ok(builder.body(AsyncBody::from_reader(rx.into_async_read()))?)
}

fn stream_curl_body(
    mut reader: BufReader<ChildStdout>,
    mut stderr: ChildStderr,
    mut child: Child,
    tx: smol::channel::Sender<io::Result<Vec<u8>>>,
) {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                match child.wait() {
                    Ok(status) if status.success() => {}
                    Ok(status) => {
                        let mut stderr_text = String::new();
                        let _ = stderr.read_to_string(&mut stderr_text);
                        let detail = stderr_text.trim();
                        let message = if detail.is_empty() {
                            format!("curl 异常退出（状态码 {status}）")
                        } else {
                            format!("HTTP 读取失败：{detail}")
                        };
                        let _ = tx.send_blocking(Err(io::Error::other(message)));
                    }
                    Err(error) => {
                        let _ = tx.send_blocking(Err(error));
                    }
                }
                return;
            }
            Ok(read) => {
                if tx.send_blocking(Ok(buffer[..read].to_vec())).is_err() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = tx.send_blocking(Err(error));
                return;
            }
        }
    }
}

struct ParsedResponseHead {
    version: Version,
    status: StatusCode,
    headers: Vec<(HeaderName, HeaderValue)>,
}

fn read_final_response_head(
    reader: &mut BufReader<ChildStdout>,
    follows_redirects: bool,
) -> Result<ParsedResponseHead> {
    loop {
        let mut status_line = String::new();
        let read = reader.read_line(&mut status_line)?;
        if read == 0 {
            bail!("HTTP 响应为空");
        }
        let trimmed = status_line.trim_end_matches(['\r', '\n']);
        let (version, status, reason) = parse_status_line(trimmed)?;

        let mut headers = Vec::new();
        let mut has_location = false;
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && let (Ok(name), Ok(value)) = (
                    HeaderName::from_bytes(name.trim().as_bytes()),
                    HeaderValue::from_str(value.trim()),
                )
            {
                if name == LOCATION {
                    has_location = true;
                }
                headers.push((name, value));
            }
        }

        let is_proxy_tunnel =
            status == StatusCode::OK && reason.eq_ignore_ascii_case("connection established");
        let is_redirect = follows_redirects
            && matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
            && has_location;
        if status.is_informational() || is_proxy_tunnel || is_redirect {
            continue;
        }
        return Ok(ParsedResponseHead {
            version,
            status,
            headers,
        });
    }
}

fn parse_status_line(line: &str) -> Result<(Version, StatusCode, &str)> {
    let mut parts = line.splitn(3, ' ');
    let version_str = parts
        .next()
        .ok_or_else(|| anyhow!("无效的 HTTP 状态行：{line}"))?;
    let status_str = parts
        .next()
        .ok_or_else(|| anyhow!("无效的 HTTP 状态行：{line}"))?;
    let reason = parts.next().unwrap_or("");
    let version = match version_str {
        "HTTP/1.0" => Version::HTTP_10,
        "HTTP/1.1" => Version::HTTP_11,
        "HTTP/2" | "HTTP/2.0" => Version::HTTP_2,
        "HTTP/3" | "HTTP/3.0" => Version::HTTP_3,
        _ => bail!("不支持的 HTTP 版本：{line}"),
    };
    let code: u16 = status_str
        .parse()
        .with_context(|| format!("无效的 HTTP 状态码：{line}"))?;
    let status = StatusCode::from_u16(code)?;
    Ok((version, status, reason))
}

#[cfg(test)]
#[path = "../test/network_client_tests.rs"]
mod tests;
