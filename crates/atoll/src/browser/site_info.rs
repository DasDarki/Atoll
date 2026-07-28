use std::rc::Rc;

use atoll_core::address::{display_host, display_url, origin_of};
use atoll_core::time::calendar_date;
use atoll_engine::{SecurityInfo, ZoomCommand};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Browser;
use crate::ui::{OverlayState, PageSecurity, SiteInfoDetail};

/// What the site information popup says about a page.
#[derive(Debug, Clone, PartialEq)]
struct SiteSummary {
    host: String,
    security: PageSecurity,
    title: &'static str,
    message: &'static str,
    details: Vec<(&'static str, String)>,
}

impl Browser {
    /// Click on the symbol in the address bar: shows connection and certificate details.
    pub fn show_site_info(&self) {
        let Some(window) = self.window() else { return };
        let Some(url) = self.current_page_url() else {
            return;
        };
        self.dismiss_suggestions();
        let mut security = None;
        let mut zoom = 100;
        self.with_live_view(|view| {
            security = view.security_info();
            zoom = view.zoom_percent();
        });
        let summary = site_summary(&url, security.as_ref());
        let details: Vec<SiteInfoDetail> = summary
            .details
            .iter()
            .map(|(label, value)| SiteInfoDetail {
                label: (*label).into(),
                value: value.as_str().into(),
            })
            .collect();
        let overlay = window.global::<OverlayState>();
        overlay.set_site_info_host(summary.host.as_str().into());
        overlay.set_site_info_security(summary.security);
        overlay.set_site_info_title(summary.title.into());
        overlay.set_site_info_message(summary.message.into());
        overlay.set_site_info_details(ModelRc::from(Rc::new(VecModel::from(details))));
        overlay.set_site_info_zoom(i32::try_from(zoom).unwrap_or(100));
        overlay.set_site_info_can_clear(is_web_address(&url) && origin_of(&url).is_some());
        self.show_site_adblock(&url);
        overlay.set_site_info_visible(true);
    }

    pub fn close_site_info(&self) {
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_site_info_visible(false);
        }
        self.focus_content();
    }

    pub fn site_info_reset_zoom(&self) {
        self.zoom(ZoomCommand::Reset);
        if let Some(window) = self.window() {
            window.global::<OverlayState>().set_site_info_zoom(100);
        }
    }

    /// Deletes cookies and stored data of the current site after asking, then reloads it.
    pub fn site_info_clear_data(&self) {
        let Some(url) = self.current_page_url() else {
            return;
        };
        let Some(origin) = origin_of(&url) else {
            return;
        };
        self.close_site_info();
        let host = display_host(&url).unwrap_or_else(|| origin.clone());
        let this = self.this();
        self.confirm(
            "Websitedaten löschen?",
            &format!(
                "Cookies, gespeicherte Daten und der Cache von {host} werden gelöscht. Du wirst dort abgemeldet."
            ),
            "Löschen",
            move || {
                let Some(browser) = this.upgrade() else { return };
                let mut cleared = false;
                browser.with_live_view(|view| cleared = view.clear_site_data(&origin));
                if cleared {
                    browser.show_toast("Websitedaten gelöscht");
                    browser.reload(false);
                } else {
                    browser.show_toast("Websitedaten konnten nicht gelöscht werden");
                }
            },
        );
    }
}

/// The security shown in the address bar for `address`.
pub(super) fn page_security(address: &str) -> PageSecurity {
    if address.is_empty() {
        PageSecurity::None
    } else if address.starts_with("https://") {
        PageSecurity::Secure
    } else if address.starts_with("http://") {
        PageSecurity::Insecure
    } else {
        PageSecurity::Internal
    }
}

fn is_web_address(address: &str) -> bool {
    address.starts_with("https://") || address.starts_with("http://")
}

