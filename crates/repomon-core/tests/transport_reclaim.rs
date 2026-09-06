//! Isolates listener lifetime from unit tests that spawn children and can inherit its descriptor.

#![cfg(unix)]

use std::os::unix::fs::FileTypeExt;

use repomon_core::transport::{self, Endpoint};

#[tokio::test]
async fn reclaims_a_stale_socket_file_with_no_live_listener() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stale.sock");
    let endpoint = Endpoint::from_path(&path);
    let listener = transport::listen(&endpoint).await.unwrap();
    drop(listener);

    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_socket()
    );
    match transport::connect(&endpoint).await {
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused),
        Ok(_) => panic!("the stale-socket fixture still has a live listener"),
    }

    let _reclaimed = transport::listen(&endpoint)
        .await
        .expect("a stale, unconnectable socket file must not block a fresh bind");
    transport::connect(&endpoint)
        .await
        .expect("the reclaimed socket must accept connections");
}
