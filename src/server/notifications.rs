//! Durable owner-bound completion delivery. Only bounded job metadata leaves here.
use super::notification_outbox::{read_private, Delivery, Outbox};
use super::web_policy::{canonical_http_url, is_public_ip};
use crate::common::credential_store::CredentialStore;
use crate::common::{
    audit::Audit,
    auth_store::AuthManager,
    config::AppConfig,
    errors::{HarnessError, Result},
    home::Home,
    sha256_bytes_hex,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use url::Url;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub owner: String,
    pub job_id: String,
    pub status: String,
    pub created_at: f64,
    pub finished_at: f64,
}
impl Completion {
    pub(crate) fn valid(&self) -> bool {
        super::structured_memory::valid_owner(&self.owner)
            && crate::common::is_lower_hex(&self.job_id, 32)
            && ["finished", "failed", "cancelled"].contains(&self.status.as_str())
            && self.created_at.is_finite()
            && self.created_at >= 0.0
            && self.finished_at.is_finite()
            && self.finished_at >= 0.0
    }
    pub(crate) fn event_id(&self) -> String {
        sha256_bytes_hex(&serde_json::to_vec(self).expect("finite validated completion"))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationSpec {
    id: String,
    owner: String,
    #[serde(default)]
    enabled: bool,
    url: String,
    events: Vec<String>,
    #[serde(default)]
    use_bearer: bool,
    rate_per_minute: u64,
}
#[derive(Clone)]
struct Destination {
    spec: DestinationSpec,
    url: Url,
    private: bool,
    revision: String,
    bearer: Option<String>,
    bearer_hash: Option<String>,
}
struct Settings {
    destinations: Vec<Destination>,
    capacity: usize,
    batch_size: usize,
    batch_interval: u64,
    poll_interval: u64,
    attempts: u64,
    retry_delay: u64,
    max_backoff: u64,
    jitter_percent: u64,
    retention: u64,
    max_replays: u64,
    timeout: Duration,
}
fn invalid() -> HarnessError {
    HarnessError::config("invalid notifications configuration; use version 1 owned destinations, explicit subscriptions and finite limits; legacy webhook_url needs deliberate migration")
}
fn bounded(cfg: &AppConfig, key: &str, min: u64, max: u64) -> Result<u64> {
    let defaults = AppConfig::from_str(AppConfig::embedded_default(), std::path::Path::new("defaults"))?;
    cfg.get(key)
        .or_else(|| defaults.get(key))
        .and_then(|v| v.as_u64())
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(invalid)
}
fn read_token(home: &Home, allow_plaintext: bool) -> Result<Option<String>> {
    let token = match std::env::var("CGAGENTHARNESS_WEBHOOK_TOKEN") {
        Ok(token) => Some(token),
        Err(std::env::VarError::NotPresent) if allow_plaintext => super::env_keys::read_startup_keys(&home.env_path())
            .map_err(|_| invalid())?
            .remove("CGAGENTHARNESS_WEBHOOK_TOKEN"),
        Err(std::env::VarError::NotPresent) => {
            match crate::common::credential_store::OsCredentialStore::for_home(&home.root)
                .get("CGAGENTHARNESS_WEBHOOK_TOKEN")
            {
                Ok(value) => value,
                Err(crate::common::credential_store::StoreError::Unavailable) => None,
                Err(_) => return Err(invalid()),
            }
        }
        Err(_) => return Err(invalid()),
    }
    .filter(|s| !s.trim().is_empty());
    if token
        .as_ref()
        .is_some_and(|t| t.len() > 4096 || t.bytes().any(|c| !(0x21..=0x7e).contains(&c)))
    {
        return Err(invalid());
    }
    Ok(token)
}

fn acceptable_secret(secret: &str) -> bool {
    !secret.is_empty() && secret.len() <= 4096 && secret.bytes().all(|c| (0x21..=0x7e).contains(&c))
}

fn bearer_path(home: &Home) -> PathBuf {
    home.data_dir().join("notifications/bearers.json")
}

fn revoked_path(home: &Home) -> PathBuf {
    home.data_dir().join("notifications/revoked-owners.json")
}

fn read_optional_json(path: &std::path::Path) -> Result<Option<Value>> {
    // CodeQL rust/path-injection treats `contains("..") == false` as a barrier.
    let raw = path.to_string_lossy();
    if raw.contains("..") {
        return Err(invalid());
    }
    let path = std::path::Path::new(raw.as_ref());
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => {
            let bytes = read_private(path, 65_536)?;
            Ok(Some(serde_json::from_slice(&bytes).map_err(|_| invalid())?))
        }
    }
}

fn revoked_owners(home: &Home) -> Result<BTreeSet<String>> {
    let Some(value) = read_optional_json(&revoked_path(home))? else {
        return Ok(BTreeSet::new());
    };
    if value.get("version").and_then(|v| v.as_u64()) != Some(1) {
        return Err(invalid());
    }
    let owners = value.get("owners").and_then(|v| v.as_array()).ok_or_else(invalid)?;
    let mut set = BTreeSet::new();
    for owner in owners {
        let owner = owner.as_str().ok_or_else(invalid)?;
        if !super::structured_memory::valid_owner(owner) || !set.insert(owner.to_string()) {
            return Err(invalid());
        }
    }
    Ok(set)
}

fn bearers_refused() -> HarnessError {
    HarnessError::config(
        "notifications/bearers.json must be a regular file owned by this user, not a link, \
         with mode 0600 and no ACL entries (run `chmod 600 <path>`, and on macOS \
         `chmod -N <path>`); restart after fixing",
    )
}

/// Read `bearers.json` only when it is private to this user. On Unix the
/// opened descriptor (not the path) must be a regular file owned by the euid
/// with no group/other bits and a single link; symlinks are refused by
/// `O_NOFOLLOW`. On macOS the descriptor must also carry no extended ACL
/// entries, because an ACL can grant other accounts read access that the mode
/// bits do not show. The error never includes the file's contents.
fn read_bearers_json(path: &std::path::Path) -> Result<Option<Value>> {
    use std::io::Read;
    // CodeQL rust/path-injection treats `contains("..") == false` as a barrier.
    let raw = path.to_string_lossy();
    if raw.contains("..") {
        return Err(invalid());
    }
    let path = std::path::Path::new(raw.as_ref());
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(meta) if !meta.is_file() => return Err(bearers_refused()),
        Ok(_) => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path).map_err(|_| bearers_refused())?;
    let meta = file.metadata().map_err(|_| bearers_refused())?;
    if !meta.is_file() {
        return Err(bearers_refused());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        let euid = unsafe { libc::geteuid() };
        if meta.uid() != euid || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
            return Err(bearers_refused());
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        if macos_acl::fd_has_acl_entries(file.as_raw_fd()) != Some(false) {
            return Err(bearers_refused());
        }
    }
    let limit = 65_536;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid());
    }
    Ok(Some(serde_json::from_slice(&bytes).map_err(|_| invalid())?))
}

