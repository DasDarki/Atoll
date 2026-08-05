use std::fmt;

use serde::Deserialize;

/// A login as `op item list` shows it, without secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub id: String,
    pub title: String,
    pub username: String,
    pub vault: String,
    pub urls: Vec<String>,
}

/// What is filled into a login form.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    /// The current one-time password if the login has one.
    pub one_time_code: Option<String>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"…")
            .field("one_time_code", &self.one_time_code.as_ref().map(|_| "…"))
            .finish()
    }
}

#[derive(Deserialize)]
struct ListedItem {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    additional_information: String,
    #[serde(default)]
    vault: Option<Vault>,
    #[serde(default)]
    urls: Vec<ItemUrl>,
}

#[derive(Deserialize)]
struct Vault {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct ItemUrl {
    #[serde(default)]
    href: String,
}

#[derive(Deserialize)]
struct FullItem {
    #[serde(default)]
    fields: Vec<Field>,
}

#[derive(Deserialize)]
struct Field {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    totp: Option<String>,
}

/// Parses the output of `op item list --format json`.
pub(crate) fn parse_logins(json: &str) -> serde_json::Result<Vec<Login>> {
    let items: Vec<ListedItem> = serde_json::from_str(json)?;
    Ok(items
        .into_iter()
        .map(|item| Login {
            id: item.id,
            title: item.title,
            username: item.additional_information,
            vault: item.vault.map(|vault| vault.name).unwrap_or_default(),
            urls: item
                .urls
                .into_iter()
                .map(|url| url.href)
                .filter(|href| !href.is_empty())
                .collect(),
        })
        .collect())
}

/// Parses the output of `op item get --format json`.
pub(crate) fn parse_credentials(json: &str) -> serde_json::Result<Credentials> {
    let item: FullItem = serde_json::from_str(json)?;
    let value_for = |purpose: &str| {
        item.fields
            .iter()
            .find(|field| field.purpose.as_deref() == Some(purpose))
            .and_then(|field| field.value.clone())
            .unwrap_or_default()
    };
    Ok(Credentials {
        username: value_for("USERNAME"),
        password: value_for("PASSWORD"),
        one_time_code: item
            .fields
            .iter()
            .find(|field| field.kind == "OTP")
            .and_then(|field| field.totp.clone())
            .filter(|code| !code.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
      {
        "id": "abc123",
        "title": "GitHub",
        "version": 4,
        "vault": { "id": "v1", "name": "Privat" },
        "category": "LOGIN",
        "additional_information": "octo@example.org",
        "urls": [
          { "primary": true, "href": "https://github.com/login" },
          { "href": "https://gist.github.com" }
        ]
      },
      { "id": "def456", "title": "Ohne Adresse", "category": "LOGIN" }
    ]"#;

    const ITEM: &str = r#"{
      "id": "abc123",
      "title": "GitHub",
      "category": "LOGIN",
      "fields": [
        { "id": "notesPlain", "type": "STRING", "purpose": "NOTES", "label": "notesPlain" },
        { "id": "username", "type": "STRING", "purpose": "USERNAME", "label": "username", "value": "octo@example.org" },
        { "id": "password", "type": "CONCEALED", "purpose": "PASSWORD", "label": "password", "value": "s3cr\"et" },
        { "id": "TOTP_1", "type": "OTP", "label": "one-time password", "value": "otpauth://totp/x", "totp": "492039" }
      ]
    }"#;

    #[test]
    fn listed_logins_keep_title_username_vault_and_addresses() {
        let logins = parse_logins(LIST).unwrap();
        assert_eq!(
            logins[0],
            Login {
                id: "abc123".to_owned(),
                title: "GitHub".to_owned(),
                username: "octo@example.org".to_owned(),
                vault: "Privat".to_owned(),
                urls: vec![
                    "https://github.com/login".to_owned(),
                    "https://gist.github.com".to_owned()
                ],
            }
        );
        assert!(logins[1].urls.is_empty());
        assert_eq!(logins[1].username, "");
    }

    #[test]
    fn credentials_come_from_the_username_password_and_otp_fields() {
        let credentials = parse_credentials(ITEM).unwrap();
        assert_eq!(credentials.username, "octo@example.org");
        assert_eq!(credentials.password, "s3cr\"et");
        assert_eq!(credentials.one_time_code.as_deref(), Some("492039"));
    }

    #[test]
    fn debug_output_hides_secrets() {
        let credentials = parse_credentials(ITEM).unwrap();
        let debug = format!("{credentials:?}");
        assert!(debug.contains("octo@example.org"));
        assert!(!debug.contains("s3cr"));
        assert!(!debug.contains("492039"));
    }

    #[test]
    fn broken_output_is_an_error() {
        assert!(parse_logins("Du bist nicht angemeldet").is_err());
        assert!(parse_credentials("kein JSON").is_err());
    }
}
