use host_core::whois::{read_http_response_for_test, WhoIsError};
use tokio::io::{duplex, AsyncWriteExt};

async fn read_response(response: &[u8]) -> Result<Vec<u8>, WhoIsError> {
    let (mut client, mut server) = duplex(4096);
    server.write_all(response).await.unwrap();
    server.shutdown().await.unwrap();
    read_http_response_for_test(&mut client).await
}

#[tokio::test]
async fn reads_content_length_response() {
    let body = read_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
        .await
        .unwrap();
    assert_eq!(body, b"{}");
}

#[tokio::test]
async fn reads_chunked_response() {
    let body = read_response(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n",
    )
    .await
    .unwrap();
    assert_eq!(body, br#"{"a":1}"#);
}

#[tokio::test]
async fn rejects_non_success_status() {
    let error = read_response(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap_err();
    assert_eq!(error, WhoIsError::HttpStatus(503));
}

#[tokio::test]
async fn reads_to_eof_without_length_or_chunking() {
    let body = read_response(b"HTTP/1.1 200 OK\r\n\r\nbody-from-eof")
        .await
        .unwrap();
    assert_eq!(body, b"body-from-eof");
}