/// Minimal libSystem ACL bindings (no crate): only what the `bearers.json`
/// check needs. Declarations follow `<sys/acl.h>` on Darwin.
#[cfg(target_os = "macos")]
mod macos_acl {
    use std::os::raw::{c_int, c_void};

    type AclT = *mut c_void;
    type AclEntryT = *mut c_void;
    /// `ACL_TYPE_EXTENDED` in `<sys/acl.h>`.
    const ACL_TYPE_EXTENDED: u32 = 0x0000_0100;
    /// `ACL_FIRST_ENTRY` in `<sys/acl.h>`.
    const ACL_FIRST_ENTRY: c_int = 0;

    extern "C" {
        fn acl_get_fd_np(fd: c_int, kind: u32) -> AclT;
        fn acl_get_entry(acl: AclT, entry_id: c_int, entry: *mut AclEntryT) -> c_int;
        fn acl_free(obj: *mut c_void) -> c_int;
    }

    /// Frees the ACL on every return path.
    struct Acl(AclT);

    impl Drop for Acl {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a non-null ACL from `acl_get_fd_np`, freed once.
            unsafe {
                acl_free(self.0);
            }
        }
    }

    /// `Some(true)` when the open file has at least one extended ACL entry,
    /// `Some(false)` when it has none (or the filesystem has no ACLs), and
    /// `None` when the ACL could not be read; callers refuse on `None`.
    pub(super) fn fd_has_acl_entries(fd: c_int) -> Option<bool> {
        // SAFETY: `fd` is a live descriptor owned by the caller for this call.
        let raw = unsafe { acl_get_fd_np(fd, ACL_TYPE_EXTENDED) };
        if raw.is_null() {
            return match std::io::Error::last_os_error().raw_os_error() {
                // No extended ACL on this file, or none possible on this filesystem.
                Some(libc::ENOENT) | Some(libc::ENOTSUP) => Some(false),
                _ => None,
            };
        }
        let acl = Acl(raw);
        let mut entry: AclEntryT = std::ptr::null_mut();
        // SAFETY: `acl.0` is a valid ACL and `entry` is a valid out-pointer.
        let found = unsafe { acl_get_entry(acl.0, ACL_FIRST_ENTRY, &mut entry) } == 0;
        Some(found)
    }
}

