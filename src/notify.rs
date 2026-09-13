use crate::config::{Config, EmailCfg};
use crate::model::{Candidate, Tier};
use anyhow::{bail, Context};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

fn tier_of(c: &Candidate) -> Tier {
    match c.tier.as_str() {
        "exceptional" => Tier::Exceptional,
        "strong" => Tier::Strong,
        _ => Tier::Marginal,
    }
}

/// HTTP header values must be visible ASCII. ntfy titles/companies can contain
/// unicode, so fold anything non-ASCII down before it goes in a header.
fn ascii(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { ' ' })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Instant phone push via ntfy.sh. Title/priority/click/actions ride in headers.
pub async fn push_ntfy(
    http: &reqwest::Client,
    cfg: &Config,
    c: &Candidate,
) -> anyhow::Result<()> {
    let url = format!("{}/{}", cfg.ntfy.server.trim_end_matches('/'), cfg.ntfy.topic);
    let dash = format!("{}/", cfg.server.base_url.trim_end_matches('/'));
    let title = ascii(&format!("{} - {}", c.title, c.company));
    // Body is not a header, so unicode is fine here.
    let msg = format!(
        "Match {:.0} · {} · {}",
        c.score,
        c.age_str(),
        c.location.clone().unwrap_or_default()
    );
    // Two action buttons: open the post, open the dashboard.
    let actions = ascii(&format!("view, Open post, {}; view, Dashboard, {}", c.url, dash));

    http.post(&url)
        .header("Title", title)
        .header("Priority", tier_of(c).ntfy_priority())
        .header("Tags", "briefcase")
        .header("Click", dash)
        .header("Actions", actions)
        .body(msg)
        .send()
        .await
        .context("ntfy push failed")?;
    Ok(())
}

fn transport(cfg: &EmailCfg) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
    let pass = cfg
        .smtp_password
        .clone()
        .context("SMTP_PASSWORD not set")?;
    let creds = Credentials::new(cfg.smtp_user.clone(), pass);
    let tp = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)?
        .port(cfg.smtp_port)
        .credentials(creds)
        .build();
    Ok(tp)
}

/// The alert to YOU: the match plus the ready-to-edit draft and a dashboard link.
/// ntfy is the ping; this email is the record.
pub async fn email_self(cfg: &Config, c: &Candidate) -> anyhow::Result<()> {
    let dash = format!("{}/", cfg.server.base_url.trim_end_matches('/'));
    let draft = c
        .draft_body
        .clone()
        .unwrap_or_else(|| "(no draft generated)".into());
    let channel = match c.apply_kind.as_str() {
        "email" => format!("Email → {}", c.apply_target.clone().unwrap_or_default()),
        "dm" => "LinkedIn DM (send manually from the dashboard)".into(),
        "external" => format!("Apply link → {}", c.apply_target.clone().unwrap_or_default()),
        _ => "Unknown apply channel".into(),
    };
    let body = format!(
        "New match: {title} at {company}\n\
         Match score: {score:.0}  ·  {age}  ·  {loc}\n\
         Apply via: {channel}\n\
         Post: {url}\n\n\
         ---- draft ----\n{draft}\n---------------\n\n\
         Review / send / dismiss on the dashboard: {dash}\n",
        title = c.title,
        company = c.company,
        score = c.score,
        age = c.age_str(),
        loc = c.location.clone().unwrap_or_default(),
        channel = channel,
        url = c.url,
        draft = draft,
        dash = dash,
    );

    let email = Message::builder()
        .from(cfg.email.from.parse()?)
        .to(cfg.profile.email.parse()?)
        .subject(format!("[radar] {} at {}", c.title, c.company))
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;

    transport(&cfg.email)?.send(email).await?;
    Ok(())
}

/// Outbound application email to the recruiter. Only valid for the email channel;
/// triggered by the dashboard "Send" button after you've edited the draft.
pub async fn send_application(
    cfg: &Config,
    c: &Candidate,
    subject: &str,
    body: &str,
) -> anyhow::Result<()> {
    let to = match c.apply_kind.as_str() {
        "email" => c
            .apply_target
            .clone()
            .context("email channel has no target address")?,
        other => bail!("cannot auto-send for '{other}' channel — do it manually"),
    };
    let email = Message::builder()
        .from(cfg.email.from.parse()?)
        .to(to.parse()?)
        .subject(subject.to_string())
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?;
    transport(&cfg.email)?.send(email).await?;
    Ok(())
}

/// Hourly digest of marginal matches — one email, not a stream.
pub async fn email_digest(cfg: &Config, items: &[Candidate]) -> anyhow::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut body = String::from("Marginal matches this hour (below the instant bar):\n\n");
    for c in items {
        body.push_str(&format!(
            "• {} at {} — match {:.0} — {}\n  {}\n",
            c.title, c.company, c.score, c.age_str(), c.url
        ));
    }
    let email = Message::builder()
        .from(cfg.email.from.parse()?)
        .to(cfg.profile.email.parse()?)
        .subject(format!("[radar] hourly digest — {} matches", items.len()))
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;
    transport(&cfg.email)?.send(email).await?;
    Ok(())
}
