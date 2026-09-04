use std::time::Duration;

use prompting_time_core::providers::ProviderError;
use prompting_time_core::providers::process::{
    EVENT_CHANNEL_CAPACITY, JsonLineProcess, MAX_LINE_BYTES,
};
use serde_json::json;
use tokio::process::Command;

fn shell(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(script);
    command
}

#[tokio::test]
async fn transport_reads_valid_delayed_json_lines_in_order() {
    let command =
        shell("printf '%s\n' '{\"sequence\":1}'; sleep 0.02; printf '%s\n' '{\"sequence\":2}'");
    let mut process = JsonLineProcess::spawn(command).unwrap();

    assert_eq!(EVENT_CHANNEL_CAPACITY, 256);
    assert_eq!(
        process.recv().await.unwrap().unwrap(),
        json!({"sequence": 1})
    );
    assert_eq!(
        process.recv().await.unwrap().unwrap(),
        json!({"sequence": 2})
    );
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn transport_writes_json_lines_to_child_stdin() {
    let mut process =
        JsonLineProcess::spawn(shell("IFS= read -r line; printf '%s\n' \"$line\"")).unwrap();

    process.send(&json!({"request": "fixture"})).await.unwrap();

    assert_eq!(
        process.recv().await.unwrap().unwrap(),
        json!({"request": "fixture"})
    );
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn transport_surfaces_malformed_json_as_typed_error() {
    let mut process = JsonLineProcess::spawn(shell("printf '%s\n' 'not-json'")).unwrap();

    assert!(matches!(
        process.recv().await.unwrap(),
        Err(ProviderError::MalformedJson)
    ));
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn transport_rejects_frames_larger_than_eight_mib() {
    let script = "dd if=/dev/zero bs=1048576 count=9 2>/dev/null | tr '\\0' x; printf '\n'";
    let mut process = JsonLineProcess::spawn(shell(script)).unwrap();

    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), process.recv())
            .await
            .unwrap()
            .unwrap(),
        Err(ProviderError::OversizedFrame { limit }) if limit == 8 * 1024 * 1024
    ));
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn inbound_frame_limit_accepts_exact_boundary() {
    let payload_bytes = MAX_LINE_BYTES - 2;
    let script =
        format!("printf '\"'; head -c {payload_bytes} /dev/zero | tr '\\0' x; printf '\"\\n'");
    let mut process = JsonLineProcess::spawn(shell(&script)).unwrap();

    let value = tokio::time::timeout(Duration::from_secs(5), process.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(value.as_str().unwrap().len(), payload_bytes);
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn inbound_frame_limit_accepts_exact_boundary_with_crlf() {
    let payload_bytes = MAX_LINE_BYTES - 2;
    let script =
        format!("printf '\"'; head -c {payload_bytes} /dev/zero | tr '\\0' x; printf '\"\\r\\n'");
    let mut process = JsonLineProcess::spawn(shell(&script)).unwrap();

    let value = tokio::time::timeout(Duration::from_secs(5), process.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(value.as_str().unwrap().len(), payload_bytes);
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn stdout_eof_terminates_a_still_running_child_and_closes_the_stream() {
    let mut process = JsonLineProcess::spawn(shell("exec 1>&-; sleep 30")).unwrap();
    let pid = process.id();

    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), process.recv())
            .await
            .expect("stdout EOF must be surfaced promptly")
            .expect("stdout EOF must be surfaced as a typed error"),
        Err(ProviderError::StreamClosed)
    ));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), process.recv())
            .await
            .expect("event stream must close after the EOF error")
            .is_none()
    );
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
        "event stream closure must imply that the child was reaped"
    );
    tokio::time::timeout(Duration::from_secs(2), process.shutdown())
        .await
        .expect("shutdown after owner-driven cleanup must remain safe")
        .unwrap();
}

#[tokio::test]
async fn shutdown_kills_and_awaits_a_running_child() {
    let process = JsonLineProcess::spawn(shell("sleep 30")).unwrap();

    tokio::time::timeout(Duration::from_secs(2), process.shutdown())
        .await
        .expect("shutdown must not leave an owned child running")
        .unwrap();
}

