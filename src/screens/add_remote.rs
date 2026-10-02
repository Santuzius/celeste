//! Add-remote flow for every provider Celeste supports:
//!
//! - WebDAV / Nextcloud / Owncloud — raw username + password
//! - Proton Drive — username + password + optional TOTP
//! - Dropbox / Google Drive / pCloud — OAuth2 via `rclone authorize`
//!   (launches the default browser; the user confirms, rclone prints
//!   the token, we pass it to config/create)

use iced::{
    Alignment, Element, Length,
    widget::{Space, button, column, container, pick_list, row, text::Shaping, text_input},
};

use std::sync::Arc;

use celeste_go::proton::HumanVerification;

use crate::{
    services::auth::{AuthorizeHandle, OAuthProvider, WebDavVendor},
    theme::{self, CAPTION, HEADING, ROW_SPACING, TEXT},
    widgets::{bullet, text},
};

#[derive(Debug, Clone)]
pub enum Msg {
    NameChanged(String),
    ProviderChanged(ProviderKind),
    UrlChanged(String),
    UserChanged(String),
    PassChanged(String),
    TotpChanged(String),
    ClientIdChanged(String),
    ClientSecretChanged(String),
    Submit,
    Cancel,
    /// Re-open the human-verification page in the browser.
    OpenVerification,
    /// Open / copy the OAuth link of a running `rclone authorize`.
    OpenAuthLink,
    CopyAuthLink,
    /// Open rclone's guide for creating a Google Drive client ID.
    OpenClientIdGuide,
    /// Copy Celeste's privacy policy link (for the Google consent screen's branding).
    CopyPrivacyLink,
}

/// rclone's step-by-step guide for creating a Google Drive OAuth client.
pub const GDRIVE_CLIENT_ID_GUIDE: &str = "https://rclone.org/drive/#making-your-own-client-id";

/// Celeste's privacy policy, usable as privacy policy link in the Google consent screen's branding.
pub const PRIVACY_POLICY: &str = "https://github.com/Santuzius/celeste/blob/main/PRIVACY.md";

/// The set of backends Celeste's sync algorithm has been exercised
/// against. WebDAV / Nextcloud / Owncloud / Dropbox / pCloud are
/// deliberately absent from the Add Remote picker: the snapshot
/// algorithm is backend-agnostic so they should work, but none of them
/// have been rate-limit-tested the way Proton and Google have. The
/// enum variants stay in place so the auth / RPC code paths keep
/// compiling — just the UI surface is slimmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    #[allow(dead_code)]
    WebDav,
    #[allow(dead_code)]
    Nextcloud,
    #[allow(dead_code)]
    Owncloud,
    ProtonDrive,
    #[allow(dead_code)]
    Dropbox,
    GDrive,
    #[allow(dead_code)]
    PCloud,
}

impl ProviderKind {
    /// Providers visible in the Add Remote UI today. Order matches the
    /// picker: Proton first (most recently tested), Google next.
    pub const ALL: [ProviderKind; 2] = [ProviderKind::ProtonDrive, ProviderKind::GDrive];

    pub fn webdav_vendor(self) -> Option<WebDavVendor> {
        match self {
            ProviderKind::WebDav => Some(WebDavVendor::WebDav),
            ProviderKind::Nextcloud => Some(WebDavVendor::Nextcloud),
            ProviderKind::Owncloud => Some(WebDavVendor::Owncloud),
            _ => None,
        }
    }

    pub fn oauth_provider(self) -> Option<OAuthProvider> {
        match self {
            ProviderKind::Dropbox => Some(OAuthProvider::Dropbox),
            ProviderKind::GDrive => Some(OAuthProvider::GDrive),
            ProviderKind::PCloud => Some(OAuthProvider::PCloud),
            _ => None,
        }
    }

    pub fn is_webdav_family(self) -> bool {
        self.webdav_vendor().is_some()
    }

    pub fn is_oauth(self) -> bool {
        self.oauth_provider().is_some()
    }

    /// Google retires rclone's shared OAuth client, so these need the
    /// user's own client ID + secret.
    pub fn needs_own_client_id(self) -> bool {
        matches!(self, ProviderKind::GDrive)
    }

    pub fn is_proton_drive(self) -> bool {
        matches!(self, ProviderKind::ProtonDrive)
    }
}

/// Pick-list wrapper over `ProviderKind`. Kept as a distinct type so
/// future UI annotations (e.g. per-provider glyphs) have a place to
/// live without polluting the core enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProviderOption(ProviderKind);

impl std::fmt::Display for ProviderOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

