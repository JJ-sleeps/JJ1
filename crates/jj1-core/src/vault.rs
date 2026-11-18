//! Saved sign-ins the agent can use but never see.
//!
//! Passwords live in the operating system's credential store (Windows
//! Credential Manager), under `jj1:<host>`. `jj1_login` fills them into the
//! page directly; the password never appears in a tool result, the action
//! map, a diff or the audit log, so a malicious page cannot talk the agent
//! into revealing it. A sign-in is only filled on the host it was saved for
//! (or a subdomain of it), and only into a form that posts back to that host.

use jj1_net::host_matches;

pub struct Login {
    pub host: String,
    pub user: String,
    pub password: String,
}

/// Where sign-ins are kept. The real store is the OS credential manager;
/// tests use [`MemoryVault`].
pub trait Vault {
    fn save(&mut self, host: &str, user: &str, password: &str) -> Result<(), String>;
    fn list(&self) -> Result<Vec<(String, String)>, String>;
    fn get(&self, host: &str) -> Result<Option<Login>, String>;
    fn remove(&mut self, host: &str) -> Result<bool, String>;

    /// The saved sign-in that applies to `host`: an exact match, or the
    /// closest parent domain (a login saved for `example.com` works on
    /// `accounts.example.com`).
    fn find(&self, host: &str) -> Result<Option<Login>, String> {
        let host = host.to_ascii_lowercase();
        let mut best: Option<String> = None;
        for (h, _) in self.list()? {
            if host_matches(&host, &h) && best.as_ref().is_none_or(|b| h.len() > b.len()) {
                best = Some(h);
            }
        }
        match best {
            Some(h) => self.get(&h),
            None => Ok(None),
        }
    }
}

pub fn normalize_host(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    let h = h.strip_prefix("https://").or_else(|| h.strip_prefix("http://")).unwrap_or(&h);
    h.split(['/', '?', '#']).next().unwrap_or("").trim_start_matches("www.").to_string()
}

/// In-memory vault for tests.
#[derive(Default)]
pub struct MemoryVault(pub Vec<Login>);

impl Vault for MemoryVault {
    fn save(&mut self, host: &str, user: &str, password: &str) -> Result<(), String> {
        let host = normalize_host(host);
        self.0.retain(|l| l.host != host);
        self.0.push(Login { host, user: user.into(), password: password.into() });
        Ok(())
    }
    fn list(&self) -> Result<Vec<(String, String)>, String> {
        Ok(self.0.iter().map(|l| (l.host.clone(), l.user.clone())).collect())
    }
    fn get(&self, host: &str) -> Result<Option<Login>, String> {
        Ok(self.0.iter().find(|l| l.host == host).map(|l| Login { host: l.host.clone(), user: l.user.clone(), password: l.password.clone() }))
    }
    fn remove(&mut self, host: &str) -> Result<bool, String> {
        let n = self.0.len();
        self.0.retain(|l| l.host != normalize_host(host));
        Ok(self.0.len() != n)
    }
}

/// The operating system's credential store.
pub struct OsVault;

/// Read a line from the terminal without showing what is typed (Windows
/// console); piped input is read as-is.
pub fn read_secret(prompt: &str) -> Result<String, String> {
    use std::io::Write;
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    #[cfg(windows)]
    let restore = {
        use windows::Win32::System::Console::{
            GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE,
        };
        // SAFETY: plain console calls on this process's stdin handle.
        unsafe {
            GetStdHandle(STD_INPUT_HANDLE).ok().and_then(|h| {
                let mut mode = CONSOLE_MODE(0);
                GetConsoleMode(h, &mut mode).ok()?;
                SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT).ok()?;
                Some((h, mode))
            })
        }
    };
    let mut line = String::new();
    let r = std::io::stdin().read_line(&mut line);
    #[cfg(windows)]
    if let Some((h, mode)) = restore {
        // SAFETY: restoring the mode read above.
        unsafe {
            let _ = windows::Win32::System::Console::SetConsoleMode(h, mode);
        }
        eprintln!();
    }
    r.map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

const PREFIX: &str = "jj1:";

