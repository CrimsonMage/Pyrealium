//! Exercise the executable's transport selection, not just its option parser.
use std::{
    net::{TcpListener, UdpSocket},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn free_game_address() -> String {
    for _ in 0..100 {
        let base = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = base.local_addr().unwrap();
        if address.port() < u16::MAX && UdpSocket::bind((address.ip(), address.port() + 1)).is_ok()
        {
            return address.to_string();
        }
    }
    panic!("no adjacent UDP ports available");
}

#[test]
fn custom_adapter_is_opt_in_and_legacy_explicit_option_still_works() {
    let root = std::env::temp_dir().join(format!(
        "gun-startup-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for (case, option) in [
        ("default", None),
        ("adapter", Some(("--gun-adapter-bind", "127.0.0.1:0"))),
        ("legacy", Some(("--gun-bind", "127.0.0.1:0"))),
        ("off", Some(("--gun-adapter-bind", "off"))),
        ("legacy-off", Some(("--gun-bind", "off"))),
    ] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pyrealium"));
        cmd.env("PYREALIUM_DATABASE_URL", pyrealium::db::testdb::fresh().url());
        cmd.args([
            "serve",
            "--bind",
            &free_game_address(),
            "--seconds",
            "1",
            "--content",
            "none",
            "--data",
        ])
        .arg(root.join(case));
        if let Some((option, value)) = option {
            cmd.args([option, value]);
        }
        let enabled = option.is_some_and(|(_, value)| value != "off");
        let result = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{case}: {stdout}\n{stderr}");
        assert_eq!(
            stdout.contains("GUN adapter listening on TCP"),
            enabled,
            "{stdout}"
        );
        assert!(!stdout.contains("GUN service listening"), "{stdout}");
        if enabled {
            assert!(
                stdout.contains("incompatible with stock GunDll.dll"),
                "{stdout}"
            );
        }
    }
    // An explicitly requested listener must not be silently skipped.
    let busy = TcpListener::bind("127.0.0.1:0").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_pyrealium"))
        .env("PYREALIUM_DATABASE_URL", pyrealium::db::testdb::fresh().url())
        .args([
            "serve",
            "--gun-adapter-bind",
            &busy.local_addr().unwrap().to_string(),
            "--data",
        ])
        .arg(root.join("busy"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("GUN adapter listening"));
    std::fs::remove_dir_all(root).unwrap();
}
