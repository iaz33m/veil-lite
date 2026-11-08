//! Prompts through `zenity` (GNOME and most desktops) or `kdialog` (KDE), and typing warnings as desktop
//! notifications. No toolkit is linked; when neither program exists the prompt resolves to its safe choice
//! and a notification explains what happened.
//!
//! Safety rule for both backends: Escape, closing the window and the default button all map to the
//! prompt's safe choice ([`Prompt::cancel_choice`]).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use veil_core::platform::{Choice, Prompt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Zenity,
    Kdialog,
}

/// A prompt turned into a command line plus the mapping back to choices.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub backend: Backend,
    pub args: Vec<String>,
    safe: Choice,
    /// Choice per exit code 0 (zenity OK / kdialog Yes).
    on_zero: Option<Choice>,
    /// Choice for kdialog exit code 1 (No) when it is not the safe one.
    on_one: Option<Choice>,
    /// zenity extra buttons: label printed on stdout → choice.
    extras: Vec<(String, Choice)>,
}

fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn plan(backend: Backend, prompt: &Prompt) -> Plan {
    let safe = prompt.cancel_choice();
    let others: Vec<Choice> = prompt.options.iter().copied().filter(|c| *c != safe).collect();
    let label = |c: Choice| prompt.label(c).to_string();
    let mut p = Plan { backend, args: Vec::new(), safe, on_zero: None, on_one: None, extras: Vec::new() };
    match backend {
        Backend::Zenity => {
            let text = escape_markup(&prompt.message);
            if others.is_empty() {
                p.args =
                    vec!["--warning".into(), "--title=Veil".into(), "--width=420".into(), format!("--text={text}")];
                return p;
            }
            p.args = vec![
                "--question".into(),
                "--title=Veil".into(),
                "--width=420".into(),
                format!("--text={text}"),
                format!("--ok-label={}", label(others[0])),
                format!("--cancel-label={}", label(safe)),
                "--default-cancel".into(),
            ];
            p.on_zero = Some(others[0]);
            for c in &others[1..] {
                p.args.push(format!("--extra-button={}", label(*c)));
                p.extras.push((label(*c), *c));
            }
        }
        Backend::Kdialog => {
            let base = vec!["--title".to_string(), "Veil".to_string()];
            p.args = base;
            match others.len() {
                0 => p.args.extend(["--sorry".into(), prompt.message.clone()]),
                1 => {
                    p.args.extend([
                        "--warningyesno".into(),
                        prompt.message.clone(),
                        "--yes-label".into(),
                        label(others[0]),
                        "--no-label".into(),
                        label(safe),
                    ]);
                    p.on_zero = Some(others[0]);
                }
                _ => {
                    p.args.extend([
                        "--warningyesnocancel".into(),
                        prompt.message.clone(),
                        "--yes-label".into(),
                        label(others[0]),
                        "--no-label".into(),
                        label(others[1]),
                        "--cancel-label".into(),
                        label(safe),
                    ]);
                    p.on_zero = Some(others[0]);
                    p.on_one = Some(others[1]);
                }
            }
        }
    }
    p
}

/// Map the dialog's exit code and stdout back to a choice. Anything unexpected is the safe choice.
pub fn parse(plan: &Plan, code: Option<i32>, stdout: &str) -> Choice {
    match (plan.backend, code) {
        (_, Some(0)) => plan.on_zero.unwrap_or(plan.safe),
        (Backend::Zenity, Some(1)) => {
            let out = stdout.trim();
            plan.extras.iter().find(|(l, _)| l == out).map(|(_, c)| *c).unwrap_or(plan.safe)
        }
        (Backend::Kdialog, Some(1)) => plan.on_one.unwrap_or(plan.safe),
        _ => plan.safe,
    }
}

pub fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

pub fn available_backend() -> Option<Backend> {
    let kde = std::env::var("XDG_CURRENT_DESKTOP").map(|d| d.to_ascii_uppercase().contains("KDE")).unwrap_or(false);
    let zenity = find_program("zenity").is_some();
    let kdialog = find_program("kdialog").is_some();
    match (kde, zenity, kdialog) {
        (true, _, true) => Some(Backend::Kdialog),
        (_, true, _) => Some(Backend::Zenity),
        (_, _, true) => Some(Backend::Kdialog),
        _ => None,
    }
}

/// Show the prompt and wait. Blocks the calling (worker) thread.
pub fn ask(prompt: &Prompt) -> Choice {
    let Some(backend) = available_backend() else {
        tracing::warn!("neither zenity nor kdialog is installed; using the safe choice");
        notify(&format!("{}\n\n(Install zenity or kdialog to choose. Veil used the safe option.)", prompt.message));
        return prompt.cancel_choice();
    };
    let plan = plan(backend, prompt);
    let program = match backend {
        Backend::Zenity => "zenity",
        Backend::Kdialog => "kdialog",
    };
    match Command::new(program)
        .args(&plan.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) => parse(&plan, out.status.code(), &String::from_utf8_lossy(&out.stdout)),
        Err(e) => {
            tracing::warn!("{program} failed: {e}");
            prompt.cancel_choice()
        }
    }
}