fn site_summary(url: &str, security: Option<&SecurityInfo>) -> SiteSummary {
    let host = display_host(url).unwrap_or_else(|| display_url(url));
    let mut summary = SiteSummary {
        host,
        security: page_security(url),
        title: "",
        message: "",
        details: Vec::new(),
    };
    match summary.security {
        PageSecurity::Secure if security.is_none_or(|info| info.secure_connection) => {
            summary.title = "Verbindung ist sicher";
            summary.message = "Passwörter, Zahlungsdaten und alles andere werden verschlüsselt an diese Website übertragen.";
            if let Some(info) = security {
                summary.details = certificate_details(info);
            }
        }
        PageSecurity::Secure => {
            summary.security = PageSecurity::Insecure;
            summary.title = "Verbindung ist nicht vollständig sicher";
            summary.message =
                "Die Seite wurde nicht über eine geprüfte, verschlüsselte Verbindung geladen.";
        }
        PageSecurity::Insecure => {
            summary.title = "Verbindung ist nicht sicher";
            summary.message = "Gib auf dieser Seite keine Passwörter oder Zahlungsdaten ein, sie werden unverschlüsselt übertragen.";
        }
        PageSecurity::Internal | PageSecurity::None => {
            summary.security = PageSecurity::Internal;
            if url.starts_with("file:") {
                summary.title = "Lokale Datei";
                summary.message = "Diese Datei liegt auf deinem Computer.";
            } else {
                summary.title = "Interne Seite";
                summary.message = "Diese Seite wurde nicht aus dem Internet geladen.";
            }
        }
    }
    summary
}

fn certificate_details(info: &SecurityInfo) -> Vec<(&'static str, String)> {
    let mut details = Vec::new();
    if let Some(certificate) = &info.certificate {
        let subject = match &certificate.organization {
            Some(organization) => format!("{} ({organization})", certificate.subject),
            None => certificate.subject.clone(),
        };
        details.push(("Ausgestellt für", subject));
        details.push(("Ausgestellt von", certificate.issuer.clone()));
        details.push(("Gültig bis", german_date(certificate.valid_until)));
    }
    if let Some(protocol) = info.protocol {
        details.push(("Protokoll", protocol.to_owned()));
    }
    details
}

fn german_date(unix_seconds: i64) -> String {
    let (year, month, day) = calendar_date(unix_seconds);
    format!("{day:02}.{month:02}.{year}")
}

#[cfg(test)]
mod tests {
    use atoll_engine::CertificateInfo;

    use super::*;

    fn secure_connection() -> SecurityInfo {
        SecurityInfo {
            secure_connection: true,
            protocol: Some("TLS 1.3"),
            certificate: Some(CertificateInfo {
                subject: "*.example.org".to_owned(),
                organization: None,
                issuer: "Let's Encrypt".to_owned(),
                valid_from: 1_750_000_000,
                valid_until: 1_758_153_600,
            }),
        }
    }

    #[test]
    fn secure_pages_list_their_certificate() {
        let summary = site_summary("https://www.example.org/a", Some(&secure_connection()));
        assert_eq!(summary.host, "example.org");
        assert_eq!(summary.security, PageSecurity::Secure);
        assert_eq!(
            summary.details,
            vec![
                ("Ausgestellt für", "*.example.org".to_owned()),
                ("Ausgestellt von", "Let's Encrypt".to_owned()),
                ("Gültig bis", "18.09.2025".to_owned()),
                ("Protokoll", "TLS 1.3".to_owned()),
            ]
        );
    }

    #[test]
    fn unauthenticated_https_is_reported_as_insecure() {
        let info = SecurityInfo {
            secure_connection: false,
            protocol: None,
            certificate: None,
        };
        let summary = site_summary("https://example.org/", Some(&info));
        assert_eq!(summary.security, PageSecurity::Insecure);
        assert!(summary.details.is_empty());
    }

    #[test]
    fn plain_and_local_pages_are_classified() {
        assert_eq!(
            site_summary("http://example.org/", None).security,
            PageSecurity::Insecure
        );
        let local = site_summary("file:///home/user/page.html", None);
        assert_eq!(local.security, PageSecurity::Internal);
        assert_eq!(local.title, "Lokale Datei");
        assert_eq!(page_security(""), PageSecurity::None);
    }
}