const PROVIDER_OPTIONS: [ProviderOption; 2] = [
    ProviderOption(ProviderKind::ProtonDrive),
    ProviderOption(ProviderKind::GDrive),
];

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            ProviderKind::WebDav => "Generic WebDAV",
            ProviderKind::Nextcloud => "Nextcloud",
            ProviderKind::Owncloud => "Owncloud",
            ProviderKind::ProtonDrive => "Proton Drive",
            ProviderKind::Dropbox => "Dropbox",
            ProviderKind::GDrive => "Google Drive",
            ProviderKind::PCloud => "pCloud",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub name: String,
    pub provider: Option<ProviderKind>,
    // WebDAV-family:
    pub url: String,
    // WebDAV + Proton Drive:
    pub user: String,
    pub pass: String,
    // Proton Drive:
    pub totp: String,
    // OAuth:
    pub client_id: String,
    pub client_secret: String,

    pub error: Option<String>,
    /// True while `rclone authorize` is running (or the blocking
    /// WebDAV validation is in flight). Disables the Submit button
    /// and swaps the header for a "please wait" hint.
    pub busy: bool,
    /// When `Some`, the dialog is a reauthentication flow for an
    /// existing remote (not a new one). The name and provider are
    /// locked, submit reuses the existing DB row + session path
    /// instead of inserting a new one.
    pub reauth: bool,
    /// Pending Proton human-verification challenge (CAPTCHA). Sent along
    /// with the next submit once the user has solved it in the browser.
    pub hv: Option<HumanVerification>,
    /// Running OAuth browser flow (cancel handle + authorization link).
    pub oauth: Option<Arc<AuthorizeHandle>>,
    /// The privacy policy link was copied; the button says so.
    pub privacy_link_copied: bool,
}

impl Draft {
    /// The dialog may be closed: nothing running, or only a browser
    /// flow that can be cancelled.
    pub fn can_cancel(&self) -> bool {
        !self.busy || self.oauth.is_some()
    }
}

const LABEL_WIDTH: f32 = 110.0;

fn field<'a>(label: &'static str, control: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    row![text(label).size(TEXT).width(Length::Fixed(LABEL_WIDTH)), control.into()]
        .align_y(Alignment::Center)
        .spacing(ROW_SPACING)
        .into()
}

fn input<'a>(placeholder: &'a str, value: &'a str, on_input: fn(String) -> Msg) -> text_input::TextInput<'a, Msg> {
    text_input(placeholder, value)
        .on_input(on_input)
        .on_submit(Msg::Submit)
        .padding(7)
        .size(TEXT)
        .style(theme::input)
}

fn hint<'a>(s: &'a str) -> Element<'a, Msg> {
    text(s).size(TEXT - 1.0).style(theme::muted).into()
}

/// Instruction text the user has to act on — body size and colour.
fn instruction<'a>(s: &'a str) -> Element<'a, Msg> {
    text(s).size(TEXT).into()
}

