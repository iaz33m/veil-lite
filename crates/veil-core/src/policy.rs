//! Policy: one YAML file, hot-reloaded. See docs/ARCHITECTURE.md §7.

use crate::findings::{CheckId, Finding, FindingKind};
use crate::types::{AppContext, Channel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Allow,
    /// Popup: Send anyway / Don't send / Redact & send.
    Ask,
    /// No override.
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeMode {
    AllApps,
    ListedApps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailMode {
    Open,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    pub mode: ScopeMode,
    /// Apps never inspected (bundle ids / executable names, case-insensitive).
    pub exclude_apps: Vec<String>,
    /// Apps inspected in `listed_apps` mode.
    pub include_apps: Vec<String>,
    /// Sites inspected in browsers in `listed_apps` mode (e.g. `chatgpt.com`).
    pub include_sites: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pii {
    pub enabled: bool,
    /// `universal`, `us`, `uk`, `eu`, `apac`.
    pub regions: Vec<String>,
    /// Minimum recognizer score.
    pub min_score: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckRule {
    pub threshold: f32,
    pub action: Action,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FailModes {
    pub text: FailMode,
    pub files: FailMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Actions {
    /// Action for keyword and personal-data findings.
    pub default: Action,
    pub allow_redact: bool,
    pub fail_mode: FailModes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PausePolicy {
    pub allowed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityPolicy {
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelPolicy {
    pub enabled: bool,
    /// Max 512-token chunks per file.
    pub max_chunks: usize,
    /// Unload after this many idle seconds.
    pub idle_unload_secs: u64,
    /// Background pre-scoring delay after typing pauses.
    pub prescore_delay_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub version: u32,
    pub scope: Scope,
    pub keywords: Vec<String>,
    pub pii: Pii,
    pub checks: BTreeMap<String, CheckRule>,
    pub actions: Actions,
    pub pause: PausePolicy,
    pub activity_log: ActivityPolicy,
    pub model: ModelPolicy,
    /// Labels that identify Send-type buttons (lower case, any language).
    pub send_labels: Vec<String>,
    /// Labels that identify file-picker confirm buttons.
    pub pick_labels: Vec<String>,
}

pub const DEFAULT_EXCLUDES: &[&str] = &[
    // terminals
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "dev.warp.Warp-Stable",
    "com.mitchellh.ghostty",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "com.github.wez.wezterm",
    "WindowsTerminal.exe",
    "OpenConsole.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "gnome-terminal-server",
    "konsole",
    "xterm",
    "kitty",
    "alacritty",
    "wezterm-gui",
    "ptyxis",
    // file managers and shell
    "com.apple.finder",
    "com.apple.dock",
    "com.apple.Spotlight",
    "explorer.exe",
    "nautilus",
    "dolphin",
    // system prompts and password managers
    "com.apple.UserNotificationCenter",
    "com.apple.loginwindow",
    "com.apple.SecurityAgent",
    "com.apple.keychainaccess",
    "com.apple.Passwords",
    "LogonUI.exe",
    "consent.exe",
    "CredentialUIBroker.exe",
    "com.1password.1password",
    "1Password.exe",
    "1password",
    "com.bitwarden.desktop",
    "Bitwarden.exe",
    "bitwarden",
    "KeePassXC.exe",
    "keepassxc",
    // Veil itself
    "dev.veil.app",
    "veil-engine",
    "veil-engine.exe",
    "Veil.exe",
    "veil",
];

impl Default for Policy {
    fn default() -> Self {
        let mut checks = BTreeMap::new();
        checks.insert("secrets".into(), CheckRule { threshold: 0.60, action: Action::Ask, enabled: true });
        checks.insert("personal_info".into(), CheckRule { threshold: 0.70, action: Action::Ask, enabled: true });
        checks.insert("confidential".into(), CheckRule { threshold: 0.70, action: Action::Ask, enabled: true });
        checks.insert("source_code".into(), CheckRule { threshold: 0.75, action: Action::Ask, enabled: true });
        Policy {
            version: 1,
            scope: Scope::default(),
            keywords: vec!["confidential".into()],
            pii: Pii::default(),
            checks,
            actions: Actions::default(),
            pause: PausePolicy { allowed: true },
            activity_log: ActivityPolicy { enabled: true },
            model: ModelPolicy::default(),
            send_labels: ["send", "submit", "generate", "post", "reply", "share", "ask", "enviar", "envoyer", "senden", "invia", "verzenden"]
                .map(String::from)
                .to_vec(),
            pick_labels: ["open", "upload", "choose", "attach", "select", "insert", "abrir", "ouvrir", "öffnen", "apri"]
                .map(String::from)
                .to_vec(),
        }
    }
}

impl Default for Scope {
    fn default() -> Self {
        Scope {
            mode: ScopeMode::AllApps,
            exclude_apps: DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect(),
            include_apps: Vec::new(),
            include_sites: ["chatgpt.com", "chat.openai.com", "claude.ai", "gemini.google.com", "copilot.microsoft.com", "perplexity.ai", "chat.deepseek.com", "grok.com", "chat.mistral.ai"]
                .map(String::from)
                .to_vec(),
        }
    }
}

impl Default for Pii {
    fn default() -> Self {
        Pii { enabled: true, regions: ["universal", "us", "uk", "eu", "apac"].map(String::from).to_vec(), min_score: 0.5 }
    }
}

impl Default for Actions {
    fn default() -> Self {
        Actions {
            default: Action::Ask,
            allow_redact: true,
            fail_mode: FailModes { text: FailMode::Open, files: FailMode::Closed },
        }
    }
}

impl Default for PausePolicy {
    fn default() -> Self {
        PausePolicy { allowed: true }
    }
}

impl Default for ActivityPolicy {
    fn default() -> Self {
        ActivityPolicy { enabled: true }
    }
}

impl Default for ModelPolicy {
    fn default() -> Self {
        ModelPolicy { enabled: true, max_chunks: 32, idle_unload_secs: 300, prescore_delay_ms: 400 }
    }
}

/// The outcome of evaluating findings against the policy.
#[derive(Clone, Debug, PartialEq)]
pub struct PolicyDecision {
    pub action: Action,
    /// Findings that triggered the action (layer-3 findings below threshold are dropped).
    pub findings: Vec<Finding>,
}

impl Policy {
    pub fn load(path: &Path) -> anyhow::Result<Policy> {
        let text = std::fs::read_to_string(path)?;
        Self::from_yaml(&text)
    }

    pub fn from_yaml(text: &str) -> anyhow::Result<Policy> {
        let p: Policy = serde_yaml_ng::from_str(text)?;
        p.validate()?;
        Ok(p)
    }

    pub fn to_yaml(&self) -> String {
        serde_yaml_ng::to_string(self).unwrap_or_default()
    }

    /// Load the policy, writing the default one if none exists yet.
    pub fn load_or_create(path: &Path) -> anyhow::Result<Policy> {
        if path.exists() {
            return Self::load(path);
        }
        let p = Policy::default();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, p.to_yaml())?;
        Ok(p)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        for (key, rule) in &self.checks {
            anyhow::ensure!(CheckId::from_key(key).is_some(), "unknown check `{key}`");
            anyhow::ensure!((0.0..=1.0).contains(&rule.threshold), "threshold for `{key}` must be between 0 and 1");
        }
        for region in &self.pii.regions {
            anyhow::ensure!(
                ["universal", "us", "uk", "eu", "apac"].contains(&region.as_str()),
                "unknown PII region `{region}`"
            );
        }
        Ok(())
    }

    pub fn check_rule(&self, id: CheckId) -> Option<CheckRule> {
        self.checks.get(id.key()).copied().filter(|r| r.enabled)
    }

    /// Is this app (and site) inspected?
    pub fn protects(&self, app: &AppContext) -> bool {
        let id = app.app_id.to_ascii_lowercase();
        let name = app.name.to_ascii_lowercase();
        let excluded = self.scope.exclude_apps.iter().any(|e| {
            let e = e.to_ascii_lowercase();
            e == id || e == name
        });
        if excluded || id.is_empty() {
            return false;
        }
        match self.scope.mode {
            ScopeMode::AllApps => true,
            ScopeMode::ListedApps => {
                let listed = self.scope.include_apps.iter().any(|e| {
                    let e = e.to_ascii_lowercase();
                    e == id || e == name
                });
                let site_listed = app.site().is_some_and(|site| {
                    self.scope
                        .include_sites
                        .iter()
                        .any(|s| site == s.to_ascii_lowercase() || site.ends_with(&format!(".{}", s.to_ascii_lowercase())))
                });
                listed || site_listed
            }
        }
    }

    /// Decide what to do with the findings for a channel. The strictest triggered action wins.
    pub fn decide(&self, findings: &[Finding], _channel: Channel) -> PolicyDecision {
        let mut action = Action::Allow;
        let mut triggered = Vec::new();
        for f in findings {
            let a = match &f.kind {
                FindingKind::Keyword(_) => Some(self.actions.default),
                FindingKind::Pii(_) => (f.score >= self.pii.min_score).then_some(self.actions.default),
                FindingKind::Check(id) => self
                    .check_rule(*id)
                    .and_then(|rule| (f.score >= rule.threshold).then_some(rule.action)),
            };
            if let Some(a) = a {
                if a != Action::Allow {
                    triggered.push(f.clone());
                }
                action = action.max(a);
            }
        }
        PolicyDecision { action, findings: triggered }
    }

    pub fn is_send_label(&self, label: &str) -> bool {
        let l = label.to_lowercase();
        // Labels of Veil's own popup buttons never count.
        if l.contains("send anyway") || l.contains("don't send") || l.contains("redact") {
            return false;
        }
        self.send_labels.iter().any(|w| l.split(|c: char| !c.is_alphanumeric()).any(|tok| tok == w))
    }

    pub fn is_pick_label(&self, label: &str) -> bool {
        let l = label.to_lowercase();
        let first = l.split(|c: char| !c.is_alphanumeric()).find(|t| !t.is_empty()).unwrap_or("");
        self.pick_labels.iter().any(|w| first == w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, url: Option<&str>) -> AppContext {
        AppContext { app_id: id.into(), name: id.into(), pid: 1, url: url.map(String::from), ..Default::default() }
    }

    #[test]
    fn default_policy_round_trips_through_yaml() {
        let p = Policy::default();
        let back = Policy::from_yaml(&p.to_yaml()).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn partial_yaml_uses_defaults() {
        let p = Policy::from_yaml("keywords: [secret]\nactions:\n  default: block\n  allow_redact: false\n  fail_mode: {text: open, files: closed}\n").unwrap();
        assert_eq!(p.keywords, vec!["secret"]);
        assert_eq!(p.actions.default, Action::Block);
        assert!(p.check_rule(CheckId::Secrets).is_some());
    }

    #[test]
    fn invalid_values_are_rejected() {
        assert!(Policy::from_yaml("checks:\n  nope: {threshold: 0.5, action: ask}\n").is_err());
        assert!(Policy::from_yaml("checks:\n  secrets: {threshold: 1.5, action: ask}\n").is_err());
        assert!(Policy::from_yaml("pii:\n  regions: [mars]\n").is_err());
    }

    #[test]
    fn scope_all_apps_respects_exclusions() {
        let p = Policy::default();
        assert!(p.protects(&app("com.tinyspeck.slackmacgap", None)));
        assert!(!p.protects(&app("com.apple.Terminal", None)));
        assert!(!p.protects(&app("WINDOWSTERMINAL.EXE", None)));
        assert!(!p.protects(&app("", None)));
    }

    #[test]
    fn scope_listed_apps_matches_apps_and_sites() {
        let mut p = Policy::default();
        p.scope.mode = ScopeMode::ListedApps;
        p.scope.include_apps = vec!["slack.exe".into()];
        assert!(p.protects(&app("slack.exe", None)));
        assert!(p.protects(&app("chrome.exe", Some("https://chatgpt.com/c/1"))));
        assert!(p.protects(&app("chrome.exe", Some("https://www.perplexity.ai/"))));
        assert!(!p.protects(&app("chrome.exe", Some("https://mail.google.com/"))));
    }

    #[test]
    fn strictest_action_wins_and_thresholds_apply() {
        let mut p = Policy::default();
        p.checks.get_mut("secrets").unwrap().action = Action::Block;
        let f = vec![
            Finding::keyword("confidential", 0..12, "m"),
            Finding::check(CheckId::Secrets, 0.55, "m"),
            Finding::check(CheckId::SourceCode, 0.9, "m"),
        ];
        let d = p.decide(&f, Channel::Message);
        assert_eq!(d.action, Action::Ask);
        assert_eq!(d.findings.len(), 2, "secrets below threshold is dropped");

        let f = vec![Finding::check(CheckId::Secrets, 0.95, "m")];
        assert_eq!(p.decide(&f, Channel::Message).action, Action::Block);
        assert_eq!(p.decide(&[], Channel::Message).action, Action::Allow);
    }

    #[test]
    fn low_score_pii_is_ignored() {
        let p = Policy::default();
        let d = p.decide(&[Finding::pii("PHONE_NUMBER", 0..5, 0.3, "m")], Channel::Message);
        assert_eq!(d.action, Action::Allow);
    }

    #[test]
    fn send_and_pick_labels() {
        let p = Policy::default();
        assert!(p.is_send_label("Send message"));
        assert!(p.is_send_label(" send now"));
        assert!(p.is_send_label("Generate (⌘Enter)"));
        assert!(!p.is_send_label("Send anyway"));
        assert!(!p.is_send_label("Don't send"));
        assert!(!p.is_send_label("Sender settings"));
        assert!(p.is_pick_label("Open"));
        assert!(p.is_pick_label("Upload files"));
        assert!(!p.is_pick_label("Cancel"));
    }
}
