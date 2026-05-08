// OS-keychain wrapper for the two API keys we hold.
// macOS uses Keychain, Windows uses Credential Manager, Linux uses Secret Service.

use keyring::Entry;

const SERVICE: &str = "com.ryan.burnbar";

#[derive(Clone, Copy, Debug)]
pub enum KeyKind {
    AnthropicAdmin,
    OpenAiAdmin,
}

impl KeyKind {
    fn account(self) -> &'static str {
        match self {
            KeyKind::AnthropicAdmin => "anthropic-admin-key",
            KeyKind::OpenAiAdmin => "openai-admin-key",
        }
    }
}

fn entry(k: KeyKind) -> Result<Entry, String> {
    Entry::new(SERVICE, k.account()).map_err(|e| e.to_string())
}

pub fn save(k: KeyKind, secret: &str) -> Result<(), String> {
    entry(k)?.set_password(secret).map_err(|e| e.to_string())
}

pub fn load(k: KeyKind) -> Option<String> {
    entry(k).ok()?.get_password().ok()
}

pub fn clear(k: KeyKind) -> Result<(), String> {
    match entry(k)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
