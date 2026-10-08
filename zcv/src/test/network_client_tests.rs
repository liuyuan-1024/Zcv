use super::*;

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;

#[test]
fn follows_redirects_and_streams_response() {
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

    // 本地测试服务器必须直连，避免系统代理接管 127.0.0.1 请求。
    let client = build_client(reqwest::Client::builder().no_proxy()).unwrap();
    let mut response = futures::executor::block_on(client.get(
        &format!("http://{address}/start"),
        AsyncBody::empty(),
        true,
    ))
    .unwrap();
    assert_eq!(response.status().as_u16(), 200, "必须跟随重定向");
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