fn bearer_tokens(home: &Home) -> Result<std::collections::BTreeMap<String, String>> {
    let Some(value) = read_bearers_json(&bearer_path(home))? else {
        return Ok(std::collections::BTreeMap::new());
    };
    if value.get("version").and_then(|v| v.as_u64()) != Some(1) {
        return Err(invalid());
    }
    let tokens = value.get("tokens").and_then(|v| v.as_object()).ok_or_else(invalid)?;
    let mut map = std::collections::BTreeMap::new();
    for (id, secret) in tokens {
        let secret = secret.as_str().ok_or_else(invalid)?;
        if id.is_empty()
            || id.len() > 64
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || !acceptable_secret(secret)
            || map.insert(id.clone(), secret.to_string()).is_some()
        {
            return Err(invalid());
        }
    }
    Ok(map)
}

/// Each `use_bearer` destination keeps its own secret. The house
/// `CGAGENTHARNESS_WEBHOOK_TOKEN` is only the legacy secret for a single
/// owner's bearer destinations. A second owner must bring its own secret.
fn assign_bearers(home: &Home, destinations: &mut [Destination], allow_plaintext: bool) -> Result<()> {
    let file = bearer_tokens(home)?;
    let legacy = read_token(home, allow_plaintext)?;
    let mut owners = BTreeSet::new();
    for destination in destinations.iter().filter(|d| d.spec.enabled && d.spec.use_bearer) {
        owners.insert(destination.spec.owner.clone());
    }
    for destination in destinations.iter_mut().filter(|d| d.spec.enabled && d.spec.use_bearer) {
        let secret = if let Some(secret) = file.get(&destination.spec.id) {
            secret.clone()
        } else if owners.len() == 1 {
            legacy.clone().ok_or_else(invalid)?
        } else {
            return Err(invalid());
        };
        destination.bearer_hash = Some(sha256_bytes_hex(secret.as_bytes()));
        destination.bearer = Some(secret);
    }
    Ok(())
}