#[cfg(windows)]
mod win {
    use super::*;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_pwstr(p: PWSTR) -> String {
        if p.is_null() {
            return String::new();
        }
        // SAFETY: Windows returns NUL-terminated wide strings here.
        unsafe { p.to_string().unwrap_or_default() }
    }

    impl Vault for OsVault {
        fn save(&mut self, host: &str, user: &str, password: &str) -> Result<(), String> {
            let host = normalize_host(host);
            if host.is_empty() {
                return Err("Give the site's host name, like example.com.".into());
            }
            let mut target = wide(&format!("{PREFIX}{host}"));
            let mut user_w = wide(user);
            let mut comment = wide("Saved by JJ1 for jj1_login");
            let mut blob = password.as_bytes().to_vec();
            let cred = CREDENTIALW {
                Flags: CRED_FLAGS(0),
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR(target.as_mut_ptr()),
                Comment: PWSTR(comment.as_mut_ptr()),
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                UserName: PWSTR(user_w.as_mut_ptr()),
                ..Default::default()
            };
            // SAFETY: every pointer in `cred` points into a live local buffer.
            let r = unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("could not save to Credential Manager: {e}"));
            blob.iter_mut().for_each(|b| *b = 0);
            r
        }

        fn list(&self) -> Result<Vec<(String, String)>, String> {
            let filter = wide(&format!("{PREFIX}*"));
            let mut count = 0u32;
            let mut creds: *mut *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: out-pointers are valid locals; the array is freed below.
            if unsafe { CredEnumerateW(PCWSTR(filter.as_ptr()), None, &mut count, &mut creds) }.is_err() {
                return Ok(Vec::new()); // ERROR_NOT_FOUND: nothing saved yet
            }
            let mut out = Vec::new();
            for i in 0..count as usize {
                // SAFETY: `creds` holds `count` valid credential pointers.
                let c = unsafe { &**creds.add(i) };
                let target = from_pwstr(c.TargetName);
                if let Some(host) = target.strip_prefix(PREFIX) {
                    out.push((host.to_string(), from_pwstr(c.UserName)));
                }
            }
            // SAFETY: allocated by CredEnumerateW.
            unsafe { CredFree(creds as *const _) };
            out.sort();
            Ok(out)
        }

        fn get(&self, host: &str) -> Result<Option<Login>, String> {
            let target = wide(&format!("{PREFIX}{host}"));
            let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: out-pointer is a valid local; freed below.
            if unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut cred) }.is_err() {
                return Ok(None);
            }
            // SAFETY: CredReadW succeeded, so `cred` is valid until CredFree.
            let login = unsafe {
                let c = &*cred;
                let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
                Login { host: host.to_string(), user: from_pwstr(c.UserName), password: String::from_utf8_lossy(blob).into_owned() }
            };
            // SAFETY: allocated by CredReadW.
            unsafe { CredFree(cred as *const _) };
            Ok(Some(login))
        }

        fn remove(&mut self, host: &str) -> Result<bool, String> {
            let target = wide(&format!("{PREFIX}{}", normalize_host(host)));
            // SAFETY: valid NUL-terminated target name.
            Ok(unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) }.is_ok())
        }
    }
}

#[cfg(not(windows))]
impl Vault for OsVault {
    fn save(&mut self, _: &str, _: &str, _: &str) -> Result<(), String> {
        Err("The JJ1 vault currently uses Windows Credential Manager; other systems are not supported yet.".into())
    }
    fn list(&self) -> Result<Vec<(String, String)>, String> {
        Ok(Vec::new())
    }
    fn get(&self, _: &str) -> Result<Option<Login>, String> {
        Ok(None)
    }
    fn remove(&mut self, _: &str) -> Result<bool, String> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_domain_logins_apply_to_subdomains() {
        let mut v = MemoryVault::default();
        v.save("https://www.Example.com/login", "ann", "pw1").unwrap();
        v.save("accounts.example.com", "ann2", "pw2").unwrap();
        assert_eq!(v.find("example.com").unwrap().unwrap().user, "ann");
        assert_eq!(v.find("shop.example.com").unwrap().unwrap().user, "ann");
        assert_eq!(v.find("accounts.example.com").unwrap().unwrap().user, "ann2");
        assert!(v.find("example.com.evil.test").unwrap().is_none());
        assert!(v.find("notexample.com").unwrap().is_none());
    }
}