/// Transient, low-urgency desktop notification (fire and forget).
pub fn notify(text: &str) {
    let text = text.to_string();
    let _ = std::thread::Builder::new().name("veil-notify".into()).spawn(move || {
        let res = notify_rust::Notification::new()
            .appname("Veil")
            .summary("Veil")
            .body(&text)
            .icon("dialog-warning")
            .urgency(notify_rust::Urgency::Low)
            .hint(notify_rust::Hint::Transient(true))
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();
        if let Err(e) = res {
            tracing::debug!("notification failed: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use veil_core::platform::PromptKind;
    use veil_core::types::AppContext;

    fn prompt(kind: PromptKind, options: Vec<Choice>) -> Prompt {
        Prompt {
            title: "Veil".into(),
            message: "Contains <b>secrets</b> & more".into(),
            app: AppContext::default(),
            kind,
            options,
        }
    }

    #[test]
    fn zenity_three_buttons_with_safe_cancel() {
        let p = prompt(PromptKind::Send, vec![Choice::DontSend, Choice::SendAnyway, Choice::Redact]);
        let plan = plan(Backend::Zenity, &p);
        assert!(plan.args.contains(&"--question".to_string()));
        assert!(plan.args.contains(&"--text=Contains &lt;b&gt;secrets&lt;/b&gt; &amp; more".to_string()));
        assert!(plan.args.contains(&"--ok-label=Send anyway".to_string()));
        assert!(plan.args.contains(&"--cancel-label=Don't send".to_string()));
        assert!(plan.args.contains(&"--default-cancel".to_string()));
        assert!(plan.args.contains(&"--extra-button=Redact & send".to_string()));
        assert_eq!(parse(&plan, Some(0), ""), Choice::SendAnyway);
        assert_eq!(parse(&plan, Some(1), "Redact & send\n"), Choice::Redact);
        assert_eq!(parse(&plan, Some(1), ""), Choice::DontSend, "Esc / Cancel");
        assert_eq!(parse(&plan, Some(5), ""), Choice::DontSend, "timeout");
        assert_eq!(parse(&plan, None, ""), Choice::DontSend, "killed");
    }

    #[test]
    fn zenity_notice_has_no_choices() {
        let p = prompt(PromptKind::BlockNotice, vec![Choice::Ok]);
        let plan = plan(Backend::Zenity, &p);
        assert_eq!(plan.args[0], "--warning");
        assert_eq!(parse(&plan, Some(0), ""), Choice::Ok);
    }

    #[test]
    fn drop_prompt_keeps_ok_as_the_safe_choice() {
        let p = prompt(PromptKind::Drop, vec![Choice::Ok, Choice::AllowFor30s]);
        let plan = plan(Backend::Zenity, &p);
        assert!(plan.args.contains(&"--ok-label=Allow drops for 30 s".to_string()));
        assert!(plan.args.contains(&"--cancel-label=OK".to_string()));
        assert_eq!(parse(&plan, Some(1), ""), Choice::Ok);
        assert_eq!(parse(&plan, Some(0), ""), Choice::AllowFor30s);
    }

    #[test]
    fn kdialog_maps_escape_to_the_safe_choice() {
        let p = prompt(PromptKind::Paste, vec![Choice::DontSend, Choice::SendAnyway, Choice::Redact]);
        let plan = plan(Backend::Kdialog, &p);
        assert!(plan.args.contains(&"--warningyesnocancel".to_string()));
        let i = plan.args.iter().position(|a| a == "--cancel-label").unwrap();
        assert_eq!(plan.args[i + 1], "Don't paste");
        assert_eq!(parse(&plan, Some(0), ""), Choice::SendAnyway);
        assert_eq!(parse(&plan, Some(1), ""), Choice::Redact);
        assert_eq!(parse(&plan, Some(2), ""), Choice::DontSend);

        let p = prompt(PromptKind::Upload, vec![Choice::DontSend, Choice::SendAnyway]);
        let plan = super::plan(Backend::Kdialog, &p);
        assert!(plan.args.contains(&"--warningyesno".to_string()));
        assert_eq!(parse(&plan, Some(0), ""), Choice::SendAnyway);
        assert_eq!(parse(&plan, Some(1), ""), Choice::DontSend);

        let p = prompt(PromptKind::BlockNotice, vec![Choice::Ok]);
        assert_eq!(parse(&super::plan(Backend::Kdialog, &p), Some(0), ""), Choice::Ok);
    }
}
