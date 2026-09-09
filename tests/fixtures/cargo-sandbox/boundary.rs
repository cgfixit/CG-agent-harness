fn boundaries() {
    use std::{fs, path::Path, net::{TcpStream, SocketAddr}, time::Duration};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let markers = fs::read_to_string(root.join("probe-paths.txt")).unwrap();
    let paths: Vec<_> = markers.lines().collect();
    assert!(fs::read_to_string(paths[0]).is_err(), "outside synthetic data read must fail");
    assert!(fs::read_to_string(root.join("sensitive-alias")).is_err(), "symlink read must fail");
    for target in [".git/config", ".git/index", ".git/hooks/probe", "src/lib.rs", "nested/.git/config"] {
        assert!(fs::write(root.join(target), "denied").is_err(), "candidate write must fail: {target}");
    }
    assert!(fs::rename(root.join(".git"), root.join("renamed-git")).is_err());
    assert!(fs::write(paths[1], "denied").is_err(), "vendor write must fail");
    assert!(fs::write(paths[2], "denied").is_err(), "outside write must fail");
    let addr: SocketAddr = paths[3].parse().unwrap();
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err(), "listening loopback network must fail");
    let temp = std::env::temp_dir().join(format!("scratch-{}", std::process::id()));
    fs::write(&temp, "permitted").unwrap();
    fs::remove_file(temp).unwrap();
}
