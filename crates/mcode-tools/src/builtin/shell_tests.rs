// Rust guideline compliant 2026-08-27.

use super::*;

fn decode_utf16le_base64(encoded: &str) -> String {
    let bytes = BASE64_STANDARD.decode(encoded).unwrap();
    let (chunks, remainder) = bytes.as_chunks::<2>();
    let units = chunks
        .iter()
        .copied()
        .map(u16::from_le_bytes)
        .collect::<Vec<_>>();
    assert!(remainder.is_empty());
    String::from_utf16(&units).unwrap()
}

#[test]
fn powershell_encoding_is_direct_and_round_trips_without_a_wrapper() {
    let command = "using namespace System.Text\nWrite-Output '中文 ''quote'' \"double\" & $()'";
    let executable = Path::new("pwsh.exe");
    let encoded = encode_powershell_command(command, executable).unwrap();
    let decoded = decode_utf16le_base64(&encoded);

    assert_eq!(decoded, command);
    for forbidden in [
        "UTF8Encoding]::new",
        "ScriptBlock]::Create",
        "Encoding]::Unicode.GetString",
        "Convert]::FromBase64String",
    ] {
        assert!(
            !decoded.contains(forbidden),
            "unexpected wrapper API: {forbidden}"
        );
    }
    assert!(
        powershell_command_line_units(executable, encoded.len()).unwrap()
            <= WINDOWS_COMMAND_LINE_LIMIT_UTF16_UNITS
    );
}

#[test]
fn powershell_command_line_budget_counts_the_utf16_terminator_exactly() {
    let executable = Path::new("pwsh.exe");
    let maximum = maximum_encoded_command_chars(executable).unwrap();

    assert_eq!(
        powershell_command_line_units(executable, maximum),
        Some(WINDOWS_COMMAND_LINE_LIMIT_UTF16_UNITS)
    );
    assert_eq!(
        powershell_command_line_units(executable, maximum + 1),
        Some(WINDOWS_COMMAND_LINE_LIMIT_UTF16_UNITS + 1)
    );
}

#[test]
fn powershell_encoding_rejects_commands_above_the_exact_limit() {
    let executable = Path::new("pwsh.exe");
    let err = encode_powershell_command(&"界".repeat(20_000), executable).unwrap_err();
    assert!(matches!(err, ToolError::InvalidArgs(_)));
    assert!(err.to_string().contains("32,767 UTF-16-code-unit"), "{err}");
    assert!(
        err.to_string().contains("including the terminator"),
        "{err}"
    );
    assert!(err.to_string().contains("maximum for executable"), "{err}");
}

#[test]
fn powershell_encoding_error_does_not_embed_absolute_executable_path() {
    let executable = Path::new("C:/Users/host/.mcode/bin/powershell/7.6.5/x86_64/pwsh.exe");
    let err = encode_powershell_command(&"世".repeat(20_000), executable).unwrap_err();
    let msg = err.to_string();
    assert!(!msg.contains("C:/Users/host"), "{msg}");
    assert!(!msg.contains(".mcode"), "{msg}");
    assert!(msg.contains("pwsh.exe"), "{msg}");
    assert!(msg.contains("maximum for executable"), "{msg}");
}

#[test]
fn backend_preference_order_matches_the_platform_contract() {
    #[cfg(windows)]
    assert_eq!(WINDOWS_SHELL_EXECUTABLE, "pwsh.exe");
    #[cfg(not(windows))]
    assert_eq!(
        SHELL_CANDIDATES
            .iter()
            .map(|candidate| candidate.executable)
            .collect::<Vec<_>>(),
        ["/bin/bash", "bash", "sh"]
    );
}

#[cfg(windows)]
#[tokio::test]
async fn kill_and_reap_already_exited_child_is_success() {
    let executable = Path::new(WINDOWS_SHELL_EXECUTABLE);
    let encoded = encode_powershell_command("exit 0", executable).unwrap();
    let (mut child, process_tree) =
        match spawn_windows_candidate(executable, &encoded, Path::new(".")) {
            Ok(spawned) => spawned,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => panic!("spawn already-exited teardown fixture: {err}"),
        };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    process_tree
        .kill_and_reap(&mut child)
        .await
        .expect("already-exited child must not be a teardown failure");
}

#[cfg(windows)]
#[tokio::test]
async fn kill_and_reap_terminates_an_enrolled_shell() {
    let executable = Path::new(WINDOWS_SHELL_EXECUTABLE);
    let encoded = encode_powershell_command("Start-Sleep -Seconds 30", executable).unwrap();
    let (mut child, process_tree) =
        match spawn_windows_candidate(executable, &encoded, Path::new(".")) {
            Ok(spawned) => spawned,
            // This low-level test never provisions or accesses the network.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => panic!("spawn enrolled shell: {err}"),
        };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        process_tree.kill_and_reap(&mut child),
    )
    .await
    .expect("shell should terminate promptly")
    .expect("teardown should reap the enrolled shell");
}

#[cfg(unix)]
#[tokio::test]
async fn kill_and_reap_already_exited_child_is_success() {
    let dir = tempfile::tempdir().unwrap();
    let SpawnedShell {
        mut child,
        process_tree,
        ..
    } = spawn("exit 0", dir.path()).await.expect("spawn fixture");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    process_tree
        .kill_and_reap(&mut child)
        .await
        .expect("already-exited child must not be a teardown failure");
}
