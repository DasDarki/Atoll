//! Signing in with Onyx the way RFC 8252 asks of an app on a computer: the authorization code
//! flow with PKCE, and the answer caught on a port of the loopback address that is only open
//! while the user signs in. Onyx accepts any port of `127.0.0.1` without registering it.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use atoll_i18n::t;
use atoll_sync::wire::ServerConfig;
use data_encoding::BASE64URL_NOPAD;
use sha2::{Digest, Sha256};

/// The path Onyx sends the browser back to.
const CALLBACK_PATH: &str = "/atoll/callback";

/// What Atoll asks Onyx to share.
const SCOPES: &str = "openid username avatar";

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("the port for the answer could not be opened: {0}")]
    Port(std::io::Error),
    #[error("signing in was cancelled")]
    Cancelled,
    #[error("signing in took too long")]
    TimedOut,
    #[error("Onyx refused: {0}")]
    Refused(String),
}

/// What comes back when the user signed in: the code to exchange and what proves it is ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorized {
    pub code: String,
    pub redirect_uri: String,
    pub verifier: String,
}

/// A sign-in under way.
pub struct Login {
    /// The page to show the user, at Onyx.
    pub authorize_url: String,
    redirect_uri: String,
    verifier: String,
    state: String,
    listener: TcpListener,
}

impl Login {
    /// Opens the port for the answer and puts the address of the sign-in page together.
    pub fn start(config: &ServerConfig) -> Result<Self, LoginError> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(LoginError::Port)?;
        listener.set_nonblocking(true).map_err(LoginError::Port)?;
        let port = listener.local_addr().map_err(LoginError::Port)?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let verifier = random_text(32);
        let state = random_text(16);
        let challenge = BASE64URL_NOPAD.encode(&Sha256::digest(verifier.as_bytes()));
        let query = [
            ("response_type", "code"),
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("scope", SCOPES),
            ("state", state.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ]
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");
        Ok(Self {
            authorize_url: format!(
                "{}/oauth/authorize?{query}",
                config.issuer.trim_end_matches('/')
            ),
            redirect_uri,
            verifier,
            state,
            listener,
        })
    }

    /// Waits for Onyx to send the browser back, until `timeout` or until `cancel` is set.
    pub fn wait(self, timeout: Duration, cancel: &AtomicBool) -> Result<Authorized, LoginError> {
        let started = Instant::now();
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(LoginError::Cancelled);
            }
            if started.elapsed() > timeout {
                return Err(LoginError::TimedOut);
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(answer) = self.answer(stream) {
                        return answer;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(LoginError::Port(error)),
            }
        }
    }

    /// Reads one request. Answers `None` for anything that is not the answer of Onyx, such as the
    /// browser asking for a favicon, so waiting goes on.
    fn answer(&self, mut stream: TcpStream) -> Option<Result<Authorized, LoginError>> {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).ok()?;
        let target = line.split_whitespace().nth(1)?;
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path != CALLBACK_PATH {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return None;
        }
        let value = |name: &str| {
            query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(key, _)| *key == name)
                .map(|(_, value)| decode(value))
        };
        let result = if value("state").as_deref() != Some(self.state.as_str()) {
            Err(LoginError::Refused(
                "the answer belongs to another sign-in".to_owned(),
            ))
        } else if let Some(error) = value("error") {
            Err(LoginError::Refused(
                value("error_description").unwrap_or(error),
            ))
        } else if let Some(code) = value("code") {
            Ok(Authorized {
                code,
                redirect_uri: self.redirect_uri.clone(),
                verifier: self.verifier.clone(),
            })
        } else {
            Err(LoginError::Refused("Onyx sent no code".to_owned()))
        };
        let (title, text) = match &result {
            Ok(_) => (
                t!("Angemeldet"),
                t!("Atoll ist angemeldet. Dieser Tab schließt sich gleich."),
            ),
            Err(_) => (
                t!("Anmeldung fehlgeschlagen"),
                t!("Atoll konnte nicht angemeldet werden."),
            ),
        };
        let page = format!(
            "<!doctype html><html><head><meta charset=utf-8><title>{title}</title></head>\
             <body style=\"font:16px sans-serif;background:#111;color:#eee;display:grid;place-items:center;height:100vh;margin:0\">\
             <div><h1>{title}</h1><p>{text}</p></div></body></html>"
        );
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{page}",
            page.len()
        );
        Some(result)
    }
}