#[tokio::test]
async fn exited_child_drains_more_than_one_channel_of_output() {
    let mut process = JsonLineProcess::spawn(shell(
        "i=0; while [ $i -lt 300 ]; do printf '{\"sequence\":%s}\\n' \"$i\"; i=$((i+1)); done",
    ))
    .unwrap();

    for expected in 0..300 {
        assert_eq!(
            process.recv().await.unwrap().unwrap(),
            json!({"sequence": expected})
        );
    }
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn stderr_is_drained_into_a_bounded_tail() {
    let mut process = JsonLineProcess::spawn(shell(
        "dd if=/dev/zero bs=1024 count=80 2>/dev/null | tr '\\0' e >&2; printf '{}\\n'",
    ))
    .unwrap();

    assert_eq!(process.recv().await.unwrap().unwrap(), json!({}));
    assert_eq!(process.stderr_snapshot().len(), 64 * 1024);
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_cancels_a_backpressured_reader_after_child_exit() {
    let process = JsonLineProcess::spawn(shell(
        "i=0; while [ $i -lt 300 ]; do printf '{\"sequence\":%s}\\n' \"$i\"; i=$((i+1)); done",
    ))
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::time::timeout(Duration::from_secs(2), process.shutdown())
        .await
        .expect("shutdown must cancel a stdout reader blocked by backpressure")
        .unwrap();
}

#[tokio::test]
async fn outbound_frame_limit_accepts_exact_boundary_and_rejects_one_more_byte() {
    let process = JsonLineProcess::spawn(shell(&format!(
        "head -c {} >/dev/null; sleep 30",
        MAX_LINE_BYTES + 1
    )))
    .unwrap();
    let sender = process.sender();
    let exact = "x".repeat(MAX_LINE_BYTES - 2);
    sender.send(&json!(exact)).await.unwrap();

    let oversized = "x".repeat(MAX_LINE_BYTES - 1);
    assert!(matches!(
        sender.send(&json!(oversized)).await,
        Err(ProviderError::OversizedFrame { limit }) if limit == MAX_LINE_BYTES
    ));
    process.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_cancels_a_pipe_filling_outbound_write() {
    let process = JsonLineProcess::spawn(shell("sleep 30")).unwrap();
    let sender = process.sender();
    let write =
        tokio::spawn(async move { sender.send(&json!("x".repeat(MAX_LINE_BYTES - 2))).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::time::timeout(Duration::from_secs(2), process.shutdown())
        .await
        .expect("shutdown must cancel a blocked stdin write")
        .unwrap();
    assert!(write.await.unwrap().is_err());
}

#[tokio::test]
async fn outbound_backpressure_is_bounded_without_killing_the_child() {
    let process = JsonLineProcess::spawn(shell("sleep 30")).unwrap();
    let pid = process.id();
    let mut writes = Vec::new();
    for _ in 0..4 {
        let sender = process.sender();
        writes.push(tokio::spawn(async move {
            sender.send(&json!("x".repeat(MAX_LINE_BYTES - 2))).await
        }));
    }
    eventually_finished(&writes).await;

    assert!(
        std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
        "outbound queue pressure must not terminate the provider"
    );
    process.shutdown().await.unwrap();
    let mut saw_backpressure = false;
    for write in writes {
        if matches!(
            write.await.unwrap(),
            Err(ProviderError::Transport { category }) if category == "stdin-backpressure"
        ) {
            saw_backpressure = true;
        }
    }
    assert!(saw_backpressure);
}

async fn eventually_finished(writes: &[tokio::task::JoinHandle<Result<(), ProviderError>>]) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !writes.iter().any(|write| write.is_finished()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded outbound queue must reject excess work promptly");
}

#[tokio::test]
async fn drop_requests_child_reaping_through_the_owner() {
    let process = JsonLineProcess::spawn(shell("sleep 30")).unwrap();
    let pid = process.id();
    let sender = process.sender();
    let write =
        tokio::spawn(async move { sender.send(&json!("x".repeat(MAX_LINE_BYTES - 2))).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(process);

    tokio::time::timeout(Duration::from_secs(2), async {
        while std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("drop must cause the owner task to reap its child");
    assert!(write.await.unwrap().is_err());
}