impl Settings {
    fn load(home: &Home, cfg: &AppConfig) -> Result<Self> {
        if cfg.get("notifications.schema_version").and_then(|v| v.as_u64()) != Some(1)
            || !cfg.str_or("notifications.webhook_url", "").is_empty()
        {
            return Err(invalid());
        }
        let specs: Vec<DestinationSpec> =
            serde_json::from_value(cfg.json("notifications.destinations")).map_err(|_| invalid())?;
        if specs.len() > 8 {
            return Err(invalid());
        }
        let grants = cfg
            .get("notifications.private_url_allowlist")
            .and_then(|v| v.as_sequence())
            .ok_or_else(invalid)?;
        if grants.len() > 8 {
            return Err(invalid());
        }
        let mut allowed = Vec::new();
        for grant in grants {
            allowed.push(canonical_http_url(grant.as_str().ok_or_else(invalid)?, true).map_err(|_| invalid())?);
        }
        let mut ids = BTreeSet::new();
        let mut destinations = Vec::new();
        for spec in specs {
            if spec.id.is_empty()
                || spec.id.len() > 64
                || !spec
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !ids.insert(spec.id.clone())
                || !super::structured_memory::valid_owner(&spec.owner)
                || spec.events.is_empty()
                || spec.events.len() > 3
                || spec
                    .events
                    .iter()
                    .any(|s| !["finished", "failed", "cancelled"].contains(&s.as_str()))
                || !(1..=120).contains(&spec.rate_per_minute)
            {
                return Err(invalid());
            }
            let url = canonical_http_url(&spec.url, true).map_err(|_| invalid())?;
            let private = allowed.contains(&url);
            if url.fragment().is_some() || (url.scheme() != "https" && !private) {
                return Err(invalid());
            }
            destinations.push(Destination {
                spec,
                url,
                private,
                revision: String::new(),
                bearer: None,
                bearer_hash: None,
            });
        }
        let revoked = revoked_owners(home)?;
        for destination in &mut destinations {
            if revoked.contains(&destination.spec.owner) {
                destination.spec.enabled = false;
            }
            destination.revision = sha256_bytes_hex(&serde_json::to_vec(
                &json!({"spec": destination.spec, "private": destination.private}),
            )?);
        }
        assign_bearers(
            home,
            &mut destinations,
            cfg.flag_is_true(crate::common::credential_store::PLAINTEXT_KEY_FILE),
        )?;
        Ok(Self {
            destinations,
            capacity: bounded(cfg, "notifications.queue_capacity", 1, 512)? as usize,
            batch_size: bounded(cfg, "notifications.batch_size", 1, 32)? as usize,
            batch_interval: bounded(cfg, "notifications.batch_interval_sec", 1, 3600)?,
            poll_interval: bounded(cfg, "notifications.poll_interval_sec", 1, 60)?,
            attempts: bounded(cfg, "notifications.max_attempts", 1, 3)?,
            retry_delay: bounded(cfg, "notifications.retry_delay_sec", 1, 60)?,
            max_backoff: bounded(cfg, "notifications.max_backoff_sec", 1, 3600)?,
            jitter_percent: bounded(cfg, "notifications.jitter_percent", 0, 50)?,
            retention: bounded(cfg, "notifications.retention_sec", 60, 2592000)?,
            max_replays: bounded(cfg, "notifications.max_replays", 0, 10)?,
            timeout: Duration::from_secs(bounded(cfg, "notifications.timeout_sec", 1, 30)?),
        })
    }
}
struct Inner {
    outbox: Mutex<Outbox>,
    settings: Settings,
    suppressed_owners: Mutex<BTreeSet<String>>,
    home: Home,
    config_path: PathBuf,
    auth: Option<Arc<AuthManager>>,
    audit: Audit,
    stop: tokio_util::sync::CancellationToken,
}
#[derive(Clone)]
pub struct Notifier(Arc<Inner>);
impl Notifier {
    pub fn start(home: &Home, cfg: &AppConfig, auth: Option<Arc<AuthManager>>) -> Result<Option<Self>> {
        if !cfg.flag_is_true("notifications.enabled") {
            return Ok(None);
        }
        if cfg.flag_is_true("auth.enabled") != auth.is_some() {
            return Err(invalid());
        }
        let settings = Settings::load(home, cfg)?;
        let suppressed_owners = revoked_owners(home)?;
        let outbox = Outbox::open(&home.data_dir().join("notifications/outbox.json"))?;
        let inner = Arc::new(Inner {
            outbox: Mutex::new(outbox),
            settings,
            suppressed_owners: Mutex::new(suppressed_owners),
            home: Home::at(home.root.clone()),
            config_path: cfg.path.clone(),
            auth,
            audit: Audit::from_home(&home.root, cfg),
            stop: tokio_util::sync::CancellationToken::new(),
        });
        let weak = Arc::downgrade(&inner);
        let interval = Duration::from_secs(inner.settings.poll_interval);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                if inner.stop.is_cancelled() {
                    break;
                }
                tokio::select! { _ = inner.stop.cancelled() => break, _ = inner.tick() => {} }
            }
        });
        Ok(Some(Self(inner)))
    }
    pub fn stop(&self) {
        self.0.stop.cancel();
    }
    pub fn enqueue(&self, event: Completion) {
        if let Err(code) = self.0.enqueue(event.clone()) {
            self.0
                .audit
                .log(json!({"event":"notification_dropped","job_id":if event.valid(){Some(&event.job_id)}else{None},"code":code}));
        }
    }
    pub fn status(&self, owner: &str) -> Value {
        let g = self.0.outbox.lock().unwrap_or_else(|p| p.into_inner());
        let suppressed = self
            .0
            .suppressed_owners
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let destinations: Vec<_> = self.0.settings.destinations.iter().filter(|d| d.spec.owner == owner).map(|d|
            json!({"id":d.spec.id,"enabled":d.spec.enabled && !suppressed.contains(&d.spec.owner),"authorized_now":self.0.authorized(d),"events":d.spec.events,"rate_per_minute":d.spec.rate_per_minute,"bearer_configured":d.spec.use_bearer})).collect();
        let deliveries: Vec<_> = g.data.deliveries.values().filter(|d| d.owner == owner).map(|d|
            json!({"delivery_id":d.delivery_id,"event_id":d.event_id,"destination_id":d.destination_id,"job_id":d.completion.job_id,"job_status":d.completion.status,"finished_at":d.completion.finished_at,"state":d.state,"attempts":d.attempts,"replays":d.replays,"next_attempt_at":d.next_attempt_at,"last_code":d.last_code})).collect();
        json!({"enabled":true,"destinations":destinations,"deliveries":deliveries,"semantics":"at_least_once_with_receiver_deduplication"})
    }
    pub fn replay(&self, owner: &str, id: &str) -> Result<()> {
        let mut g = self.0.outbox.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = g.data.clone();
        let d = next
            .deliveries
            .get_mut(id)
            .filter(|d| d.owner == owner)
            .ok_or_else(|| HarnessError::new("DELIVERY_NOT_FOUND", "no such retained delivery"))?;
        let destination = self
            .0
            .settings
            .destinations
            .iter()
            .find(|x| x.spec.id == d.destination_id && x.revision == d.destination_revision)
            .ok_or_else(|| HarnessError::new("DELIVERY_REVOKED", "destination grant changed"))?;
        if !self.0.authorized(destination) {
            return Err(HarnessError::new(
                "DELIVERY_REVOKED",
                "current destination or owner authority refused delivery",
            ));
        }
        let now = crate::common::now_ts();
        if !["failed", "delivered"].contains(&d.state.as_str())
            || d.replays >= self.0.settings.max_replays
            || d.completion.finished_at + self.0.settings.retention as f64 <= now
        {
            return Err(HarnessError::new(
                "DELIVERY_REPLAY_REFUSED",
                "delivery is pending, revoked, expired or at its replay limit",
            ));
        }
        d.state = "pending".into();
        d.attempts = 0;
        d.replays += 1;
        d.next_attempt_at = now + self.0.settings.batch_interval as f64;
        d.last_code = None;
        g.commit(next)?;
        self.0
            .audit
            .log(json!({"event":"notification_replay","delivery_id":id}));
        Ok(())
    }

    /// Stop this owner's destinations until an operator edits the grant and restarts.
    /// Other owners are not written into the revocation file.
    pub fn disable_owner(&self, owner: &str) -> Result<usize> {
        if !super::structured_memory::valid_owner(owner) {
            return Ok(0);
        }
        let path = revoked_path(&self.0.home);
        // CodeQL rust/path-injection treats `contains("..") == false` as a barrier.
        let raw = path.to_string_lossy();
        if raw.contains("..") {
            return Err(invalid());
        }
        let path = std::path::Path::new(raw.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut owners = revoked_owners(&self.0.home)?;
        owners.insert(owner.to_string());
        let body = serde_json::to_vec(&json!({"version": 1, "owners": owners}))?;
        crate::common::atomic::write_atomic(path, &body, Some(0o600))?;
        self.0
            .suppressed_owners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(owner.to_string());
        Ok(self
            .0
            .settings
            .destinations
            .iter()
            .filter(|destination| destination.spec.owner == owner)
            .count())
    }

    /// One worker pass; the background task calls `tick` directly.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn poll_once(&self) {
        self.0.tick().await;
    }
}
impl Inner {
    fn owner_suppressed(&self, owner: &str) -> bool {
        self.suppressed_owners
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .contains(owner)
    }