fn random_text(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("the system has no source of randomness");
    BASE64URL_NOPAD.encode(&buffer)
}

/// Percent-encodes a query value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Reads a percent-encoded query value, `+` as a space.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let escaped = (byte == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (escaped, byte) {
            (Some(decoded), _) => {
                out.push(decoded);
                index += 3;
                continue;
            }
            (None, b'+') => out.push(b' '),
            (None, byte) => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ServerConfig {
        ServerConfig {
            issuer: "https://auth.example/".to_owned(),
            client_id: "atoll".to_owned(),
        }
    }

    fn query_of(url: &str, name: &str) -> String {
        let query = url.split_once('?').unwrap().1;
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| decode(value))
            .unwrap()
    }

    fn get(port_url: &str, path: &str) -> String {
        let address = port_url
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap()
            .to_owned();
        let mut stream = TcpStream::connect(address).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut answer = String::new();
        let _ = std::io::Read::read_to_string(&mut stream, &mut answer);
        answer
    }

    #[test]
    fn the_sign_in_page_carries_pkce_and_a_loopback_redirect() {
        let login = Login::start(&config()).unwrap();
        let url = &login.authorize_url;
        assert!(url.starts_with("https://auth.example/oauth/authorize?"));
        assert_eq!(query_of(url, "code_challenge_method"), "S256");
        assert_eq!(query_of(url, "scope"), SCOPES);
        let redirect = query_of(url, "redirect_uri");
        assert!(redirect.starts_with("http://127.0.0.1:") && redirect.ends_with(CALLBACK_PATH));
        let challenge = BASE64URL_NOPAD.encode(&Sha256::digest(login.verifier.as_bytes()));
        assert_eq!(query_of(url, "code_challenge"), challenge);
    }

    #[test]
    fn the_code_is_caught_on_the_loopback_port() {
        let login = Login::start(&config()).unwrap();
        let redirect = query_of(&login.authorize_url, "redirect_uri");
        let state = query_of(&login.authorize_url, "state");
        let waiting = std::thread::spawn(move || {
            login.wait(Duration::from_secs(10), &AtomicBool::new(false))
        });
        let favicon = get(&redirect, "/favicon.ico");
        assert!(
            favicon.starts_with("HTTP/1.1 404"),
            "a favicon request does not end waiting"
        );
        let page = get(
            &redirect,
            &format!("{CALLBACK_PATH}?code=abc%2Fdef&state={state}"),
        );
        assert!(page.starts_with("HTTP/1.1 200"));
        let authorized = waiting.join().unwrap().unwrap();
        assert_eq!(authorized.code, "abc/def");
        assert_eq!(authorized.redirect_uri, redirect);
    }

    #[test]
    fn an_answer_for_another_sign_in_is_refused() {
        let login = Login::start(&config()).unwrap();
        let redirect = query_of(&login.authorize_url, "redirect_uri");
        let waiting = std::thread::spawn(move || {
            login.wait(Duration::from_secs(10), &AtomicBool::new(false))
        });
        get(&redirect, &format!("{CALLBACK_PATH}?code=abc&state=forged"));
        assert!(matches!(
            waiting.join().unwrap(),
            Err(LoginError::Refused(_))
        ));
    }

    #[test]
    fn a_refusal_of_onyx_is_passed_on() {
        let login = Login::start(&config()).unwrap();
        let redirect = query_of(&login.authorize_url, "redirect_uri");
        let state = query_of(&login.authorize_url, "state");
        let waiting = std::thread::spawn(move || {
            login.wait(Duration::from_secs(10), &AtomicBool::new(false))
        });
        get(
            &redirect,
            &format!(
                "{CALLBACK_PATH}?error=access_denied&error_description=Abgelehnt+vom+Nutzer&state={state}"
            ),
        );
        match waiting.join().unwrap() {
            Err(LoginError::Refused(reason)) => assert_eq!(reason, "Abgelehnt vom Nutzer"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn query_values_are_read_back_as_written() {
        assert_eq!(decode(&encode("a b/ü&=%")), "a b/ü&=%");
        assert_eq!(decode("100%"), "100%", "a lone percent sign stays");
        assert_eq!(decode("%zz"), "%zz");
    }

    #[test]
    fn waiting_ends_when_cancelled() {
        let login = Login::start(&config()).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            login.wait(Duration::from_secs(10), &cancel),
            Err(LoginError::Cancelled)
        ));
    }
}
