use captures_account::{AccountClient, Error, Vault, VaultError};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

#[derive(Default)]
struct State {
    token: Option<String>,
    fail_load: bool,
    fail_save: bool,
    fail_delete: bool,
    calls: usize,
}
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);
impl Vault for Fake {
    fn load(&self) -> Result<Option<String>, VaultError> {
        let state = self.0.lock().unwrap();
        if state.fail_load {
            return Err(VaultError::Inaccessible);
        }
        Ok(state.token.clone())
    }
    fn save(&self, token: &str) -> Result<(), VaultError> {
        let mut state = self.0.lock().unwrap();
        state.calls += 1;
        if state.fail_save {
            return Err(VaultError::Inaccessible);
        }
        state.token = Some(token.into());
        Ok(())
    }
    fn delete(&self) -> Result<(), VaultError> {
        let mut state = self.0.lock().unwrap();
        if state.fail_delete {
            return Err(VaultError::Unavailable);
        }
        state.token = None;
        Ok(())
    }
}

#[derive(Debug)]
struct Request {
    line: String,
    headers: String,
    body: String,
}
fn server(replies: Vec<(&str, &str)>) -> (String, thread::JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let replies: Vec<_> = replies
        .into_iter()
        .map(|(status, body)| (status.to_owned(), body.to_owned()))
        .collect();
    let handle = thread::spawn(move || {
        replies
            .into_iter()
            .map(|(status, body)| {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline, "expected HTTP request did not arrive");
                            thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    assert!(reader.read_line(&mut header).unwrap() > 0, "request ended before headers");
                    if header == "\r\n" {
                        break;
                    }
                    if header.to_ascii_lowercase().starts_with("content-length:") {
                        length = header.split(':').nth(1).unwrap().trim().parse().unwrap();
                    }
                    headers.push_str(&header.to_ascii_lowercase());
                }
                let mut data = vec![0; length];
                reader.read_exact(&mut data).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nSet-Cookie: captures_session=wrong; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                Request {
                    line,
                    headers,
                    body: String::from_utf8(data).unwrap(),
                }
            })
            .collect()
    });
    (url, handle)
}

