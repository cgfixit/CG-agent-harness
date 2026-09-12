//! The terminal is an HTTP client of the same account/policy endpoints as chat.
//! It neither edits policy files directly nor wraps subprocesses.
use crate::common::{
    atomic::write_atomic,
    home::{HarnessSettings, Home},
};
use clap::Subcommand;
use reqwest::blocking::Client;
use rustls::pki_types::{pem::PemObject, CertificateDer};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{BufRead, IsTerminal, Read, Write},
    path::PathBuf,
    time::Duration,
};

#[derive(Subcommand)]
pub enum WebCommand {
    Status,
    /// Add an exact URL or explicit wildcard; each seed must fit the rule.
    Allow {
        url: String,
        #[arg(long, default_value = "default")]
        group: String,
        #[arg(long)]
        seed: Vec<String>,
    },
    Deny {
        rule: String,
    },
    Fetch {
        url: String,
    },
    Search {
        query: String,
        #[arg(long)]
        group: Option<String>,
    },
    Research {
        query: String,
        #[arg(long)]
        group: Option<String>,
    },
    Cancel,
    Inject,
    Forget,
    On,
    Off,
}
#[derive(Subcommand)]
pub enum AccountCommand {
    /// Read a password from the terminal without echo, or one stdin line.
    Login {
        username: String,
    },
    /// Read current and replacement passwords as two terminal/stdin lines.
    Password,
    Whoami,
    Logout,
}
#[derive(Subcommand)]
pub enum TlsCommand {
    /// Print the public certificate only. Does not install any trust roots.
    Certificate,
    /// Replace generated material explicitly while the owning server is stopped.
    Renew,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedSession {
    origin: String,
    certificate_sha256: String,
    cookie: String,
}
struct Portal {
    client: Client,
    origin: String,
    certificate_sha256: String,
    csrf: String,
    cookie: String,
    path: PathBuf,
}

fn certificate(home: &Home, cfg: &crate::common::config::AppConfig) -> anyhow::Result<Vec<u8>> {
    let provided = cfg.str_opt("tls.cert_file").unwrap_or_default();
    let bytes = if provided.is_empty() {
        super::transport::read_material(&home.root.join("tls/server.pem"), true)?
    } else {
        super::transport::read_material(&home.anchor(&provided), false)?
    };
    Ok(CertificateDer::from_pem_slice(&bytes)?.as_ref().to_vec())
}

impl Portal {
    fn open(origin: Option<String>, use_session: bool) -> anyhow::Result<Self> {
        let home = Home::resolve(None);
        let cfg = home.load_config()?;
        let tls = cfg.flag_is_true("tls.enabled");
        let origin = match origin {
            Some(origin) => origin,
            None => format!(
                "{}://127.0.0.1:{}",
                if tls { "https" } else { "http" },
                HarnessSettings::load(&home)?.port
            ),
        };
        let url = url::Url::parse(&origin)?;
        let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
        anyhow::ensure!(
            matches!(host, "127.0.0.1" | "localhost" | "::1")
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "use an exact loopback origin without credentials, path, query, or fragment"
        );
        anyhow::ensure!(
            url.scheme() == if tls { "https" } else { "http" },
            "portal scheme must match tls.enabled; HTTP downgrade refused"
        );
        let mut client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(
                cfg.u64_or("web.research_seconds", 300).clamp(10, 1800) + 10,
            ));
        let certificate_sha256 = if tls {
            let der = certificate(&home, &cfg)?;
            let hash = crate::common::sha256_bytes_hex(&der);
            client = client
                .use_preconfigured_tls(crate::common::local_tls::client_config(der, host).map_err(anyhow::Error::msg)?);
            hash
        } else {
            String::new()
        };
        let origin = url.origin().ascii_serialization();
        let path = home.root.join("cli-session.json");
        let cookie = if !use_session {
            String::new()
        } else {
            match std::fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e.into()),
                Ok(_) => {
                    let saved: SavedSession = serde_json::from_slice(&super::transport::read_material(&path, true)?)?;
                    anyhow::ensure!(saved.origin==origin && saved.certificate_sha256==certificate_sha256,"saved CLI session belongs to another listener or certificate; remove cli-session.json explicitly and log in again");
                    anyhow::ensure!(
                        saved.cookie.starts_with("cgagentharness_session=")
                            && saved.cookie.len() <= 256
                            && saved
                                .cookie
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"_=-".contains(&b)),
                        "invalid saved CLI cookie"
                    );
                    saved.cookie
                }
            }
        };
        let client = client.build()?;
        let response = client.get(format!("{origin}/")).send()?;
        anyhow::ensure!(response.status().is_success(), "portal page refused");
        let mut html = String::new();
        response.take(2 * 1024 * 1024 + 1).read_to_string(&mut html)?;
        anyhow::ensure!(html.len() <= 2 * 1024 * 1024, "portal page exceeds bound");
        let csrf = html
            .split_once("<meta name=\"csrf-token\" content=\"")
            .and_then(|(_, s)| s.split_once('"'))
            .map(|(token, _)| token.to_string())
            .ok_or_else(|| anyhow::anyhow!("portal CSRF token missing"))?;
        anyhow::ensure!(
            (20..=256).contains(&csrf.len()) && csrf.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "invalid portal CSRF token"
        );
        Ok(Self {
            client,
            origin,
            certificate_sha256,
            csrf,
            cookie,
            path,
        })
    }
    fn call(&mut self, method: &str, path: &str, body: Option<Value>) -> anyhow::Result<Value> {
        let mut request = self
            .client
            .request(method.parse()?, format!("{}{path}", self.origin))
            .header("X-CyClaw-CSRF", &self.csrf)
            .header("Origin", &self.origin);
        if !self.cookie.is_empty() {
            request = request.header("Cookie", &self.cookie);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send()?;
        let status = response.status();
        let changed = response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .map(|h| h.to_str().map(str::to_string))
            .transpose()?;
        let mut bytes = Vec::new();
        response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 4 * 1024 * 1024, "portal response exceeds bound");
        let mut data: Value = serde_json::from_slice(&bytes)?;
        if !status.is_success() {
            // Server errors are structured, but arbitrary backend detail is not
            // copied into terminal diagnostics that could contain credentials.
            anyhow::bail!(
                "portal HTTP {}: {}",
                status.as_u16(),
                data["detail"]["code"].as_str().unwrap_or("request refused")
            );
        }
        if let Some(changed) = changed {
            let cookie = changed.split(';').next().unwrap_or_default().to_string();
            if changed.contains("Max-Age=0") {
                match std::fs::remove_file(&self.path) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                };
                self.cookie.clear();
            } else {
                anyhow::ensure!(
                    cookie.starts_with("cgagentharness_session=") && cookie.len() <= 256,
                    "invalid session response"
                );
                write_atomic(
                    &self.path,
                    &serde_json::to_vec(&SavedSession {
                        origin: self.origin.clone(),
                        certificate_sha256: self.certificate_sha256.clone(),
                        cookie: cookie.clone(),
                    })?,
                    Some(0o600),
                )?;
                self.cookie = cookie;
            }
        }
        if let Some(map) = data.as_object_mut() {
            map.remove("csrf_token");
        }
        Ok(data)
    }
}