    /// Fresh disk checks can remove startup authority, never expand it. New
    /// destinations/credentials require restart; ordinary config reload adds none.
    fn authorized(&self, destination: &Destination) -> bool {
        if !destination.spec.enabled || self.owner_suppressed(&destination.spec.owner) {
            return false;
        }
        let owner_ok = match &self.auth {
            Some(auth) => auth.list_users().iter().any(|u| {
                u.user_id == destination.spec.owner
                    && !u.disabled
                    && !u.must_change_password
                    && matches!(u.role.as_str(), "admin" | "operator")
            }),
            None => destination.spec.owner == "local",
        };
        if !owner_ok {
            return false;
        }
        let current = (|| -> Result<Settings> {
            let bytes = read_private(&self.config_path, 1_048_576)?;
            let cfg = AppConfig::from_str(std::str::from_utf8(&bytes).map_err(|_| invalid())?, &self.config_path)?;
            if !cfg.flag_is_true("notifications.enabled") || cfg.flag_is_true("auth.enabled") != self.auth.is_some() {
                return Err(invalid());
            }
            Settings::load(&self.home, &cfg)
        })();
        current.is_ok_and(|s| {
            s.destinations.iter().any(|d| {
                d.spec.enabled
                    && d.spec.id == destination.spec.id
                    && d.revision == destination.revision
                    && (!destination.spec.use_bearer || d.bearer_hash == destination.bearer_hash)
            })
        })
    }
    fn enqueue(&self, event: Completion) -> std::result::Result<(), &'static str> {
        if !event.valid() {
            return Err("invalid_event");
        }
        let now = crate::common::now_ts();
        if event.finished_at + self.settings.retention as f64 <= now {
            return Ok(());
        }
        let mut g = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = g.data.clone();
        next.deliveries
            .retain(|_, d| d.completion.finished_at + self.settings.retention as f64 > now);
        let mut pressure = false;
        for destination in self
            .settings
            .destinations
            .iter()
            .filter(|d| d.spec.owner == event.owner && d.spec.events.contains(&event.status))
        {
            if !self.authorized(destination) {
                continue;
            }
            let event_id = event.event_id();
            let delivery_id =
                sha256_bytes_hex(format!("{event_id}:{}:{}", destination.spec.id, destination.revision).as_bytes());
            if next.deliveries.contains_key(&delivery_id) {
                continue;
            }
            if next.deliveries.len() >= self.settings.capacity {
                let oldest = next
                    .deliveries
                    .iter()
                    .filter(|(_, d)| d.state != "pending")
                    .min_by(|(_, a), (_, b)| a.completion.finished_at.total_cmp(&b.completion.finished_at))
                    .map(|(id, _)| id.clone());
                if let Some(id) = oldest {
                    next.deliveries.remove(&id);
                    self.audit
                        .log(json!({"event":"notification_evicted","delivery_id":id,"code":"retention_pressure"}));
                } else {
                    pressure = true;
                    continue;
                }
            }
            next.deliveries.insert(
                delivery_id.clone(),
                Delivery {
                    delivery_id,
                    event_id,
                    owner: event.owner.clone(),
                    destination_id: destination.spec.id.clone(),
                    destination_revision: destination.revision.clone(),
                    completion: event.clone(),
                    state: "pending".into(),
                    attempts: 0,
                    replays: 0,
                    next_attempt_at: now + self.settings.batch_interval as f64,
                    last_code: None,
                },
            );
        }
        g.commit(next).map_err(|_| "outbox_persist_failed")?;
        if pressure {
            Err("queue_full")
        } else {
            Ok(())
        }
    }
    async fn tick(&self) {
        // Sweep even with zero configured destinations: removing the last grant
        // must revoke retained work rather than leave it misleadingly pending.
        {
            let now = crate::common::now_ts();
            let allowed: BTreeSet<_> = self
                .settings
                .destinations
                .iter()
                .filter(|d| self.authorized(d))
                .map(|d| (d.spec.id.clone(), d.revision.clone()))
                .collect();
            let mut g = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
            let mut next = g.data.clone();
            next.deliveries
                .retain(|_, d| d.completion.finished_at + self.settings.retention as f64 > now);
            for d in next.deliveries.values_mut().filter(|d| d.state == "pending") {
                if !allowed.contains(&(d.destination_id.clone(), d.destination_revision.clone())) {
                    d.state = "revoked".into();
                    d.last_code = Some("authority_revoked".into());
                } else if d.attempts >= self.settings.attempts {
                    d.state = "failed".into();
                    d.last_code = Some("attempts_exhausted".into());
                }
            }
            if g.commit(next).is_err() {
                self.audit.log(json!({"event":"notification_store_failed"}));
                return;
            }
        }
        for destination in &self.settings.destinations {
            if self.stop.is_cancelled() {
                return;
            }
            let authorized = self.authorized(destination);
            let now = crate::common::now_ts();
            let batch = {
                let mut g = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
                let mut next = g.data.clone();
                next.deliveries
                    .retain(|_, d| d.completion.finished_at + self.settings.retention as f64 > now);
                for d in next.deliveries.values_mut().filter(|d| d.state == "pending") {
                    let exists = self
                        .settings
                        .destinations
                        .iter()
                        .any(|x| x.spec.id == d.destination_id && x.revision == d.destination_revision);
                    if !exists || (d.destination_id == destination.spec.id && !authorized) {
                        d.state = "revoked".into();
                        d.last_code = Some("authority_revoked".into());
                    } else if d.attempts >= self.settings.attempts {
                        d.state = "failed".into();
                        d.last_code = Some("attempts_exhausted".into());
                    }
                }
                let mut batch = Vec::new();
                let rate_ok = next.next_send_at.get(&destination.spec.id).is_none_or(|t| *t <= now);
                if authorized && rate_ok {
                    for d in next
                        .deliveries
                        .values_mut()
                        .filter(|d| {
                            d.state == "pending"
                                && d.destination_id == destination.spec.id
                                && d.destination_revision == destination.revision
                                && d.next_attempt_at <= now
                        })
                        .take(self.settings.batch_size)
                    {
                        d.attempts += 1;
                        let base = (self.settings.retry_delay.saturating_mul(1u64 << (d.attempts - 1)))
                            .min(self.settings.max_backoff);
                        use rand::Rng;
                        let jitter = rand::thread_rng()
                            .gen_range(0.0..=base as f64 * self.settings.jitter_percent as f64 / 100.0);
                        d.next_attempt_at = now + self.settings.timeout.as_secs_f64() + base as f64 + jitter;
                        batch.push(d.clone());
                    }
                    if !batch.is_empty() {
                        next.next_send_at.insert(
                            destination.spec.id.clone(),
                            now + 60.0 / destination.spec.rate_per_minute as f64,
                        );
                    }
                }
                next.next_send_at
                    .retain(|id, _| self.settings.destinations.iter().any(|d| &d.spec.id == id));
                if g.commit(next).is_err() {
                    self.audit.log(json!({"event":"notification_store_failed"}));
                    continue;
                }
                batch
            };
            if batch.is_empty() {
                continue;
            }
            let ids: Vec<_> = batch.iter().map(|d| d.delivery_id.as_str()).collect();
            let batch_id = sha256_bytes_hex(ids.join(":").as_bytes());
            let events:Vec<_>=batch.iter().map(|d|json!({"event_id":d.event_id,"delivery_id":d.delivery_id,"job_id":d.completion.job_id,"status":d.completion.status,"created_at":d.completion.created_at,"finished_at":d.completion.finished_at})).collect();
            let payload = json!({"version":2,"batch_id":batch_id,"events":events});
            let result = if serde_json::to_vec(&payload).map_or(true, |b| b.len() > 65536) {
                Err("payload_too_large")
            } else if !self.authorized(destination) {
                Err("authority_revoked")
            } else {
                tokio::time::timeout(
                    self.settings.timeout,
                    deliver(
                        destination,
                        self.settings.timeout,
                        if destination.spec.use_bearer {
                            destination.bearer.as_deref()
                        } else {
                            None
                        },
                        &payload,
                        &batch_id,
                    ),
                )
                .await
                .unwrap_or(Err("timeout"))
            };
            let retry = matches!(
                result,
                Err("timeout" | "transport_failed" | "dns_failed" | "retryable_status")
            );
            let mut g = self.outbox.lock().unwrap_or_else(|p| p.into_inner());
            let mut next = g.data.clone();
            for sent in &batch {
                if let Some(d) = next.deliveries.get_mut(&sent.delivery_id) {
                    d.state = if result.is_ok() {
                        "delivered"
                    } else if result == Err("authority_revoked") {
                        "revoked"
                    } else if retry && d.attempts < self.settings.attempts {
                        "pending"
                    } else {
                        "failed"
                    }
                    .into();
                    d.last_code = result.err().map(str::to_string);
                    // Waiting time excludes an unused request timeout budget.
                    if d.state == "pending" {
                        d.next_attempt_at = crate::common::now_ts()
                            + (sent.next_attempt_at - now - self.settings.timeout.as_secs_f64()).max(0.0);
                    }
                }
            }
            let persisted = g.commit(next).is_ok();
            self.audit.log(json!({"event":"notification_delivery","batch_id":batch_id,"count":batch.len(),"ok":result.is_ok(),"code":result.err(),"result_persisted":persisted}));
        }
    }
}