#[test]
fn explicit_bearer_flow_and_retry_without_second_otp() {
    let (url, handle) = server(vec![
        ("202 Accepted", r#"{"challengeId":"challenge"}"#),
        (
            "200 OK",
            r#"{"user":{"id":"u12","email":"a@example.com"},"token":"opaqueSECRET"}"#,
        ),
        ("200 OK", r#"{"user":{"id":"u12","email":"a@example.com"}}"#),
        ("204 No Content", ""),
    ]);
    let vault = Fake::default();
    vault.0.lock().unwrap().fail_save = true;
    let mut client = AccountClient::new(&url, vault.clone()).unwrap();
    assert_eq!(vault.0.lock().unwrap().calls, 0);
    assert_eq!(client.request_code("a@example.com").unwrap(), "challenge");
    assert_eq!(
        client.verify("challenge", "AB12CD"),
        Err(Error::Vault(VaultError::Inaccessible))
    );
    assert_eq!(
        client.verify("challenge", "AB12CD"),
        Err(Error::InvalidInput)
    );
    vault.0.lock().unwrap().fail_save = false;
    assert_eq!(client.retry_save().unwrap().unwrap().id, "u12");
    assert_eq!(
        vault.0.lock().unwrap().token.as_deref(),
        Some("opaqueSECRET")
    );
    assert_eq!(client.me().unwrap().email, "a@example.com");
    client.logout().unwrap();
    assert!(vault.0.lock().unwrap().token.is_none());
    let requests = handle.join().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(
        requests[0]
            .line
            .starts_with("POST /api/auth/email/request ")
    );
    assert_eq!(
        requests[1].body,
        r#"{"challengeId":"challenge","code":"AB12CD","transport":"bearer"}"#
    );
    assert!(requests[2].line.starts_with("GET /api/account/me "));
    for request in &requests {
        assert!(!request.headers.contains("cookie:"));
        assert!(!request.headers.contains("origin:"));
        assert!(!request.headers.contains("sec-fetch-site:"));
        assert!(!request.line.contains("opaqueSECRET"));
    }
    for request in &requests[2..] {
        assert!(
            request
                .headers
                .contains("authorization: bearer opaquesecret")
        );
    }
    assert!(!format!("{:?}", Error::Offline).contains("opaqueSECRET"));
}

#[test]
fn invalid_otp_malformed_offline_and_service_failure() {
    let (url, handle) = server(vec![
        (
            "400 Bad Request",
            r#"{"error":"invalid or expired code","attemptsRemaining":2}"#,
        ),
        (
            "503 Service Unavailable",
            r#"{"error":"accounts unavailable"}"#,
        ),
        ("200 OK", r#"{"user":{"id":"x","email":"e"}}"#),
    ]);
    let mut client = AccountClient::new(&url, Fake::default()).unwrap();
    assert_eq!(
        client.verify("id", "AAAAAA"),
        Err(Error::InvalidCode {
            attempts_remaining: Some(2)
        })
    );
    assert_eq!(client.verify("id", "AAAAAA"), Err(Error::Unavailable));
    assert_eq!(client.verify("id", "AAAAAA"), Err(Error::MalformedResponse));
    handle.join().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let client = AccountClient::new(&closed, Fake::default()).unwrap();
    assert_eq!(client.request_code("a@example.com"), Err(Error::Offline));
    assert_eq!(
        AccountClient::new("http://example.com", Fake::default()).err(),
        Some(Error::InvalidInput)
    );
}

#[test]
fn transient_lookup_keeps_vault_but_401_invalidates_and_retries_delete() {
    let (url, handle) = server(vec![
        ("503 Service Unavailable", ""),
        ("401 Unauthorized", ""),
    ]);
    let vault = Fake::default();
    vault.0.lock().unwrap().token = Some("opaqueSECRET".into());
    let mut client = AccountClient::new(&url, vault.clone()).unwrap();
    assert!(client.load().unwrap());
    assert_eq!(client.me(), Err(Error::Unavailable));
    assert_eq!(
        vault.0.lock().unwrap().token.as_deref(),
        Some("opaqueSECRET")
    );
    vault.0.lock().unwrap().fail_delete = true;
    assert_eq!(client.me(), Err(Error::Vault(VaultError::Unavailable)));
    assert_eq!(client.me(), Err(Error::InvalidSession));
    vault.0.lock().unwrap().fail_delete = false;
    client.clear_invalid().unwrap();
    assert!(vault.0.lock().unwrap().token.is_none());
    assert!(!client.load().unwrap());
    assert_eq!(handle.join().unwrap().len(), 2);
}

#[test]
fn logout_failure_retains_session_then_local_delete_retry() {
    let (url, handle) = server(vec![
        ("503 Service Unavailable", ""),
        ("204 No Content", ""),
    ]);
    let vault = Fake::default();
    vault.0.lock().unwrap().token = Some("opaqueSECRET".into());
    let mut client = AccountClient::new(&url, vault.clone()).unwrap();
    assert!(client.load().unwrap());
    assert_eq!(client.logout(), Err(Error::Unavailable));
    assert!(vault.0.lock().unwrap().token.is_some());
    vault.0.lock().unwrap().fail_delete = true;
    assert_eq!(client.logout(), Err(Error::Vault(VaultError::Unavailable)));
    vault.0.lock().unwrap().fail_delete = false;
    client.logout().unwrap();
    assert!(vault.0.lock().unwrap().token.is_none());
    assert_eq!(handle.join().unwrap().len(), 2);
}

#[test]
fn locked_vault_is_not_signed_out_and_construction_does_not_open_it() {
    let vault = Fake::default();
    vault.0.lock().unwrap().fail_load = true;
    let mut client = AccountClient::new("https://captur.es", vault.clone()).unwrap();
    assert_eq!(client.load(), Err(Error::Vault(VaultError::Inaccessible)));
    vault.0.lock().unwrap().fail_load = false;
    assert_eq!(client.load(), Ok(false));
}

#[test]
fn invalid_email_is_input_error() {
    let (url, handle) = server(vec![("400 Bad Request", r#"{"error":"invalid email"}"#)]);
    let client = AccountClient::new(&url, Fake::default()).unwrap();
    assert_eq!(
        client.request_code("not-an-address"),
        Err(Error::InvalidInput)
    );
    assert_eq!(handle.join().unwrap().len(), 1);
}

#[test]
fn malformed_responses_do_not_expose_tokens_or_follow_redirects() {
    let oversized = format!(r#"{{"challengeId":"{}"}}"#, "x".repeat(8192));
    let (url, handle) = server(vec![
        ("200 OK", r#"{"token":"secret-response-value","user":null}"#),
        ("202 Accepted", &oversized),
        ("302 Found\r\nLocation: /unexpected-route", ""),
    ]);
    let mut client = AccountClient::new(&url, Fake::default()).unwrap();
    let error = client.verify("id", "AB12CD").unwrap_err();
    assert_eq!(error, Error::MalformedResponse);
    assert!(!format!("{error:?}").contains("secret-response-value"));
    assert_eq!(
        client.request_code("a@example.com"),
        Err(Error::MalformedResponse)
    );
    assert_eq!(
        client.request_code("a@example.com"),
        Err(Error::MalformedResponse)
    );
    assert_eq!(handle.join().unwrap().len(), 3);
}
