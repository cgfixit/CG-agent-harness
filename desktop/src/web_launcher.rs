//! A native-confirmed, fixed launcher. No renderer paths, argv or credentials.
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn requested(url: &tauri::Url, origin: &str) -> bool {
    super::backend::allowed_navigation(url, origin)
        && url.path() == "/__desktop/open-web"
        && url.query().is_none()
        && url.fragment().is_none()
}

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn open(home: &Path, origin: &str) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| "Cannot locate app bundle.")?;
    let script = executable
        .parent()
        .ok_or("Invalid bundle layout.")?
        .join("../Resources/start-web.sh");
    let bundled = std::fs::read(&script).map_err(|_| "Bundled web launcher is missing; reinstall the complete app.")?;
    if bundled != include_bytes!("../../scripts/start-web.sh") {
        return Err("Bundled web launcher differs from this build; reinstall the complete app.".into());
    }
    let script = script.to_str().ok_or("Web launcher path must be UTF-8.")?;
    // Atomically replace a fixed private command file. create_new refuses links;
    // rename replaces a stale link itself, never the link's target.
    let temporary = home.join(format!(".desktop-web-{:016x}.command", rand::random::<u64>()));
    let command = home.join("desktop-web.command");
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&temporary)
            .map_err(|_| "Cannot create the private Terminal launcher.")?;
        // Terminal starts its own shell. Clear inherited startup hooks/options
        // before bash reads the fixed, integrity-checked bundled script.
        writeln!(
            file,
            "#!/bin/bash\nexec /usr/bin/env -i PATH=/usr/bin:/bin /bin/bash {} --desktop-url {}",
            quoted(script),
            quoted(origin)
        )
        .map_err(|_| "Cannot write the Terminal launcher.")?;
        file.sync_all().map_err(|_| "Cannot save the Terminal launcher.")?;
        std::fs::rename(&temporary, &command).map_err(|_| "Cannot install the Terminal launcher.")?;
        let status = Command::new("/usr/bin/open")
            .args(["-a", "Terminal", "--"])
            .arg(&command)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| "Cannot open Terminal.")?;
        if !status.success() {
            return Err("Terminal refused the web launcher.".into());
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_signal_is_exact_and_owned() {
        let origin = "https://127.0.0.1:54321"; // DevSkim: ignore DS162092 because this synthetic origin is parsed, never dialed.
        assert!(requested(
            &format!("{origin}/__desktop/open-web").parse().unwrap(),
            origin
        ));
        for url in [
            "https://example.com/__desktop/open-web",
            "https://127.0.0.1:54322/__desktop/open-web", // DevSkim: ignore DS162092 because this synthetic origin is parsed, never dialed.
            "https://user@127.0.0.1:54321/__desktop/open-web", // DevSkim: ignore DS162092 because this synthetic origin is parsed, never dialed.
            "https://127.0.0.1:54321/__desktop/open-web?command=x", // DevSkim: ignore DS162092 because this synthetic origin is parsed, never dialed.
            "https://127.0.0.1:54321/__desktop/open-web#x", // DevSkim: ignore DS162092 because this synthetic origin is parsed, never dialed.
            "https://127.0.0.1:54321/__desktop/open-web/", // DevSkim: ignore DS162092 because this synthetic origin is parsed, never dialed.
        ] {
            assert!(!requested(&url.parse().unwrap(), origin), "{url}");
        }
    }

    #[test]
    fn shell_paths_are_literal_data() {
        let _serial = crate::instance::CHILD_PROCESS.lock().unwrap_or_else(|e| e.into_inner());
        let path = "/Applications/CG ' $HOME $(echo wrong) `echo wrong`.app/start-web.sh";
        let output = Command::new("/bin/bash")
            .args(["-c", &format!("printf %s {}", quoted(path))])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), path);
    }
}