fn allowed_addresses(addresses: &[SocketAddr], private: bool) -> bool {
    !addresses.is_empty()
        && addresses.len() <= 32
        && addresses.iter().all(|a| {
            is_public_ip(a.ip())
                || private
                    && match a.ip() {
                        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
                        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
                    }
        })
}

async fn deliver(
    destination: &Destination,
    timeout: Duration,
    token: Option<&str>,
    payload: &serde_json::Value,
    batch_id: &str,
) -> std::result::Result<(), &'static str> {
    let host = destination
        .url
        .host_str()
        .ok_or("invalid_url")?
        .trim_matches(['[', ']']);
    let port = destination.url.port_or_known_default().ok_or("invalid_url")?;
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "dns_failed")?
        .take(33)
        .collect();
    send_pinned(destination, timeout, token, payload, batch_id, &addresses).await
}

async fn send_pinned(
    destination: &Destination,
    timeout: Duration,
    token: Option<&str>,
    payload: &Value,
    batch_id: &str,
    addresses: &[SocketAddr],
) -> std::result::Result<(), &'static str> {
    let host = destination
        .url
        .host_str()
        .ok_or("invalid_url")?
        .trim_matches(['[', ']']);
    if !allowed_addresses(addresses, destination.private)
        || (destination.url.scheme() != "https" && addresses.iter().any(|a| is_public_ip(a.ip())))
    {
        return Err("destination_refused");
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(0)
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(super::web_search::RefuseDns))
        .resolve_to_addrs(host, addresses)
        .timeout(timeout)
        .build()
        .map_err(|_| "client_failed")?;
    let mut request = client
        .post(destination.url.clone())
        .header("X-CGAgentHarness-Batch-ID", batch_id)
        .json(payload);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|_| "transport_failed")?;
    let status = response.status();
    // Response bodies and URLs may contain secrets. Neither is read or logged.
    if status.is_success() {
        Ok(())
    } else if status.as_u16() == 429 || status.is_server_error() {
        Err("retryable_status")
    } else {
        Err("status_refused")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    mod bearers_private {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        const SECRET: &str = "bearer-unit-fixture-secret";

        fn write(dir: &std::path::Path, mode: u32) -> std::path::PathBuf {
            let path = dir.join("bearers.json");
            let body = format!(r#"{{"version":1,"tokens":{{"hook":"{SECRET}"}}}}"#);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        }

        fn assert_refused(path: &std::path::Path) {
            let err = read_bearers_json(path).unwrap_err();
            let shown = format!("{err} {err:?}");
            assert!(shown.contains("chmod 600"), "error must say how to fix it");
            assert!(!shown.contains(SECRET), "error leaked the bearer secret");
        }

        #[test]
        fn private_file_loads() {
            let dir = tempfile::tempdir().unwrap();
            let value = read_bearers_json(&write(dir.path(), 0o600)).unwrap().unwrap();
            assert_eq!(value["tokens"]["hook"], SECRET);
        }

        #[test]
        fn missing_file_is_none() {
            let dir = tempfile::tempdir().unwrap();
            assert!(read_bearers_json(&dir.path().join("bearers.json")).unwrap().is_none());
        }

        #[test]
        fn group_or_world_bits_are_refused() {
            for mode in [0o644, 0o640, 0o604, 0o620, 0o660] {
                let dir = tempfile::tempdir().unwrap();
                assert_refused(&write(dir.path(), mode));
            }
        }

        #[test]
        fn hard_link_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let path = write(dir.path(), 0o600);
            std::fs::hard_link(&path, dir.path().join("other")).unwrap();
            assert_refused(&path);
        }

        #[test]
        fn symlink_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("real");
            std::fs::create_dir(&target).unwrap();
            let real = write(&target, 0o600);
            let link = dir.path().join("bearers.json");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_refused(&link);
        }

        #[test]
        fn directory_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("bearers.json");
            std::fs::create_dir(&path).unwrap();
            assert_refused(&path);
        }
    }

    #[test]
    fn optional_json_refuses_parent_components() {
        let err = read_optional_json(std::path::Path::new("/tmp/harness/../bearers.json")).unwrap_err();
        assert!(err.to_string().contains("invalid notifications configuration"));
    }
    #[test]
    fn destination_answers_fail_closed() {
        let addr = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(!allowed_addresses(&[], true));
        assert!(!allowed_addresses(&[addr("127.0.0.1:80")], false)); // DevSkim: ignore DS162092 because this unit test pins loopback for the webhook destination grant.
        assert!(allowed_addresses(&[addr("127.0.0.1:80")], true)); // DevSkim: ignore DS162092 because this unit test pins loopback for the webhook destination grant.
        assert!(!allowed_addresses(&[addr("8.8.8.8:443"), addr("127.0.0.1:80")], false)); // DevSkim: ignore DS162092 because this unit test pins loopback for the webhook destination grant.
        assert!(!allowed_addresses(&[addr("169.254.169.254:80")], true));
        assert!(!allowed_addresses(&[addr("[::ffff:127.0.0.1]:80")], true)); // DevSkim: ignore DS162092 because this unit test pins IPv4-mapped loopback for the webhook destination grant.
    }
    #[tokio::test]
    async fn pinned_transport_does_not_reresolve_and_refuses_rebound_answers() {
        use axum::{
            http::{HeaderMap, StatusCode},
            routing::post,
            Router,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // DevSkim: ignore DS162092 because this owned test receiver must bind only to loopback.
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = hits.clone();
        let router = Router::new().route(
            "/hook",
            post(move |headers: HeaderMap| {
                let count = count.clone();
                async move {
                    assert_eq!(
                        headers.get("authorization").unwrap(),
                        "Bearer isolated-transport-fixture"
                    );
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        // .invalid never resolves. Only the validated pinned loopback address
        // can reach this owned fixture; RefuseDns forbids any fallback lookup.
        let url = Url::parse(&format!("http://receiver.invalid:{}/hook", address.port())).unwrap(); // DevSkim: ignore DS137138 because the test pins this nonresolving hostname to its owned loopback HTTP receiver.
        let mut destination = Destination {
            spec: DestinationSpec {
                id: "fixture".into(),
                owner: "local".into(),
                enabled: true,
                url: url.to_string(),
                events: vec!["finished".into()],
                use_bearer: true,
                rate_per_minute: 60,
            },
            url,
            private: true,
            revision: "fixture".into(),
            bearer: Some("isolated-transport-fixture".into()),
            bearer_hash: None,
        };
        assert_eq!(
            send_pinned(
                &destination,
                Duration::from_secs(2),
                Some("isolated-transport-fixture"),
                &json!({}),
                "fixture",
                &[address]
            )
            .await,
            Ok(())
        );
        destination.private = false;
        assert_eq!(
            send_pinned(
                &destination,
                Duration::from_secs(2),
                None,
                &json!({}),
                "fixture",
                &[address]
            )
            .await,
            Err("destination_refused")
        );
        destination.private = true;
        let public: SocketAddr = "8.8.8.8:80".parse().unwrap();
        assert_eq!(
            send_pinned(
                &destination,
                Duration::from_secs(2),
                None,
                &json!({}),
                "fixture",
                &[address, public]
            )
            .await,
            Err("destination_refused"),
            "public HTTP cannot be authorized by a private URL grant"
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        task.abort();
    }
}
