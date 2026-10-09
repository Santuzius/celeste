//! Credential storage on Android: one file per entry in the app's private storage, encrypted with AES-GCM under a key that never leaves the Android Keystore (`Bridge.encrypt`/`decrypt`). Same contract as the desktop keyring: `store` overwrites, `load` is `None` when missing, `delete` is idempotent.

use std::path::PathBuf;

use jni::objects::{JByteArray, JValue};

use super::{bridge, with_context};

fn path(account: &str) -> PathBuf {
    // Account names contain remote names; hex keeps any character out of the file name.
    let name: String = account.bytes().map(|b| format!("{b:02x}")).collect();
    crate::util::get_data_dir().join("secrets").join(format!("{name}.bin"))
}

/// Persist `value` under `account`, overwriting any existing entry.
pub fn store(account: &str, value: &str) -> Result<(), String> {
    let sealed = crypt("encrypt", value.as_bytes()).ok_or("Keystore encryption failed")?;
    let path = path(account);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("secret store failed: {e}"))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, sealed).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| format!("secret store failed: {e}"))
}

/// Read `account`. `Ok(None)` when no entry exists yet.
pub fn load(account: &str) -> Result<Option<String>, String> {
    let sealed = match std::fs::read(path(account)) {
        Ok(sealed) => sealed,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("secret read failed: {err}")),
    };
    let plain = crypt("decrypt", &sealed).ok_or("Keystore decryption failed")?;
    String::from_utf8(plain).map(Some).map_err(|e| format!("secret read failed: {e}"))
}

/// Remove `account`; a missing entry counts as success.
pub fn delete(account: &str) -> Result<(), String> {
    match std::fs::remove_file(path(account)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(format!("secret delete failed: {err}")),
        _ => Ok(()),
    }
}

/// `Bridge.encrypt` or `Bridge.decrypt`: `byte[] → byte[]`, `null` on failure.
fn crypt(method: &str, input: &[u8]) -> Option<Vec<u8>> {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        let input = env.byte_array_from_slice(input)?;
        let output = env.call_static_method(class, method, "([B)[B", &[JValue::Object(&input)])?.l()?;
        if output.is_null() {
            return Ok(None);
        }
        Ok(Some(env.convert_byte_array(JByteArray::from(output))?))
    })
    .flatten()
}