/// The dialog card; the app centres it over a dimmed backdrop.
pub fn view(draft: &Draft) -> Element<'_, Msg> {
    let heading = text(if draft.reauth { "Sign in again" } else { "Add remote" }).size(HEADING + 2.0);

    // In reauth mode the name and provider are fixed (the name keys the DB row + session), so show them as plain text.
    let mut body = column![heading].spacing(12);
    if draft.reauth {
        let provider = draft.provider.map(|p| p.to_string()).unwrap_or_default();
        body = body.push(hint("Enter fresh credentials. Folders, exclusions and schedule are kept — only the sign-in session is replaced."));
        body = body.push(field("Remote", text(format!("{} ({provider})", draft.name)).size(TEXT)));
    } else {
        let picker = pick_list(&PROVIDER_OPTIONS[..], draft.provider.map(ProviderOption), |o| Msg::ProviderChanged(o.0))
            .text_shaping(Shaping::Advanced)
            .text_size(TEXT)
            .padding([6, 10])
            .width(Length::Fill)
            .style(theme::pick_list)
            .menu_style(theme::menu)
            .placeholder("Choose a provider");
        body = body.push(field("Provider", picker));
        body = body.push(field("Name", input("e.g. ProtonDrive", &draft.name, Msg::NameChanged)));
    }

    match draft.provider {
        Some(p) if p.is_webdav_family() => {
            body = body.push(field("URL", input("https://cloud.example.org", &draft.url, Msg::UrlChanged)));
            body = body.push(field("Username", input("username", &draft.user, Msg::UserChanged)));
            body = body.push(field("Password", input("password", &draft.pass, Msg::PassChanged).secure(true)));
        }
        Some(p) if p.is_proton_drive() => {
            // Wording required by Proton's guidelines for third-party Drive clients.
            body = body.push(instruction("This is a third-party application not officially supported by Proton."));
            body = body.push(field("E-mail", input("example@proton.me", &draft.user, Msg::UserChanged)));
            body = body.push(field("Password", input("password", &draft.pass, Msg::PassChanged).secure(true)));
            body = body.push(field("2FA code", input("only if two-factor authentication is on", &draft.totp, Msg::TotpChanged)));
            body = body.push(hint(
                "With 2FA enabled, Proton eventually expires the session and Celeste can't renew it on its own (the code is single-use). You'll then be asked to sign in again.",
            ));
        }
        Some(p) if p.needs_own_client_id() => {
            body = body.push(instruction("Google Drive needs your own OAuth client ID. Creating one takes a few minutes in the Google Cloud console — just follow the guide."));
            body = body.push(
                column![
                    instruction("When the guide gets to the consent screen's Branding, enter:"),
                    bullet("Application home page: your website or social media profile, e.g. https://t.me/YourTelegramName"),
                    bullet("Application privacy policy link: Celeste's privacy policy (copy it with the button below)"),
                ]
                .spacing(4),
            );
            body = body.push(
                row![
                    button(text("How to create a client ID").size(TEXT))
                        .padding([6, 14])
                        .style(theme::button_secondary)
                        .on_press(Msg::OpenClientIdGuide),
                    button(text(if draft.privacy_link_copied { "Link copied" } else { "Copy privacy policy link" }).size(TEXT))
                        .padding([6, 14])
                        .style(theme::button_secondary)
                        .on_press(Msg::CopyPrivacyLink),
                ]
                .spacing(ROW_SPACING),
            );
            body = body.push(field("Client ID", input("…apps.googleusercontent.com", &draft.client_id, Msg::ClientIdChanged)));
            body = body.push(field("Client secret", input("client secret", &draft.client_secret, Msg::ClientSecretChanged).secure(true)));
            body = body.push(instruction("Then click Connect and grant access in your browser."));
        }
        Some(p) if p.is_oauth() => {
            body = body.push(hint("Click Connect and grant access in your browser. Client ID and secret are optional — leave them empty to use rclone's defaults."));
            body = body.push(field("Client ID", input("optional", &draft.client_id, Msg::ClientIdChanged)));
            body = body.push(field("Client secret", input("optional", &draft.client_secret, Msg::ClientSecretChanged).secure(true)));
        }
        Some(_) | None => {}
    }

    if draft.hv.is_some() && !draft.busy {
        body = body.push(
            container(
                column![
                    text("Proton wants to make sure you're human").size(TEXT),
                    text("1. Solve the CAPTCHA on the page that just opened in your browser.").size(CAPTION),
                    text(format!("2. Come back and click {}.", if draft.reauth { "Sign in" } else { "Add" })).size(CAPTION),
                    button(text("Open verification page again").size(CAPTION))
                        .padding([5, 12])
                        .style(theme::button_secondary)
                        .on_press(Msg::OpenVerification),
                ]
                .spacing(6),
            )
            .padding([10, 12])
            .width(Length::Fill)
            .style(theme::warning_bar),
        );
    }

    if draft.busy {
        match &draft.oauth {
            Some(handle) => {
                let mut col = column![text("Waiting for you to finish in the browser…").size(TEXT)].spacing(6);
                // Wrong browser, or none at all? The link works in any browser on this computer.
                if handle.url().is_some() {
                    col = col.push(text("If no browser opened (or the wrong one), open the authorization link in any browser on this computer.").size(CAPTION).style(theme::muted));
                    col = col.push(
                        row![
                            button(text("Copy link").size(CAPTION)).padding([5, 12]).style(theme::button_secondary).on_press(Msg::CopyAuthLink),
                            button(text("Open again").size(CAPTION)).padding([5, 12]).style(theme::button_secondary).on_press(Msg::OpenAuthLink),
                        ]
                        .spacing(ROW_SPACING),
                    );
                }
                body = body.push(col);
            }
            None => body = body.push(text("Signing in…").size(CAPTION)),
        }
    }

    if let Some(err) = &draft.error {
        body = body.push(text(err).size(CAPTION).style(theme::danger_text));
    }

    let submit_label = if draft.reauth {
        "Sign in"
    } else {
        match draft.provider {
            Some(p) if p.is_oauth() => "Connect",
            _ => "Add",
        }
    };
    let submit = button(text(submit_label).size(TEXT))
        .padding([6, 16])
        .style(theme::button_primary)
        .on_press_maybe((!draft.busy && draft.provider.is_some()).then_some(Msg::Submit));
    let cancel = button(text("Cancel").size(TEXT))
        .padding([6, 16])
        .style(theme::button_secondary)
        .on_press_maybe(draft.can_cancel().then_some(Msg::Cancel));

    body = body.push(Space::new().height(Length::Fixed(4.0)));
    body = body.push(row![Space::new().width(Length::Fill), cancel, submit].spacing(ROW_SPACING));

    container(body).padding(22).max_width(540).style(theme::dialog).into()
}
