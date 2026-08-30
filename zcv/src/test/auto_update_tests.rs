use super::*;

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;

#[test]
fn platform_key_is_apple_silicon_only() {
    assert!(matches!(platform_key(), Ok("macos-aarch64")));
}

#[test]
fn declared_size_and_digest_are_both_required() {
    let content = b"zcv";
    let mut digest = Sha256::new();
    digest.update(content);
    let digest = digest.finalize();
    let asset = ReleaseAsset {
        url: "https://example.com/Zcv.zip".to_owned(),
        size: content.len() as u64,
        sha256: format!("{digest:x}"),
    };
    assert!(ensure_download_matches(content.len() as u64, &digest, &asset).is_ok());
    assert!(ensure_download_matches(1, &digest, &asset).is_err());
}

#[test]
fn each_update_failure_is_notified_only_once() {
    let idle = UpdateStatus::Idle;
    let failed = UpdateStatus::Failed {
        message: Arc::from("签名无效"),
    };

    assert_eq!(new_failure(&idle, &failed).as_deref(), Some("签名无效"));
    assert!(new_failure(&failed, &failed).is_none());
    assert!(new_failure(&failed, &idle).is_none());
}

#[test]
fn update_http_client_follows_redirects_and_streams_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for response in [
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{address}/latest.json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nmanifest".to_owned(),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            requests.push(request);
            stream.write_all(response.as_bytes()).unwrap();
        }
        requests
    });

    let client = new_http_client().unwrap();
    let mut response = futures::executor::block_on(client.get(
        &format!("http://{address}/start"),
        AsyncBody::empty(),
        true,
    ))
    .unwrap();
    let mut body = String::new();
    futures::executor::block_on(smol::io::AsyncReadExt::read_to_string(
        response.body_mut(),
        &mut body,
    ))
    .unwrap();

    assert_eq!(body, "manifest");
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with("GET /start HTTP/1.1\r\n"));
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains(&format!("zcv/{}", env!("CARGO_PKG_VERSION")))
    );
    assert!(requests[1].starts_with("GET /latest.json HTTP/1.1\r\n"));
}