fn secret(input: &mut impl BufRead, label: &str) -> anyhow::Result<String> {
    let terminal = std::io::stdin().is_terminal();
    #[cfg(unix)]
    struct Echo(Option<libc::termios>);
    #[cfg(unix)]
    impl Drop for Echo {
        fn drop(&mut self) {
            if let Some(old) = self.0 {
                unsafe {
                    libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &old);
                }
            }
        }
    }
    #[cfg(unix)]
    let _echo = if terminal {
        // SAFETY: stdin is a terminal, termios is initialized before use, and
        // the guard restores echo on every ordinary error/return path.
        let mut old = std::mem::MaybeUninit::<libc::termios>::uninit();
        anyhow::ensure!(
            unsafe { libc::tcgetattr(libc::STDIN_FILENO, old.as_mut_ptr()) } == 0,
            "cannot secure terminal input"
        );
        let old = unsafe { old.assume_init() };
        let mut quiet = old;
        quiet.c_lflag &= !libc::ECHO;
        anyhow::ensure!(
            unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &quiet) } == 0,
            "cannot secure terminal input"
        );
        Echo(Some(old))
    } else {
        Echo(None)
    };
    #[cfg(not(unix))]
    anyhow::ensure!(!terminal, "pipe password lines through stdin on this platform");
    if terminal {
        eprint!("{label}: ");
        std::io::stderr().flush()?;
    }
    let mut value = String::new();
    input.take(4097).read_line(&mut value)?;
    if terminal {
        eprintln!();
    }
    anyhow::ensure!(value.len() <= 4096, "password input exceeds bound");
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    anyhow::ensure!(!value.is_empty(), "password input is empty");
    Ok(value)
}

