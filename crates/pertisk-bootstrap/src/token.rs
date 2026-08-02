//! Bootstrap token generation (kubeadm-compatible id.secret form).

use rand::Rng;

/// Generate `abcdef.0123456789abcdef` style bootstrap token.
pub fn generate_bootstrap_token() -> String {
    let mut rng = rand::thread_rng();
    let id: String = (0..6)
        .map(|_| {
            let idx = rng.gen_range(0..36u8);
            char::from_digit(idx as u32, 36).unwrap_or('a')
        })
        .collect();
    let secret: String = (0..16)
        .map(|_| {
            let idx = rng.gen_range(0..36u8);
            char::from_digit(idx as u32, 36).unwrap_or('a')
        })
        .collect();
    format!("{id}.{secret}")
}

pub fn split_token(token: &str) -> Option<(&str, &str)> {
    let (id, secret) = token.split_once('.')?;
    if id.len() == 6 && secret.len() == 16 {
        Some((id, secret))
    } else {
        None
    }
}
