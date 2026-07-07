use cef::sys::cef_ssl_version_t;
use cef::{
    Basetime, BrowserHost, CefString, CefStringList, ImplBrowserHost, ImplNavigationEntry,
    ImplSslstatus, ImplX509CertPrincipal, ImplX509Certificate, SslVersion, X509CertPrincipal,
    X509Certificate,
};

use super::handlers::list;

/// Seconds between the Windows epoch (1601) used by Chromium and the Unix epoch.
const WINDOWS_EPOCH_OFFSET_SECONDS: i64 = 11_644_473_600;

/// The connection security of the page a view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityInfo {
    /// The page was loaded over an authenticated, encrypted connection.
    pub secure_connection: bool,
    /// The protocol version, for example `TLS 1.3`.
    pub protocol: Option<&'static str>,
    pub certificate: Option<CertificateInfo>,
}

/// The server certificate of a secure connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateInfo {
    /// Common name of the subject, usually the host name.
    pub subject: String,
    /// Organization the certificate was issued to, if it names one.
    pub organization: Option<String>,
    /// Organization, or else common name, of the issuing authority.
    pub issuer: String,
    /// Start of the validity period in seconds since the Unix epoch.
    pub valid_from: i64,
    /// End of the validity period in seconds since the Unix epoch.
    pub valid_until: i64,
}

pub(crate) fn security_info(host: &BrowserHost) -> Option<SecurityInfo> {
    let entry = host.visible_navigation_entry()?;
    if entry.is_valid() == 0 {
        return None;
    }
    let status = entry.sslstatus()?;
    Some(SecurityInfo {
        secure_connection: status.is_secure_connection() != 0,
        protocol: protocol_name(&status.sslversion()),
        certificate: status
            .x509_certificate()
            .and_then(|certificate| certificate_info(&certificate)),
    })
}

fn certificate_info(certificate: &X509Certificate) -> Option<CertificateInfo> {
    let subject = certificate.subject()?;
    let issuer = certificate.issuer()?;
    Some(CertificateInfo {
        subject: principal_name(&subject),
        organization: first_organization(&subject),
        issuer: first_organization(&issuer).unwrap_or_else(|| principal_name(&issuer)),
        valid_from: unix_seconds(certificate.valid_start()),
        valid_until: unix_seconds(certificate.valid_expiry()),
    })
}

fn principal_name(principal: &X509CertPrincipal) -> String {
    let common_name = CefString::from(&principal.common_name()).to_string();
    if common_name.is_empty() {
        CefString::from(&principal.display_name()).to_string()
    } else {
        common_name
    }
}

fn first_organization(principal: &X509CertPrincipal) -> Option<String> {
    let mut names = CefStringList::new();
    principal.organization_names(Some(&mut names));
    list(Some(&mut names))
        .into_iter()
        .find(|name| !name.is_empty())
}

fn protocol_name(version: &SslVersion) -> Option<&'static str> {
    match *version.as_ref() {
        cef_ssl_version_t::SSL_CONNECTION_VERSION_SSL3 => Some("SSL 3.0"),
        cef_ssl_version_t::SSL_CONNECTION_VERSION_TLS1 => Some("TLS 1.0"),
        cef_ssl_version_t::SSL_CONNECTION_VERSION_TLS1_1 => Some("TLS 1.1"),
        cef_ssl_version_t::SSL_CONNECTION_VERSION_TLS1_2 => Some("TLS 1.2"),
        cef_ssl_version_t::SSL_CONNECTION_VERSION_TLS1_3 => Some("TLS 1.3"),
        cef_ssl_version_t::SSL_CONNECTION_VERSION_QUIC => Some("QUIC"),
        _ => None,
    }
}

fn unix_seconds(time: Basetime) -> i64 {
    time.val / 1_000_000 - WINDOWS_EPOCH_OFFSET_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_times_are_converted_to_unix_seconds() {
        let first_of_2025 = 1_735_689_600;
        let chromium = (first_of_2025 + WINDOWS_EPOCH_OFFSET_SECONDS) * 1_000_000;
        assert_eq!(unix_seconds(Basetime { val: chromium }), first_of_2025);
    }
}