pub fn account(command: AccountCommand, origin: Option<String>) -> anyhow::Result<()> {
    let mut portal = Portal::open(origin, !matches!(command, AccountCommand::Login { .. }))?;
    let mut input = std::io::stdin().lock();
    let value=match command {
        AccountCommand::Login{username}=>portal.call("POST","/api/auth/login",Some(json!({"username":username,"password":secret(&mut input,"Password")?})))?,
        AccountCommand::Password=>portal.call("POST","/api/auth/password",Some(json!({"current_password":secret(&mut input,"Current password")?,"password":secret(&mut input,"New password")?})))?,
        AccountCommand::Whoami=>portal.call("GET","/api/auth/whoami",None)?,
        AccountCommand::Logout=>portal.call("POST","/api/auth/logout",Some(json!({})))?,
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
pub fn web(command: WebCommand, origin: Option<String>) -> anyhow::Result<()> {
    let (method, path, body) = match command {
        WebCommand::Status => ("GET", "/api/web", None),
        WebCommand::Allow { url, group, seed } => (
            "POST",
            "/api/web/allow",
            Some(json!({"url":url,"group":group,"seeds":seed})),
        ),
        WebCommand::Deny { rule } => ("POST", "/api/web/deny", Some(json!({"url":rule}))),
        WebCommand::Fetch { url } => ("POST", "/api/web/fetch", Some(json!({"url":url}))),
        WebCommand::Search { query, group } => ("POST", "/api/web/search", Some(json!({"query":query,"group":group}))),
        WebCommand::Research { query, group } => {
            ("POST", "/api/web/research", Some(json!({"query":query,"group":group})))
        }
        WebCommand::Cancel => ("POST", "/api/web/research/cancel", Some(json!({}))),
        WebCommand::Inject => ("POST", "/api/web/inject", Some(json!({}))),
        WebCommand::Forget => ("POST", "/api/web/forget", Some(json!({}))),
        WebCommand::On => ("POST", "/api/web", Some(json!({"enabled":true}))),
        WebCommand::Off => ("POST", "/api/web", Some(json!({"enabled":false}))),
    };
    let value = Portal::open(origin, true)?.call(method, path, body)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
pub fn tls(command: TlsCommand) -> anyhow::Result<()> {
    let home = Home::resolve(None);
    let cfg = home.load_config()?;
    if matches!(command, TlsCommand::Renew) {
        let _lock = crate::common::home_lock::HomeLock::acquire(&home.root)?;
        super::transport::Transport::renew(&home, &cfg)?;
    }
    use base64::Engine;
    println!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----",
        base64::engine::general_purpose::STANDARD.encode(certificate(&home, &cfg)?)
    );
    Ok(())
}
