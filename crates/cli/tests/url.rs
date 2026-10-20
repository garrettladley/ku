//! HTTP dataset loading through a local server, including redirects and failures.

use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

const PUZZLE: &str =
    "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

fn response(status: &str, body: &[u8], extra_headers: &str) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

// Real HTTP exchange with no external network, proxies, or unbounded accept.
fn run_with_responses(args: &[&str], responses: Vec<Vec<u8>>) -> Output {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/puzzles", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        for response in responses {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "HTTP request never arrived");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accepting HTTP request: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 8192, "unexpected request size");
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"GET /"));
            stream.write_all(&response).unwrap();
        }
    });
    let output = Command::new(env!("CARGO_BIN_EXE_ku"))
        .args(args)
        .args(["--url", &url])
        .env("NO_PROXY", "*")
        .output()
        .unwrap();
    server.join().unwrap();
    output
}

#[test]
fn url_import_transforms_each_supported_dataset_format() {
    for body in [
        format!("{PUZZLE}\n"),
        format!("quizzes,solutions\n{PUZZLE},{SOLUTION}\n"),
        format!("000000000000 {PUZZLE} 1.0\n"),
    ] {
        let output = run_with_responses(&["import"], vec![response("200 OK", body.as_bytes(), "")]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout,
            format!("puzzle,solution\n{PUZZLE},{SOLUTION}\n").as_bytes()
        );
    }
}

#[test]
fn url_solve_follows_redirects() {
    let output = run_with_responses(
        &["solve", "--unique"],
        vec![
            response("302 Found", b"", "Location: /redirected\r\n"),
            response("200 OK", PUZZLE.as_bytes(), ""),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        format!("puzzle,solution\n{PUZZLE},{SOLUTION}\n").as_bytes()
    );
}

#[test]
fn url_tui_loads_before_terminal_setup() {
    let output = run_with_responses(&["tui"], vec![response("200 OK", PUZZLE.as_bytes(), "")]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    assert!(!output.stdout.contains(&0x1b));
}

#[test]
fn bad_http_responses_fail_without_csv_output() {
    for (reply, message) in [
        (response("404 Not Found", b"missing", ""), "fetching"),
        (response("200 OK", b"\xff", ""), "reading"),
        (response("200 OK", b"123", ""), "parsing"),
        (response("200 OK", b"# empty\n", ""), "contains no puzzles"),
    ] {
        let output = run_with_responses(&["import"], vec![reply]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
    }
}

#[test]
fn redirect_loops_are_bounded() {
    let output = run_with_responses(
        &["import"],
        vec![response("302 Found", b"", "Location: /puzzles\r\n"); 6],
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("redirect"));
}

#[test]
fn source_conflicts_and_invalid_urls_fail_before_network_access() {
    for args in [
        vec!["import"],
        vec!["import", "file.csv", "--url", "http://127.0.0.1:1"],
        vec!["solve", PUZZLE, "--url", "http://127.0.0.1:1"],
        vec!["solve", "--pretty", "--url", "http://127.0.0.1:1"],
        vec!["tui", "--seed", "2", "--url", "http://127.0.0.1:1"],
        vec!["import", "--url", "file:///tmp/puzzles"],
        vec!["import", "--url", "http://user:pass@example.com/puzzles"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ku"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
